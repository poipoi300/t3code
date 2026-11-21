use crate::registry::HandlerRegistry;
#[cfg(feature = "local-services")]
use crate::registry::{Failure, ServerDefect};
use t3_contracts::rpc::*;

/// Methods this module serves. Every other `C-rpc-core` method waits for a service that is not
/// ported yet and stays in `unsupported.rs`.
pub const REGISTERED: &[&str] = &[
    WsServerProbeRpc::TAG,
    WsServerGetConfigRpc::TAG,
    WsSubscribeServerConfigRpc::TAG,
    WsSubscribeAuthAccessRpc::TAG,
    WsSubscribeServerLifecycleRpc::TAG,
    WsServerUpsertKeybindingRpc::TAG,
    WsServerRemoveKeybindingRpc::TAG,
    WsServerUpdateSettingsRpc::TAG,
];

pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.register::<WsServerProbeRpc, _, _>(|_, _| async { Ok(Empty {}) })?;
    #[cfg(feature = "local-services")]
    {
        use futures::StreamExt;
        registry.register::<WsServerGetConfigRpc, _, _>(|ctx, _| async move {
            #[cfg(feature = "providers")]
            return crate::adapters::providers::config_for(&ctx.services, ctx.secret_references)
                .await
                .map_err(Failure::Typed);
            #[cfg(not(feature = "providers"))]
            ctx.services
                .local
                .get_config_for(ctx.secret_references)
                .await
                .map_err(internal)
        })?;
        registry.register_stream::<WsSubscribeServerConfigRpc, _, _, _>(
            |ctx, input| async move {
                #[cfg(feature = "providers")]
                return Ok(crate::adapters::providers::config_stream_for(
                    ctx.services,
                    ctx.secret_references,
                    input.environment_themes.flatten() == Some(true),
                )
                .map(|v| v.map_err(Failure::Typed)));
                #[cfg(not(feature = "providers"))]
                Ok(crate::adapters::config::config_stream_for(
                    ctx.services,
                    ctx.secret_references,
                    input.environment_themes.flatten() == Some(true),
                )
                .map(|v| v.map_err(internal)))
            },
        )?;
        registry.register_stream::<WsSubscribeAuthAccessRpc, _, _, _>(|ctx, _| async move {
            Ok(
                crate::adapters::auth::access_stream(ctx.services, ctx.session_id)
                    .map(|v| v.map_err(internal)),
            )
        })?;
        registry.register_stream::<WsSubscribeServerLifecycleRpc, _, _, _>(
            |ctx, _| async move {
                Ok(crate::adapters::config::lifecycle_stream(ctx.services)
                    .map(|v| v.map_err(internal)))
            },
        )?;
        registry.register::<WsServerUpsertKeybindingRpc, _, _>(|ctx, input| async move {
            ctx.services
                .local
                .upsert_keybinding(input)
                .await
                .map_err(Failure::Typed)
        })?;
        registry.register::<WsServerRemoveKeybindingRpc, _, _>(|ctx, input| async move {
            ctx.services
                .local
                .remove_keybinding(input)
                .await
                .map_err(Failure::Typed)
        })?;
        registry.register::<WsServerUpdateSettingsRpc, _, _>(|ctx, input| async move {
            ctx.services
                .local
                .update_settings(input, ctx.secret_references)
                .await
                .map_err(|e| Failure::Typed(WsServerRunStorageCleanupError::ServerSettingsError(e)))
        })?;
    }
    #[cfg(not(feature = "local-services"))]
    {
        registry.unimplemented::<WsServerGetConfigRpc>()?;
        registry.unimplemented::<WsSubscribeServerConfigRpc>()?;
        registry.unimplemented::<WsSubscribeAuthAccessRpc>()?;
        registry.unimplemented::<WsSubscribeServerLifecycleRpc>()?;
        registry.unimplemented::<WsServerUpsertKeybindingRpc>()?;
        registry.unimplemented::<WsServerRemoveKeybindingRpc>()?;
        registry.unimplemented::<WsServerUpdateSettingsRpc>()?;
    }
    Ok(())
}

#[cfg(feature = "local-services")]
fn internal<E>(e: crate::Error) -> Failure<E> {
    Failure::Defect(ServerDefect::Internal {
        message: e.to_string(),
    })
}
