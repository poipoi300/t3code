//! Pure MCP Apps bridge. Host effects are completed explicitly and cannot outlive disposal.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use t3_contracts::mcp_apps::McpAppCallToolResult;

pub const MCP_APP_PROTOCOL_VERSION: &str = "2026-01-26";
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;
pub const MAX_PENDING_REQUESTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppResource {
    pub uri: String,
    pub mime: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppDownload {
    Embedded {
        name: String,
        mime_type: String,
        bytes: Vec<u8>,
    },
    Link {
        name: String,
        uri: String,
        mime_type: Option<String>,
    },
}
pub fn mcp_resource_bytes(content: &Value) -> Option<Vec<u8>> {
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text.as_bytes().to_vec());
    }
    decode_base64(content.get("blob")?.as_str()?)
}
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    let length = text.len();
    for (index, chunk) in text.as_bytes().as_chunks::<4>().0.iter().enumerate() {
        let mut n = [0u8; 4];
        let mut padding = 0;
        for (i, &b) in chunk.iter().enumerate() {
            n[i] = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 && index * 4 + 4 == length => {
                    padding += 1;
                    0
                }
                _ => return None,
            };
            if padding > 0 && b != b'=' {
                return None;
            }
        }
        out.push((n[0] << 2) | (n[1] >> 4));
        if padding < 2 {
            out.push((n[1] << 4) | (n[2] >> 2));
        }
        if padding == 0 {
            out.push((n[2] << 6) | n[3]);
        }
    }
    Some(out)
}
fn download_name(uri: Option<&str>, name: Option<&str>) -> String {
    let raw = name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| uri.and_then(|uri| uri.split(['/', '?', '#']).rfind(|s| !s.is_empty())))
        .unwrap_or("download");
    let mut safe = String::new();
    let mut length = 0;
    for c in raw.chars() {
        let c = if c < ' ' || "\\/:*?\"<>|".contains(c) {
            '-'
        } else {
            c
        };
        if length + c.len_utf16() > 200 {
            break;
        }
        safe.push(c);
        length += c.len_utf16();
    }
    if safe.is_empty() {
        "download".into()
    } else {
        safe
    }
}
pub fn read_downloads(value: &Value) -> Option<Vec<AppDownload>> {
    let contents = value.as_array()?;
    if contents.is_empty() {
        return None;
    }
    contents
        .iter()
        .map(|entry| match entry.get("type")?.as_str()? {
            "resource" => {
                let resource = entry.get("resource")?;
                Some(AppDownload::Embedded {
                    name: download_name(
                        resource.get("uri").and_then(Value::as_str),
                        resource.get("name").and_then(Value::as_str),
                    ),
                    mime_type: resource
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .into(),
                    bytes: mcp_resource_bytes(resource)?,
                })
            }
            "resource_link" => {
                let uri = entry.get("uri")?.as_str()?;
                Some(AppDownload::Link {
                    name: download_name(Some(uri), entry.get("name").and_then(Value::as_str)),
                    uri: uri.into(),
                    mime_type: entry
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
            }
            _ => None,
        })
        .collect()
}
pub fn normalize_tool_result(result: &McpAppCallToolResult) -> Value {
    let mut value = json!({"content":result.content});
    if let Some(Some(structured)) = &result.structured_content
        && structured.is_object()
    {
        value["structuredContent"] = structured.clone();
    }
    if result.is_error == Some(Some(true)) {
        value["isError"] = json!(true);
    }
    if let Some(Some(meta)) = &result._meta
        && meta.is_object()
    {
        value["_meta"] = meta.clone();
    }
    value
}
#[derive(Debug, Clone, PartialEq)]
pub enum AppOperation {
    CallTool { name: String, arguments: Value },
    ReadResource(String),
    OpenLink(String),
    Message(String),
    UpdateModelContext(Value),
    DisplayMode(String),
    Download(Vec<AppDownload>),
}
#[derive(Debug, Clone, PartialEq)]
pub enum AppEffect {
    Post(Value),
    Run {
        token: u64,
        operation: AppOperation,
    },
    SizeChanged {
        width: Option<f64>,
        height: Option<f64>,
    },
    RequestTeardown,
    Disposed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppFailure {
    Refused(String),
    Failed(String),
}
struct Pending {
    id: Value,
    download: bool,
    tool: bool,
    display_mode: bool,
}
pub struct McpAppHost {
    initialized: bool,
    disposed: bool,
    version: String,
    sandbox: Value,
    context: Value,
    sent_context: Option<Value>,
    app_modes: Option<Vec<String>>,
    tool_call: Option<(Value, Option<McpAppCallToolResult>)>,
    tool_sent: bool,
    next: u64,
    pending: BTreeMap<u64, Pending>,
    teardown: Option<(String, u64)>,
}
fn post(id: Value, result: Value) -> AppEffect {
    AppEffect::Post(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
fn fail(id: Value, code: i32, message: impl Into<String>) -> AppEffect {
    AppEffect::Post(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}}))
}
fn notify(method: &str, params: Value) -> AppEffect {
    AppEffect::Post(json!({"jsonrpc":"2.0","method":method,"params":params}))
}
fn is_id(value: &Value) -> bool {
    value.is_string() || value.is_number()
}
impl McpAppHost {
    pub fn new(version: String, sandbox: Value, context: Value) -> Self {
        Self {
            initialized: false,
            disposed: false,
            version,
            sandbox,
            context,
            sent_context: None,
            app_modes: None,
            tool_call: None,
            tool_sent: false,
            next: 0,
            pending: BTreeMap::new(),
            teardown: None,
        }
    }
    pub fn receive_json(&mut self, text: &str) -> Vec<AppEffect> {
        if text.len() > MAX_MESSAGE_BYTES {
            // Do not allocate a JSON tree for an oversized untrusted frame.
            return vec![];
        }
        let Ok(value) = serde_json::from_str(text) else {
            return vec![];
        };
        self.receive(value)
    }
    pub fn receive(&mut self, value: Value) -> Vec<AppEffect> {
        if self.disposed || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return vec![];
        }
        let id = value.get("id").filter(|id| is_id(id));
        let method = value.get("method").and_then(Value::as_str);
        // Value callers already own the input; stream serialization bounds the additional allocation.
        if !fits_message(&value) {
            return if let (Some(id), Some(_)) = (id, method) {
                vec![fail(id.clone(), -32600, "Message too large.")]
            } else {
                vec![]
            };
        }
        let Some(method) = method else {
            if (value.get("result").is_some() || value.get("error").is_some())
                && self
                    .teardown
                    .as_ref()
                    .is_some_and(|(expected, _)| id == Some(&json!(expected)))
            {
                return self.dispose();
            }
            return vec![];
        };
        let params = value
            .get("params")
            .filter(|p| p.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(id) = id {
            return self.request(id.clone(), method, params);
        }
        match method {
            "ui/notifications/initialized" => {
                if self.initialized {
                    return vec![];
                }
                self.initialized = true;
                let mut effects = self.send_tool_call();
                effects.extend(self.context_update());
                effects
            }
            "ui/notifications/size-changed" => {
                let width = params.get("width").and_then(Value::as_f64);
                let height = params.get("height").and_then(Value::as_f64);
                if width.is_some() || height.is_some() {
                    vec![AppEffect::SizeChanged { width, height }]
                } else {
                    vec![]
                }
            }
            "ui/notifications/request-teardown" => vec![AppEffect::RequestTeardown],
            _ => vec![],
        }
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Vec<AppEffect> {
        if method == "ui/initialize" {
            self.app_modes = params
                .get("appCapabilities")
                .and_then(|v| v.get("availableDisplayModes"))
                .and_then(Value::as_array)
                .map(|modes| {
                    modes
                        .iter()
                        .filter_map(|m| m.as_str().map(str::to_owned))
                        .collect()
                });
            self.sent_context = Some(self.context.clone());
            return vec![post(
                id,
                json!({"protocolVersion":MCP_APP_PROTOCOL_VERSION,"hostInfo":{"name":"t3-code","version":self.version},"hostCapabilities":{"openLinks":{},"serverTools":{},"serverResources":{},"logging":{},"message":{"text":{}},"updateModelContext":{"text":{},"structuredContent":{}},"downloadFile":{},"sandbox":self.sandbox},"hostContext":self.context}),
            )];
        }
        if method == "ping" {
            return vec![post(id, json!({}))];
        }
        let operation = match self.operation(method, &params) {
            Ok(operation) => operation,
            Err((code, message)) => return vec![fail(id, code, message)],
        };
        let Some(operation) = operation else {
            return vec![post(id, json!({"mode":self.context.get("displayMode")}))];
        };
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return vec![fail(id, -32000, "Too many requests in flight.")];
        }
        self.next = self.next.wrapping_add(1);
        let token = self.next;
        self.pending.insert(
            token,
            Pending {
                id,
                download: matches!(operation, AppOperation::Download(_)),
                tool: matches!(operation, AppOperation::CallTool { .. }),
                display_mode: matches!(operation, AppOperation::DisplayMode(_)),
            },
        );
        vec![AppEffect::Run { token, operation }]
    }
    fn operation(&self, method: &str, p: &Value) -> Result<Option<AppOperation>, (i32, String)> {
        let invalid = |message: &str| (-32602, message.to_owned());
        Ok(Some(match method {
            "tools/call" => {
                let name = p
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| invalid("tools/call needs a tool name and object arguments."))?;
                let args = p
                    .get("arguments")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !args.is_object() {
                    return Err(invalid(
                        "tools/call needs a tool name and object arguments.",
                    ));
                }
                AppOperation::CallTool {
                    name: name.into(),
                    arguments: args,
                }
            }
            "resources/read" => AppOperation::ReadResource(
                p.get("uri")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| invalid("resources/read needs a uri."))?
                    .into(),
            ),
            "ui/open-link" => {
                let url = p
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("Only http(s) links can be opened."))?;
                if !url.to_ascii_lowercase().starts_with("http://")
                    && !url.to_ascii_lowercase().starts_with("https://")
                {
                    return Err(invalid("Only http(s) links can be opened."));
                }
                AppOperation::OpenLink(url.into())
            }
            "ui/message" => {
                let content = p.get("content").ok_or_else(|| {
                    invalid("Only text messages from the user role are supported.")
                })?;
                let single = [content.clone()];
                let blocks = content.as_array().map_or(single.as_slice(), Vec::as_slice);
                if p.get("role").and_then(Value::as_str) != Some("user") || blocks.is_empty() {
                    return Err(invalid(
                        "Only text messages from the user role are supported.",
                    ));
                }
                let texts = blocks
                    .iter()
                    .map(|b| {
                        if b.get("type").and_then(Value::as_str) == Some("text") {
                            b.get("text").and_then(Value::as_str)
                        } else {
                            None
                        }
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        invalid("Only text messages from the user role are supported.")
                    })?;
                let text = texts.join("\n").trim().to_owned();
                if text.is_empty() {
                    return Err(invalid("ui/message needs non-empty text."));
                }
                AppOperation::Message(text)
            }
            "ui/request-display-mode" => {
                let mode = p.get("mode").and_then(Value::as_str);
                let allowed = mode.is_some_and(|m| {
                    self.context
                        .get("availableDisplayModes")
                        .and_then(Value::as_array)
                        .is_some_and(|modes| modes.contains(&json!(m)))
                        && self
                            .app_modes
                            .as_ref()
                            .is_none_or(|modes| modes.iter().any(|mode| mode == m))
                });
                if !allowed || p.get("mode") == self.context.get("displayMode") {
                    return Ok(None);
                }
                AppOperation::DisplayMode(mode.unwrap_or_default().into())
            }
            "ui/update-model-context" => {
                if p.get("content").is_some_and(|v| !v.is_array())
                    || p.get("structuredContent").is_some_and(|v| !v.is_object())
                {
                    return Err(invalid(
                        "Model context needs content blocks or an object of structured content.",
                    ));
                }
                let mut context = json!({});
                for key in ["content", "structuredContent"] {
                    if let Some(v) = p.get(key) {
                        context[key] = v.clone();
                    }
                }
                AppOperation::UpdateModelContext(context)
            }
            "ui/download-file" => AppOperation::Download(
                read_downloads(p.get("contents").unwrap_or(&Value::Null)).ok_or_else(|| {
                    invalid("ui/download-file needs embedded resources or resource links.")
                })?,
            ),
            _ => return Err((-32601, format!("Method not found: {method}"))),
        }))
    }
    pub fn complete(&mut self, token: u64, result: Result<Value, AppFailure>) -> Vec<AppEffect> {
        let Some(pending) = self.pending.remove(&token) else {
            return vec![];
        };
        if self.disposed {
            return vec![];
        }
        let effect = match result {
            Ok(value) => {
                let value = if pending.tool {
                    serde_json::from_value::<McpAppCallToolResult>(value)
                        .map(|v| normalize_tool_result(&v))
                        .unwrap_or_else(|_| json!({"content":[]}))
                } else if pending.display_mode {
                    json!({"mode":value})
                } else {
                    value
                };
                post(pending.id, value)
            }
            Err(AppFailure::Refused(_)) if pending.download => {
                post(pending.id, json!({"isError":true}))
            }
            Err(AppFailure::Refused(message) | AppFailure::Failed(message)) => fail(
                pending.id,
                -32000,
                if message.trim().is_empty() {
                    "Request failed.".into()
                } else {
                    message
                },
            ),
        };
        vec![effect]
    }
    pub fn set_tool_call(
        &mut self,
        args: Value,
        result: Option<McpAppCallToolResult>,
    ) -> Vec<AppEffect> {
        if self.disposed {
            return vec![];
        }
        if self.tool_call.is_none() || !self.tool_sent {
            if self.tool_call.as_ref().is_none_or(|(_, r)| r.is_none()) {
                self.tool_call = Some((args, result));
            }
            return self.send_tool_call();
        }
        if let Some((_, old)) = self.tool_call.as_mut()
            && old.is_none()
            && let Some(result) = result
        {
            let effect = notify(
                "ui/notifications/tool-result",
                normalize_tool_result(&result),
            );
            *old = Some(result);
            return vec![effect];
        }
        vec![]
    }
    fn send_tool_call(&mut self) -> Vec<AppEffect> {
        if !self.initialized || self.tool_sent {
            return vec![];
        }
        let Some((args, result)) = &self.tool_call else {
            return vec![];
        };
        self.tool_sent = true;
        let mut effects = vec![notify(
            "ui/notifications/tool-input",
            json!({"arguments":if args.is_object(){args.clone()}else {json!({})}}),
        )];
        if let Some(result) = result {
            effects.push(notify(
                "ui/notifications/tool-result",
                normalize_tool_result(result),
            ));
        }
        effects
    }
    pub fn update_host_context(&mut self, context: Value) -> Vec<AppEffect> {
        self.context = context;
        if self.disposed {
            vec![]
        } else {
            self.context_update()
        }
    }
    fn context_update(&mut self) -> Vec<AppEffect> {
        if !self.initialized {
            return vec![];
        }
        let Some(old) = self.sent_context.as_ref() else {
            return vec![];
        };
        let mut changes = serde_json::Map::new();
        if let Some(next) = self.context.as_object() {
            for (key, value) in next {
                if old.get(key) != Some(value) {
                    changes.insert(key.clone(), value.clone());
                }
            }
        }
        self.sent_context = Some(self.context.clone());
        if changes.is_empty() {
            vec![]
        } else {
            vec![notify(
                "ui/notifications/host-context-changed",
                Value::Object(changes),
            )]
        }
    }
    pub fn teardown(&mut self, now: u64) -> Vec<AppEffect> {
        if self.disposed || !self.initialized {
            return self.dispose();
        }
        if self.teardown.is_some() {
            return vec![];
        }
        let id = format!("t3-host-{}", self.next);
        self.next = self.next.wrapping_add(1);
        self.teardown = Some((id.clone(), now.saturating_add(2000)));
        vec![AppEffect::Post(
            json!({"jsonrpc":"2.0","id":id,"method":"ui/resource-teardown","params":{}}),
        )]
    }
    pub fn tick(&mut self, now: u64) -> Vec<AppEffect> {
        if self
            .teardown
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.dispose()
        } else {
            vec![]
        }
    }
    pub fn dispose(&mut self) -> Vec<AppEffect> {
        if self.disposed {
            return vec![];
        }
        self.disposed = true;
        self.pending.clear();
        self.teardown = None;
        vec![AppEffect::Disposed]
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
fn fits_message(value: &Value) -> bool {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > MAX_MESSAGE_BYTES {
                return Err(std::io::Error::other("message too large"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), value).is_ok()
}
pub fn mcp_app_style_variables(theme: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let pairs = [
        ("--color-background-primary", "--background"),
        ("--color-background-secondary", "--card"),
        ("--color-background-tertiary", "--muted"),
        ("--color-background-inverse", "--foreground"),
        ("--color-background-info", "--info"),
        ("--color-background-danger", "--destructive-surface"),
        ("--color-background-warning", "--warning-surface"),
        ("--color-text-primary", "--foreground"),
        ("--color-text-secondary", "--muted-foreground"),
        ("--color-text-tertiary", "--muted-foreground"),
        ("--color-text-inverse", "--background"),
        ("--color-text-info", "--info-foreground"),
        ("--color-text-danger", "--destructive-foreground"),
        ("--color-text-success", "--success-foreground"),
        ("--color-text-warning", "--warning-foreground"),
        ("--color-border-primary", "--border"),
        ("--color-border-secondary", "--input"),
        ("--color-border-danger", "--destructive"),
        ("--color-ring-primary", "--ring"),
        ("--font-sans", "--font-sans"),
        ("--font-mono", "--font-mono"),
        ("--border-radius-md", "--radius"),
    ];
    pairs
        .into_iter()
        .filter_map(|(to, from)| {
            theme
                .get(from)
                .filter(|v| !v.is_empty())
                .map(|v| (to.into(), v.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Value {
        json!({"theme":"dark","displayMode":"inline","availableDisplayModes":["inline","fullscreen"],"containerDimensions":{"width":600,"maxHeight":2000},"platform":"web"})
    }
    fn host() -> McpAppHost {
        McpAppHost::new(
            "1.0.0".into(),
            json!({"csp":{"connectDomains":["https://api.test"]}}),
            context(),
        )
    }
    fn request(host: &mut McpAppHost, method: &str, params: Value) -> Vec<AppEffect> {
        host.receive(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
    }
    fn initialized(host: &mut McpAppHost) {
        request(host, "ui/initialize", json!({}));
        host.receive(json!({"jsonrpc":"2.0","method":"ui/notifications/initialized"}));
    }
    fn result() -> McpAppCallToolResult {
        serde_json::from_value(json!({"content":[{"type":"text","text":"sunny"}],"structuredContent":null,"_meta":null,"isError":false})).expect("contract fixture")
    }
    #[test]
    fn initialization_replays_input_and_result_once_and_normalizes_nulls() {
        let mut h = host();
        assert!(
            h.set_tool_call(json!({"city":"Oslo"}), Some(result()))
                .is_empty()
        );
        let effects = request(&mut h, "ui/initialize", json!({"protocolVersion":"old"}));
        let AppEffect::Post(response) = &effects[0] else {
            panic!("initialize response");
        };
        assert_eq!(
            response["result"]["protocolVersion"],
            MCP_APP_PROTOCOL_VERSION
        );
        assert!(response["result"]["hostCapabilities"]["downloadFile"].is_object());
        let effects = h.receive(json!({"jsonrpc":"2.0","method":"ui/notifications/initialized"}));
        assert_eq!(effects.len(), 2);
        let AppEffect::Post(response) = &effects[1] else {
            panic!("notification");
        };
        assert!(response["params"].get("_meta").is_none());
        assert!(response["params"].get("structuredContent").is_none());
        assert!(
            h.receive(json!({"jsonrpc":"2.0","method":"ui/notifications/initialized"}))
                .is_empty()
        );
    }
    #[test]
    fn running_call_receives_only_one_late_result() {
        let mut h = host();
        initialized(&mut h);
        assert_eq!(h.set_tool_call(json!({"a":1}), None).len(), 1);
        assert_eq!(h.set_tool_call(json!({"a":2}), Some(result())).len(), 1);
        assert!(h.set_tool_call(json!({}), Some(result())).is_empty());
    }
    #[test]
    fn message_accepts_one_or_many_text_blocks_and_rejects_other_roles() {
        for content in [
            json!({"type":"text","text":" hi "}),
            json!([{"type":"text","text":"hi"}]),
        ] {
            let mut h = host();
            assert!(
                matches!(request(&mut h,"ui/message",json!({"role":"user","content":content})).as_slice(),[AppEffect::Run{operation:AppOperation::Message(text),..}] if text=="hi")
            );
        }
        let mut h = host();
        for content in [
            json!([]),
            json!({"type":"image","text":"hi"}),
            json!({"type":"text","text":"  "}),
        ] {
            assert!(
                matches!(request(&mut h,"ui/message",json!({"role":"user","content":content})).as_slice(),[AppEffect::Post(v)] if v["error"]["code"]==-32602)
            );
        }
    }
    #[test]
    fn catches_up_context_after_initialization_and_emits_only_changes() {
        let mut h = host();
        request(&mut h, "ui/initialize", json!({}));
        let mut next = context();
        next["theme"] = json!("light");
        assert!(h.update_host_context(next.clone()).is_empty());
        let effects = h.receive(json!({"jsonrpc":"2.0","method":"ui/notifications/initialized"}));
        assert_eq!(
            effects,
            vec![notify(
                "ui/notifications/host-context-changed",
                json!({"theme":"light"})
            )]
        );
        assert!(h.update_host_context(next).is_empty());
    }
    #[test]
    fn bounded_pending_operations_refusal_and_disposal() {
        let mut h = host();
        let mut tokens = vec![];
        for _ in 0..16 {
            let effects = request(&mut h, "tools/call", json!({"name":"time"}));
            let AppEffect::Run { token, .. } = effects[0] else {
                panic!("tool call");
            };
            tokens.push(token);
        }
        assert_eq!(h.pending_count(), 16);
        assert!(
            matches!(request(&mut h,"tools/call",json!({"name":"time"})).as_slice(),[AppEffect::Post(v)] if v["error"]["message"]=="Too many requests in flight.")
        );
        assert!(
            matches!(h.complete(tokens[0],Err(AppFailure::Refused("denied".into()))).as_slice(),[AppEffect::Post(v)] if v["error"]["message"]=="denied")
        );
        h.dispose();
        assert!(h.complete(tokens[1], Ok(json!({"content":[]}))).is_empty());
        assert!(request(&mut h, "ping", json!({})).is_empty());
    }
    #[test]
    fn rejects_invalid_methods_arguments_links_and_large_messages() {
        let mut h = host();
        for (method, params, code) in [
            ("unknown", json!({}), -32601),
            ("tools/call", json!({"name":"x","arguments":[]}), -32602),
            ("resources/read", json!({"uri":" "}), -32602),
            ("ui/open-link", json!({"url":"javascript:evil"}), -32602),
            ("ui/update-model-context", json!({"content":null}), -32602),
        ] {
            assert!(
                matches!(request(&mut h,method,params).as_slice(),[AppEffect::Post(v)] if v["error"]["code"]==code)
            );
        }
        assert!(
            matches!(request(&mut h,"ping",json!({"large":"x".repeat(MAX_MESSAGE_BYTES)})).as_slice(),[AppEffect::Post(v)] if v["error"]["code"]==-32600)
        );
    }
    #[test]
    fn display_modes_require_both_sides_and_model_context_can_clear() {
        let mut h = host();
        request(
            &mut h,
            "ui/initialize",
            json!({"appCapabilities":{"availableDisplayModes":["inline"]}}),
        );
        assert_eq!(
            request(
                &mut h,
                "ui/request-display-mode",
                json!({"mode":"fullscreen"})
            ),
            vec![post(json!(1), json!({"mode":"inline"}))]
        );
        request(
            &mut h,
            "ui/initialize",
            json!({"appCapabilities":{"availableDisplayModes":["fullscreen"]}}),
        );
        assert!(
            matches!(request(&mut h,"ui/request-display-mode",json!({"mode":"fullscreen"})).as_slice(),[AppEffect::Run{operation:AppOperation::DisplayMode(mode),..}] if mode=="fullscreen")
        );
        assert!(
            matches!(request(&mut h,"ui/update-model-context",json!({})).as_slice(),[AppEffect::Run{operation:AppOperation::UpdateModelContext(v),..}] if v==&json!({}))
        );
    }
    #[test]
    fn resources_download_safely_and_refusal_is_a_result() {
        let files=read_downloads(&json!([{"type":"resource","resource":{"uri":"file://x","name":"../../test","text":"hello"}},{"type":"resource_link","uri":"https://test/a.txt"}])).expect("downloads");
        assert_eq!(
            files[0],
            AppDownload::Embedded {
                name: "..-..-test".into(),
                mime_type: "application/octet-stream".into(),
                bytes: b"hello".to_vec()
            }
        );
        assert_eq!(
            mcp_resource_bytes(&json!({"blob":"aGVsbG8="})),
            Some(b"hello".to_vec())
        );
        assert!(mcp_resource_bytes(&json!({"blob":"??=="})).is_none());
        let mut h = host();
        let effects = request(
            &mut h,
            "ui/download-file",
            json!({"contents":[{"type":"resource_link","uri":"https://test/a"}]}),
        );
        let AppEffect::Run { token, .. } = effects[0] else {
            panic!("download");
        };
        assert_eq!(
            h.complete(token, Err(AppFailure::Refused("cancelled".into()))),
            vec![post(json!(1), json!({"isError":true}))]
        );
    }
    #[test]
    fn teardown_waits_for_response_or_injected_deadline() {
        let mut h = host();
        initialized(&mut h);
        let effects = h.teardown(10);
        let AppEffect::Post(request) = &effects[0] else {
            panic!("teardown");
        };
        assert!(h.tick(2009).is_empty());
        assert_eq!(
            h.receive(json!({"jsonrpc":"2.0","id":request["id"],"result":{}})),
            vec![AppEffect::Disposed]
        );
        let mut h = host();
        initialized(&mut h);
        h.teardown(10);
        assert_eq!(h.tick(2010), vec![AppEffect::Disposed]);
    }
}
