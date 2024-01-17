cargo_component_bindings::generate!();

use {
    bindings::{
        component::guest::original_interface_async,
        exports::component::guest::original_interface_async::{FooFuture, Guest, Ready},
        exports::isyswasfa::guest::shared::Guest as IsyswasfaGuest,
        isyswasfa::guest::shared::{self, PollOutput, PollOutputPending},
    },
    std::task::{Wake, Waker},
};

// library crate
mod isyswasfa_guest {
    fn dummy_waker() -> Waker {
        struct DummyWaker;

        impl Wake for DummyWaker {
            fn wake(self: Arc<Self>) {}
        }

        static WAKER: Lazy<Arc<DummyWaker>> = Lazy::new(|| Arc::new(DummyWaker));

        WAKER.clone().into()
    }

    enum CancelState {
        Pending,
        Cancel,
        Listening,
    }

    struct CancelOnDrop(Rc<RefCell<CancelState>>);

    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            match mem::replace(self.0.borrow_mut(), CancelState::Cancel) {
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
            ready: Ready,
            future: BoxFuture<'static, Box<dyn Any + Send + Sync>>,
            cancel_states: Vec<Rc<RefCell<CancelState>>>,
        },
        Cancelled(Option<Cancel>),
        Ready(Option<Box<dyn Any + Send + Sync>>),
    }

    impl Drop for FutureState {
        fn drop(&mut self) {
            match self {
                Self::Pending { .. } => unreachable!(),
                Self::Cancelled(cancel) => push(PollOutput::CancelComplete(cancel.take().unwrap())),
                Self::Ready(None) => assert!(ready.is_none()),
            }
        }
    }

    fn push_listens(future_state: Rc<RefCell<FutureState>>) {
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

    fn first_poll<T>(future: impl Future<Output = T>) -> Result<T, Pending> {
        match future.poll(&mut Context::from_waker(&dummy_waker())) {
            Poll::Pending => {
                let (pending, cancel, ready) = isyswasfa::new();
                let future_state = Rc::new(RefCell::new(FutureState::Pending {
                    ready,
                    future,
                    cancels: Vec::new(),
                }));

                push_listens(future_state);

                isyswasfa_guest::push(PollOutput::Pending(PollOutputPending {
                    cancel,
                    state: u32::try_from(Rc::into_raw(future_state) as usize).unwrap(),
                }));

                Err(pending)
            }
            Poll::Ready(result) => {
                isyswasfa_guest::clear_pending();
                Ok(result)
            }
        }
    }

    fn get_ready<T>(ready: Ready) -> T {
        match unsafe { Rc::from_raw(ready.state() as usize as *const FutureState) }
            .borrow_mut()
            .deref_mut()
        {
            FutureState::Ready(value) => *value.take().unwrap().downcast().unwrap(),
            _ => unreachable!(),
        }
    }

    fn cancel_all(cancels: Vec<Rc<RefCell<CancelState>>>) {
        for cancel in cancels {
            match mem::replace(cancel.borrow_mut(), CancelState::Cancel) {
                CancelState::Pending | CancelState::Cancel => {}
                CancelState::Listening(cancel) => push(PollOutput::Cancel(cancel)),
            }
        }
    }

    fn poll(input: Vec<PollInput>) -> Vec<PollOutput> {
        for input in input {
            match input {
                PollInput::Listening(PollInputListening { state, cancel }) => {
                    let listen_state =
                        unsafe { (state as usize as *const ListenState).as_ref().unwrap() };

                    let listening = match listen_state.cancel_state.borrow() {
                        CancelState::Pending => true,
                        CancelState::Cancel => false,
                        CancelState::Listening(_) => unreachable!(),
                    };

                    if listening {
                        match listen_state.future_state.borrow_mut().deref_mut() {
                            FutureState::Pending { cancels, .. } => {
                                cancel_states.push(listen_state.cancel_state.clone())
                            }
                            _ => unreachable!(),
                        }

                        listen_state.cancel_state.borrow_mut() = CancelState::Listening(cancel)
                    } else {
                        push(PollOutput::Cancel(cancel));
                    }
                }
                PollInput::Ready(PollInputReady { state, ready }) => {
                    let listen_state =
                        *unsafe { Box::from_raw(state as usize as *const ListenState) };

                    listen_state.cancel_state.borrow_mut() = CancelState::Cancel;

                    _ = listen_state.tx.send(ready);

                    pollables.insert(ByAddress(listen_state.future_state.clone()), state);
                }
                PollInput::Cancel(PollInputCancel { state, cancel }) => {
                    let future_state =
                        unsafe { Rc::from_raw(state as usize as *const FutureState) };

                    let old = mem::replace(
                        future_state.borrow_mut().deref_mut(),
                        FutureState::Cancelled(Some(cancel)),
                    );

                    match old {
                        FutureState::Pending { cancel_states, .. } => cancel_all(cancel_states),
                        FutureState::Cancelled(_) => unreachable!(),
                        FutureState::Ready(ready) => ready.take(),
                    }
                }
                PollInput::CancelComplete(state) => unsafe {
                    drop(Box::from_raw(state as usize as *const ListenState))
                },
            }
        }

        for future_state in pollables.into_values() {
            let poll = match future_state.borrow_mut().deref_mut() {
                Future::Pending { future, .. } => {
                    future.poll(&mut Context::from_waker(&dummy_waker()))
                }
                _ => continue,
            };

            match poll {
                Poll::Pending => push_listens(future_state),
                Poll::Ready(result) => {
                    clear_pending();

                    let old = mem::replace(
                        future_state.borrow_mut().deref_mut(),
                        FutureState::Ready(Some(Box::new(result))),
                    );

                    let Future::Pending {
                        ready,
                        cancel_states,
                        ..
                    } = old
                    else {
                        unreachable!()
                    };

                    cancel_all(cancel_states);

                    push(PollOutput::Ready(PollOutputListen {
                        ready,
                        state: u32::try_from(Rc::into_raw(future_state) as usize).unwrap(),
                    }));
                }
            }
        }

        unsafe { mem::take(&mut POLL_OUTPUT) }
    }
}

// generated
mod original_interface_async_async {
    async fn foo(s: String) -> String {
        match original_interface_async::foo(s) {
            FooFuture::Pending(pending) => {
                let (tx, rx) = oneshot::channel();
                let cancel_state = Rc::new(RefCell::new(CancelState::Pending));
                isyswasfa_guest::add_pending(pending, tx, cancel_state.clone());
                original_interface_async::foo_result({
                    let cancel_on_drop = CancelOnDrop(cancel_state);
                    rx.await.unwrap()
                });
            }
            FooFuture::Ready(result) => result,
        }
    }
}

// generated
struct Component;

// generated
impl IsyswasfaGuest for Component {
    fn poll_abc123(input: Vec<PollInput>) -> Vec<PollOutput> {
        isyswasfa_guest::poll(input)
    }
}

// generated
impl Guest for Component {
    fn foo(s: String) -> Result<String, Pending> {
        isyswasfa_guest::first_poll(<ComponentAsync as GuestAsync>::foo(s))
    }

    fn foo_result(ready: Ready) -> String {
        isyswasfa_guest::get_ready(ready)
    }
}

// generated
#[async_trait]
trait GuestAsync {
    async fn foo(s: String) -> String;
}

// written by app dev
struct ComponentAsync;

// written by app dev
#[async_trait]
impl GuestAsync for ComponentAsync {
    async fn foo(s: String) -> String {
        original_interface_async_async::foo(s).await
    }
}
