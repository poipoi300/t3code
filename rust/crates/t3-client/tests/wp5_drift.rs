#![allow(clippy::unwrap_used)]
use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use std::{cell::Cell, rc::Rc};
use t3_client::{
    ClientError,
    environment::SessionIdentity,
    rpc::{RpcEndpoint, RpcMethod},
    services::{
        resources::{Resource, ResourcePolicy},
        sessions::{ServiceSession, follow_resource},
    },
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
};

struct Number;
impl RpcMethod for Number {
    type Input = ();
    type Output = u64;
    const NAME: &'static str = "server.subscribeServerLifecycle";
}
struct Search;
impl RpcMethod for Search {
    type Input = ();
    type Output = u64;
    const NAME: &'static str = "orchestration.searchThreadStream";
}
struct Failing {
    attempts: Cell<usize>,
    decode: bool,
}
impl RpcEndpoint for Failing {
    fn call<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        async { Err(ClientError::Disconnected) }.boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.attempts.set(self.attempts.get() + 1);
        let error = if self.decode {
            ClientError::InvalidPayload("RPC response schema".into())
        } else {
            ClientError::Disconnected
        };
        futures::stream::iter([Ok(serde_json::json!(7)), Err(error)])
            .map(|value| value.and_then(|value| serde_json::from_value(value).map_err(Into::into)))
            .boxed_local()
    }
}
fn session(rpc: Rc<Failing>, generation: u64) -> ServiceSession<Failing> {
    ServiceSession {
        identity: SessionIdentity {
            environment: EnvironmentId("drift".into()),
            generation: Generation(generation),
            account: None,
            dpop_thumbprint: None,
        },
        endpoint: rpc,
    }
}
#[test]
fn durable_failures_retry_on_the_same_session_without_resetting_after_first_values() {
    for decode in [false, true] {
        let platform = FakePlatform::default();
        let resource = Resource::new(ResourcePolicy::default(), Generation(0));
        let (tx, rx) = mpsc::unbounded();
        let rpc = Rc::new(Failing {
            attempts: Cell::new(0),
            decode,
        });
        tx.unbounded_send(Some(session(rpc.clone(), 1))).unwrap();
        let mut work = Box::pin(follow_resource::<Number, _, _>(
            &resource,
            &platform,
            EnvironmentId("drift".into()),
            rx.boxed_local(),
            (),
            |_| false,
        ));
        assert!(work.as_mut().now_or_never().is_none());
        assert_eq!(rpc.attempts.get(), 1);
        for (index, delay) in [1000, 2000, 4000, 8000, 16000, 30000, 30000]
            .into_iter()
            .enumerate()
        {
            platform.advance(delay - 1);
            assert!(work.as_mut().now_or_never().is_none());
            assert_eq!(rpc.attempts.get(), index + 1);
            platform.advance(1);
            assert!(work.as_mut().now_or_never().is_none());
            assert_eq!(rpc.attempts.get(), index + 2);
        }
        let replacement = Rc::new(Failing {
            attempts: Cell::new(0),
            decode,
        });
        tx.unbounded_send(Some(session(replacement.clone(), 2)))
            .unwrap();
        assert!(work.as_mut().now_or_never().is_none());
        platform.advance(1000);
        assert!(work.as_mut().now_or_never().is_none());
        assert_eq!(replacement.attempts.get(), 2);
        assert_eq!(rpc.attempts.get(), 8);
        drop(work);
        assert_eq!(platform.pending_timers(), 0);
    }
}
#[test]
fn finite_search_surfaces_one_decode_failure_without_retrying() {
    let platform = FakePlatform::default();
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let rpc = Failing {
        attempts: Cell::new(0),
        decode: true,
    };
    let mut work = Box::pin(resource.follow::<Search, _, _>(&rpc, &platform, (), |_| false));
    assert!(matches!(
        work.as_mut().now_or_never(),
        Some(Err(ClientError::InvalidPayload(_)))
    ));
    platform.advance(60_000);
    assert_eq!(rpc.attempts.get(), 1);
    assert!(resource.snapshot().error.is_some());
}

#[test]
fn custom_decode_handler_parks_until_its_owner_replaces_the_subscription() {
    use t3_client::rpc::subscription::{SubscriptionOptions, recover_subscription};
    let platform = FakePlatform::default();
    let rpc = Failing {
        attempts: Cell::new(0),
        decode: true,
    };
    let defects = Rc::new(Cell::new(0));
    let recorded = defects.clone();
    let mut stream = recover_subscription(
        &platform,
        Number::NAME,
        || rpc.subscribe::<Number>(()),
        SubscriptionOptions {
            on_defect: Some(Rc::new(move |_| recorded.set(recorded.get() + 1))),
            ..Default::default()
        },
    );
    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        7
    );
    assert!(stream.next().now_or_never().is_none());
    platform.advance(60_000);
    assert!(stream.next().now_or_never().is_none());
    assert_eq!(rpc.attempts.get(), 1);
    assert_eq!(defects.get(), 1);
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn pending_approval_is_a_typed_remote_http_failure() {
    use t3_client::{
        platform::{Credentials, HttpRequest, HttpResponse},
        rpc::{HttpRpcError, execute_environment_http_request},
    };
    use t3_contracts::environment_http::EnvironmentHttpCommonError;
    let platform = FakePlatform::default();
    let body = serde_json::json!({"_tag":"EnvironmentConnectApprovalPendingError","code":"connect_approval_pending","traceId":"trace"});
    platform.push_http(Ok(HttpResponse {
        status: 403,
        headers: Default::default(),
        body: serde_json::to_vec(&body).unwrap(),
    }));
    let result =
        futures::executor::block_on(execute_environment_http_request::<serde_json::Value>(
            &platform,
            HttpRequest {
                method: "POST".into(),
                url: "https://host.test/api/auth/token".into(),
                headers: Default::default(),
                body: Vec::new(),
                credentials: Credentials::Omit,
                max_response_bytes: 1024,
            },
            1000,
        ));
    let Err(HttpRpcError::Rejected(error)) = result else {
        panic!("approval must retain its declared error")
    };
    assert!(matches!(
        *error,
        EnvironmentHttpCommonError::EnvironmentConnectApprovalPendingError(_)
    ));
    assert_eq!(serde_json::to_value(error).unwrap(), body);
}
