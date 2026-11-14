#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};
use t3_provider::{cursor_account::CursorAccountReader, sensitive::*, transport::*};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Http {
    calls: Mutex<Vec<(String, Value)>>,
    reject: AtomicUsize,
    teams: Value,
}
fn event(model: &str) -> Value {
    json!({"timestamp":1,"model":model,"conversationId":"session","tokenUsage":{"inputTokens":4,"cacheReadTokens":5,"cacheWriteTokens":6,"outputTokens":7,"totalCents":12}})
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert!(!format!("{request:?}").contains("private-key"));
        let method = request.url.rsplit('/').next().unwrap().to_owned();
        let body = serde_json::from_str(request.body.as_ref().unwrap().expose()).unwrap();
        self.calls.lock().await.push((method.clone(), body));
        let response = match method.as_str() {
            "exchange_user_api_key" => {
                json!({"accessToken":format!("header.{}.signature",URL_SAFE_NO_PAD.encode(json!({"sub":"auth|demo","exp":4102444800_u64}).to_string()))})
            }
            "GetTeams" => self.teams.clone(),
            "GetMe" => json!({"userId":7}),
            "GetFilteredUsageEvents" => {
                let calls = self.calls.lock().await;
                let body = &calls.last().unwrap().1;
                if body.get("teamId").is_some() {
                    assert_eq!(body["userId"], 7);
                } else {
                    assert!(body.get("userId").is_none());
                }
                if self.reject.load(Ordering::SeqCst) > 0 {
                    return Ok(HttpResponse {
                        status: 401,
                        body: Sensitive::new(json!({"message":"private diagnostic"})),
                    });
                }
                json!({"totalUsageEventsCount":1,"usageEventsDisplay":[event("cursor-grok-4-thinking-high-fast")]})
            }
            _ => panic!("unexpected account request"),
        };
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(response),
        })
    }
}
fn setup(teams: Value) -> (Arc<CursorAccountReader>, Arc<Http>, Arc<Clock>) {
    let http = Arc::new(Http {
        calls: Mutex::new(vec![]),
        reject: AtomicUsize::new(0),
        teams,
    });
    let clock = Arc::new(Clock(AtomicI64::new(0)));
    (
        CursorAccountReader::new(http.clone(), clock.clone()),
        http,
        clock,
    )
}
#[tokio::test]
async fn personal_and_own_user_team_history_share_membership_but_keep_distinct_billed_rows() {
    let (reader, http, clock) = setup(json!({"teams":[{"id":10},{"id":20},{"id":10}]}));
    let key = Sensitive::new("private-key".into());
    let first = reader.read(&key, 0., 10.).await;
    assert!(first.error.is_none());
    assert_eq!(first.records.len(), 3);
    assert_eq!(first.records[0].rate_model.as_deref(), Some("xai/grok-4"));
    assert_eq!(first.records[0].totals.cache_creation_tokens, 6);
    assert_eq!(
        first.records[0].dedupe_key.as_deref(),
        Some(
            "cursor-account:2188c26da759c4fae3aa0252fc6d725eff0922ef0b022551e6f0987d02bfab33:69f10f4f0cc7bedbcd7395dfed21ed957a0421d819edbcbf202bbfad87af5f06:0"
        )
    );

    assert_ne!(first.records[0].dedupe_key, first.records[1].dedupe_key);
    assert_ne!(first.records[1].dedupe_key, first.records[2].dedupe_key);
    reader.read(&key, 0., 10.).await;
    assert_eq!(
        http.calls
            .lock()
            .await
            .iter()
            .filter(|call| call.0 == "GetMe")
            .count(),
        1
    );
    clock.0.store(300_000, Ordering::SeqCst);
    reader.read(&key, 0., 10.).await;
    assert_eq!(
        http.calls
            .lock()
            .await
            .iter()
            .filter(|call| call.0 == "GetMe")
            .count(),
        2
    );
    http.reject.store(1, Ordering::SeqCst);
    let rejected = reader.read(&key, 0., 10.).await;
    assert!(rejected.records.is_empty());
    assert_eq!(
        rejected.error.as_deref(),
        Some("Sign in to Cursor again to read account usage.")
    );
    http.reject.store(0, Ordering::SeqCst);
    reader.read(&key, 0., 10.).await;
    reader
        .read(&Sensitive::new("another-private-key".into()), 0., 10.)
        .await;
    assert_eq!(
        http.calls
            .lock()
            .await
            .iter()
            .filter(|call| call.0 == "GetMe")
            .count(),
        4
    );
}
#[tokio::test]
async fn malformed_membership_never_keeps_partial_personal_totals() {
    for teams in [
        json!({"teams":[{"id":0}]}),
        json!({"teams":[{"id":10.5}]}),
        json!({"teams":null}),
        json!({"error":"private"}),
    ] {
        let (reader, http, _) = setup(teams);
        let result = reader
            .read(&Sensitive::new("private-key".into()), 0., 10.)
            .await;
        assert!(result.error.is_some());
        assert!(result.records.is_empty());
        assert!(
            !http
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call.0 == "GetFilteredUsageEvents")
        );
    }
}

