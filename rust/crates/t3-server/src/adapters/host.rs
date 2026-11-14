//! The only contact surface with `t3-host`; compiled with the `host` feature.
//!
//! Background policy leases (`background/BackgroundPolicy.ts`): clients report their activity
//! per socket, and the leases of a socket are dropped when it closes.
use std::sync::Arc;
use t3_contracts::base_schemas::{AuthSessionId, RpcClientId};
use t3_host::background::{BackgroundPolicy, Clock, HostPowerMonitor, SystemClock};

pub struct HostAdapter {
    pub background: BackgroundPolicy,
}

impl HostAdapter {
    /// Host power starts unknown and stale: no native power source is assembled, so host
    /// constraints never pause work (TypeScript treats a stale snapshot the same way).
    pub fn start() -> crate::Result<Arc<Self>> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let power = HostPowerMonitor::unknown(clock.now_ms())
            .map_err(|e| crate::Error::Service(e.to_string()))?;
        Ok(Arc::new(Self {
            background: BackgroundPolicy::new(clock, Arc::new(power)),
        }))
    }

    /// The RPC client id of a socket: one per connection, as Effect's RPC client id is.
    pub fn rpc_client(socket: u64) -> Option<RpcClientId> {
        serde_json::from_value(serde_json::json!(socket)).ok()
    }

    /// Drops every lease the socket reported; called when it closes.
    pub async fn socket_closed(&self, session: &AuthSessionId, socket: u64) {
        if let Some(client) = Self::rpc_client(socket) {
            self.background.remove_rpc_client(session, &client).await;
        }
    }
}
