use {
    anyhow::{anyhow, Result},
    async_trait::async_trait,
    isyswasfa_host::{IsyswasfaCtx, IsyswasfaView},
    std::time::Duration,
    tokio::{fs, process::Command},
    wasmtime::{
        component::{Component, Linker, ResourceTable},
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
                        isyswasfa_host::{self, IsyswasfaView},
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
                                        isyswasfa_host::await_ready(&mut store, pending).await?;
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
        super::isyswasfa::isyswasfa::isyswasfa::{
            Host, HostCancel, HostPending, HostReady, PollInput, PollInputCancel,
            PollInputListening, PollInputReady, PollOutput, PollOutputListen, PollOutputPending,
            PollOutputReady,
        },
        anyhow::{anyhow, bail},
        futures::{
            channel::oneshot,
            future::{self, Either, FutureExt, Select},
            stream::{FuturesUnordered, ReadyChunks, StreamExt},
        },
        once_cell::sync::Lazy,
        std::{
            any::Any,
            collections::HashMap,
            future::Future,
            pin::Pin,
            sync::Arc,
            task::{Context, Poll, Wake, Waker},
        },
        wasmparser::{ComponentExternalKind, Parser, Payload},
        wasmtime::{
            component::{Instance, Resource, ResourceTable, TypedFunc},
            StoreContextMut,
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

    #[derive(Copy, Clone)]
    struct StatePoll {
        state: u32,
        poll: usize,
    }

    type BoxFuture = Pin<Box<dyn Future<Output = (u32, Box<dyn Any + Send>)> + Send + 'static>>;

    pub struct Task {
        reference_count: u32,
        state: TaskState,
        listen: Option<StatePoll>,
    }

    pub struct GuestPending {
        on_cancel: Option<StatePoll>,
    }

    pub enum TaskState {
        HostPending(Option<oneshot::Sender<()>>),
        HostReady(Option<Box<dyn Any + Send>>),
        GuestPending(GuestPending),
        GuestReady(u32),
    }

    type PollFunc = TypedFunc<(Vec<PollInput>,), (Vec<PollOutput>,)>;

    pub struct IsyswasfaCtx {
        table: ResourceTable,
        futures: ReadyChunks<FuturesUnordered<Select<oneshot::Receiver<()>, BoxFuture>>>,
        polls: Vec<PollFunc>,
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
                futures: FuturesUnordered::new().ready_chunks(1024),
                polls: Vec::new(),
            }
        }

        pub fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }

        fn guest_pending(
            &mut self,
        ) -> wasmtime::Result<(Resource<Task>, Resource<Task>, Resource<Task>)> {
            let pending = self.table.push(Task {
                reference_count: 3,
                state: TaskState::GuestPending(GuestPending { on_cancel: None }),
                listen: None,
            })?;
            let cancel = Resource::new_own(pending.rep());
            let ready = Resource::new_own(pending.rep());

            Ok((pending, cancel, ready))
        }

        pub fn first_poll<T: Send + 'static>(
            &mut self,
            future: impl Future<Output = wasmtime::Result<T>> + Send + 'static,
        ) -> wasmtime::Result<Result<T, Resource<Task>>> {
            let (tx, rx) = oneshot::channel();
            let task = self.table.push(Task {
                reference_count: 1,
                state: TaskState::HostPending(Some(tx)),
                listen: None,
            })?;
            let rep = task.rep();
            let mut future =
                Box::pin(future.map(move |v| (rep, Box::new(v) as Box<dyn Any + Send>)))
                    as BoxFuture;

            Ok(
                match future
                    .as_mut()
                    .poll(&mut Context::from_waker(&dummy_waker()))
                {
                    Poll::Ready((_, result)) => {
                        self.drop(task)?;
                        Ok((*result.downcast::<wasmtime::Result<T>>().unwrap())?)
                    }
                    Poll::Pending => {
                        self.futures.get_mut().push(future::select(rx, future));
                        Err(task)
                    }
                },
            )
        }

        fn guest_state(&self, ready: Resource<Task>) -> wasmtime::Result<u32> {
            match &self.table.get(&ready)?.state {
                TaskState::GuestReady(guest_state) => Ok(*guest_state),
                _ => Err(anyhow!("unexpected task state")),
            }
        }

        fn drop(&mut self, handle: Resource<Task>) -> wasmtime::Result<()> {
            let task = self.table.get_mut(&handle)?;
            task.reference_count = task.reference_count.checked_sub(1).unwrap();
            if task.reference_count == 0 {
                self.table.delete(handle)?;
            }
            Ok(())
        }

        pub fn get_ready<T: 'static>(&mut self, ready: Resource<Task>) -> wasmtime::Result<T> {
            let value = match &mut self.table.get_mut(&ready)?.state {
                TaskState::HostReady(value) => *value.take().unwrap().downcast().unwrap(),
                _ => bail!("unexpected task state"),
            };

            self.drop(ready)?;

            value
        }
    }

    fn task<'a, T: IsyswasfaView + Send>(
        store: &'a mut StoreContextMut<'a, T>,
        handle: &Resource<Task>,
    ) -> wasmtime::Result<&'a mut Task> {
        Ok(store.data_mut().isyswasfa().table().get_mut(handle)?)
    }

    fn state<'a, T: IsyswasfaView + Send>(
        store: &'a mut StoreContextMut<'a, T>,
        handle: &Resource<Task>,
    ) -> wasmtime::Result<&'a mut TaskState> {
        Ok(&mut task(store, handle)?.state)
    }

    fn guest_pending<'a, T: IsyswasfaView + Send>(
        store: &'a mut StoreContextMut<'a, T>,
        handle: &Resource<Task>,
    ) -> wasmtime::Result<&'a mut GuestPending> {
        Ok::<_, wasmtime::Error>(match state(store, handle)? {
            TaskState::GuestPending(pending) => pending,
            _ => unreachable!(),
        })
    }

    fn isyswasfa<'a, T: IsyswasfaView + Send>(
        store: &'a mut StoreContextMut<'a, T>,
    ) -> &'a mut IsyswasfaCtx {
        store.data_mut().isyswasfa()
    }

    fn drop<'a, T: IsyswasfaView + Send>(
        store: &'a mut StoreContextMut<'a, T>,
        handle: Resource<Task>,
    ) -> wasmtime::Result<()> {
        isyswasfa(store).drop(handle)
    }

    pub fn load_poll_funcs<S: wasmtime::AsContextMut>(
        mut store: S,
        component: &[u8],
        instance: &Instance,
    ) -> wasmtime::Result<()>
    where
        <S as wasmtime::AsContext>::Data: IsyswasfaView + Send,
    {
        let mut names = Vec::new();
        for payload in Parser::new(0).parse_all(component) {
            if let Payload::ComponentExportSection(reader) = payload? {
                for export in reader {
                    let export = export?;
                    if let ComponentExternalKind::Func = export.kind {
                        if export.name.0.starts_with("isyswasfa-poll") {
                            names.push(export.name.0);
                        }
                    }
                }
            }
        }

        if names.is_empty() {
            bail!("unable to find any function exports with names starting with `isyswasfa-poll`");
        }

        let polls = {
            let mut store = store.as_context_mut();
            let mut exports = instance.exports(&mut store);
            let mut exports = exports.root();
            names
                .into_iter()
                .map(|name| exports.typed_func::<(Vec<PollInput>,), (Vec<PollOutput>,)>(name))
                .collect::<wasmtime::Result<_>>()?
        };

        store.as_context_mut().data_mut().isyswasfa().polls = polls;

        Ok(())
    }

    pub async fn await_ready<S: wasmtime::AsContextMut>(
        mut store: S,
        pending: Resource<Task>,
    ) -> wasmtime::Result<Resource<Task>>
    where
        <S as wasmtime::AsContext>::Data: IsyswasfaView + Send,
    {
        let polls = isyswasfa(&mut store.as_context_mut()).polls.clone();

        let mut result = None;
        let mut input = HashMap::new();
        loop {
            for (index, poll) in polls.iter().enumerate() {
                let output = poll
                    .call_async(
                        store.as_context_mut(),
                        (input.remove(&index).unwrap_or_else(Vec::new),),
                    )
                    .await?
                    .0;
                poll.post_return_async(store.as_context_mut()).await?;

                for output in output {
                    match output {
                        PollOutput::Ready(PollOutputReady {
                            state: guest_state,
                            ready,
                        }) => {
                            if ready.rep() == pending.rep() {
                                *state(&mut store.as_context_mut(), &ready)? =
                                    TaskState::GuestReady(guest_state);

                                result = Some(ready);
                            } else if let Some(listen) =
                                task(&mut store.as_context_mut(), &ready)?.listen
                            {
                                input.entry(listen.poll).or_default().push(PollInput::Ready(
                                    PollInputReady {
                                        state: listen.state,
                                        ready,
                                    },
                                ));
                            } else {
                                let tmp = Resource::new_own(ready.rep());
                                *state(&mut store.as_context_mut(), &tmp)? =
                                    TaskState::GuestReady(guest_state);
                            }
                        }
                        PollOutput::Listen(PollOutputListen { state, pending }) => {
                            let context = &mut store.as_context_mut();
                            let task = task(context, &pending)?;
                            task.listen = Some(StatePoll { state, poll: index });

                            match task.state {
                                TaskState::HostPending(_) => input.entry(index).or_default().push(
                                    PollInput::Listening(PollInputListening {
                                        state,
                                        cancel: pending,
                                    }),
                                ),
                                TaskState::HostReady(_) => input.entry(index).or_default().push(
                                    PollInput::Ready(PollInputReady {
                                        state,
                                        ready: pending,
                                    }),
                                ),
                                TaskState::GuestPending(GuestPending { on_cancel: Some(_) }) => {
                                    input.entry(index).or_default().push(PollInput::Listening(
                                        PollInputListening {
                                            state,
                                            cancel: pending,
                                        },
                                    ));
                                }
                                TaskState::GuestPending(_) => {
                                    drop(&mut store.as_context_mut(), pending)?;
                                }
                                TaskState::GuestReady(_) => {
                                    input.entry(index).or_default().push(PollInput::Ready(
                                        PollInputReady {
                                            state,
                                            ready: pending,
                                        },
                                    ));
                                }
                            }
                        }
                        PollOutput::Pending(PollOutputPending { state, cancel }) => {
                            if cancel.rep() == pending.rep() {
                                drop(&mut store.as_context_mut(), cancel)?;
                            } else {
                                guest_pending(&mut store.as_context_mut(), &cancel)?.on_cancel =
                                    Some(StatePoll { state, poll: index });

                                if let Some(listen) =
                                    task(&mut store.as_context_mut(), &cancel)?.listen
                                {
                                    input.entry(listen.poll).or_default().push(
                                        PollInput::Listening(PollInputListening {
                                            state: listen.state,
                                            cancel,
                                        }),
                                    );
                                }
                            }
                        }
                        PollOutput::Cancel(cancel) => {
                            let context = &mut store.as_context_mut();
                            let task = task(context, &cancel)?;
                            let listen = task.listen.unwrap();

                            let mut cancel_host_task = |store, cancel| {
                                drop(store, cancel)?;
                                input
                                    .entry(listen.poll)
                                    .or_default()
                                    .push(PollInput::CancelComplete(listen.state));
                                Ok::<_, wasmtime::Error>(())
                            };

                            match &mut task.state {
                                TaskState::HostPending(cancel_tx) => {
                                    cancel_tx.take();
                                    cancel_host_task(&mut store.as_context_mut(), cancel)?;
                                }
                                TaskState::HostReady(_) => {
                                    cancel_host_task(&mut store.as_context_mut(), cancel)?;
                                }
                                TaskState::GuestPending(GuestPending { on_cancel, .. }) => {
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
                            let listen =
                                task(&mut store.as_context_mut(), &cancel)?.listen.unwrap();

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
                    drop(&mut store.as_context_mut(), pending)?;

                    break Ok(ready);
                } else if let Some(values) =
                    isyswasfa(&mut store.as_context_mut()).futures.next().await
                {
                    for value in values {
                        match value {
                            Either::Left(_) => {}
                            Either::Right(((task_rep, result), _)) => {
                                let ready = Resource::new_own(task_rep);
                                let context = &mut store.as_context_mut();
                                let task = task(context, &ready)?;
                                task.reference_count += 1;
                                task.state = TaskState::HostReady(Some(result));

                                if let Some(listen) = task.listen {
                                    input.entry(listen.poll).or_default().push(PollInput::Ready(
                                        PollInputReady {
                                            state: listen.state,
                                            ready,
                                        },
                                    ));
                                }
                            }
                        }
                    }
                } else {
                    bail!("guest task is pending with no pending host tasks");
                }
            }
        }
    }

    pub trait IsyswasfaView {
        type State: 'static;

        fn isyswasfa(&mut self) -> &mut IsyswasfaCtx;
        fn state(&self) -> Self::State;
    }

    impl<T: IsyswasfaView> HostPending for T {
        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl<T: IsyswasfaView> HostCancel for T {
        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl<T: IsyswasfaView> HostReady for T {
        fn state(&mut self, this: Resource<Task>) -> wasmtime::Result<u32> {
            self.isyswasfa().guest_state(this)
        }

        fn drop(&mut self, this: Resource<Task>) -> wasmtime::Result<()> {
            self.isyswasfa().drop(this)
        }
    }

    impl<T: IsyswasfaView> Host for T {
        fn make_task(
            &mut self,
        ) -> wasmtime::Result<(Resource<Task>, Resource<Task>, Resource<Task>)> {
            self.isyswasfa().guest_pending()
        }
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
        Ok(fs::read(format!("{src_path}/target/wasm32-wasi/debug/{name}.wasm")).await?)
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

        fn isyswasfa(&mut self) -> &mut IsyswasfaCtx {
            &mut self.isyswasfa
        }
        fn state(&self) -> Self::State {}
    }

    #[async_trait]
    impl isyswasfa_bindings::component::guest::original_interface_async::Host for Ctx {
        async fn foo(_state: (), s: String) -> wasmtime::Result<String> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(format!("{s} - entered host - exited host"))
        }
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    let component_bytes = build_component("../guest", "guest").await?;

    let component = Component::new(&engine, &component_bytes)?;

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

    let (command, instance) =
        isyswasfa_bindings::OriginalWorldAsync::instantiate_async(&mut store, &component, &linker)
            .await?;

    isyswasfa_host::load_poll_funcs(&mut store, &component_bytes, &instance)?;

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
