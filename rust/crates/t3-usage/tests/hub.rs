//! `apps/server/src/usage/cliproxyApi.test.ts`, ported against a fake hub transport.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_usage::{
    hub::{CliproxyApi, HubFuture, HubHttp, HubRequest, HubResponse, HubTransportError},
    limits::LimitSourceProbe,
};

const URL: &str = "http://hub.test:8317";
const KEY: &str = "management-secret";
/// 2026-09-06T00:00:00.000Z, TypeScript's `TestClock.setTime(1788710400000)`.
const NOW: f64 = 1_788_710_400_000.0;

struct Clock;
impl t3_usage::service::Clock for Clock {
    fn now_ms(&self) -> f64 {
        NOW
    }
}

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    body: Option<Value>,
}
type Upstream = Box<dyn Fn(&Value) -> (u16, Value) + Send + Sync>;
struct Fixture {
    accounts: Value,
    upstream: Option<Upstream>,
    cooldown_status: u16,
    requests: Mutex<Vec<Seen>>,
}
fn credit(id: &str, expires: &str) -> Value {
    json!({"id":id,"expires_at":expires,"status":"available","reset_type":"codex_rate_limits"})
}
fn accounts() -> Value {
    json!([
        {"id":"first.json","auth_index":"a","provider":"codex","email":"first@example.com","id_token":{"chatgpt_account_id":"account-a"}},
        {"id":"second.json","auth_index":"b","provider":"codex","email":"second@example.com","id_token":{"chatgpt_account_id":"account-b"}}
    ])
}
impl Fixture {
    fn new() -> Self {
        Self {
            accounts: accounts(),
            upstream: None,
            cooldown_status: 200,
            requests: Mutex::default(),
        }
    }
    fn accounts(mut self, accounts: Value) -> Self {
        self.accounts = accounts;
        self
    }
    fn upstream(mut self, upstream: impl Fn(&Value) -> (u16, Value) + Send + Sync + 'static) -> Self {
        self.upstream = Some(Box::new(upstream));
        self
    }
    fn api(self) -> (Arc<Self>, CliproxyApi) {
        let fixture = Arc::new(self);
        let api = CliproxyApi::new(fixture.clone(), Arc::new(Clock), "linux", "x64");
        (fixture, api)
    }
    fn requests(&self) -> Vec<Seen> {
        self.requests.lock().unwrap().clone()
    }
    fn answer(&self, request: &HubRequest) -> (u16, Value) {
        assert_eq!(request.authorization, "Bearer management-secret");
        let path = url::Url::parse(&request.url).unwrap().path().to_owned();
        let body: Option<Value> = request.body.as_ref().map(|b| serde_json::from_slice(b).unwrap());
        self.requests.lock().unwrap().push(Seen {
            path: path.clone(),
            body: body.clone(),
        });
        if path.ends_with("/auth-files") {
            return (200, json!({"files": self.accounts}));
        }
        if path.ends_with("/reset-quota") {
            return (self.cooldown_status, json!({}));
        }
        assert!(path.ends_with("/v0/management/api-call"), "{path}");
        let body = body.unwrap();
        assert_eq!(body["header"]["Authorization"], "Bearer $TOKEN$");
        let url = body["url"].as_str().unwrap_or_default().to_owned();
        let (status, upstream) = match &self.upstream {
            Some(upstream) => upstream(&body),
            None if url.ends_with("/consume") => (200, json!({"code":"reset"})),
            None if url.ends_with("/rate-limit-reset-credits") => (
                200,
                json!({"credits":[
                    credit("later","2099-02-01T00:00:00Z"),
                    credit("first","2099-01-01T00:00:00Z"),
                    credit("expired","2000-01-01T00:00:00Z"),
                    {"id":"used","expires_at":"2099-01-01T00:00:00Z","status":"redeemed","reset_type":"codex_rate_limits"}
                ]}),
            ),
            None => (
                200,
                json!({"plan_type":"pro","rate_limit":{"secondary_window":{"used_percent":78,"reset_at":4070908800u64,"limit_window_seconds":604800}}}),
            ),
        };
        (200, json!({"status_code":status,"body":upstream.to_string()}))
    }
}
impl HubHttp for Fixture {
    fn send(&self, request: HubRequest) -> HubFuture<'_> {
        let (status, body) = self.answer(&request);
        Box::pin(async move {
            Ok(HubResponse {
                status,
                body: body.to_string().into_bytes(),
            })
        })
    }
}

