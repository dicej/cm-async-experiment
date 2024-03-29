#[allow(clippy::all)]
mod bindings {
    use super::Component;
    pub use bindings::*;
    cargo_component_bindings::generate!();
}

use {
    async_trait::async_trait,
    bindings::{
        exports::component::guest::original_interface_async::Guest as OriginalInterfaceAsync,
        isyswasfa::isyswasfa::isyswasfa::{Pending, PollInput, PollOutput, Ready},
        Guest,
    },
};

// library crate
mod isyswasfa_guest {
    use {
        super::bindings::isyswasfa::isyswasfa::isyswasfa::{
            self, Cancel, Pending, PollInput, PollInputCancel, PollInputListening, PollInputReady,
            PollOutput, PollOutputListen, PollOutputPending, PollOutputReady, Ready,
        },
        by_address::ByAddress,
        futures::{channel::oneshot, future::FutureExt},
        once_cell::sync::Lazy,
        std::{
            any::Any,
            cell::RefCell,
            collections::HashMap,
            future::Future,
            mem,
            ops::{Deref, DerefMut},
            pin::Pin,
            rc::Rc,
            sync::Arc,
            task::{Context, Poll, Wake, Waker},
        },
    };

    fn dummy_waker() -> Waker {
        struct DummyWaker;

        impl Wake for DummyWaker {
            fn wake(self: Arc<Self>) {}
        }

        static WAKER: Lazy<Arc<DummyWaker>> = Lazy::new(|| Arc::new(DummyWaker));

        WAKER.clone().into()
    }

    type BoxFuture = Pin<Box<dyn Future<Output = Box<dyn Any>> + 'static>>;

    enum CancelState {
        Pending,
        Cancel,
        Listening(Cancel),
    }

