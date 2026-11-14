//! Package A: the device RPCs over the one `DeviceRuntime` (`ws.ts` device block). The RPC
//! guard has already applied each method's scope, `device.list`'s effective one included.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

#[cfg(feature = "device")]
mod live {
    use super::*;
    use crate::registry::{Failure, ServerDefect};
    use t3_device::Error;

    fn internal<E>(error: Error) -> Failure<E> {
        Failure::Defect(ServerDefect::Internal {
            message: error.to_string(),
        })
    }
    fn device(error: Error) -> Failure<WsDeviceTestHostError> {
        match error {
            Error::Device { wire, .. } => Failure::Typed(WsDeviceTestHostError::DeviceError(wire)),
            other => internal(other),
        }
    }

    pub fn register(
        registry: &mut HandlerRegistry,
        adapter: std::sync::Arc<crate::adapters::device::DeviceAdapter>,
    ) -> crate::Result<()> {
        macro_rules! unary {
            ($rpc:ty, |$s:ident, $input:ident| $call:expr) => {{
                let a = adapter.clone();
                registry.register::<$rpc, _, _>(move |_, $input| {
                    let a = a.clone();
                    async move {
                        let $s = &a.runtime.service;
                        $call.await.map_err(device)
                    }
                })?;
            }};
        }
        unary!(WsDeviceTestHostRpc, |s, input| s.test_host(input));
        unary!(WsDeviceListRpc, |s, input| async move {
            s.list_request(input)
                .await
                .map(t3_device::service::wire_state)
        });
        unary!(WsDeviceConfigureRpc, |s, input| async move {
            s.configure(input).await.map(t3_device::service::wire_state)
        });
        unary!(WsDeviceCloseRpc, |s, input| s.close(input));
        unary!(WsDeviceShutdownRpc, |s, input| s.shutdown(input));
        unary!(WsDeviceDetailRpc, |s, input| s.detail(input));
        unary!(WsDeviceActionRpc, |s, input| s.action(input));
        let a = adapter.clone();
        registry.register::<WsDeviceOpenRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                let context = a.open_context(&input.thread_id.0).await;
                a.runtime.service.open(input, context).await.map_err(device)
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsDeviceEasAccountRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.runtime
                    .service
                    .eas_account(input)
                    .await
                    .map_err(|error| match error {
                        Error::EasAccount(error) => Failure::Typed(
                            WsDeviceEasAccountError::DeviceEasAccountError(Box::new(error)),
                        ),
                        other => internal(other),
                    })
            }
        })?;
        let a = adapter.clone();
        registry.register::<WsDeviceSimlockIdentityRpc, _, _>(move |_, input| {
            let a = a.clone();
            async move {
                a.runtime
                    .service
                    .simlock_identity(input)
                    .await
                    .map_err(|error| match error {
                        Error::SimlockIdentity(error) => Failure::Typed(
                            WsDeviceSimlockIdentityError::DeviceSimlockIdentityError(Box::new(
                                error,
                            )),
                        ),
                        other => internal(other),
                    })
            }
        })?;
        // Already split into hosts/easHosts/simlockHosts; dropping the stream unsubscribes.
        let a = adapter;
        registry.register_stream::<WsSubscribeDeviceStateRpc, _, _, _>(move |_, _| {
            let runtime = a.runtime.clone();
            // `state_stream` borrows the runtime, so the stream owns its clone.
            async move {
                Ok(async_stream::stream! {
                    use futures::StreamExt;
                    let states = runtime.state_stream().await;
                    futures::pin_mut!(states);
                    while let Some(state) = states.next().await {
                        yield Ok(state);
                    }
                })
            }
        })?;
        Ok(())
    }
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "device")]
    if let Some(adapter) = domains.get::<crate::adapters::device::DeviceAdapter>() {
        return live::register(registry, adapter);
    }
    let _ = domains;
    registry.unimplemented::<WsDeviceTestHostRpc>()?;
    registry.unimplemented::<WsDeviceEasAccountRpc>()?;
    registry.unimplemented::<WsDeviceSimlockIdentityRpc>()?;
    registry.unimplemented::<WsDeviceListRpc>()?;
    registry.unimplemented::<WsDeviceConfigureRpc>()?;
    registry.unimplemented::<WsDeviceOpenRpc>()?;
    registry.unimplemented::<WsDeviceCloseRpc>()?;
    registry.unimplemented::<WsDeviceShutdownRpc>()?;
    registry.unimplemented::<WsDeviceDetailRpc>()?;
    registry.unimplemented::<WsDeviceActionRpc>()?;
    registry.unimplemented::<WsSubscribeDeviceStateRpc>()
}
