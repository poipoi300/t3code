use crate::{McpError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use t3_contracts::{
    base_schemas::{ThreadId, TurnItemId},
    mcp_apps::*,
};

pub const MCP_APP_MODEL_CONTEXT_MAX_BYTES: usize = 16 * 1024;
pub const MCP_APP_MAX_HTML_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct AppReference {
    pub server: String,
    pub tool: String,
    pub provider_thread_id: Option<String>,
}

#[async_trait]
pub trait AppSession: Send + Sync {
    async fn list_tools(&self, server: &str) -> Result<Vec<Value>>;
    async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<McpAppCallToolResult>;
    async fn read_resource(&self, server: &str, uri: &str) -> Result<McpAppReadResourceResult>;
}

#[async_trait]
pub trait AppBackend: Send + Sync {
    async fn app(&self, thread_id: &ThreadId, item_id: &TurnItemId)
    -> Result<Option<AppReference>>;
    /// Returns only an already running session; never starts or resumes a provider.
    async fn session(&self, provider_thread_id: &str) -> Result<Option<Arc<dyn AppSession>>>;
    async fn fork_parent(&self, thread_id: &ThreadId) -> Result<Option<ThreadId>>;
    async fn set_model_context(&self, input: ModelContextInput) -> Result<()>;
}

pub fn callable_by_app(meta: &Value) -> bool {
    meta["ui"]["visibility"]
        .as_array()
        .is_none_or(|visibility| visibility.iter().any(|entry| entry == "app"))
}

fn fail(thread: &ThreadId, reason: &str) -> McpError {
    McpError::Domain {
        tag: "McpAppRequestError".into(),
        message: format!("MCP app request failed: {reason}"),
        fields: json!({"threadId":thread,"reason":reason}),
    }
}

fn adapter_failure(thread: &ThreadId, error: McpError) -> McpError {
    fail(
        thread,
        if matches!(&error,McpError::Domain {tag,..} if tag == "ProviderAdapterSessionStoppedError")
        {
            "session-stopped"
        } else {
            "request-failed"
        },
    )
}

pub struct AppRequests {
    backend: Arc<dyn AppBackend>,
}
impl AppRequests {
    pub fn new(backend: Arc<dyn AppBackend>) -> Self {
        Self { backend }
    }
    async fn resolve_app(&self, thread: &ThreadId, item: &TurnItemId) -> Result<AppReference> {
        self.backend
            .app(thread, item)
            .await
            .map_err(|_| fail(thread, "request-failed"))?
            .filter(|app| app.provider_thread_id.is_some())
            .ok_or_else(|| fail(thread, "not-an-app"))
    }
    async fn resolve(
        &self,
        thread: &ThreadId,
        item: &TurnItemId,
    ) -> Result<(AppReference, Arc<dyn AppSession>)> {
        let app = self.resolve_app(thread, item).await?;
        let session = self
            .backend
            .session(
                app.provider_thread_id
                    .as_deref()
                    .expect("resolved provider thread"),
            )
            .await
            .map_err(|error| adapter_failure(thread, error))?
            .ok_or_else(|| fail(thread, "session-stopped"))?;
        Ok((app, session))
    }
    pub async fn tool_info(&self, input: McpAppToolInfoInput) -> Result<McpAppToolInfo> {
        let (app, session) = self.resolve(&input.thread_id, &input.item_id).await?;
        let tool = session
            .list_tools(&app.server)
            .await
            .map_err(|error| adapter_failure(&input.thread_id, error))?
            .into_iter()
            .find(|tool| tool["name"] == input.name.0);
        let title = tool
            .as_ref()
            .and_then(|tool| tool["annotations"]["title"].as_str())
            .map(str::to_owned);
        Ok(McpAppToolInfo {
            callable: tool
                .as_ref()
                .is_some_and(|tool| callable_by_app(&tool["_meta"])),
            read_only: tool
                .as_ref()
                .is_some_and(|tool| tool["annotations"]["readOnlyHint"] == true),
            title: title.map(Some),
            tool: tool.map(Some),
        })
    }
    pub async fn call_tool(&self, input: McpAppCallToolInput) -> Result<McpAppCallToolResult> {
        let (app, session) = self.resolve(&input.thread_id, &input.item_id).await?;
        let tools = session
            .list_tools(&app.server)
            .await
            .map_err(|error| adapter_failure(&input.thread_id, error))?;
        if !tools
            .iter()
            .any(|tool| tool["name"] == input.name.0 && callable_by_app(&tool["_meta"]))
        {
            return Err(fail(&input.thread_id, "tool-not-callable"));
        }
        session
            .call_tool(
                &app.server,
                &input.name.0,
                serde_json::to_value(input.arguments).map_err(|_| McpError::Internal)?,
            )
            .await
            .map_err(|error| adapter_failure(&input.thread_id, error))
    }
    pub async fn read_resource(
        &self,
        input: McpAppReadResourceInput,
    ) -> Result<McpAppReadResourceResult> {
        let (app, session) = self.resolve(&input.thread_id, &input.item_id).await?;
        session
            .read_resource(&app.server, &input.uri.0)
            .await
            .map_err(|error| adapter_failure(&input.thread_id, error))
    }
    pub async fn update_model_context(&self, input: McpAppUpdateModelContextInput) -> Result<()> {
        let app = self.resolve_app(&input.thread_id, &input.item_id).await?;
        let mut current = Some(input.conversation_thread_id.clone());
        let mut seen = HashSet::new();
        let mut descends = false;
        while let Some(thread) = current {
            if thread == input.thread_id {
                descends = true;
                break;
            }
            if !seen.insert(thread.clone()) {
                break;
            }
            current = self
                .backend
                .fork_parent(&thread)
                .await
                .map_err(|_| fail(&input.thread_id, "request-failed"))?;
        }
        if !descends {
            return Err(fail(&input.thread_id, "not-an-app"));
        }
        let mut texts = Vec::new();
        if let Some(Some(content)) = input.content {
            for block in content {
                if block["type"] != "text" || !block["text"].is_string() {
                    return Err(fail(&input.thread_id, "unsupported-content"));
                }
                texts.push(block["text"].as_str().expect("checked text").to_owned());
            }
        }
        if let Some(content) = input.structured_content {
            texts.push(
                serde_json::to_value(content)
                    .map_err(|_| McpError::Internal)?
                    .to_string(),
            );
        }
        let text = texts.join("\n");
        if text.len() > MCP_APP_MODEL_CONTEXT_MAX_BYTES {
            return Err(fail(&input.thread_id, "request-failed"));
        }
        self.backend
            .set_model_context(ModelContextInput {
                thread_id: input.conversation_thread_id,
                item_id: input.item_id,
                server: app.server,
                tool: app.tool,
                text,
            })
            .await
            .map_err(|_| fail(&input.thread_id, "request-failed"))
    }
}

#[derive(Clone, Debug)]
pub struct ModelContextInput {
    pub thread_id: ThreadId,
    pub item_id: TurnItemId,
    pub server: String,
    pub tool: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelContextEntry {
    pub item_id: String,
    pub server: String,
    pub tool: String,
    pub text: String,
}

pub struct ModelContextStore {
    database: t3_persistence::Database,
}
impl ModelContextStore {
    pub fn new(database: t3_persistence::Database) -> Self {
        Self { database }
    }
    pub async fn set(&self, input: ModelContextInput, updated_at: String) -> Result<()> {
        self.database.writer(move |connection| {
            if input.text.trim().is_empty() {
                connection.execute("DELETE FROM mcp_app_model_context WHERE thread_id = ?1 AND item_id = ?2",(&input.thread_id.0,&input.item_id.0))?;
            } else {
                connection.execute("INSERT INTO mcp_app_model_context (thread_id,item_id,server,tool,text,updated_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(thread_id,item_id) DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at",(&input.thread_id.0,&input.item_id.0,&input.server,&input.tool,&input.text,&updated_at))?;
            }
            Ok(())
        }).await.map_err(|_|McpError::unavailable())
    }
    pub async fn for_thread(&self, thread_id: ThreadId) -> Result<Vec<ModelContextEntry>> {
        self.database.reader(move |connection| {
            let mut statement=connection.prepare("SELECT context.item_id,context.server,context.tool,context.text FROM mcp_app_model_context AS context JOIN orchestration_v2_projection_turn_items AS item ON item.turn_item_id = context.item_id LEFT JOIN orchestration_v2_projection_runs AS run ON run.run_id = item.run_id WHERE context.thread_id = ?1 AND (run.status IS NULL OR run.status <> 'rolled_back' OR run.thread_id <> context.thread_id) ORDER BY context.updated_at, context.item_id")?;
            let rows=statement.query_map([&thread_id.0],|row|Ok(ModelContextEntry {item_id:row.get(0)?,server:row.get(1)?,tool:row.get(2)?,text:row.get(3)?}))?;
            Ok(rows.collect::<std::result::Result<Vec<_>,_>>()?)
        }).await.map_err(|_|McpError::unavailable())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppCsp {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connect_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frame_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_uri_domains: Vec<String>,
}

pub fn read_csp(value: &Value) -> Option<AppCsp> {
    static DOMAIN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = DOMAIN.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^(?:https?|wss?)://(?:\*\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)*(?::\d{1,5})?$",
        )
        .expect("CSP domain pattern")
    });
    let domains = |key| {
        value[key]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| s.encode_utf16().count() <= 256 && pattern.is_match(s))
                    .take(32)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let csp = AppCsp {
        connect_domains: domains("connectDomains"),
        resource_domains: domains("resourceDomains"),
        frame_domains: domains("frameDomains"),
        base_uri_domains: domains("baseUriDomains"),
    };
    (csp != AppCsp::default()).then_some(csp)
}

pub fn content_security_policy(csp: Option<&AppCsp>) -> String {
    let empty = AppCsp::default();
    let csp = csp.unwrap_or(&empty);
    let list = |domains: &[String]| {
        if domains.is_empty() {
            "'none'".into()
        } else {
            domains.join(" ")
        }
    };
    let resources = csp.resource_domains.join(" ");
    [
        "default-src 'none'".to_owned(),
        format!("script-src 'unsafe-inline' {resources}")
            .trim()
            .into(),
        format!("style-src 'unsafe-inline' {resources}")
            .trim()
            .into(),
        format!("img-src data: blob: {resources}").trim().into(),
        format!("font-src data: {resources}").trim().into(),
        format!("media-src data: blob: {resources}").trim().into(),
        format!("connect-src {}", list(&csp.connect_domains)),
        format!("frame-src {}", list(&csp.frame_domains)),
        "object-src 'none'".into(),
        format!("base-uri {}", list(&csp.base_uri_domains)),
        "form-action 'none'".into(),
    ]
    .join("; ")
}

pub fn inject_csp(html: &str, csp: Option<&AppCsp>) -> String {
    let policy = content_security_policy(csp)
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">");
    static DOCTYPE: OnceLock<regex::Regex> = OnceLock::new();
    let doctype = DOCTYPE.get_or_init(|| {
        regex::Regex::new(r"(?i)^\u{feff}?\s*<!doctype[^>]*>").expect("doctype pattern")
    });
    if let Some(prefix) = doctype.find(html) {
        format!("{}{meta}{}", prefix.as_str(), &html[prefix.end()..])
    } else {
        format!("<!doctype html>{meta}{html}")
    }
}

