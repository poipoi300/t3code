#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome as Outcome;
use t3_provider::{environment::Platform, reset_credits::*, sensitive::*, transport::*};
use tokio::sync::{Mutex, mpsc};
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
#[test]
fn usable_claude_grants_keep_native_grouping_order_expiries_and_next_grant_eligibility() {
    let block = json!({"eligible":true,"next_grant_id":"session-a","grants":[{"id":"session-a","resets_left":2,"usable_now":true,"clears":["five_hour"],"ends_at":"2028-01-01T00:00:00+00:00"},{"id":"session-b","resets_left":3,"usable_now":true,"type":"session"},{"id":"weekly","resets_left":4,"usable_now":true,"clears":["seven_day"]},{"id":"fable","resets_left":1,"usable_now":true,"gives_back_weekly":true},{"id":"expired","resets_left":100,"usable_now":true,"ends_at":"2020-01-01T00:00:00Z"},{"id":"paused","resets_left":100,"usable_now":true,"paused":true},{"id":"unknown","resets_left":100},{"id":"bad","resets_left":100,"usable_now":true,"ends_at":"2028-02-31T00:00:00Z"}]});
    let credits = serde_json::to_value(claude_credits(&block, 1_800_000_000_000).unwrap()).unwrap();
    assert_eq!(credits["availableCount"], 10);
    assert_eq!(credits["nextExpiresAt"], "2028-01-01T00:00:00.000Z");
    assert_eq!(credits["options"][0]["availableCount"], 5);
    assert_eq!(credits["options"][1]["label"], "Weekly reset");
    assert_eq!(credits["options"][2]["label"], "Fable reset");
    let mut missing = block.clone();
    missing["next_grant_id"] = json!("not-live");
    let credits =
        serde_json::to_value(claude_credits(&missing, 1_800_000_000_000).unwrap()).unwrap();
    assert_eq!(credits["availableCount"], 0);
    assert!(credits.get("options").is_none());
    assert!(claude_credits(&json!({"eligible":false}), 0).is_none());
}
#[tokio::test]
async fn unsettled_redemptions_and_disconnected_clients_reuse_the_same_account_idempotency_key() {
    let coordinator = Arc::new(ResetCreditCoordinator::default());
    let mut keys = vec![];
    let result = coordinator
        .redeem(
            "account",
            |key| {
                keys.push(key);
                async { Err::<Outcome, _>(ClaudeResetError::Unconfirmed) }
            },
            ClaudeResetError::settled,
        )
        .await;
    assert_eq!(result.unwrap_err(), ClaudeResetError::Unconfirmed);
    let (sent, mut received) = mpsc::unbounded_channel();
    let request = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .redeem(
                    "account",
                    |key| async move {
                        sent.send(key).unwrap();
                        std::future::pending::<Result<Outcome, ClaudeResetError>>().await
                    },
                    ClaudeResetError::settled,
                )
                .await
        }
    });
    let admitted = received.recv().await.unwrap();
    assert_eq!(admitted, keys[0]);
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    coordinator
        .redeem(
            "account",
            |key| {
                keys.push(key);
                async { Ok::<_, ClaudeResetError>(Outcome::Reset) }
            },
            ClaudeResetError::settled,
        )
        .await
        .unwrap();
    assert_eq!(keys[0], keys[1]);
    coordinator
        .redeem(
            "account",
            |key| {
                keys.push(key);
                async { Err::<Outcome, _>(ClaudeResetError::CoolingDown) }
            },
            ClaudeResetError::settled,
        )
        .await
        .unwrap_err();
    assert_ne!(keys[1], keys[2]);
    coordinator
        .redeem(
            "account",
            |key| {
                keys.push(key);
                async { Ok::<_, ClaudeResetError>(Outcome::NoCredit) }
            },
            ClaudeResetError::settled,
        )
        .await
        .unwrap();
    assert_ne!(keys[2], keys[3]);
}
struct Http {
    calls: Mutex<Vec<HttpRequest>>,
    replies: Mutex<VecDeque<HttpResponse>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        self.calls.lock().await.push(request);
        Ok(self.replies.lock().await.pop_front().unwrap())
    }
}
#[tokio::test]
async fn claude_reset_client_uses_recorded_responses_explicit_files_and_private_headers() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join(".credentials.json"),
        json!({"claudeAiOauth":{"accessToken":"secret-claude-oauth"}}).to_string(),
    )
    .unwrap();
    let account = directory.path().join(".claude.json");
    std::fs::write(
        &account,
        json!({"oauthAccount":{"organizationUuid":"org/value"}}).to_string(),
    )
    .unwrap();
    let http=Arc::new(Http {calls:Mutex::new(vec![]),replies:Mutex::new(vec![HttpResponse {status:200,body:Sensitive::new(json!({"cedar_ember":{"eligible":true,"next_grant_id":"grant","grants":[{"id":"grant","resets_left":1,"usable_now":true}]}}))},HttpResponse {status:200,body:Sensitive::new(json!({"result":"reset"}))},HttpResponse {status:429,body:Sensitive::new(json!({"error":"secret-claude-oauth"}))}].into())});
    let client = ClaudeResetClient {
        http: http.clone(),
        clock: Arc::new(Clock),
        platform: Platform::Windows,
    };
    assert_eq!(
        client
            .read(directory.path(), "2.1.301")
            .await
            .unwrap()
            .available_count,
        1
    );
    assert_eq!(
        client
            .consume(directory.path(), &account, "2.1.301", "grant", "request")
            .await
            .unwrap(),
        Outcome::Reset
    );
    let error = client
        .consume(directory.path(), &account, "2.1.301", "grant", "request")
        .await
        .unwrap_err();
    assert_eq!(error, ClaudeResetError::RateLimited);
    assert!(!format!("{error:?} {error}").contains("secret-claude-oauth"));
    let calls = http.calls.lock().await;
    assert_eq!(
        calls[1].url,
        "https://api.anthropic.com/api/organizations/org%2Fvalue/reset_rate_limits"
    );
    assert_eq!(
        calls[1].headers["authorization"].expose(),
        "Bearer secret-claude-oauth"
    );
    assert_eq!(
        serde_json::from_str::<Value>(calls[1].body.as_ref().unwrap().expose()).unwrap(),
        json!({"program":"cedar_ember","grant_id":"grant","request_id":"request"})
    );
    assert!(!format!("{:?}", calls[1]).contains("secret-claude-oauth"));
}
