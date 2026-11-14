#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use t3_provider::{environment::Environment, grok_usage::*, sensitive::*, transport::*};
struct Http {
    status: u16,
    body: serde_json::Value,
    calls: AtomicUsize,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert_eq!(request.url, USAGE_URL);
        assert_eq!(
            request.headers["authorization"].expose(),
            "Bearer never-grok-secret"
        );
        assert!(!format!("{request:?}").contains("never-grok-secret"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HttpResponse {
            status: self.status,
            body: Sensitive::new(self.body.clone()),
        })
    }
}
#[tokio::test]
async fn only_the_official_account_is_read_and_custom_auth_or_endpoints_never_receive_its_token() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("grok");
    std::fs::create_dir(&home).unwrap();
    let config = root.path().join("system");
    let environment=Environment::from([("GROK_HOME".into(),home.to_string_lossy().into_owned()),("GROK_AUTH".into(),json!({"https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828":{"key":" never-grok-secret ","email":" person@example.invalid "},"https://other.invalid":{"key":"must-not-use"}}).to_string())]);
    let http = Http {
        status: 200,
        body: json!({"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-11-01T00:00:00Z","end":"2026-11-08T00:00:00Z"}}}),
        calls: AtomicUsize::new(0),
    };
    let account = read(
        &http,
        &environment,
        root.path(),
        &config,
        "2026-11-02T00:00:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(account.email.as_deref(), Some("person@example.invalid"));
    let limits = serde_json::to_value(account.usage_limits).unwrap();
    assert_eq!(limits["windows"][0]["usedPercent"], 0.);
    assert_eq!(limits["windows"][0]["windowDurationMins"], 10080);
    for key in [
        "XAI_API_KEY",
        "GROK_OAUTH2_ISSUER",
        "GROK_MODELS_BASE_URL",
        "GROK_CONFIG_PATH",
    ] {
        let mut alternate = environment.clone();
        alternate.insert(key.into(), "custom".into());
        let limits = read(
            &http,
            &alternate,
            root.path(),
            &config,
            "2026-11-02T00:00:00.000Z",
        )
        .await
        .unwrap()
        .usage_limits;
        assert_eq!(
            serde_json::to_value(limits).unwrap()["unavailable"]["reason"],
            "unsupported"
        );
    }
    std::fs::write(
        home.join("config.toml"),
        "[grok_com_config]\nbase_url='https://other.invalid'\n",
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(
            read(
                &http,
                &environment,
                root.path(),
                &config,
                "2026-11-02T00:00:00.000Z"
            )
            .await
            .unwrap()
            .usage_limits
        )
        .unwrap()["unavailable"]["reason"],
        "unsupported"
    );
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn malformed_credentials_and_privacy_refusals_publish_safe_unavailability_and_no_arbitrary_account_fallback()
 {
    let root = tempfile::tempdir().unwrap();
    let http = Http {
        status: 403,
        body: json!({"error":"WKE=privacy-mode:old never-response-secret"}),
        calls: AtomicUsize::new(0),
    };
    let checked = "2026-11-02T00:00:00.000Z";
    for (auth, reason) in [
        (json!({"other":{"key":"never-grok-secret"}}), "unsupported"),
        (json!({"other":{"key":null}}), "probeFailed"),
        (
            json!({"https://accounts.x.ai/sign-in":{"key":"never-grok-secret"}}),
            "probeFailed",
        ),
    ] {
        let environment = BTreeMap::from([("GROK_AUTH".into(), auth.to_string())]);
        let result = read(
            &http,
            &environment,
            root.path(),
            &root.path().join("system"),
            checked,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(&result.usage_limits).unwrap()["unavailable"]["reason"],
            reason
        );
        assert!(!format!("{:?}", result.usage_limits).contains("never-"));
    }
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn zero_usage_requires_a_complete_valid_period_and_invalid_optional_types_are_rejected() {
    let checked = "2026-11-02T00:00:00.000Z";
    for (start, end, count) in [
        ("2026-11-01T00:00:00Z", "2026-11-08T00:00:00Z", 1),
        ("2026-02-30T00:00:00Z", "2026-03-08T00:00:00Z", 0),
        ("2026-11-08T00:00:00Z", "2026-11-01T00:00:00Z", 0),
        ("2026-11-01t00:00:00z", "2026-11-08T00:00:00Z", 0),
    ] {
        let result = limits(
            &json!({"config":{"currentPeriod":{"type":"WEEKLY","start":start,"end":end}}}),
            checked,
        )
        .unwrap();
        assert_eq!(result.windows.len(), count);
    }
    assert!(limits(&json!({"config":{"creditUsagePercent":null}}), checked).is_err());
    assert_eq!(
        serde_json::to_value(
            limits(&json!({"config":{"creditUsagePercent":140}}), checked).unwrap()
        )
        .unwrap()["windows"][0]["usedPercent"],
        100.
    );
}
