//! WebSocket method contracts, independent of the RPC transport.
use serde::{Serialize, de::DeserializeOwned};

mod types;
pub use types::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Empty {}

/// Encodes an absent Effect Void result as JSON null.
pub type Void = ();

fn deserialize_null<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    <() as serde::Deserialize>::deserialize(deserializer)
}

pub trait RpcMethod {
    type Payload: Serialize + DeserializeOwned;
    type Success: Serialize + DeserializeOwned;
    type Error: Serialize + DeserializeOwned;
    const TAG: &'static str;
    const STREAMS: bool;
    /// Incremental client guard; None does not imply public server access.
    const REQUIRED_SCOPE: Option<crate::auth::AuthEnvironmentScope>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsMethodDescriptor {
    pub tag: &'static str,
    pub streams: bool,
    pub required_scope: Option<crate::auth::AuthEnvironmentScope>,
}

impl WsMethodDescriptor {
    pub const fn of<M: RpcMethod>() -> Self {
        Self {
            tag: M::TAG,
            streams: M::STREAMS,
            required_scope: M::REQUIRED_SCOPE,
        }
    }
}

macro_rules! methods {
    ($($name:ident => ($tag:literal, $payload:ty, $success:ty, $error:ty, $stream:literal, $scope:expr)),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
            pub struct $name;
            impl RpcMethod for $name {
                type Payload = $payload;
                type Success = $success;
                type Error = $error;
                const TAG: &'static str = $tag;
                const STREAMS: bool = $stream;
                const REQUIRED_SCOPE: Option<crate::auth::AuthEnvironmentScope> = $scope;
            }
        )+
        pub const WS_METHODS: &[WsMethodDescriptor] = &[$(WsMethodDescriptor::of::<$name>(),)+];
    };
}

mod methods;
pub use methods::*;

/// Methods with no TypeScript counterpart, so absent from the generated `fixtures/rpc.json`.
/// `vcs.trustRepository`: the Rust server refuses a folder whose Git repository changed after it
/// was bound (`checkpoint.git.repository-changed`, t3-git H-1), which TypeScript never does; this is
/// the user's explicit way out of that refusal.
pub const RUST_ONLY_METHODS: &[&str] = &["vcs.trustRepository"];

pub struct WsRpcGroup;
impl WsRpcGroup {
    pub const METHODS: &'static [WsMethodDescriptor] = WS_METHODS;
}
