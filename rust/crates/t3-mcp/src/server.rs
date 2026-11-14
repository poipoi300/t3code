use crate::{
    AccessStore, Capability, InvocationScope, McpError, Result, Services,
    access::{authorize, tool_rule},
    catalog::{catalog, tool},
    decode_input,
    exposure::{ToolExposure, enabled},
    output,
};
use async_trait::async_trait;
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use serde_json::{Value, json};
use std::{borrow::Cow, sync::Arc};

#[async_trait]
pub trait PeerService: Send + Sync {
    /// The server loads the local caller, checks capabilities and rejects fenced links first.
    /// Preserve context limits and caller identity at the destination; namespace retry IDs,
    /// remove environmentId from arguments and derive sender metadata from the local caller.
    async fn call(
        &self,
        context: crate::CallContext,
        environment_id: &str,
        tool: &str,
        input: Value,
    ) -> Result<Value>;
}

pub struct McpServer {
    pub services: Services,
    pub access: Arc<dyn AccessStore>,
    pub exposure: ToolExposure,
    pub peers: Option<Arc<dyn PeerService>>,
    pub artifacts: Option<Arc<dyn output::ArtifactStore>>,
}

impl McpServer {
    pub fn new(services: Services, access: Arc<dyn AccessStore>) -> Self {
        Self {
            services,
            access,
            exposure: ToolExposure::default(),
            peers: None,
            artifacts: None,
        }
    }
    pub fn list(&self, scope: &InvocationScope, deferred: bool) -> Value {
        self.exposure.list(scope, deferred)
    }
    pub fn list_protocol(&self, scope: &InvocationScope, deferred: bool, modern: bool) -> Value {
        let mut list = self.list(scope, deferred);
        if modern {
            list["resultType"] = json!("complete");
            list["ttlMs"] = json!(0);
            list["cacheScope"] = json!("private");
            list["_meta"] = json!({"io.modelcontextprotocol/serverInfo":{"name":"T3 Code","version":catalog().server_version}});
        } else if let Some(tools) = list["tools"].as_array_mut() {
            for tool in tools {
                if tool
                    .get("outputSchema")
                    .is_some_and(|schema| schema["type"] != "object")
                {
                    tool.as_object_mut()
                        .expect("tool object")
                        .remove("outputSchema");
                }
            }
        }
        list
    }
    pub async fn call(
        &self,
        scope: InvocationScope,
        name: &str,
        arguments: Value,
    ) -> std::result::Result<Value, ErrorData> {
        self.call_with_scopes(scope, name, arguments, None).await
    }
    pub async fn call_with_scopes(
        &self,
        scope: InvocationScope,
        name: &str,
        arguments: Value,
        granted_scopes: Option<&[t3_contracts::auth::AuthEnvironmentScope]>,
    ) -> std::result::Result<Value, ErrorData> {
        self.call_with_cancellation(scope,name,arguments,granted_scopes,tokio_util::sync::CancellationToken::new()).await
    }
    pub async fn call_with_cancellation(
        &self,scope:InvocationScope,name:&str,arguments:Value,
        granted_scopes:Option<&[t3_contracts::auth::AuthEnvironmentScope]>,
        cancellation:tokio_util::sync::CancellationToken,
    )->std::result::Result<Value,ErrorData> {
        if tool(name).is_none() || !enabled(&scope, name) {
            return Err(ErrorData::invalid_params(
                format!("Tool '{name}' not found"),
                None,
            ));
        }
        let request = arguments.clone();
        let result = self.invoke(scope, name, arguments, granted_scopes,cancellation).await;
        match result {
            Ok(value) => output::success(name, &request, value, self.artifacts.as_deref())
                .await
                .map_err(|_| {
                    ErrorData::internal_error(
                        "Tool execution failed due to an internal server error.",
                        None,
                    )
                }),
            Err(error) => Ok(output::failure(name, error)),
        }
    }
    async fn invoke(
        &self,
        scope: InvocationScope,
        name: &str,
        arguments: Value,
        granted_scopes: Option<&[t3_contracts::auth::AuthEnvironmentScope]>,
        cancellation:tokio_util::sync::CancellationToken,
    ) -> Result<Value> {
        if let Some(scopes) = granted_scopes {
            crate::authorization::authorize_tool(scopes, name, &arguments)?;
        }
        let mut input = decode_input(name, arguments)?.into_value();
        let routed = catalog().definitions[name].routed;
        let destination = if routed {
            input
                .get("environmentId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let remote = destination
            .as_deref()
            .is_some_and(|id| id != scope.environment_id.0);
        if remote && scope.linked_environments == Some(false) {
            return Err(McpError::failure(
                "capability_denied",
                crate::instructions::fixture()["linkedEnvironmentsOff"]["message"].as_str().ok_or(McpError::Internal)?,
            ));
        }
        let mut context = authorize(
            self.access.clone(),
            scope,
            tool_rule(name, &input)?,
            name,
            &input,
            remote,
        )
        .await?;
        context.cancellation=cancellation;
        let scope = &context.scope;
        let kit = &catalog().definitions[name].toolkit;
        match kit.as_str() {
            "preview" | "previewControls" => {
                scope.require_capability(Capability::Preview)?;
                scope.require_thread("This tool")?;
            }
            "device" => {
                if !scope.capabilities.contains(&Capability::Device) {
                    let reason = "Agent device access is turned off for this environment. Ask the user to turn on Agent device access in Settings > Integrations > Devices; it applies when the agent session next starts, such as in a new thread.";
                    return Err(McpError::Domain {
                        tag: "DeviceToolUnavailableError".into(),
                        message: reason.into(),
                        fields: json!({"reason":reason}),
                    });
                }
                scope.require_thread("This tool")?;
            }
            "worktree" => scope.require_capability(Capability::Worktree)?,
            "pullRequests" => scope.require_capability(Capability::PullRequests)?,
            "orchestrator" | "project" | "environment" | "thread" | "attachment" | "message"
            | "theme"
                if !scope.capabilities.contains(&Capability::Orchestration) =>
            {
                return Err(McpError::failure(
                    "capability_denied",
                    "This credential cannot control threads.",
                ));
            }
            _ => {}
        }
        if remote {
            context.check_forwarding()?;
            let peers=self.peers.as_ref().ok_or_else(||McpError::failure("orchestration_error","This environment cannot reach linked environments."))?;
            return peers.call(context,destination.as_deref().ok_or(McpError::Internal)?,name,input).await;
        }
        if routed {input.as_object_mut().ok_or(McpError::Internal)?.remove("environmentId");}
        if name == "tool_search" {
            return Ok(self.exposure.search(
                &scope.require_thread("tool_search")?.provider_session_id,
                input["query"].as_str().unwrap_or_default(),
            ));
        }
        if name == "t3_theme_set"
            && (input.get("theme").is_some() && input.get("themeId").is_none()
                || input.get("themeId").is_none() && input.get("appearance").is_none())
        {
            return Err(McpError::failure(
                "invalid_request",
                crate::instructions::fixture()["themeSetInvalidRequest"]
                    .as_str()
                    .ok_or(McpError::Internal)?,
            ));
        }
        if kit == "html" {
            let has_html = input.get("html").is_some();
            let has_path = input.get("path").is_some();
            if has_html == has_path {
                return Err(McpError::failure(
                    "invalid_request",
                    "Pass exactly one of html or path.",
                ));
            }
            if has_path {
                let caller = self
                    .access
                    .thread_shell(&scope.require_thread("This tool")?.thread_id)
                    .await?
                    .filter(|thread|!thread.deleted)
                    .ok_or_else(|| {
                        McpError::failure("thread_not_found", "The calling thread was not found.")
                    })?;
                if caller.modes.runtime_mode
                    != t3_contracts::provider_policy::RuntimeMode::FullAccess
                {
                    return Err(McpError::failure(
                        "capability_denied",
                        "Pass html instead of path: only full-access threads may pass a path.",
                    ));
                }
            }
        }
        if name == "preview_open" {
            if input.get("target").is_some() {
                input.as_object_mut().expect("object").remove("url");
            }
            let open = input.get("open").or_else(|| input.get("show")).cloned();
            if let Some(open) = open {
                input["show"] = open.clone();
                input["open"] = open;
            }
            if input.get("reuseExistingTab").is_none() {
                input["reuseExistingTab"] = json!(true);
            }
        }
        let decoded = crate::decode::DecodedInput::from_normalized(input);
        self.services
            .dispatch(name, context.clone(), decoded)
            .await
            .map_err(|error| context.recorded_refusal().unwrap_or(error))
    }
}

#[derive(Clone)]
pub struct RequestScope {
    pub scope: InvocationScope,
    pub deferred: bool,
    pub granted_scopes: Option<Vec<t3_contracts::auth::AuthEnvironmentScope>>,
}

fn request_scope(
    context: &RequestContext<RoleServer>,
) -> std::result::Result<RequestScope, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<RequestScope>())
        .cloned()
        .ok_or_else(|| {
            ErrorData::internal_error("MCP request has no authenticated invocation scope", None)
        })
}

