use {
    anyhow::{anyhow, Result},
    async_trait::async_trait,
    isyswasfa_host::{IsyswasfaCtx, IsyswasfaView, Task},
    std::time::Duration,
    tokio::{fs, process::Command},
    wasmtime::{
        component::{Component, Linker, Resource, ResourceTable},
        Config, Engine, Store,
    },
    wasmtime_wasi::preview2::{command, WasiCtx, WasiCtxBuilder, WasiView},
};

wasmtime::component::bindgen!({
    path: "../guest/wit",
    async: {
        only_imports: []
    },
    with: {
        "isyswasfa:isyswasfa/isyswasfa/ready": isyswasfa_host::Task,
        "isyswasfa:isyswasfa/isyswasfa/pending": isyswasfa_host::Task,
        "isyswasfa:isyswasfa/isyswasfa/cancel": isyswasfa_host::Task,
    }
});

// generated
mod isyswasfa_bindings {
    pub struct OriginalWorldAsync(super::OriginalWorldAsync);

    impl OriginalWorldAsync {
        pub async fn instantiate_async<T: Send>(
            store: impl wasmtime::AsContextMut<Data = T>,
            component: &wasmtime::component::Component,
            linker: &wasmtime::component::Linker<T>,
        ) -> wasmtime::Result<(Self, wasmtime::component::Instance)> {
            let (a, b) =
                super::OriginalWorldAsync::instantiate_async(store, component, linker).await?;

            Ok((Self(a), b))
        }

        pub fn component_guest_original_interface_async(
            &self,
        ) -> exports::component::guest::original_interface_async::Guest {
            exports::component::guest::original_interface_async::Guest(
                self.0.component_guest_original_interface_async(),
            )
        }
    }

    pub mod component {
        pub mod guest {
            pub mod original_interface_async {
                use {
                    super::super::super::super::{
                        component::guest::original_interface_async::Host as SyncHost,
                        isyswasfa_host::{IsyswasfaView, Task},
                    },
                    async_trait::async_trait,
                    wasmtime::component::Resource,
                };

                #[async_trait]
                pub trait Host: IsyswasfaView + Send + 'static {
                    async fn foo(state: Self::State, s: String) -> wasmtime::Result<String>;
                }

                impl<T: Host> SyncHost for T {
                    fn foo(
                        &mut self,
                        s: String,
                    ) -> wasmtime::Result<Result<String, Resource<Task>>> {
                        let future = <T as Host>::foo(self.state(), s);
                        self.isyswasfa().first_poll(future)
                    }

