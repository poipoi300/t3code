#![allow(clippy::unwrap_used)]

use futures::executor::block_on;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap};
use t3_client::{
    authorization::{AuthorizationError, AuthorizedEnvironment, HttpAuthorization, Sensitive},
    environment::SessionIdentity,
    platform::{Credentials, HttpResponse},
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
    workspace::{
        pr_views::{diff_http::*, queries::QueryRegistry, service::load_registered_diff},
        pull_requests::QueryKind,
    },
};
use t3_contracts::pull_request::PullRequestDiffInput;
fn input() -> PullRequestDiffInput {
    serde_json::from_value(json!({"projectId":"project","repository":"acme/web","number":7}))
        .unwrap()
}
fn prepared(authorization: HttpAuthorization) -> AuthorizedEnvironment {
    AuthorizedEnvironment {
        session: SessionIdentity {
            environment: EnvironmentId("origin".into()),
            generation: Generation(1),
            account: None,
            dpop_thumbprint: None,
        },
        http_base_url: "https://example.test".into(),
        socket_url: Sensitive("wss://example.test/ws".into()),
        authorization,
        descriptor: None,
    }
}
fn response(status: u16, value: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&value).unwrap(),
    }
}
fn patch() -> Value {
    json!({"patch":"diff --git a/a b/a\n","truncated":false,"nextCursor":null})
}

#[test]
fn diff_requests_use_cookie_or_bearer_auth_and_the_protocol_header() {
    for authorization in [
        HttpAuthorization::Cookie,
        HttpAuthorization::Bearer(Sensitive("private-token".into())),
    ] {
        let cookie = matches!(authorization, HttpAuthorization::Cookie);
        let platform = FakePlatform::default();
        platform.push_http(Ok(response(200, patch())));
        let loader = DiffLoader {
            platform: &platform,
            signer: None,
            remote_authorization: None,
        };
        assert!(
            !block_on(loader.load(&prepared(authorization), &input(), false, || true, None))
                .unwrap()
                .truncated
        );
        let request = platform.http_requests().pop().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://example.test/api/pull-requests/diff");
        assert_eq!(
            request.credentials,
            if cookie {
                Credentials::Include
            } else {
                Credentials::Omit
            }
        );
        assert_eq!(
            request
                .headers
                .get(t3_contracts::environment::ORCHESTRATION_PROTOCOL_HEADER)
                .unwrap(),
            t3_contracts::environment::ORCHESTRATION_PROTOCOL_VERSION_TEXT
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            if cookie {
                None
            } else {
                Some("Bearer private-token")
            }
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap(),
            json!(input())
        );
        assert_eq!(platform.pending_timers(), 0);
    }
}

#[test]
fn rejected_diff_credentials_preserve_the_typed_trace_and_request_identity() {
    let platform = FakePlatform::default();
    platform.push_http(Ok(response(401,json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":"trace"}))));
    let loader = DiffLoader {
        platform: &platform,
        signer: None,
        remote_authorization: None,
    };
    match block_on(loader.load(
        &prepared(HttpAuthorization::Cookie),
        &input(),
        false,
        || true,
        None,
    ))
    .unwrap_err()
    {
        DiffLoadError::CredentialRejected {
            repository,
            number,
            trace_id,
            ..
        } => {
            assert_eq!(repository, "acme/web");
            assert_eq!(number, 7);
            assert_eq!(trace_id, "trace");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(platform.http_requests().len(), 1);
}

#[test]
fn diff_loads_reject_invalid_payloads_http_failures_and_cancelled_sessions() {
    for (status, body, expected) in [
        (200, json!({"patch":12}), "decode"),
        (503, json!({}), "http"),
    ] {
        let platform = FakePlatform::default();
        platform.push_http(Ok(response(status, body)));
        let loader = DiffLoader {
            platform: &platform,
            signer: None,
            remote_authorization: None,
        };
        let result = block_on(loader.load(
            &prepared(HttpAuthorization::Cookie),
            &input(),
            false,
            || true,
            None,
        ));
        assert!(matches!(
            (expected, result),
            ("decode", Err(DiffLoadError::InvalidResponse))
                | ("http", Err(DiffLoadError::Http { status: 503, .. }))
        ));
    }
    let platform = FakePlatform::default();
    let loader = DiffLoader {
        platform: &platform,
        signer: None,
        remote_authorization: None,
    };
    assert!(matches!(
        block_on(loader.load(
            &prepared(HttpAuthorization::Cookie),
            &input(),
            false,
            || false,
            None
        )),
        Err(DiffLoadError::Cancelled)
    ));
    assert!(platform.http_requests().is_empty());
    assert!(matches!(
        block_on(loader.load(
            &prepared(HttpAuthorization::Dpop {
                access_token: Sensitive("token".into()),
                expires_at_epoch_ms: 90_000
            }),
            &input(),
            false,
            || true,
            None
        )),
        Err(DiffLoadError::Authorization(
            AuthorizationError::SessionPending
        ))
    ));
}

#[test]
fn registered_http_diffs_publish_success_and_keep_cached_data_on_error() {
    let registry = RefCell::new(QueryRegistry::default());
    let environment = EnvironmentId("origin".into());
    let key = registry
        .borrow_mut()
        .register(environment, QueryKind::Diff, &input())
        .unwrap();
    let platform = FakePlatform::default();
    let loader = DiffLoader {
        platform: &platform,
        signer: None,
        remote_authorization: None,
    };
    let prepared = prepared(HttpAuthorization::Cookie);
    platform.push_http(Ok(response(200, patch())));
    assert!(
        block_on(load_registered_diff(
            &registry,
            &key,
            Generation(1),
            &loader,
            &prepared,
            false,
            || true
        ))
        .unwrap()
    );
    platform.push_http(Ok(response(503, json!({}))));
    assert!(
        block_on(load_registered_diff(
            &registry,
            &key,
            Generation(1),
            &loader,
            &prepared,
            false,
            || true
        ))
        .is_err()
    );
    assert_eq!(
        registry
            .borrow()
            .get(&key)
            .unwrap()
            .state
            .value
            .as_ref()
            .unwrap()["patch"],
        patch()["patch"]
    );
    assert!(registry.borrow().get(&key).unwrap().state.error.is_some());
    assert!(
        !block_on(load_registered_diff(
            &registry,
            &key,
            Generation(1),
            &loader,
            &prepared,
            false,
            || false
        ))
        .unwrap()
    );
}