/// A hub whose every request fails with `code`, or answers with `status`.
struct Failing {
    code: Option<&'static str>,
    status: Option<u16>,
}
impl HubHttp for Failing {
    fn send(&self, _: HubRequest) -> HubFuture<'_> {
        let (code, status) = (self.code, self.status);
        Box::pin(async move {
            match status {
                Some(status) => Ok(HubResponse {
                    status,
                    body: json!({"error":"management-secret is wrong"}).to_string().into_bytes(),
                }),
                None => Err(HubTransportError {
                    code: code.map(str::to_owned),
                }),
            }
        })
    }
}
async fn failure(http: Failing) -> String {
    let api = CliproxyApi::new(Arc::new(http), Arc::new(Clock), "linux", "x64");
    match api.accounts(URL, KEY).await.unwrap_err() {
        t3_usage::Error::Probe(detail) => detail,
        other => panic!("{other}"),
    }
}
async fn read(api: &CliproxyApi) -> Vec<Value> {
    json!(api.accounts(URL, KEY).await.unwrap())
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn names_the_tls_code_when_the_hubs_certificate_is_not_trusted() {
    let detail = failure(Failing { code: Some("SELF_SIGNED_CERT_IN_CHAIN"), status: None }).await;
    assert!(detail.contains("SELF_SIGNED_CERT_IN_CHAIN"), "{detail}");
    assert!(detail.contains("NODE_EXTRA_CA_CERTS"), "{detail}");
    assert_eq!(
        failure(Failing { code: Some("ECONNREFUSED"), status: None }).await,
        "The hub request failed (ECONNREFUSED)."
    );
}

#[tokio::test]
async fn ignores_a_transport_code_that_is_not_a_short_node_error_code() {
    for code in ["not a code", "lower_case", &"A".repeat(65)] {
        let code: &'static str = Box::leak(code.to_owned().into_boxed_str());
        assert_eq!(
            failure(Failing { code: Some(code), status: None }).await,
            "The hub management request failed."
        );
    }
}

#[tokio::test]
async fn names_the_status_not_the_body_when_the_hub_rejects_the_request() {
    let detail = failure(Failing { code: None, status: Some(401) }).await;
    assert!(detail.contains("401"), "{detail}");
    assert!(!detail.contains("management-secret"), "{detail}");
}

#[tokio::test]
async fn reads_both_accounts_and_their_earliest_unexpired_credits() {
    let (fixture, api) = Fixture::new().api();
    let result = read(&api).await;
    for account in &result {
        assert_eq!(
            account["usageLimits"]["resetCredits"],
            json!({"availableCount":2,"nextCreditId":"first","nextExpiresAt":"2099-01-01T00:00:00.000Z"})
        );
    }
    let window = &result[0]["usageLimits"]["windows"][0];
    assert_eq!((window["id"].as_str(), window["kind"].as_str()), (Some("secondary"), Some("weekly")));
    assert_eq!(window["usedPercent"].as_f64(), Some(78.0));
    assert_eq!(result[0]["plan"], "ChatGPT Pro 200 Subscription");
    assert_eq!(result[0]["email"], "first@example.com");
    assert_eq!(result[0]["workspaceId"], "account-a");
    let calls: Vec<Value> = fixture.requests().into_iter().filter_map(|r| r.body).filter(|b| b.get("url").is_some()).collect();
    let mut indexes: Vec<&str> = calls.iter().map(|c| c["auth_index"].as_str().unwrap()).collect();
    indexes.sort_unstable();
    assert_eq!(indexes, ["a", "a", "b", "b"]);
    let b = calls.iter().find(|c| c["auth_index"] == "b").unwrap();
    assert_eq!(b["header"]["Chatgpt-Account-Id"], "account-b");
}

#[tokio::test]
async fn resolves_a_missing_usage_plan_from_current_then_legacy_token_plans() {
    let (_, api) = Fixture::new()
        .accounts(json!([
            {"id":"first.json","auth_index":"a","provider":"codex","id_token":{"chatgpt_account_id":"account-a","plan_type":"free"}},
            {"id":"second.json","auth_index":"b","provider":"codex","id_token":{"chatgpt_account_id":"account-b","plan_type":"go","chatgpt_plan_type":"plus"}},
            {"id":"third.json","auth_index":"c","provider":"codex","id_token":{"chatgpt_account_id":"account-c","chatgpt_plan_type":"free"}}
        ]))
        .upstream(|request| {
            let mut body = json!({"rate_limit":{"primary_window":{"used_percent":12}}});
            if request["auth_index"] == "a" {
                body["plan_type"] = json!("plus");
            }
            (200, body)
        })
        .api();
    let result = read(&api).await;
    let plans: Vec<(Value, Value)> = result
        .iter()
        .map(|a| (a["plan"].clone(), a["usageLimits"]["windows"][0]["kind"].clone()))
        .collect();
    assert_eq!(
        plans,
        [
            (json!("ChatGPT Plus Subscription"), json!("session")),
            (json!("ChatGPT Go Subscription"), json!("monthly")),
            (json!("ChatGPT Free Subscription"), json!("monthly")),
        ]
    );
}

#[tokio::test]
async fn keeps_the_path_of_a_hub_served_under_a_prefix() {
    for url in [
        "https://tools.test/cliproxy",
        "https://tools.test/cliproxy/",
        "https://tools.test/cliproxy/management.html",
        "https://tools.test/cliproxy/management.html/",
        "https://tools.test/cliproxy/MANAGEMENT.HTML",
        "https://tools.test/cliproxy/management.html?tab=accounts",
        "https://tools.test/cliproxy/management.html#accounts",
        "https://tools.test/cliproxy/management.html/?tab=accounts",
        "https://tools.test/cliproxy/management.html/#accounts",
    ] {
        let (fixture, api) = Fixture::new().api();
        api.accounts(url, KEY).await.unwrap();
        let requests = fixture.requests();
        assert!(!requests.is_empty());
        assert!(
            requests.iter().all(|r| r.path.starts_with("/cliproxy/v0/management/")),
            "{url}: {requests:?}"
        );
    }
}

#[tokio::test]
async fn preserves_a_hub_path_that_only_ends_with_the_panel_filename() {
    let (fixture, api) = Fixture::new().api();
    api.accounts("https://tools.test/team-management.html", KEY).await.unwrap();
    assert!(
        fixture
            .requests()
            .iter()
            .all(|r| r.path.starts_with("/team-management.html/v0/management/"))
    );
}

#[tokio::test]
async fn an_unparseable_hub_url_is_a_typed_failure_without_a_request() {
    let (fixture, api) = Fixture::new().api();
    match api.accounts("not a url", KEY).await.unwrap_err() {
        t3_usage::Error::Probe(detail) => assert_eq!(detail, "The hub URL is not valid."),
        other => panic!("{other}"),
    }
    assert!(fixture.requests().is_empty());
}

#[tokio::test]
async fn keeps_usage_when_the_credits_endpoint_fails() {
    let (_, api) = Fixture::new()
        .upstream(|request| {
            if request["url"].as_str().unwrap().ends_with("rate-limit-reset-credits") {
                (503, json!({"token":"do-not-publish"}))
            } else {
                (200, json!({"rate_limit":{"primary_window":{"used_percent":12}}}))
            }
        })
        .api();
    let result = read(&api).await;
    assert_eq!(result[0]["usageLimits"]["windows"][0]["usedPercent"].as_f64(), Some(12.0));
    assert!(result[0]["usageLimits"].get("resetCredits").is_none());
}

#[tokio::test]
async fn isolates_a_failed_account_and_never_publishes_upstream_error_bodies() {
    let (_, api) = Fixture::new()
        .upstream(|request| {
            if request["auth_index"] == "a" {
                (401, json!({"token":"do-not-publish"}))
            } else if request["url"].as_str().unwrap().ends_with("rate-limit-reset-credits") {
                (200, json!({"credits":[]}))
            } else {
                (200, json!({"rate_limit":{"primary_window":{"used_percent":12}}}))
            }
        })
        .api();
    let result = read(&api).await;
    assert_eq!(result[0]["usageLimits"]["unavailable"]["reason"], "probeFailed");
    assert_eq!(result[1]["usageLimits"]["windows"][0]["usedPercent"].as_f64(), Some(12.0));
    assert!(!json!(result).to_string().contains("do-not-publish"));
}

#[tokio::test]
async fn maps_claude_scoped_windows_without_a_scheduler_plugin() {
    let (_, api) = Fixture::new()
        .accounts(json!([{"id":"first.json","auth_index":"a","provider":"claude","email":"first@example.com"}]))
        .upstream(|request| {
            let claude_agent = request["header"]["User-Agent"]
                .as_str()
                .is_some_and(|ua| ua.starts_with("claude-code/"));
            (
                if claude_agent { 200 } else { 429 },
                json!({"five_hour":{"utilization":10,"resets_at":null},"seven_day":{"utilization":50,"resets_at":"2099-01-01T00:00:00Z"},
                       "limits":[{"kind":"weekly_scoped","percent":80,"resets_at":null,"scope":{"model":{"display_name":"Fable"}}}]}),
            )
        })
        .api();
    let result = read(&api).await;
    let windows: Vec<(Value, Option<f64>)> = result[0]["usageLimits"]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["id"].clone(), w["usedPercent"].as_f64()))
        .collect();
    assert_eq!(
        windows,
        [(json!("five_hour"), Some(10.0)), (json!("seven_day"), Some(50.0)), (json!("seven_day_fable"), Some(80.0))]
    );
    assert_eq!(result[0]["driver"], "claudeAgent");
}

