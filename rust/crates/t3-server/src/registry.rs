use futures::{StreamExt, stream::BoxStream};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, future::Future, sync::Arc};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::AuthSessionId,
    rpc::{RpcMethod, WS_METHODS},
};
use t3_rpc::Exit;

#[derive(Clone)]
pub struct Context {
    pub services: Arc<crate::services::ServiceRegistry>,
    pub scopes: Vec<AuthEnvironmentScope>,
    pub session_id: AuthSessionId,
    /// The socket's `clientCapabilities` include `settingsSecretReferences`.
    pub secret_references: bool,
    /// The socket and request this call runs for; detached outside a WebSocket.
    pub call: CallScope,
}

type AckHook = Arc<dyn Fn() + Send + Sync>;

/// One WebSocket connection: an identity unique for the server's lifetime and the ACK hooks
/// of its live calls. Built by the WebSocket transport only.
pub struct SocketScope {
    id: u64,
    acks: std::sync::Mutex<std::collections::HashMap<String, AckHook>>,
}
impl SocketScope {
    pub(crate) fn new() -> Arc<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Arc::new(Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            acks: Default::default(),
        })
    }
    fn key(request: &t3_rpc::RequestId) -> String {
        serde_json::to_string(request).unwrap_or_default()
    }
    fn hooks(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, AckHook>> {
        self.acks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// A real client ACK for `request`'s latest chunk.
    pub(crate) fn acknowledged(&self, request: &t3_rpc::RequestId) {
        let hook = self.hooks().get(&Self::key(request)).cloned();
        if let Some(hook) = hook {
            hook();
        }
    }
    /// The call ended (finished, interrupted or dropped); its hook is released.
    pub(crate) fn release(&self, request: &t3_rpc::RequestId) {
        self.hooks().remove(&Self::key(request));
    }
}

/// The socket and request a handler runs for. `Default` is detached: no socket, no ACKs.
#[derive(Clone, Default)]
pub struct CallScope {
    socket: Option<Arc<SocketScope>>,
    request: Option<t3_rpc::RequestId>,
}
impl CallScope {
    pub(crate) fn socket(socket: Arc<SocketScope>) -> Self {
        Self {
            socket: Some(socket),
            request: None,
        }
    }
    pub(crate) fn for_request(&self, request: t3_rpc::RequestId) -> Self {
        Self {
            socket: self.socket.clone(),
            request: Some(request),
        }
    }
    pub(crate) fn socket_scope(&self) -> Arc<SocketScope> {
        self.socket.clone().unwrap_or_else(SocketScope::new)
    }
    /// Unique to the socket for the server's lifetime; `None` when detached.
    pub fn socket_id(&self) -> Option<u64> {
        self.socket.as_ref().map(|socket| socket.id)
    }
    /// Runs `on_ack` for each real client ACK of this call's stream chunks until the call
    /// ends. Returns false when detached, so a caller never waits on ACKs that cannot come.
    pub fn on_ack(&self, on_ack: impl Fn() + Send + Sync + 'static) -> bool {
        let (Some(socket), Some(request)) = (&self.socket, &self.request) else {
            return false;
        };
        socket
            .hooks()
            .insert(SocketScope::key(request), Arc::new(on_ack));
        true
    }
}
impl Context {
    fn authorize(&self, method: &str, payload: &Value) -> std::result::Result<(), Exit> {
        #[cfg(feature = "local-services")]
        {
            crate::adapters::auth::authorize(&self.scopes, method, payload)
        }
        #[cfg(not(feature = "local-services"))]
        {
            let _ = (method, payload);
            Err(Exit::die("Authentication service unavailable"))
        }
    }
}

/// Structured defects use Effect's Die channel, preserving each method's Fail union.
#[derive(Debug, Serialize)]
#[serde(tag = "_tag")]
pub enum ServerDefect {
    NotImplemented { method: String },
    InvalidPayload { method: String },
    Internal { message: String },
}
impl ServerDefect {
    pub fn exit(self) -> Exit {
        Exit::die(serde_json::to_value(self).unwrap_or(Value::Null))
    }
}
pub enum Failure<E> {
    Typed(E),
    Defect(ServerDefect),
}
impl<E: Serialize> Failure<E> {
    fn exit(self) -> Exit {
        match self {
            Self::Typed(e) => match serde_json::to_value(e) {
                Ok(e) => Exit::fail(e),
                Err(_) => Exit::die("Error serialization failed"),
            },
            Self::Defect(e) => e.exit(),
        }
    }
}
pub enum Response {
    Chunk(Vec<Value>),
    Exit(Exit),
}
pub type Work = BoxStream<'static, Response>;
type Handler = dyn Fn(Context, Value) -> std::result::Result<Work, Exit> + Send + Sync;
struct Entry {
    handler: Arc<Handler>,
    implemented: bool,
    streams: bool,
}
#[derive(Default)]
pub struct HandlerRegistry {
    entries: BTreeMap<&'static str, Entry>,
}
impl HandlerRegistry {
    fn insert<M: RpcMethod>(
        &mut self,
        handler: Arc<Handler>,
        implemented: bool,
    ) -> crate::Result<()> {
        if self.entries.contains_key(M::TAG) {
            return Err(crate::Error::Registry(format!("duplicate {}", M::TAG)));
        }
        self.entries.insert(
            M::TAG,
            Entry {
                handler,
                implemented,
                streams: M::STREAMS,
            },
        );
        Ok(())
    }
    pub fn register<M, F, Fut>(&mut self, handler: F) -> crate::Result<()>
    where
        M: RpcMethod + 'static,
        M::Payload: Send,
        M::Success: Send,
        M::Error: Send,
        F: Fn(Context, M::Payload) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<M::Success, Failure<M::Error>>> + Send + 'static,
    {
        if M::STREAMS {
            return Err(crate::Error::Registry(format!(
                "{} requires register_stream",
                M::TAG
            )));
        }
        let handler = Arc::new(handler);
        self.insert::<M>(
            Arc::new(move |ctx, payload| {
                let handler = handler.clone();
                let input = decode::<M>(&ctx, payload)?;
                Ok(futures::stream::once(async move {
                    Response::Exit(match handler(ctx, input).await {
                        Ok(value) => encode_success(value),
                        Err(e) => e.exit(),
                    })
                })
                .boxed())
            }),
            true,
        )
    }
    pub fn register_stream<M, F, Fut, S>(&mut self, handler: F) -> crate::Result<()>
    where
        M: RpcMethod + 'static,
        M::Payload: Send,
        M::Success: Send,
        M::Error: Send,
        F: Fn(Context, M::Payload) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<S, Failure<M::Error>>> + Send + 'static,
        S: futures::Stream<Item = std::result::Result<M::Success, Failure<M::Error>>>
            + Send
            + 'static,
    {
        if !M::STREAMS {
            return Err(crate::Error::Registry(format!("{} is unary", M::TAG)));
        }
        let handler = Arc::new(handler);
        self.insert::<M>(Arc::new(move |ctx, payload| {
            let handler = handler.clone();
            let input=decode::<M>(&ctx,payload)?;
            Ok(async_stream::stream! {
                match handler(ctx, input).await {
                    Err(e) => yield Response::Exit(e.exit()),
                    Ok(stream) => {
                        futures::pin_mut!(stream);
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(value) => match serde_json::to_value(value) {
                                    Ok(value) => yield Response::Chunk(vec![value]),
                                    Err(_) => { yield Response::Exit(Exit::die("Success serialization failed")); return; }
                                },
                                Err(e) => { yield Response::Exit(e.exit()); return; }
                            }
                        }
                        yield Response::Exit(Exit::success(Value::Null));
                    }
                }
            }.boxed())
        }), true)
    }
    pub fn unimplemented<M: RpcMethod + 'static>(&mut self) -> crate::Result<()>
    where
        M::Payload: Send,
    {
        self.insert::<M>(
            Arc::new(|ctx, payload| {
                decode::<M>(&ctx, payload)?;
                Err(ServerDefect::NotImplemented {
                    method: M::TAG.into(),
                }
                .exit())
            }),
            false,
        )
    }
    pub fn dispatch(
        &self,
        ctx: Context,
        tag: &str,
        payload: Value,
    ) -> std::result::Result<Work, Exit> {
        let Some((&method, entry)) = self.entries.get_key_value(tag) else {
            return Err(Exit::die("Unknown RPC method"));
        };
        #[cfg(feature = "local-services")]
        {
            use t3_telemetry::rpc::{self, Outcome, RequestMeasurement};
            let mut measurement = RequestMeasurement::new(method);
            let span = rpc::request_span(method);
            let mut work = match (entry.handler)(ctx, payload) {
                Ok(work) => work,
                Err(error) => {
                    measurement.complete(Outcome::Failure);
                    return Err(error);
                }
            };
            Ok(async_stream::stream! {
                while let Some(response)=rpc::poll(method,work.next(),span.clone()).await {
                    if let Response::Exit(exit)=&response {
                        let outcome=match exit {
                            Exit::Success {..}=>Outcome::Success,
                            Exit::Failure {cause} if !cause.is_empty()&&cause.iter().all(|c|matches!(c,t3_rpc::Cause::Interrupt{..}))=>Outcome::Interrupt,
                            _=>Outcome::Failure,
                        };
                        measurement.complete(outcome);
                    }
                    yield response;
                }
            }.boxed())
        }
        #[cfg(not(feature = "local-services"))]
        {
            let _ = method;
            (entry.handler)(ctx, payload)
        }
    }
    /// Every contract method is registered or explicitly unsupported exactly once, with the
    /// contract's call kind, and nothing else is present.
    pub fn validate(&self) -> crate::Result<()> {
        for method in WS_METHODS {
            match self.entries.get(method.tag) {
                None => {
                    return Err(crate::Error::Registry(format!("missing {}", method.tag)));
                }
                Some(e) if e.streams != method.streams => {
                    return Err(crate::Error::Registry(format!(
                        "kind mismatch {}",
                        method.tag
                    )));
                }
                Some(_) => {}
            }
        }
        if let Some(extra) = self
            .entries
            .keys()
            .find(|tag| !WS_METHODS.iter().any(|m| m.tag == **tag))
        {
            return Err(crate::Error::Registry(format!("extra {extra}")));
        }
        Ok(())
    }
    pub fn disposition(&self, tag: &str) -> Option<bool> {
        self.entries.get(tag).map(|e| e.implemented)
    }
}
fn decode<M: RpcMethod>(ctx: &Context, payload: Value) -> std::result::Result<M::Payload, Exit> {
    let decoded = serde_json::from_value(payload.clone()).map_err(|_| {
        ServerDefect::InvalidPayload {
            method: M::TAG.into(),
        }
        .exit()
    })?;
    ctx.authorize(M::TAG, &payload)?;
    Ok(decoded)
}
fn encode_success(value: impl Serialize) -> Exit {
    match serde_json::to_value(value) {
        Ok(v) => Exit::success(v),
        Err(_) => Exit::die("Success serialization failed"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use t3_contracts::rpc::{WsServerProbeRpc, WsSubscribeServerConfigRpc};

    struct ExtraUnary;
    impl RpcMethod for ExtraUnary {
        type Payload = Value;
        type Success = Value;
        type Error = Value;
        const TAG: &'static str = "test.extra";
        const STREAMS: bool = false;
        const REQUIRED_SCOPE: Option<AuthEnvironmentScope> = None;
    }

    fn message(result: crate::Result<()>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn a_method_is_registered_or_unsupported_exactly_once() {
        let mut registry = HandlerRegistry::default();
        registry.unimplemented::<WsServerProbeRpc>().unwrap();
        assert!(
            message(registry.unimplemented::<WsServerProbeRpc>())
                .contains("duplicate server.probe")
        );
        assert!(
            message(registry.register::<WsServerProbeRpc, _, _>(|_, _| async {
                Ok(t3_contracts::rpc::Empty {})
            }))
            .contains("duplicate server.probe")
        );
        assert_eq!(registry.disposition("server.probe"), Some(false));
    }

    #[test]
    fn a_registration_must_match_the_contract_call_kind() {
        let mut registry = HandlerRegistry::default();
        assert!(
            message(
                registry.register::<WsSubscribeServerConfigRpc, _, _>(|_, _| async {
                    Err(Failure::Defect(ServerDefect::Internal {
                        message: String::new(),
                    }))
                })
            )
            .contains("requires register_stream")
        );
        assert!(
            message(
                registry.register_stream::<WsServerProbeRpc, _, _, futures::stream::Empty<_>>(
                    |_, _| async { Ok(futures::stream::empty()) }
                )
            )
            .contains("is unary")
        );
        assert_eq!(registry.disposition("server.probe"), None);
    }

    #[test]
    fn validation_names_a_missing_method() {
        let mut registry = HandlerRegistry::default();
        registry.unimplemented::<WsServerProbeRpc>().unwrap();
        let error = message(registry.validate());
        assert!(
            error.starts_with("invalid handler registry: missing "),
            "{error}"
        );
    }

    #[test]
    fn validation_names_a_method_outside_the_contract() {
        let mut registry = crate::handlers::registry().unwrap();
        registry.unimplemented::<ExtraUnary>().unwrap();
        assert!(message(registry.validate()).contains("extra test.extra"));
    }
}
