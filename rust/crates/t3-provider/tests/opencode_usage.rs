#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use t3_provider::{
    environment::Environment, opencode_http::OpenCodeHttp, opencode_usage::*, sensitive::*,
    transport::*,
};
use tokio::sync::Mutex;
const CHECKED: &str = "2026-11-13T00:00:00.000Z";
struct Http {
    replies: Mutex<VecDeque<RawHttpResponse>>,
    calls: Mutex<Vec<HttpRequest>>,
}
fn reply(status: u16, value: Value) -> RawHttpResponse {
    RawHttpResponse {
        status,
        content_type: Some("application/json".into()),
        body: Sensitive::new(serde_json::to_vec(&value).unwrap()),
    }
}
#[async_trait]
impl HttpTransport for Http {
    async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        self.calls.lock().await.push(request);
        self.replies
            .lock()
            .await
            .pop_front()
            .ok_or(ServiceError::Network)
    }
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        let response = self.raw(request).await?;
        Ok(HttpResponse {
            status: response.status,
            body: Sensitive::new(
                serde_json::from_slice(response.body.expose())
                    .map_err(|_| ServiceError::Response)?,
            ),
        })
    }
}
#[tokio::test]
async fn server_usage_preserves_authoritative_absence_and_scopes_go_and_console_budgets() {
    let http=Arc::new(Http{replies:Mutex::new(vec![reply(404,json!({})),reply(200,json!({"location":{"directory":"/project"},"data":null})),reply(200,json!({"location":{"directory":"/project"},"data":{
        "workspace":{"id":"workspace","name":" Main ","usageURL":"https://opencode.ai/console"},
        "go":{"rolling":{"percent":5,"limited":true,"resetsAt":0},"weekly":{"percent":40,"limited":false,"resetsAt":0},"monthly":{"percent":50,"limited":false,"resetsAt":0}},
        "budgets":[
            {"id":"models","limit":10,"spent":12,"scope":"each","per":"model","coverage":"models","models":[{"providerID":"openai","modelID":"a","name":"A","spent":12},{"providerID":"openai","modelID":"b","name":"B","spent":1}],"shared":false,"resetsAt":0},
            {"id":"aggregate","limit":10,"spent":12,"scope":"combined","per":"model","coverage":"models","models":[{"providerID":"openai","modelID":"a","name":"A"}],"shared":false,"resetsAt":0},
            {"id":"all","limit":0,"spent":0,"scope":"each","per":"set","coverage":"all","models":[],"shared":true,"resetsAt":0}
        ]
    }})),reply(200,json!({"data":[{"id":"console","integrationID":"opencode"},{"id":"opencode-go","integrationID":"opencode"},{"id":"other","integrationID":"other"}]})),reply(200,json!({"location":{"directory":"/project"},"data":{"workspace":{"id":"bad","usageURL":"https://invalid"},"budgets":[],"go":null}}))].into()),calls:Mutex::new(vec![])});
    let connection = OpenCodeHttp::new(http.clone(), "https://remote.invalid", None).unwrap();
    assert!(server(&connection, "/project", CHECKED).await.is_none());
    let empty =
        serde_json::to_value(server(&connection, "/project", CHECKED).await.unwrap()).unwrap();
    assert_eq!(empty["unavailable"]["reason"], "unsupported");
    let limits =
        serde_json::to_value(server(&connection, "/project", CHECKED).await.unwrap()).unwrap();
    let windows = limits["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 7);
    let get = |id: &str| windows.iter().find(|window| window["id"] == id).unwrap();
    assert_eq!(get("go_rolling")["usedPercent"], 100);
    assert_eq!(get("go_rolling")["windowDurationMins"], 300);
    assert_eq!(get("budget_models_openai_a")["usedPercent"], 100);
    assert_eq!(get("budget_models_openai_b")["usedPercent"], 10);
    assert_eq!(get("budget_aggregate")["modelIds"], json!([]));
    assert_eq!(get("budget_all")["providerIds"], json!(["console"]));
    assert_eq!(limits["externalUsage"]["label"], "Main");
    let invalid =
        serde_json::to_value(server(&connection, "/project", CHECKED).await.unwrap()).unwrap();
    assert_eq!(invalid["unavailable"]["reason"], "probeFailed");
}
#[tokio::test]
async fn legacy_credentials_select_active_newest_before_decoding_and_inline_auth_overrides_local_storage()
 {
    use t3_persistence::rusqlite::{Connection, params};
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("opencode");
    std::fs::create_dir(&data).unwrap();
    let mut environment = Environment::from([
        (
            "XDG_DATA_HOME".into(),
            root.path().to_string_lossy().into_owned(),
        ),
        ("OPENCODE_API_KEY".into(), "environment-secret".into()),
    ]);
    std::fs::write(
        data.join("auth.json"),
        json!({"opencode-go":{"type":"api","key":"file-secret"}}).to_string(),
    )
    .unwrap();
    let database = Connection::open(data.join("opencode.db")).unwrap();
    database.execute_batch("CREATE TABLE credential (id TEXT,integration_id TEXT,active INTEGER,time_created INTEGER,value TEXT);").unwrap();
    for (id, integration, active, created, value) in [
        (
            "old",
            "opencode",
            1,
            1,
            json!({"type":"key","key":"old-secret"}),
        ),
        (
            "new",
            "opencode",
            1,
            2,
            json!({"type":"oauth","key":"oauth-secret"}),
        ),
        (
            "go",
            "opencode-go",
            1,
            1,
            json!({"type":"key","key":"database-secret","metadata":{"server":"https://opencode.ai/console/?ignored=yes"}}),
        ),
        (
            "inactive",
            "opencode-go",
            0,
            3,
            json!({"type":"key","key":"inactive-secret"}),
        ),
    ] {
        database
            .execute(
                "INSERT INTO credential VALUES (?,?,?,?,?)",
                params![id, integration, active, created, value.to_string()],
            )
            .unwrap();
    }
    let selected = keys(&environment, root.path()).await.unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|key| key.expose().as_str())
            .collect::<Vec<_>>(),
        ["database-secret", "file-secret"]
    );
    assert!(!format!("{selected:?}").contains("secret"));
    database.execute("UPDATE credential SET value=? WHERE id='new'",[json!({"type":"key","key":"console-secret","metadata":{"server":"https://self-hosted.invalid/console"}}).to_string()]).unwrap();
    assert_eq!(keys(&environment, root.path()).await.unwrap().len(), 2);
    environment.insert("OPENCODE_AUTH_CONTENT".into(), "{}".into());
    assert_eq!(
        keys(&environment, root.path()).await.unwrap()[0].expose(),
        "environment-secret"
    );
    environment.insert(
        "OPENCODE_AUTH_CONTENT".into(),
        json!({"opencode-go":{"type":"api","key":""}}).to_string(),
    );
    assert!(keys(&environment, root.path()).await.unwrap().is_empty());
}
#[tokio::test]
async fn legacy_usage_retries_only_rejected_credentials_and_external_servers_never_read_local_accounts()
 {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("opencode");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(
        data.join("auth.json"),
        json!({"opencode-go":{"type":"api","key":"legacy-secret"}}).to_string(),
    )
    .unwrap();
    let database = t3_persistence::rusqlite::Connection::open(data.join("opencode.db")).unwrap();
    database.execute_batch("CREATE TABLE credential (id TEXT,integration_id TEXT,active INTEGER,time_created INTEGER,value TEXT);").unwrap();
    database
        .execute(
            "INSERT INTO credential VALUES ('console','opencode',1,1,?)",
            [json!({"type":"key","key":"rejected-secret"}).to_string()],
        )
        .unwrap();
    let environment = Environment::from([(
        "XDG_DATA_HOME".into(),
        root.path().to_string_lossy().into_owned(),
    )]);
    let meter = json!({"percent":40,"resetsAt":"2026-11-14T00:00:00Z"});
    let http = Http {
        replies: Mutex::new(
            vec![
                RawHttpResponse {
                    status: 401,
                    content_type: None,
                    body: Sensitive::new(b"unauthorized".to_vec()),
                },
                reply(
                    200,
                    json!({"usage":{"rolling":meter,"weekly":meter,"monthly":meter}}),
                ),
            ]
            .into(),
        ),
        calls: Mutex::new(vec![]),
    };
    let limits = legacy(&http, &environment, root.path(), true, true, CHECKED).await;
    assert!(limits.windows.is_empty());
    assert!(http.calls.lock().await.is_empty());
    let limits = legacy(&http, &environment, root.path(), true, false, CHECKED).await;
    assert_eq!(limits.windows.len(), 3);
    assert!(limits.credential_fingerprint.is_some());
    let calls = http.calls.lock().await;
    assert!(!format!("{calls:?}").contains("legacy-secret"));
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].headers["authorization"].expose(),
        "Bearer rejected-secret"
    );
    assert_eq!(
        calls[1].headers["authorization"].expose(),
        "Bearer legacy-secret"
    );
    assert!(
        !serde_json::to_string(&limits)
            .unwrap()
            .contains("legacy-secret")
    );
}
