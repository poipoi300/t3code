use crate::registry::HandlerRegistry;
use t3_contracts::rpc::*;
pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsServerUpdateServerRpc>()?;
    registry.unimplemented::<WsServerUpdateServerWithProgressRpc>()?;
    registry.unimplemented::<WsServerCommitDesktopUpdateRpc>()?;
    Ok(())
}