pub fn safe_thread_segment(thread: &str) -> Option<String> {
    let mut segment = String::new();
    for ch in thread.trim().to_lowercase().chars() {
        let ch = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
            ch
        } else {
            '-'
        };
        if ch == '-' && segment.ends_with('-') {
            continue;
        }
        segment.push(ch);
    }
    let segment = segment
        .trim_matches(['-', '_'])
        .chars()
        .take(80)
        .collect::<String>();
    let segment = segment.trim_end_matches(['-', '_']);
    if segment.is_empty() {
        None
    } else {
        Some(if segment == "pending" {
            "_pending".into()
        } else {
            segment.into()
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub attachment_id: String,
    pub server: String,
    pub tool: String,
    pub resource_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<AppCsp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefers_border: Option<bool>,
}

pub struct SnapshotStore {
    attachments_dir: PathBuf,
}
impl SnapshotStore {
    pub fn new(attachments_dir: impl AsRef<Path>) -> Self {
        Self {
            attachments_dir: attachments_dir.as_ref().into(),
        }
    }
    /// Synchronous file IO: call from a blocking task when serving an async request.
    pub fn snapshot(
        &self,
        thread: &ThreadId,
        server: &str,
        tool: &str,
        uri: &str,
        contents: &[Value],
    ) -> Result<Option<AppSnapshot>> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        if !uri.starts_with("ui://") {
            return Ok(None);
        }
        for content in contents {
            if content.get("uri").is_some_and(|v| v != uri) {
                continue;
            }
            let mime = content["mimeType"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<String>()
                .to_ascii_lowercase();
            if mime != "text/html;profile=mcp-app" {
                continue;
            }
            let html = if let Some(text) = content["text"].as_str() {
                if text.encode_utf16().count() > MCP_APP_MAX_HTML_BYTES {
                    return Ok(None);
                }
                text.into()
            } else if let Some(blob) = content["blob"].as_str() {
                if blob.len() > MCP_APP_MAX_HTML_BYTES.div_ceil(3) * 4 {
                    return Ok(None);
                }
                let bytes = STANDARD
                    .decode(blob)
                    .map_err(|_| McpError::InvalidInput("Invalid app resource base64".into()))?;
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                continue;
            };
            if html.trim().is_empty() {
                continue;
            }
            if html.len() > MCP_APP_MAX_HTML_BYTES {
                return Ok(None);
            }
            let Some(segment) = safe_thread_segment(&thread.0) else {
                return Ok(None);
            };
            let attachment_id = format!("{segment}-{}-html", uuid::Uuid::new_v4());
            let path = self.attachments_dir.join(format!("{attachment_id}.html"));
            let ui = &content["_meta"]["ui"];
            let csp = read_csp(&ui["csp"]);
            if std::fs::write(&path, inject_csp(&html, csp.as_ref())).is_err() {
                let _ = std::fs::remove_file(path);
                return Err(McpError::unavailable());
            }
            let mut permissions = serde_json::Map::new();
            for key in ["camera", "microphone", "geolocation", "clipboardWrite"] {
                if ui["permissions"][key].is_object() {
                    permissions.insert(key.into(), json!({}));
                }
            }
            return Ok(Some(AppSnapshot {
                attachment_id,
                server: server.into(),
                tool: tool.into(),
                resource_uri: uri.into(),
                csp,
                permissions: (!permissions.is_empty()).then_some(Value::Object(permissions)),
                prefers_border: ui["prefersBorder"].as_bool(),
            }));
        }
        Ok(None)
    }
}