    struct CancelOnDrop(Rc<RefCell<CancelState>>);

    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            match mem::replace(self.0.borrow_mut().deref_mut(), CancelState::Cancel) {
                CancelState::Pending | CancelState::Cancel => {}
                CancelState::Listening(cancel) => push(PollOutput::Cancel(cancel)),
            }
        }
    }

    struct PendingState {
        pending: Pending,
        tx: oneshot::Sender<Ready>,
        cancel_state: Rc<RefCell<CancelState>>,
    }

    static mut PENDING: Vec<PendingState> = Vec::new();

    static mut POLL_OUTPUT: Vec<PollOutput> = Vec::new();

    fn push(output: PollOutput) {
        unsafe { POLL_OUTPUT.push(output) }
    }

    fn add_pending(pending_state: PendingState) {
        unsafe { PENDING.push(pending_state) }
    }

    fn take_pending() -> Vec<PendingState> {
        let pending = unsafe { mem::take(&mut PENDING) };
        assert!(!pending.is_empty());
        pending
    }

    fn clear_pending() {
        unsafe { PENDING.clear() }
    }

    struct ListenState {
        tx: oneshot::Sender<Ready>,
        future_state: Rc<RefCell<FutureState>>,
        cancel_state: Rc<RefCell<CancelState>>,
    }

    enum FutureState {
        Pending {
            ready: Option<Ready>,
            future: BoxFuture,
            cancel_states: Vec<Rc<RefCell<CancelState>>>,
        },
        Cancelled(Option<Cancel>),
        Ready(Option<Box<dyn Any>>),
    }

    impl Drop for FutureState {
        fn drop(&mut self) {
            match self {
                Self::Pending { .. } => (),
                Self::Cancelled(cancel) => push(PollOutput::CancelComplete(cancel.take().unwrap())),
                Self::Ready(ready) => assert!(ready.is_none()),
            }
        }
    }

    fn push_listens(future_state: &Rc<RefCell<FutureState>>) {
        for pending in take_pending() {
            push(PollOutput::Listen(PollOutputListen {
                pending: pending.pending,
                state: u32::try_from(Box::into_raw(Box::new(ListenState {
                    tx: pending.tx,
                    cancel_state: pending.cancel_state,
                    future_state: future_state.clone(),
                })) as usize)
                .unwrap(),
            }));
        }
    }

    pub fn first_poll<T: 'static>(future: impl Future<Output = T> + 'static) -> Result<T, Pending> {
        let mut future = Box::pin(future.map(|v| Box::new(v) as Box<dyn Any>)) as BoxFuture;

        match future
            .as_mut()
            .poll(&mut Context::from_waker(&dummy_waker()))
        {
            Poll::Pending => {
                let (pending, cancel, ready) = isyswasfa::make_task();
                let future_state = Rc::new(RefCell::new(FutureState::Pending {
                    ready: Some(ready),
                    future,
                    cancel_states: Vec::new(),
                }));

                push_listens(&future_state);

                push(PollOutput::Pending(PollOutputPending {
                    cancel,
                    state: u32::try_from(Rc::into_raw(future_state) as usize).unwrap(),
                }));

                Err(pending)
            }
            Poll::Ready(result) => {
                clear_pending();
                Ok(*result.downcast().unwrap())
            }
        }
    }

    pub fn get_ready<T: 'static>(ready: Ready) -> T {
        match unsafe { Rc::from_raw(ready.state() as usize as *const RefCell<FutureState>) }
            .borrow_mut()
            .deref_mut()
        {
            FutureState::Ready(value) => *value.take().unwrap().downcast().unwrap(),
            _ => unreachable!(),
        }
    }

    fn cancel_all(cancels: &[Rc<RefCell<CancelState>>]) {
        for cancel in cancels {
            match mem::replace(cancel.borrow_mut().deref_mut(), CancelState::Cancel) {
                CancelState::Pending | CancelState::Cancel => {}
                CancelState::Listening(cancel) => push(PollOutput::Cancel(cancel)),
            }
        }
    }

    pub fn poll(input: Vec<PollInput>) -> Vec<PollOutput> {
        let mut pollables = HashMap::new();

        for input in input {
            match input {
                PollInput::Listening(PollInputListening { state, cancel }) => {
                    let listen_state =
                        unsafe { (state as usize as *const ListenState).as_ref().unwrap() };

                    let listening = match listen_state.cancel_state.borrow().deref() {
                        CancelState::Pending => true,
                        CancelState::Cancel => false,
                        CancelState::Listening(_) => unreachable!(),
                    };

                    if listening {
                        match listen_state.future_state.borrow_mut().deref_mut() {
                            FutureState::Pending { cancel_states, .. } => {
                                cancel_states.push(listen_state.cancel_state.clone())
                            }
                            _ => unreachable!(),
                        }

                        *listen_state.cancel_state.borrow_mut() = CancelState::Listening(cancel)
                    } else {
                        push(PollOutput::Cancel(cancel));
                    }
                }
                PollInput::Ready(PollInputReady { state, ready }) => {
                    let listen_state =
                        *unsafe { Box::from_raw(state as usize as *mut ListenState) };

                    match mem::replace(
                        listen_state.cancel_state.borrow_mut().deref_mut(),
                        CancelState::Cancel,
                    ) {
                        CancelState::Pending | CancelState::Listening(_) => {
                            drop(listen_state.tx.send(ready))
                        }
                        CancelState::Cancel => {}
                    }

                    pollables.insert(
                        ByAddress(listen_state.future_state.clone()),
                        listen_state.future_state,
                    );
                }
                PollInput::Cancel(PollInputCancel { state, cancel }) => {
                    let future_state =
                        unsafe { Rc::from_raw(state as usize as *const RefCell<FutureState>) };

                    let mut old = mem::replace(
                        future_state.borrow_mut().deref_mut(),
                        FutureState::Cancelled(Some(cancel)),
                    );

                    match &mut old {
                        FutureState::Pending { cancel_states, .. } => cancel_all(cancel_states),
                        FutureState::Cancelled(_) => unreachable!(),
                        FutureState::Ready(ready) => drop(ready.take()),
                    }
                }
                PollInput::CancelComplete(state) => unsafe {
                    drop(Box::from_raw(state as usize as *mut ListenState))
                },
            }
        }

        for future_state in pollables.into_values() {
            let poll = match future_state.borrow_mut().deref_mut() {
                FutureState::Pending { future, .. } => future
                    .as_mut()
                    .poll(&mut Context::from_waker(&dummy_waker())),
                _ => continue,
            };

            match poll {
                Poll::Pending => push_listens(&future_state),
                Poll::Ready(result) => {
                    clear_pending();

                    let mut old = mem::replace(
                        future_state.borrow_mut().deref_mut(),
                        FutureState::Ready(Some(result)),
                    );

                    let FutureState::Pending {
                        ready,
                        cancel_states,
                        ..
                    } = &mut old
                    else {
                        unreachable!()
                    };

                    cancel_all(cancel_states);

                    push(PollOutput::Ready(PollOutputReady {
                        ready: ready.take().unwrap(),
                        state: u32::try_from(Rc::into_raw(future_state) as usize).unwrap(),
                    }));
                }
            }
        }

        unsafe { mem::take(&mut POLL_OUTPUT) }
    }

    pub async fn await_ready(pending: Pending) -> Ready {
        let (tx, rx) = oneshot::channel();
        let cancel_state = Rc::new(RefCell::new(CancelState::Pending));
        add_pending(PendingState {
            pending,
            tx,
            cancel_state: cancel_state.clone(),
        });
        let _cancel_on_drop = CancelOnDrop(cancel_state);
        rx.await.unwrap()
    }
}

// generated
mod isyswasfa_bindings {
    pub mod original_interface_async {
        use super::super::{bindings::component::guest::original_interface_async, isyswasfa_guest};

        pub async fn foo(s: &str) -> String {
            match original_interface_async::foo(s) {
                Ok(result) => result,
                Err(pending) => original_interface_async::foo_result(
                    isyswasfa_guest::await_ready(pending).await,
                ),
            }
        }
    }
}

// generated
struct Component;

// generated
impl Guest for Component {
    fn isyswasfa_poll_abc123(input: Vec<PollInput>) -> Vec<PollOutput> {
        isyswasfa_guest::poll(input)
    }
}

// generated
impl OriginalInterfaceAsync for Component {
    fn foo(s: String) -> Result<String, Pending> {
        isyswasfa_guest::first_poll(<ComponentAsync as GuestAsync>::foo(s))
    }

    fn foo_result(ready: Ready) -> String {
        isyswasfa_guest::get_ready(ready)
    }
}

// generated
#[async_trait(?Send)]
trait GuestAsync {
    async fn foo(s: String) -> String;
}

// written by app dev
struct ComponentAsync;

// written by app dev
#[async_trait(?Send)]
impl GuestAsync for ComponentAsync {
    async fn foo(s: String) -> String {
        format!(
            "{} - exited guest",
            isyswasfa_bindings::original_interface_async::foo(&format!("{s} - entered guest"))
                .await
        )
    }
}