                    fn foo_result(&mut self, ready: Resource<Task>) -> wasmtime::Result<String> {
                        self.isyswasfa().get_ready(ready)
                    }
                }
            }
        }
    }

    pub mod exports {
        pub mod component {
            pub mod guest {
                pub mod original_interface_async {
                    use super::super::super::super::super::{
                        exports::component::guest::original_interface_async::Guest as TheGuest,
                        isyswasfa_host::IsyswasfaView,
                    };

                    #[derive(Copy, Clone)]
                    pub struct Guest<'a>(pub &'a TheGuest);

                    impl<'a> Guest<'a> {
                        pub async fn call_foo<S: wasmtime::AsContextMut>(
                            self,
                            mut store: S,
                            arg0: &str,
                        ) -> wasmtime::Result<String>
                        where
                            <S as wasmtime::AsContext>::Data: IsyswasfaView + Send,
                        {
                            match self.0.call_foo(&mut store, arg0).await? {
                                Ok(result) => Ok(result),
                                Err(pending) => {
                                    let ready =
                                        IsyswasfaView::await_ready(&mut store, pending).await;
                                    self.0.call_foo_result(store, ready).await
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

mod isyswasfa_host {
    use {
        anyhow::{anyhow, bail},
        futures::future::FutureExt,
        once_cell::sync::Lazy,
        std::{
            any::Any,
            future::Future,
            pin::Pin,
            sync::Arc,
            task::{Context, Poll, Wake, Waker},
        },
        wasmtime::component::{Resource, ResourceTable},
    };

    fn dummy_waker() -> Waker {
        struct DummyWaker;

        impl Wake for DummyWaker {
            fn wake(self: Arc<Self>) {}
        }

        static WAKER: Lazy<Arc<DummyWaker>> = Lazy::new(|| Arc::new(DummyWaker));

        WAKER.clone().into()
    }

    type BoxFuture = Pin<Box<dyn Future<Output = (u32, Box<dyn Any>)> + Send + 'static>>;

    pub struct Task {
        reference_count: u32,
        state: TaskState,
    }

    pub enum TaskState {
        HostPending { future: BoxFuture },
        GuestPending,
        GuestReady { guest_state: u32 },
    }

    pub struct IsyswasfaCtx {
        table: ResourceTable,
        futures: FuturesUnordered<BoxFuture>,
    }

    impl Default for IsyswasfaCtx {
        fn default() -> Self {
            Self::new()
        }
    }

    impl IsyswasfaCtx {
        pub fn new() -> Self {
            Self {
                table: ResourceTable::new(),
            }
        }

        pub fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }

        pub fn guest_pending(
            &mut self,
        ) -> wasmtime::Result<(Resource<Task>, Resource<Task>, Resource<Task>)> {
            let pending = self.table.push(Task {
                reference_count: 3,
                state: TaskState::GuestPending,
            })?;
            let cancel = Resource::new_own(pending.rep());
            let ready = Resource::new_own(pending.rep());

            Ok((pending, cancel, ready))
        }

        pub fn first_poll<T: 'static>(
            &mut self,
            future: impl Future<Output = wasmtime::Result<T>> + Send + 'static,
        ) -> wasmtime::Result<Result<T, Resource<Task>>> {
            let task = self.table.push(Task {
                reference_count: 1,
                state: TaskState::HostPending,
            })?;
            let rep = task.rep();
            let mut future =
                Box::pin(future.map(move |v| (rep, Box::new(v) as Box<dyn Any>))) as BoxFuture;

            Ok(
                match future
                    .as_mut()
                    .poll(&mut Context::from_waker(&dummy_waker()))
                {
                    Poll::Ready(result) => {
                        self.drop(task);
                        Ok(*result.downcast().unwrap())
                    }
                    Poll::Pending => {
                        self.futures.push(future);
                        Err(task)
                    }
                },
            )
        }

        pub fn guest_state(&self, ready: Resource<Task>) -> wasmtime::Result<u32> {
            match &self.table.get(&ready)?.state {
                TaskState::GuestReady { guest_state } => Ok(*guest_state),
                _ => Err(anyhow!("unexpected task state")),
            }
        }

        pub fn drop(&mut self, handle: Resource<Task>) -> wasmtime::Result<()> {
            let task = self.table.get_mut(&handle)?;
            task.reference_count = task.reference_count.checked_sub(1).unwrap();
            if task.reference_count == 0 {
                self.host_tasks.remove(handle.rep());
                self.table.delete(handle)?;
            }
            Ok(())
        }

        pub fn get_ready<T: 'static>(&mut self, ready: Resource<Task>) -> wasmtime::Result<T> {
            let value = match &mut self.table.get_mut(&ready)?.state {
                TaskState::HostReady(value) => *value.take().unwrap().downcast().unwrap(),
                _ => bail!("unexpected task state"),
            }?;

            self.drop(ready);

            Ok(value)
        }

        pub async fn await_ready<S: wasmtime::AsContextMut>(
            mut store: S,
            pending: Resource<Task>,
        ) -> wasmtime::Result<String>
        where
            <S as wasmtime::AsContext>::Data: IsyswasfaView + Send,
        {
            let state = |store| {
                &mut store
                    .as_context_mut()
                    .data_mut()
                    .table()
                    .get_mut(ready)
                    .state
            };

            let guest_pending = |store| state(store).guest_pending();

            let isyswasfa = |store| store.as_context_mut().data_mut().isyswasfa();

            let drop = |store, handle| isyswasfa(store).drop(handle);

            let mut result = None;
            let input = HashMap::new();
            loop {
                for poll in polls {
                    let output = poll
                        .call_async(
                            store.as_context_mut(),
                            (&input.remove(poll).unwrap_or_else(Vec::new),),
                        )
                        .await?
                        .0;
                    poll.post_return_async(store.as_context_mut()).await?;

                    for output in output {
                        match output {
                            PollOutput::Ready(PollOutputReady { state, ready }) => {
                                if read.rep() == pending.rep() {
                                    *state(&mut store) =
                                        TaskState::GuestReady { guest_state: state };

                                    result = Some(ready);
                                } else if let Some(listen) = guest_pending(&mut store).listen {
                                    input.entry(listen.poll).or_default().push(PollInput::Ready(
                                        PollInputReady {
                                            state: listen.state,
                                            ready,
                                        },
                                    ));
                                } else {
                                    guest_pending(&mut store).ready = Some(StatePair {
                                        state,
                                        handle: ready,
                                    });
                                }
                            }
                            PollOutput::Listen(PollOutputListen { state, pending }) => {
                                let task = task(&mut store, pending);
                                task.listen = Some(Listen { state, poll });

                                match task.state {
                                    TaskState::HostPending => input.entry(poll).or_default().push(
                                        PollInput::Listening(PollInputListening {
                                            state: state,
                                            cancel: pending,
                                        }),
                                    ),
                                    TaskState::HostReady(_) => input.entry(poll).or_default().push(
                                        PollInput::Ready(PollInputReady {
                                            state: state,
                                            ready: pending,
                                        }),
                                    ),
                                    TaskState::GuestPending {
                                        pending: Some(_), ..
                                    } => {
                                        input.entry(poll).or_default().push(PollInput::Listening(
                                            PollInputListening {
                                                state: state,
                                                cancel: pending,
                                            },
                                        ));
                                    }
                                    TaskState::GuestPending { .. } => {
                                        drop(&mut store, pending);
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            PollOutput::Pending(PollOutputPending { state, cancel }) => {
                                if cancel.rep() == pending.rep() {
                                    drop(&mut store, cancel);
                                } else {
                                    guest_pending(&mut store, &cancel).on_cancel =
                                        Some(OnCancel { state, poll });

                                    if let Some(listen) = guest_pending(&mut store, &cancel).listen
                                    {
                                        input.entry(listen.poll).or_default().push(
                                            PollInput::Listening(PollInputListening {
                                                state: listen.state,
                                                cancel,
                                            }),
                                        );
                                    } else {
                                        guest_pending(&mut store).pending = Some(StatePair {
                                            state,
                                            handle: cancel,
                                        });
                                    }
                                }
                            }
                            PollOutput::Cancel(cancel) => {
                                let task = task(&mut store, &cancel);
                                let listen = task.listen.unwrap();

                                let cancel_host_task = |store, cancel| {
                                    drop(&mut store, cancel);
                                    input
                                        .entry(listen.poll)
                                        .or_default()
                                        .push(PollInput::CancelComplete(listen.state));
                                };

                                match task.state {
                                    TaskState::HostPending => {
                                        todo!("how do we remove the correct entry from FuturesUnordered?");
                                        cancel_host_task(&mut store, cancel);
                                    }
                                    TaskState::HostReady(_) => {
                                        cancel_host_task(&mut store, cancel);
                                    }
                                    TaskState::GuestPending { on_cancel, .. } => {
                                        let on_cancel = on_cancel.unwrap();

                                        input.entry(on_cancel.poll).or_default().push(
                                            PollInput::Cancel(PollInputCancel {
                                                state: listen.state,
                                                cancel,
                                            }),
                                        );
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            PollOutput::CancelComplete(cancel) => {
                                let listen = guest_pending(&mut store).listen.unwrap();

                                input
                                    .entry(listen.poll)
                                    .or_default()
                                    .push(PollInput::CancelComplete(listen.state));
                            }
                        }
                    }
                }

                if input.is_empty() {
                    if let Some(ready) = result.take() {
                        drop(&mut store, pending);

                        break Ok(ready);
                    } else {
                        let (task, result) = isyswasfa(&store).futures.next().await;

                        *state(&mut store, Resource::new_owned(task)) =
                            TaskState::HostReady(result);
                    }
                }
            }
        }
    }

    pub trait IsyswasfaView {
        type State: 'static;

        fn table(&mut self) -> &mut ResourceTable;
        fn isyswasfa(&mut self) -> &mut IsyswasfaCtx;
        fn state(&self) -> Self::State;
    }
}

async fn build_component(src_path: &str, name: &str) -> Result<Vec<u8>> {
    if Command::new("cargo")
        .current_dir(src_path)
        .args(["component", "build"])
        .status()
        .await?
        .success()
    {
        Ok(fs::read(format!("../target/wasm32-wasi/debug/{name}.wasm")).await?)
    } else {
        Err(anyhow!("cargo build failed"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    struct Ctx {
        wasi: WasiCtx,
        isyswasfa: IsyswasfaCtx,
    }

    impl WasiView for Ctx {
        fn table(&mut self) -> &mut ResourceTable {
            self.isyswasfa.table()
        }
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.wasi
        }
    }

    impl IsyswasfaView for Ctx {
        type State = ();

        fn table(&mut self) -> &mut ResourceTable {
            self.isyswasfa.table()
        }
        fn isyswasfa(&mut self) -> &mut IsyswasfaCtx {
            &mut self.isyswasfa
        }
        fn state(&self) -> Self::State {}
    }

    #[async_trait]
    impl isyswasfa_bindings::component::guest::original_interface_async::Host for Ctx {
        async fn foo(_state: (), s: String) -> wasmtime::Result<String> {
            // todo: make this await a `oneshot::Receiver` instead
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(format!("{s} - entered host - exited host"))
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::HostPending for Ctx {
        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::HostCancel for Ctx {
        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::HostReady for Ctx {
        fn state(&mut self, this: Resource<Task>) -> wasmtime::Result<u32> {
            self.isyswasfa().guest_state(this)
        }

        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::Host for Ctx {
        fn make_task(
            &mut self,
        ) -> wasmtime::Result<(Resource<Task>, Resource<Task>, Resource<Task>)> {
            self.isyswasfa().guest_pending()
        }
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    let component = Component::new(&engine, build_component("../guest", "guest").await?)?;

    let mut linker = Linker::new(&engine);

    command::add_to_linker(&mut linker)?;

    OriginalWorldAsync::add_to_linker(&mut linker, |ctx| ctx)?;

    let mut store = Store::new(
        &engine,
        Ctx {
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            isyswasfa: IsyswasfaCtx::new(),
        },
    );

    let (command, _) =
        isyswasfa_bindings::OriginalWorldAsync::instantiate_async(&mut store, &component, &linker)
            .await?;

    let value = command
        .component_guest_original_interface_async()
        .call_foo(&mut store, "hello, world!")
        .await?;

    println!("result is: {value}");

    assert_eq!(
        "hello, world! - entered guest - entered host - exited host - exited guest",
        &value
    );

    Ok(())
}
