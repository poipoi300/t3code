#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use t3_contracts::{
    base_schemas::{ThreadId, TurnItemId},
    mcp_apps::*,
};
use t3_mcp::{apps::*, *};

struct Backend {
    live: AtomicBool,
    contexts: Mutex<Vec<ModelContextInput>>,
    calls: Mutex<Vec<Value>>,
}
struct Harness(Arc<Backend>);
impl std::ops::Deref for Harness {
    type Target = Backend;
    fn deref(&self) -> &Backend {
        &self.0
    }
}
#[async_trait]
impl AppSession for Backend {
    async fn list_tools(&self, server: &str) -> Result<Vec<Value>> {
        assert_eq!(server, "todos");
        Ok(vec![
            json!({"name":"read","annotations":{"title":"Read todos","readOnlyHint":true}}),
            json!({"name":"hidden","_meta":{"ui":{"visibility":["model"]}}}),
        ])
    }
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<McpAppCallToolResult> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({"server":server,"tool":tool,"arguments":arguments}));
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":"done"}]})).unwrap())
    }
    async fn read_resource(&self, server: &str, uri: &str) -> Result<McpAppReadResourceResult> {
        assert_eq!(server, "todos");
        Ok(McpAppReadResourceResult {
            contents: vec![json!({"uri":uri,"text":"resource"})],
        })
    }
}
#[async_trait]
impl AppBackend for Harness {
    async fn app(&self, _thread: &ThreadId, item: &TurnItemId) -> Result<Option<AppReference>> {
        Ok((item.0 == "app").then(|| AppReference {
            server: "todos".into(),
            tool: "show".into(),
            provider_thread_id: Some("provider-thread".into()),
        }))
    }
    async fn session(&self, id: &str) -> Result<Option<Arc<dyn AppSession>>> {
        assert_eq!(id, "provider-thread");
        Ok(self
            .live
            .load(Ordering::Relaxed)
            .then(|| self.0.clone() as Arc<dyn AppSession>))
    }
    async fn fork_parent(&self, id: &ThreadId) -> Result<Option<ThreadId>> {
        Ok((id.0 == "fork").then(|| ThreadId("original".into())))
    }
    async fn set_model_context(&self, input: ModelContextInput) -> Result<()> {
        self.contexts.lock().unwrap().push(input);
        Ok(())
    }
}