#[tokio::test]
async fn keeps_claude_monthly_spend_beside_rolling_windows() {
    for budget in [
        json!({"spend":{"enabled":true,"used":{"amount_minor":4631,"currency":"USD","exponent":2},"limit":{"amount_minor":50000,"currency":"USD","exponent":2}}}),
        json!({"extra_usage":{"is_enabled":true,"used_credits":4631,"monthly_limit":50000,"utilization":9}}),
    ] {
        let (_, api) = Fixture::new()
            .accounts(json!([{"id":"first.json","auth_index":"a","provider":"claude"}]))
            .upstream(move |_| {
                let mut body = json!({"five_hour":{"utilization":10,"resets_at":null}});
                for (key, value) in budget.as_object().unwrap() {
                    body[key] = value.clone();
                }
                (200, body)
            })
            .api();
        let result = read(&api).await;
        let windows = result[0]["usageLimits"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2, "{result:?}");
        assert_eq!(windows[0]["id"], "five_hour");
        assert_eq!(windows[1]["id"], "monthly_spend");
        assert!((windows[1]["usedPercent"].as_f64().unwrap() - 9.262).abs() < 1e-9);
    }
}

#[tokio::test]
async fn omits_disabled_claude_budgets_without_losing_rolling_windows() {
    let (_, api) = Fixture::new()
        .accounts(json!([{"id":"first.json","auth_index":"a","provider":"claude"}]))
        .upstream(|_| {
            (200, json!({"five_hour":{"utilization":10,"resets_at":null},"spend":{"enabled":false,"used":null,"limit":null},
                         "extra_usage":{"is_enabled":false,"used_credits":null,"monthly_limit":null,"utilization":null}}))
        })
        .api();
    let result = read(&api).await;
    let ids: Vec<&Value> = result[0]["usageLimits"]["windows"].as_array().unwrap().iter().map(|w| &w["id"]).collect();
    assert_eq!(ids, [&json!("five_hour")]);
}

