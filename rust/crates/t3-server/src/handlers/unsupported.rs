use crate::registry::HandlerRegistry;
use t3_contracts::rpc::*;
pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsServerUpdateServerRpc>()?;
    registry.unimplemented::<WsServerUpdateServerWithProgressRpc>()?;
    registry.unimplemented::<WsServerCommitDesktopUpdateRpc>()?;
    registry.unimplemented::<WsServerRunStorageCleanupRpc>()?;
    registry.unimplemented::<WsServerGetStorageCleanupReportRpc>()?;
    registry.unimplemented::<WsPeerLinksListRpc>()?;
    registry.unimplemented::<WsPeerLinksLinkRpc>()?;
    registry.unimplemented::<WsPeerLinksUnlinkRpc>()?;
    registry.unimplemented::<WsPeerLinksAnswerRequestRpc>()?;
    registry.unimplemented::<WsServerCheckSecretSourceRpc>()?;
    registry.unimplemented::<WsShellOpenInEditorRpc>()?;
    registry.unimplemented::<WsGuestShareCreateRpc>()?;
    registry.unimplemented::<WsSecretsAnswerRequestRpc>()?;
    Ok(())
}