#[derive(Debug)]
struct PageGate {
    team: Option<u64>,
    page: usize,
    release: tokio::sync::oneshot::Sender<()>,
}
struct PagedHttp {
    entered: tokio::sync::mpsc::UnboundedSender<PageGate>,
}
#[async_trait]
impl HttpTransport for PagedHttp {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        let method = request.url.rsplit('/').next().unwrap();
        let body: Value = serde_json::from_str(request.body.as_ref().unwrap().expose()).unwrap();
        let reply = match method {
            "exchange_user_api_key" => {
                json!({"accessToken":format!("header.{}.signature",URL_SAFE_NO_PAD.encode(json!({"sub":"auth|pages"}).to_string()))})
            }
            "GetTeams" => json!({"teams":[{"id":10}]}),
            "GetMe" => json!({"userId":7}),
            "GetFilteredUsageEvents" => {
                let page = body["page"].as_u64().unwrap() as usize;
                let team = body["teamId"].as_u64();
                if team.is_some() {
                    assert_eq!(body["userId"], 7);
                }
                if page > 1 {
                    let (release, answer) = tokio::sync::oneshot::channel();
                    self.entered
                        .send(PageGate {
                            team,
                            page,
                            release,
                        })
                        .unwrap();
                    answer.await.map_err(|_| ServiceError::Cancelled)?;
                }
                let events =
                    vec![event(&format!("{team:?}/page{page}")); if page == 5 { 1 } else { 1000 }];
                json!({"totalUsageEventsCount":4001,"usageEventsDisplay":events})
            }
            _ => panic!("unexpected account request"),
        };
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(reply),
        })
    }
}
#[tokio::test(start_paused = true)]
async fn scopes_start_together_and_six_page_slots_keep_order_and_start_deadlines_after_admission() {
    let (entered, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let reader = CursorAccountReader::new(
        Arc::new(PagedHttp { entered }),
        Arc::new(Clock(AtomicI64::new(0))),
    );
    let reading = tokio::spawn(async move {
        reader
            .read(&Sensitive::new("private-key".into()), 0., 10.)
            .await
    });
    let mut admitted = vec![];
    for _ in 0..6 {
        admitted.push(requests.recv().await.unwrap());
    }
    assert!(admitted.iter().any(|request| request.team.is_none()));
    assert!(admitted.iter().any(|request| request.team == Some(10)));
    assert!(requests.try_recv().is_err());
    tokio::time::advance(std::time::Duration::from_secs(9)).await;
    for request in admitted.into_iter().rev() {
        assert!((2..=5).contains(&request.page));
        request.release.send(()).unwrap();
    }
    let queued = [
        requests.recv().await.unwrap(),
        requests.recv().await.unwrap(),
    ];
    assert!(requests.try_recv().is_err());
    tokio::time::advance(std::time::Duration::from_secs(9)).await;
    for request in queued {
        request.release.send(()).unwrap();
    }
    let result = reading.await.unwrap();
    assert_eq!(result.error, None);
    assert_eq!(result.records.len(), 8002);
    assert_eq!(result.records[0].model, "None/page1");
    assert_eq!(result.records[1000].model, "None/page2");
    assert_eq!(result.records[3000].model, "None/page4");
    assert_eq!(result.records[4000].model, "None/page5");
    assert_eq!(result.records[4001].model, "Some(10)/page1");
}