#[tokio::test]
async fn reads_an_xai_account_as_grok_with_grok_cli_headers() {
    let (fixture, api) = Fixture::new()
        .accounts(json!([{"id":"xai.json","auth_index":"x","provider":"xai","email":"x@example.com"}]))
        .upstream(|_| {
            (200, json!({"config":{"creditUsagePercent":40,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2099-01-01T00:00:00Z"}}}))
        })
        .api();
    let result = read(&api).await;
    assert_eq!(result[0]["driver"], "grok");
    assert_eq!(result[0]["email"], "x@example.com");
    let window = &result[0]["usageLimits"]["windows"][0];
    assert_eq!((window["id"].clone(), window["kind"].clone(), window["usedPercent"].as_f64()), (json!("subscription"), json!("weekly"), Some(40.0)));
    let call = fixture.requests().into_iter().find_map(|r| r.body.filter(|b| b.get("url").is_some())).unwrap();
    assert_eq!(call["method"], "GET");
    assert_eq!(call["url"], "https://cli-chat-proxy.grok.com/v1/billing?format=credits");
    assert_eq!(call["header"]["x-xai-token-auth"], "xai-grok-cli");
    assert!(call["header"]["x-grok-client-version"].is_string());
    assert!(call["header"]["User-Agent"].as_str().unwrap().starts_with("grok-cli/"));
}

fn antigravity_summary() -> Value {
    json!({"buckets":[{"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.75,"resetTime":"2099-01-01T00:00:00Z"}]})
}

#[tokio::test]
async fn reads_an_antigravity_summary_and_keys_the_account_by_its_google_id() {
    let (fixture, api) = Fixture::new()
        .accounts(json!([{"id":"antigravity-g@example.com.json","auth_index":"g","provider":"antigravity","email":"g@example.com","project_id":"project-7"}]))
        .upstream(|request| {
            if request["url"].as_str().unwrap().ends_with("/userinfo") {
                (200, json!({"id":"google-42"}))
            } else {
                (200, antigravity_summary())
            }
        })
        .api();
    let result = read(&api).await;
    let account = &result[0];
    assert_eq!(account["driver"], "antigravity");
    assert!(account.get("email").is_none());
    assert_eq!(
        account["usageLimits"]["credentialFingerprint"],
        "136d4952771fd67a3600053f37492cbd05828712e0b1c89ebab03d4c22ab6554"
    );
    let window = &account["usageLimits"]["windows"][0];
    assert_eq!((window["id"].clone(), window["kind"].clone(), window["usedPercent"].as_f64()), (json!("gemini-weekly"), json!("weekly"), Some(25.0)));
    let requests = fixture.requests();
    let summary = requests.iter().find_map(|r| r.body.clone().filter(|b| b["method"] == "POST")).unwrap();
    assert_eq!(summary["url"], "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary");
    assert_eq!(summary["data"], json!({"project":"project-7"}).to_string());
    assert_eq!(summary["auth_index"], "g");
    assert_eq!(
        summary["header"],
        json!({"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","User-Agent":"antigravity/hub/2.24.0 (aidev_client; os_type=linux; arch=x64; cl=963137146)"})
    );
    let identity: Vec<Value> = requests
        .iter()
        .filter_map(|r| r.body.clone())
        .filter(|b| b["url"].as_str().is_some_and(|u| u.ends_with("/userinfo")))
        .collect();
    assert_eq!(identity.len(), 1);
    assert_eq!(identity[0]["method"], "GET");
    assert_eq!(identity[0]["url"], "https://www.googleapis.com/oauth2/v2/userinfo");
}

#[tokio::test]
async fn keeps_antigravity_limits_and_listing_email_when_its_identity_read_fails() {
    let (_, api) = Fixture::new()
        .accounts(json!([{"id":"antigravity.json","auth_index":"g","provider":"antigravity","email":"g@example.com","project_id":"project-7"}]))
        .upstream(|request| {
            if request["url"].as_str().unwrap().ends_with("/userinfo") {
                (403, json!({"error":"denied"}))
            } else {
                (200, antigravity_summary())
            }
        })
        .api();
    let result = read(&api).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["email"], "g@example.com");
    assert!(result[0]["usageLimits"].get("credentialFingerprint").is_none());
    assert!(result[0]["usageLimits"].get("unavailable").is_none());
    assert_eq!(result[0]["usageLimits"]["windows"][0]["id"], "gemini-weekly");
}

#[tokio::test]
async fn reports_refused_or_malformed_xai_and_antigravity_answers_as_unreadable() {
    for (provider, status, body) in [
        ("xai", 403, json!({"error":"denied"})),
        ("xai", 200, json!({"config":"x"})),
        ("antigravity", 401, json!({})),
        ("antigravity", 200, json!([])),
        ("antigravity", 200, json!({})),
    ] {
        let (_, api) = Fixture::new()
            .accounts(json!([{"id":format!("{provider}.json"),"auth_index":"a","provider":provider,"project_id":"project-7"}]))
            .upstream(move |_| (status, body.clone()))
            .api();
        let result = read(&api).await;
        assert_eq!(result[0]["usageLimits"]["unavailable"]["reason"], "probeFailed", "{provider} {status}");
    }
}

#[tokio::test]
async fn rejects_a_missing_or_blank_antigravity_project_before_upstream_calls() {
    for project in [None, Some(""), Some("  ")] {
        let mut account = json!({"id":"antigravity.json","auth_index":"a","provider":"antigravity"});
        if let Some(project) = project {
            account["project_id"] = json!(project);
        }
        let (fixture, api) = Fixture::new().accounts(json!([account])).api();
        let result = read(&api).await;
        assert_eq!(result[0]["usageLimits"]["unavailable"]["reason"], "probeFailed");
        let paths: Vec<String> = fixture.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, ["/v0/management/auth-files"]);
    }
}

#[tokio::test]
async fn skips_accounts_of_providers_the_hub_table_does_not_know() {
    let (fixture, api) = Fixture::new()
        .accounts(json!([
            {"id":"kiro.json","auth_index":"k","provider":"kiro","email":"k@example.com"},
            {"id":"first.json","auth_index":"a","provider":"claude","email":"first@example.com"}
        ]))
        .upstream(|_| (200, json!({"five_hour":{"utilization":1,"resets_at":null}})))
        .api();
    let result = read(&api).await;
    let drivers: Vec<&Value> = result.iter().map(|a| &a["driver"]).collect();
    assert_eq!(drivers, [&json!("claudeAgent")]);
    assert!(!fixture.requests().iter().any(|r| r.body.as_ref().is_some_and(|b| b["auth_index"] == "k")));
}

#[tokio::test]
async fn a_malformed_account_list_is_a_typed_failure() {
    let (_, api) = Fixture::new().accounts(json!([{"id":"first.json","provider":"codex"}])).api();
    match api.accounts(URL, KEY).await.unwrap_err() {
        t3_usage::Error::Probe(detail) => assert_eq!(detail, "The hub could not list accounts."),
        other => panic!("{other}"),
    }
}

async fn consume(api: &CliproxyApi, account: &str, credit: &str) -> Result<Value, String> {
    api.consume(URL, KEY, account, credit)
        .await
        .map(|result| json!(result))
        .map_err(|error| match error {
            t3_usage::Error::Probe(detail) => detail,
            other => other.to_string(),
        })
}
fn redemptions(fixture: &Fixture) -> Vec<Value> {
    fixture
        .requests()
        .into_iter()
        .filter_map(|r| r.body)
        .filter(|b| b["url"].as_str().is_some_and(|u| u.ends_with("/consume")))
        .collect()
}

#[tokio::test]
async fn pins_redemption_to_the_displayed_credit_and_clears_only_that_accounts_cooldown() {
    let (fixture, api) = Fixture::new().api();
    assert_eq!(consume(&api, "second.json", "credit-b").await.unwrap(), json!({"outcome":"reset"}));
    assert_eq!(consume(&api, "second.json", "credit-b").await.unwrap(), json!({"outcome":"reset"}));
    let redemptions = redemptions(&fixture);
    assert_eq!(redemptions.len(), 2);
    assert_eq!(redemptions[0]["data"], redemptions[1]["data"]);
    assert_eq!(
        redemptions[0]["data"],
        json!({"redeem_request_id":"519d5243-011a-5b7b-91f3-44d85f095705","credit_id":"credit-b"}).to_string()
    );
    let cooldowns: Vec<Value> = fixture
        .requests()
        .into_iter()
        .filter(|r| r.path.ends_with("/reset-quota"))
        .map(|r| r.body.unwrap()["auth_index"].clone())
        .collect();
    assert_eq!(cooldowns, [json!("b"), json!("b")]);
}

#[tokio::test]
async fn reports_redemption_success_even_if_cooldown_clearing_fails() {
    let mut fixture = Fixture::new();
    fixture.cooldown_status = 404;
    let (_, api) = fixture.api();
    let result = consume(&api, "first.json", "credit").await.unwrap();
    assert_eq!(result["outcome"], "reset");
    assert!(result["warning"].as_str().unwrap().contains("cooldown"));
}

#[tokio::test]
async fn skips_disabled_accounts_and_rejects_redemption_on_them() {
    let (fixture, api) = Fixture::new()
        .accounts(json!([{"id":"first.json","auth_index":"a","provider":"codex","disabled":true,"id_token":{"chatgpt_account_id":"account-a"}}]))
        .api();
    assert!(read(&api).await.is_empty());
    assert_eq!(
        consume(&api, "first.json", "credit").await.unwrap_err(),
        "The Codex hub account is missing or disabled."
    );
    assert!(fixture.requests().iter().all(|r| r.path.ends_with("/auth-files")));
}

#[tokio::test]
async fn keeps_the_same_redemption_id_after_an_uncertain_upstream_failure() {
    let attempts = Arc::new(Mutex::new(0));
    let counter = attempts.clone();
    let (fixture, api) = Fixture::new()
        .upstream(move |_| {
            let mut attempts = counter.lock().unwrap();
            *attempts += 1;
            if *attempts == 1 { (503, json!({})) } else { (200, json!({"code":"already_redeemed"})) }
        })
        .api();
    assert_eq!(
        consume(&api, "first.json", "credit").await.unwrap_err(),
        "The provider refused the hub request (HTTP 503)."
    );
    assert_eq!(consume(&api, "first.json", "credit").await.unwrap(), json!({"outcome":"alreadyRedeemed"}));
    let redemptions = redemptions(&fixture);
    assert_eq!(redemptions[0]["data"], redemptions[1]["data"]);
    assert_eq!(fixture.requests().iter().filter(|r| r.path.ends_with("/reset-quota")).count(), 1);
}

#[tokio::test]
async fn rejects_unknown_accounts_without_forwarding_a_redemption() {
    let (fixture, api) = Fixture::new().api();
    assert!(consume(&api, "missing.json", "credit").await.is_err());
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn an_unexpected_redemption_answer_is_a_typed_failure_without_a_cooldown_reset() {
    let (fixture, api) = Fixture::new().upstream(|_| (200, json!({"code":"surprise"}))).api();
    assert_eq!(
        consume(&api, "first.json", "credit").await.unwrap_err(),
        "The hub returned an unexpected reset-credit response."
    );
    assert!(!fixture.requests().iter().any(|r| r.path.ends_with("/reset-quota")));
}

#[test]
fn the_management_key_never_appears_in_a_requests_debug_output() {
    let request = HubRequest {
        method: "GET",
        url: format!("{URL}/v0/management/auth-files"),
        authorization: format!("Bearer {KEY}"),
        body: None,
    };
    assert!(!format!("{request:?}").contains(KEY));
}
