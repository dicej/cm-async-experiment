use {
    anyhow::{anyhow, Result},
    async_trait::async_trait,
    isyswasfa::isyswasfa::isyswasfa::{Cancel, Pending, Ready},
    isyswasfa_host::{IsyswasfaCtx, IsyswasfaView},
    std::time::Duration,
    tokio::{fs, process::Command},
    wasmtime::{
        component::{Component, Linker, Resource},
        Config, Engine, Store,
    },
    wasmtime_wasi::preview2::{command, Table, WasiCtx, WasiCtxBuilder, WasiView},
};

wasmtime::component::bindgen!({
    path: "../guest/wit",
    async: {
        only_imports: []
    },
    with: {
        "isyswasfa:isyswasfa/isyswasfa/ready": MyReady,
        "isyswasfa:isyswasfa/isyswasfa/pending": MyPending,
        "isyswasfa:isyswasfa/isyswasfa/cancel": MyCancel,
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
        ) -> exports::component::guest::original_interface_async::OriginalInterfaceAsync {
            exports::component::guest::original_interface_async::OriginalInterfaceAsync(
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
                        isyswasfa::isyswasfa::isyswasfa::{Pending, Ready},
                        isyswasfa_host::IsyswasfaView,
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
                    ) -> wasmtime::Result<Result<String, Resource<Pending>>> {
                        let future = <T as Host>::foo(self.state(), s);
                        self.isyswasfa_mut().first_poll(future)
                    }

                    fn foo_result(&mut self, ready: Resource<Ready>) -> wasmtime::Result<String> {
                        self.isyswasfa_mut().get_ready(ready)
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
                        exports::component::guest::original_interface_async::OriginalInterfaceAsync as Interface,
                        isyswasfa_host::IsyswasfaView,
                    };

                    #[derive(Copy, Clone)]
                    pub struct OriginalInterfaceAsync<'a>(pub &'a Interface);

                    impl<'a> OriginalInterfaceAsync<'a> {
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
                                    let ready = store
                                        .as_context_mut()
                                        .data_mut()
                                        .isyswasfa_mut()
                                        .await_ready(pending)
                                        .await;

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
        super::isyswasfa::isyswasfa::isyswasfa::{Pending, Ready},
        std::future::Future,
        wasmtime::component::Resource,
    };

    pub struct IsyswasfaCtx;

    impl IsyswasfaCtx {
        pub fn first_poll<T: 'static>(
            &mut self,
            future: impl Future<Output = wasmtime::Result<T>> + 'static,
        ) -> wasmtime::Result<Result<T, Resource<Pending>>> {
            todo!()
        }

        pub fn get_ready<T: 'static>(&mut self, ready: Resource<Ready>) -> wasmtime::Result<T> {
            todo!()
        }

        pub async fn await_ready(&mut self, pending: Resource<Pending>) -> Resource<Ready> {
            todo!()
        }
    }

    pub trait IsyswasfaView {
        type State: 'static;

        fn isyswasfa(&self) -> &IsyswasfaCtx;
        fn isyswasfa_mut(&mut self) -> &mut IsyswasfaCtx;
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

pub struct MyPending;
pub struct MyReady {
    state: Option<u32>,
}
pub struct MyCancel;

#[tokio::main]
async fn main() -> Result<()> {
    struct Ctx {
        table: Table,
        wasi: WasiCtx,
        isyswasfa: IsyswasfaCtx,
    }

    impl WasiView for Ctx {
        fn table(&self) -> &Table {
            &self.table
        }
        fn table_mut(&mut self) -> &mut Table {
            &mut self.table
        }
        fn ctx(&self) -> &WasiCtx {
            &self.wasi
        }
        fn ctx_mut(&mut self) -> &mut WasiCtx {
            &mut self.wasi
        }
    }

    impl IsyswasfaView for Ctx {
        type State = ();

        fn isyswasfa(&self) -> &IsyswasfaCtx {
            &self.isyswasfa
        }
        fn isyswasfa_mut(&mut self) -> &mut IsyswasfaCtx {
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
        fn drop(&mut self, this: Resource<MyPending>) -> wasmtime::Result<()> {
            Ok(self.table_mut().delete(this).map(|_| ())?)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::HostCancel for Ctx {
        fn drop(&mut self, this: Resource<MyCancel>) -> wasmtime::Result<()> {
            Ok(self.table_mut().delete(this).map(|_| ())?)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::HostReady for Ctx {
        fn state(&mut self, this: Resource<MyReady>) -> wasmtime::Result<u32> {
            Ok(self.table().get(&this)?.state.unwrap())
        }

        fn drop(&mut self, this: Resource<MyReady>) -> wasmtime::Result<()> {
            Ok(self.table_mut().delete(this).map(|_| ())?)
        }
    }

    impl isyswasfa::isyswasfa::isyswasfa::Host for Ctx {
        fn new(
            &mut self,
        ) -> wasmtime::Result<(Resource<Pending>, Resource<Cancel>, Resource<Ready>)> {
            Ok((
                self.table_mut().push(MyPending)?,
                self.table_mut().push(MyCancel)?,
                self.table_mut().push(MyReady { state: None })?,
            ))
        }
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);

    let engine = Engine::new(&config)?;

    let component = Component::new(&engine, &build_component("../guest", "guest").await?)?;

    let mut linker = Linker::new(&engine);

    command::add_to_linker(&mut linker)?;

    OriginalWorldAsync::add_to_linker(&mut linker, |ctx| ctx)?;

    let mut store = Store::new(
        &engine,
        Ctx {
            table: Table::new(),
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            isyswasfa: IsyswasfaCtx,
        },
    );

    let (command, _) =
        isyswasfa_bindings::OriginalWorldAsync::instantiate_async(&mut store, &component, &linker)
            .await?;

    let value = command
        .component_guest_original_interface_async()
        .call_foo(&mut store, &"hello, world!")
        .await?;

    println!("result is: {value}");

    assert_eq!(
        "hello, world! - entered guest - entered host - exited host - exited guest",
        &value
    );

    Ok(())
}