#[tokio::test]
async fn app_requests_enforce_server_tool_visibility_stopped_sessions_and_fork_context_ownership() {
    let backend = Arc::new(Backend {
        live: AtomicBool::new(true),
        contexts: Mutex::new(vec![]),
        calls: Mutex::new(vec![]),
    });
    let requests = AppRequests::new(Arc::new(Harness(backend.clone())));
    let info = requests
        .tool_info(
            serde_json::from_value(json!({"threadId":"original","itemId":"app","name":"read"}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(info.callable && info.read_only);
    assert_eq!(info.title, Some(Some("Read todos".into())));
    for name in ["hidden", "missing"] {
        let error = requests
            .call_tool(
                serde_json::from_value(
                    json!({"threadId":"original","itemId":"app","name":name,"arguments":{}}),
                )
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.encoded()["reason"], "tool-not-callable");
    }
    requests
        .call_tool(
            serde_json::from_value(
                json!({"threadId":"original","itemId":"app","name":"read","arguments":{"value":3}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        backend.calls.lock().unwrap().as_slice(),
        &[json!({"server":"todos","tool":"read","arguments":{"value":3}})]
    );
    backend.live.store(false, Ordering::Relaxed);
    let error = requests
        .read_resource(
            serde_json::from_value(
                json!({"threadId":"original","itemId":"app","uri":"ui://todos"}),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.encoded()["reason"], "session-stopped");
    requests.update_model_context(serde_json::from_value(json!({"threadId":"original","itemId":"app","conversationThreadId":"fork","content":[{"type":"text","text":"new context"}],"structuredContent":{"count":3}})).unwrap()).await.unwrap();
    {
        let context = backend.contexts.lock().unwrap();
        assert_eq!(context[0].thread_id.0, "fork");
        assert_eq!(context[0].text, "new context\n{\"count\":3}");
    }
    for (target, content, reason) in [
        ("unrelated", json!([]), "not-an-app"),
        (
            "fork",
            json!([{"type":"image","data":"x"}]),
            "unsupported-content",
        ),
        (
            "fork",
            json!([{"type":"text","text":"界".repeat(6000)}]),
            "request-failed",
        ),
    ] {
        let error=requests.update_model_context(serde_json::from_value(json!({"threadId":"original","itemId":"app","conversationThreadId":target,"content":content})).unwrap()).await.unwrap_err();
        assert_eq!(error.encoded()["reason"], reason);
    }
}

#[tokio::test]
async fn persisted_context_filters_deleted_and_rolled_back_items_but_preserves_fork_history() {
    let database = tokio::task::spawn_blocking(|| t3_persistence::Database::open(":memory:"))
        .await
        .unwrap()
        .unwrap();
    database.writer(|connection| {
        for (item,run,status) in [("kept","run-a","completed"),("rolled","run-bb","rolled_back")] {
            connection.execute("INSERT INTO orchestration_v2_projection_runs (run_id,thread_id,ordinal,provider,status,requested_at,payload_json) VALUES (?1,'original',?2,'codex',?3,'2026-01-01','{}')",(run,run.len() as i64,status))?;
            connection.execute("INSERT INTO orchestration_v2_projection_turn_items (turn_item_id,thread_id,run_id,ordinal,type,status,updated_at,payload_json) VALUES (?1,'original',?2,0,'dynamic_tool','completed','2026-01-01','{}')",(item,run))?;
        }Ok(())
    }).await.unwrap();
    let store = ModelContextStore::new(database.clone());
    for (thread, item, text) in [
        ("original", "kept", "kept"),
        ("original", "rolled", "rolled back"),
        ("original", "deleted", "deleted"),
        ("fork", "kept", "fork kept"),
        ("fork", "rolled", "fork inherited"),
    ] {
        store
            .set(
                ModelContextInput {
                    thread_id: ThreadId(thread.into()),
                    item_id: TurnItemId(item.into()),
                    server: "todos".into(),
                    tool: "show".into(),
                    text: text.into(),
                },
                "2026-01-01T00:00:00.000Z".into(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .for_thread(ThreadId("original".into()))
            .await
            .unwrap()
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>(),
        ["kept"]
    );
    assert_eq!(
        store
            .for_thread(ThreadId("fork".into()))
            .await
            .unwrap()
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>(),
        ["fork kept", "fork inherited"]
    );
    store
        .set(
            ModelContextInput {
                thread_id: ThreadId("original".into()),
                item_id: TurnItemId("kept".into()),
                server: "other".into(),
                tool: "other".into(),
                text: "   ".into(),
            },
            "2026-01-01T00:00:00.000Z".into(),
        )
        .await
        .unwrap();
    assert!(
        store
            .for_thread(ThreadId("original".into()))
            .await
            .unwrap()
            .is_empty()
    );
    drop(store);
    database.close().await.unwrap();
}

#[test]
fn app_snapshot_validates_resources_bounds_html_and_injects_csp_before_content() {
    let temp = tempfile::tempdir().unwrap();
    let store = SnapshotStore::new(temp.path());
    let contents = [
        json!({"uri":"ui://todos","mimeType":"TEXT/HTML; profile=mcp-app","text":"<!doctype html><h1>Todos</h1>","_meta":{"ui":{"csp":{"connectDomains":["https://api.example.com","'self'","https://bad.test/path"],"resourceDomains":["https://*.example.com"]},"permissions":{"camera":{},"microphone":true},"prefersBorder":false}}}),
    ];
    let app = store
        .snapshot(
            &ThreadId("Thread:test".into()),
            "todos",
            "show",
            "ui://todos",
            &contents,
        )
        .unwrap()
        .unwrap();
    assert!(app.attachment_id.starts_with("thread-test-"));
    assert_eq!(app.permissions, Some(json!({"camera":{}})));
    let html =
        std::fs::read_to_string(temp.path().join(format!("{}.html", app.attachment_id))).unwrap();
    assert!(html.starts_with("<!doctype html><meta http-equiv=\"Content-Security-Policy\""));
    assert!(!html.contains("bad.test"));
    assert!(!html.contains("'self'"));
    assert!(
        store
            .snapshot(
                &ThreadId("test".into()),
                "todos",
                "show",
                "https://todos",
                &contents
            )
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .snapshot(
                &ThreadId("test".into()),
                "todos",
                "show",
                "ui://different",
                &contents
            )
            .unwrap()
            .is_none()
    );
    assert!(store.snapshot(&ThreadId("test".into()),"todos","show","ui://todos",&[json!({"mimeType":"text/html;profile=mcp-app","text":"界".repeat(MCP_APP_MAX_HTML_BYTES/2)})]).unwrap().is_none());
}