#[derive(Clone)]
pub struct ProtocolHandler(pub Arc<McpServer>);
impl std::ops::Deref for ProtocolHandler {
    type Target = McpServer;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl ServerHandler for ProtocolHandler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_06_18, ProtocolVersion::V_2026_07_28])
    }
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .with_server_info(Implementation::new("T3 Code", &catalog().server_version))
    }
    fn negotiate_initialize(
        &self,
        _request: &InitializeRequestParams,
    ) -> std::result::Result<InitializeResult, ErrorData> {
        Ok(self.get_info())
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let request = request_scope(&context)?;
        if request.granted_scopes.as_ref().is_some_and(|scopes| {
            !scopes.contains(&t3_contracts::auth::AuthEnvironmentScope::OrchestrationRead)
        }) {
            return Err(ErrorData::invalid_request(
                "The authenticated token is missing required scope: orchestration:read.",
                None,
            ));
        }
        let list = self.list_protocol(
            &request.scope,
            request.deferred,
            context.protocol_version() == Some(ProtocolVersion::V_2026_07_28),
        );
        serde_json::from_value(list)
            .map_err(|_| ErrorData::internal_error("Invalid generated tool list", None))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let authenticated = request_scope(&context)?;
        let name = request.name.to_string();
        let value = self
            .call_with_cancellation(
                authenticated.scope,
                &name,
                Value::Object(request.arguments.unwrap_or_default()),
                authenticated.granted_scopes.as_deref(),
                context.ct.clone(),
            )
            .await?;
        if name == "tool_search"
            && value["structuredContent"]["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        {
            let _ = context.peer.notify_tool_list_changed().await;
        }
        serde_json::from_value::<CallToolResult>(value)
            .map(Into::into)
            .map_err(|_| ErrorData::internal_error("Invalid tool result", None))
    }
}
