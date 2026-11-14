#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use t3_provider::{antigravity_usage::*, sensitive::*, transport::*};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Http {
    calls: Mutex<Vec<HttpRequest>>,
    legacy: bool,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert!(!format!("{request:?}").contains("secret"));
        let url = request.url.clone();
        let body = if url.ends_with("/token") {
            assert!(
                request
                    .body
                    .as_ref()
                    .unwrap()
                    .expose()
                    .contains("refresh_token=secret-refresh")
            );
            json!({"access_token":"secret-access","expires_in":3600})
        } else if url.ends_with("/userinfo") {
            json!({"id":"account-id"})
        } else {
            assert_eq!(
                request.headers["Authorization"].expose(),
                "Bearer secret-access"
            );
            assert_eq!(
                request.body.as_ref().unwrap().expose(),
                "{\"project\":\"work\"}"
            );
            if url.ends_with("retrieveUserQuotaSummary") {
                if self.legacy {
                    json!({"buckets":[]})
                } else {
                    json!({"buckets":[{"bucketId":"gemini-shared","window":"5h","remainingFraction":if url.contains("daily-"){0.25}else{0.75},"resetTime":"2026-11-13T19:00:00Z"}]})
                }
            } else {
                json!({"models":{"claude-a":{"quotaInfo":{"remainingFraction":0.9}},"claude-b":{"quotaInfo":{"remainingFraction":0.2}},"future":{"quotaInfo":{"remainingFraction":0.0}}}})
            }
        };
        self.calls.lock().await.push(request);
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(body),
        })
    }
}
#[test]
fn summary_buckets_keep_metered_groups_skip_disabled_and_drop_only_untouched_rolling_resets() {
    let requested = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z")
        .unwrap()
        .timestamp_millis();
    let windows=summary_windows(&json!({"groups":[{"displayName":" Argon Group ","buckets":[{"bucketId":"gemini-argon-session","remainingFraction":1,"window":"5h","resetTime":"2026-11-13T17:00:00Z"},{"bucketId":"gemini-carbon-weekly","remainingFraction":-1,"window":"weekly","description":"Exhausted","resetTime":"2026-11-20T12:00:00Z"},{"bucketId":"3p-disabled","remainingFraction":0,"disabled":true}]}],"buckets":[{"bucketId":"ignored","remainingFraction":0}]}),requested).unwrap();
    let value = serde_json::to_value(windows).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 2);
    assert_eq!(value[0]["label"], "Argon Group · 5-hour");
    assert!(value[0].get("resetsAt").is_none());
    assert_eq!(value[1]["usedPercent"], 100.0);
    assert_eq!(value[1]["windowDurationMins"], 10080);
    assert!(
        summary_windows(
            &json!({"buckets":[{"bucketId":"x","remainingFraction":null}]}),
            requested
        )
        .is_err()
    );
    assert_ne!(
        account_fingerprint(&json!({"id":"account-id"})).unwrap(),
        "account-id"
    );
}
#[tokio::test]
async fn official_endpoint_precedence_refresh_cache_and_model_fallback_use_recorded_responses() {
    for legacy in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("acp_token.json");
        std::fs::write(&path,json!({"client_id":"client","client_secret":"secret-client","refresh_token":"secret-refresh","project_id":"work"}).to_string()).unwrap();
        let clock = Arc::new(Clock(AtomicI64::new(1_000_000)));
        let http = Arc::new(Http {
            calls: Mutex::new(vec![]),
            legacy,
        });
        let source = AntigravityUsage::new(
            path,
            "oauth-personal".into(),
            "win32".into(),
            "x64".into(),
            http.clone(),
            clock.clone(),
        );
        let limits = serde_json::to_value(source.probe().await.unwrap()).unwrap();
        assert_eq!(limits["windows"].as_array().unwrap().len(), 1);
        assert_eq!(
            limits["credentialFingerprint"],
            account_fingerprint(&json!({"id":"account-id"})).unwrap()
        );
        let used = limits["windows"][0]["usedPercent"].as_f64().unwrap();
        assert!((used - if legacy { 80.0 } else { 75.0 }).abs() < 0.001);
        assert!(!limits.to_string().contains("secret"));
        source.probe().await.unwrap();
        assert_eq!(
            http.calls
                .lock()
                .await
                .iter()
                .filter(|request| request.url.ends_with("/token"))
                .count(),
            1
        );
        source.clear().await;
        source.probe().await.unwrap();
        assert_eq!(
            http.calls
                .lock()
                .await
                .iter()
                .filter(|request| request.url.ends_with("/token"))
                .count(),
            2
        );
        clock.0.store(4_600_000, Ordering::SeqCst);
        source.probe().await.unwrap();
        assert_eq!(
            http.calls
                .lock()
                .await
                .iter()
                .filter(|request| request.url.ends_with("/token"))
                .count(),
            3
        );
    }
}
