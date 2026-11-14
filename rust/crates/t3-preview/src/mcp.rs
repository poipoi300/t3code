use crate::{
    Error, HtmlService, PreviewService,
    html::{HtmlSource, PublishedSource, safe_thread_segment},
};
use async_trait::async_trait;
use futures_util::FutureExt;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{preview::*, preview_automation::*, provider_policy::RuntimeMode};
use t3_mcp::{CallContext, DecodedInput, McpError, Result};

pub struct McpAdapter {
    pub preview: PreviewService,
    pub html: HtmlService,
    pub access: Arc<dyn t3_mcp::AccessStore>,
    pub attachments_dir: PathBuf,
}
impl McpAdapter {
    fn admission(&self, context: CallContext) -> crate::Admission {
        let access = self.access.clone();
        Arc::new(move || {
            let context = context.clone();
            let access = access.clone();
            async move {
                let caller = context.scope.require_thread("Preview")?;
                let shell = access
                    .thread_shell(&caller.thread_id)
                    .await?
                    .ok_or_else(|| {
                        McpError::failure("thread_not_found", "The calling thread was not found.")
                    })?;
                context.check_dispatch(&shell)?;
                context.recheck_environment().await?;
                Ok(())
            }
            .boxed()
        })
    }
    async fn check(&self, context: &CallContext) -> Result<()> {
        let caller = context.scope.require_thread("Preview")?;
        let shell = self
            .access
            .thread_shell(&caller.thread_id)
            .await?
            .ok_or_else(|| {
                McpError::failure("thread_not_found", "The calling thread was not found.")
            })?;
        context.check_dispatch(&shell)?;
        context.recheck_environment().await
    }
    async fn invoke(
        &self,
        context: CallContext,
        operation: PreviewAutomationOperation,
        input: Value,
    ) -> Result<Value> {
        context
            .scope
            .require_capability(t3_mcp::Capability::Preview)?;
        let caller = context.scope.require_thread("Preview")?;
        let tab_id = input
            .get("tabId")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|e| McpError::InvalidInput(e.to_string()))?;
        let timeout_ms = input["timeoutMs"].as_i64().unwrap_or(
            if operation == PreviewAutomationOperation::RecordingStop {
                120000
            } else if operation == PreviewAutomationOperation::Open {
                60000
            } else {
                15000
            },
        );
        if !matches!(
            operation,
            PreviewAutomationOperation::Status
                | PreviewAutomationOperation::Snapshot
                | PreviewAutomationOperation::WaitFor
        ) {
            self.check(&context).await?;
        }
        self.preview
            .execute_admitted(
                PreviewAutomationRequest {
                    request_id: uuid::Uuid::new_v4().to_string().into(),
                    thread_id: caller.thread_id.clone(),
                    tab_id_explicit: Some(Some(tab_id.is_some())),
                    tab_id: tab_id.map(Some),
                    agent_session_id: Some(Some(caller.provider_session_id.clone())),
                    operation,
                    input,
                    timeout_ms,
                },
                self.admission(context),
            )
            .await
            .map_err(map_error)
    }
    async fn source(&self, context: &CallContext, input: &Value) -> Result<HtmlSource> {
        context.scope.require_thread("HTML rendering")?;
        match (
            input.get("html").and_then(Value::as_str),
            input.get("path").and_then(Value::as_str),
        ) {
            (Some(html), None) => Ok(HtmlSource::Html { html: html.into() }),
            (None, Some(path)) => {
                let caller = context.scope.require_thread("HTML rendering")?;
                let shell = self
                    .access
                    .thread_shell(&caller.thread_id)
                    .await?
                    .filter(|shell| !shell.deleted)
                    .ok_or_else(|| {
                        McpError::failure("thread_not_found", "The calling thread was not found.")
                    })?;
                // Read-only call contexts have a read ceiling; file access uses the live caller mode.
                if shell.modes.runtime_mode != RuntimeMode::FullAccess {
                    return Err(McpError::failure(
                        "capability_denied",
                        "Pass html instead of path: only full-access threads may pass a path.",
                    ));
                }
                Ok(HtmlSource::Path { path: path.into() })
            }
            _ => Err(McpError::failure(
                "invalid_request",
                "Pass exactly one of html or path.",
            )),
        }
    }
}
fn value(input: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(input).map_err(|e| McpError::InvalidInput(e.to_string()))
}
fn decoded<T: DeserializeOwned>(input: Value) -> Result<T> {
    serde_json::from_value(input).map_err(|e| McpError::InvalidInput(e.to_string()))
}
fn map_error(error: Error) -> McpError {
    if let Error::Mcp(error) = error {
        return error;
    }
    let mut fields = json!({});
    if let Error::InvalidUrl {
        reason,
        input_length,
        protocol,
    } = &error
    {
        fields["reason"] = reason.clone().into();
        fields["inputLength"] = (*input_length).into();
        if let Some(protocol) = protocol {
            fields["protocol"] = protocol.clone().into();
        }
    }
    if let Error::Control(reason) = &error {
        fields["reason"] = serde_json::to_value(reason).unwrap_or(Value::Null);
    }
    if let Error::Operation {
        detail: Some(detail),
        ..
    } = &error
    {
        fields["detail"] = detail.clone();
    }
    McpError::Domain {
        tag: error.tag().into(),
        message: error.to_string(),
        fields,
    }
}
fn html_error(error: Error) -> McpError {
    McpError::failure(
        if matches!(
            error.tag(),
            "HtmlRenderFileError"
                | "HtmlRenderFileTooLargeError"
                | "HtmlRenderImagesNotFoundError"
                | "HtmlRenderImageTooLargeError"
                | "HtmlRenderPageTooLargeError"
                | "HtmlRenderViewportHeightError"
        ) {
            "invalid_request"
        } else {
            "orchestration_error"
        },
        error.to_string(),
    )
}

#[async_trait]
impl t3_mcp::services::PreviewService for McpAdapter {
    async fn preview_status(
        &self,
        context: CallContext,
        input: PreviewAutomationTabTargetInput,
    ) -> Result<PreviewAutomationStatus> {
        decoded(
            self.invoke(context, PreviewAutomationOperation::Status, value(input)?)
                .await?,
        )
    }
    async fn preview_dialog(
        &self,
        context: CallContext,
        input: PreviewAutomationDialogInput,
    ) -> Result<PreviewAutomationStatus> {
        decoded(
            self.invoke(context, PreviewAutomationOperation::Dialog, value(input)?)
                .await?,
        )
    }
    async fn preview_open(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<PreviewAutomationStatus> {
        let mut input = input.into_value();
        let target = input.as_object_mut().and_then(|obj| obj.remove("target"));
        if target.is_some() {
            input
                .as_object_mut()
                .expect("open input object")
                .remove("url");
        }
        let opened = self
            .invoke(context.clone(), PreviewAutomationOperation::Open, input)
            .await?;
        if let Some(target) = target
            && opened["tabId"].is_string()
        {
            return decoded(
                self.invoke(
                    context,
                    PreviewAutomationOperation::Navigate,
                    json!({"tabId":opened["tabId"],"target":target}),
                )
                .await?,
            );
        }
        decoded(opened)
    }
    async fn preview_navigate(
        &self,
        context: CallContext,
        input: PreviewAutomationNavigateInput,
    ) -> Result<PreviewAutomationStatus> {
        decoded(
            self.invoke(context, PreviewAutomationOperation::Navigate, value(input)?)
                .await?,
        )
    }
    async fn preview_resize(
        &self,
        context: CallContext,
        input: PreviewAutomationResizeInput,
    ) -> Result<Value> {
        self.invoke(context, PreviewAutomationOperation::Resize, value(input)?)
            .await
    }
    async fn preview_set_appearance(
        &self,
        context: CallContext,
        input: PreviewAutomationSetColorSchemeInput,
    ) -> Result<Value> {
        self.invoke(
            context,
            PreviewAutomationOperation::SetColorScheme,
            value(input)?,
        )
        .await
    }
    async fn preview_snapshot(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> Result<PreviewAutomationSnapshot> {
        let mut input = input.into_value();
        if input["save"] == true {
            input["fullScreenshot"] = true.into();
        }
        decoded(
            self.invoke(context, PreviewAutomationOperation::Snapshot, input)
                .await?,
        )
    }
    async fn preview_act(
        &self,
        context: CallContext,
        input: PreviewAutomationActInput,
    ) -> Result<Value> {
        let operation = match input.action {
            PreviewAutomationAction::Click => PreviewAutomationOperation::Click,
            PreviewAutomationAction::Type => PreviewAutomationOperation::Type,
            PreviewAutomationAction::Hover => PreviewAutomationOperation::Hover,
            PreviewAutomationAction::Select => PreviewAutomationOperation::Select,
            PreviewAutomationAction::Drag => PreviewAutomationOperation::Drag,
            PreviewAutomationAction::Upload => PreviewAutomationOperation::Upload,
            PreviewAutomationAction::Press => PreviewAutomationOperation::Press,
            PreviewAutomationAction::Scroll => PreviewAutomationOperation::Scroll,
        };
        self.invoke(context, operation, value(input)?).await
    }
    async fn preview_evaluate(
        &self,
        context: CallContext,
        input: PreviewAutomationEvaluateInput,
    ) -> Result<Value> {
        Ok(
            json!({"value":self.invoke(context,PreviewAutomationOperation::Evaluate,value(input)?).await?}),
        )
    }
    async fn preview_wait_for(
        &self,
        context: CallContext,
        input: PreviewAutomationWaitForInput,
    ) -> Result<Value> {
        self.invoke(context, PreviewAutomationOperation::WaitFor, value(input)?)
            .await
    }
    async fn preview_recording_start(
        &self,
        context: CallContext,
        input: PreviewAutomationTabTargetInput,
    ) -> Result<Value> {
        self.invoke(
            context,
            PreviewAutomationOperation::RecordingStart,
            value(input)?,
        )
        .await
    }
    async fn preview_recording_stop(
        &self,
        context: CallContext,
        input: PreviewAutomationTabTargetInput,
    ) -> Result<Value> {
        let thread = context
            .scope
            .require_thread("Preview recording")?
            .thread_id
            .clone();
        let artifact = self
            .invoke(
                context,
                PreviewAutomationOperation::RecordingStop,
                value(input)?,
            )
            .await?;
        claim_recording(&self.attachments_dir, &thread, artifact)
            .await
            .map_err(map_error)
    }
}
#[async_trait]
impl t3_mcp::services::PreviewControlsService for McpAdapter {
    async fn t3_preview_list(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        context
            .scope
            .require_capability(t3_mcp::Capability::Preview)?;
        let caller = context.scope.require_thread("Preview list")?;
        let listed = self
            .preview
            .manager()
            .list(PreviewListInput {
                thread_id: caller.thread_id.clone(),
            })
            .await;
        let start = input.value()["cursor"].as_u64().unwrap_or(0) as usize;
        let count = input.value()["limit"].as_u64().unwrap_or(20) as usize;
        let end = start.saturating_add(count);
        Ok(
            json!({"sessions":listed.sessions.iter().skip(start).take(count).collect::<Vec<_>>(),"revision":listed.revision,"serverEpoch":listed.server_epoch,"nextCursor":if end<listed.sessions.len(){Some(end)}else{None}}),
        )
    }
    async fn t3_preview_close(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        context
            .scope
            .require_capability(t3_mcp::Capability::Preview)?;
        let caller = context.scope.require_thread("Preview close")?;
        let listed = self
            .preview
            .manager()
            .list(PreviewListInput {
                thread_id: caller.thread_id.clone(),
            })
            .await;
        let id = input.value()["tabId"]
            .as_str()
            .ok_or_else(|| McpError::InvalidInput("Missing tabId".into()))?;
        if listed
            .sessions
            .iter()
            .any(|s| s.tab_id.as_str() == id && s.runtime == Some(Some(PreviewRuntime::Server)))
        {
            self.invoke(
                context,
                PreviewAutomationOperation::Close,
                input.into_value(),
            )
            .await?;
        } else {
            self.check(&context).await?;
            self.preview
                .manager()
                .close(PreviewCloseInput {
                    thread_id: caller.thread_id.clone(),
                    tab_id: Some(Some(id.into())),
                })
                .await
                .map_err(map_error)?;
        }
        Ok(json!({}))
    }
}
#[async_trait]
impl t3_mcp::services::HtmlService for McpAdapter {
    async fn html_preview(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        let input = input.value();
        let source = self.source(&context, input).await?;
        let preview = self
            .html
            .preview(
                source,
                input["width"].as_f64(),
                input["appearance"].as_str(),
            )
            .await
            .map_err(html_error)?;
        let mut value = value(&preview)?;
        value.as_object_mut().expect("preview object").remove("png");
        value["screenshot"] = json!({"mimeType":"image/png","data":preview.png,"width":preview.width,"height":preview.captured_height});
        Ok(value)
    }
    async fn html_render(&self, context: CallContext, input: DecodedInput) -> Result<Value> {
        self.check(&context).await?;
        let input = input.value();
        let source = self.source(&context, input).await?;
        let caller = context.scope.require_thread("HTML rendering")?;
        let title = input["title"]
            .as_str()
            .ok_or_else(|| McpError::InvalidInput("Missing title".into()))?
            .into();
        let height = input["height"]
            .as_f64()
            .ok_or_else(|| McpError::InvalidInput("Missing height".into()))?;
        let reference = self
            .html
            .publish_admitted(
                caller.thread_id.clone(),
                PublishedSource {
                    source,
                    title,
                    height,
                },
                self.admission(context),
            )
            .await
            .map_err(html_error)?;
        Ok(
            json!({"htmlRender":reference,"message":"Shown to the reader above your reply. Don't mention or describe the page; reply with only what it doesn't already say."}),
        )
    }
}

pub async fn claim_recording(
    root: &std::path::Path,
    thread: &t3_contracts::base_schemas::ThreadId,
    mut artifact: Value,
) -> crate::Result<Value> {
    let error = || {
        Error::operation(
            "PreviewAutomationRecordingTransferError",
            "Could not transfer the recording.",
        )
    };
    let pending = artifact["uploadedAttachmentId"].as_str().ok_or_else(|| {
        Error::operation(
            "PreviewAutomationRecordingDesktopUpdateRequiredError",
            "The desktop must be updated to transfer recordings.",
        )
    })?;
    let tail = pending.strip_prefix("pending-").ok_or_else(error)?;
    let (uuid, extension) = tail.rsplit_once('-').ok_or_else(error)?;
    if uuid::Uuid::parse_str(uuid).is_err()
        || uuid.len() != 36
        || extension.is_empty()
        || extension.len() > 10
        || !extension
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(error());
    }
    let segment = safe_thread_segment(thread.0.as_str()).ok_or_else(error)?;
    let final_id = format!("{segment}-{uuid}-{extension}");
    let from = root.join(format!("{pending}.{extension}"));
    let to = root.join(format!("{final_id}.{extension}"));
    let size = artifact["sizeBytes"]
        .as_u64()
        .filter(|size| *size > 0 && *size <= 50 * 1024 * 1024)
        .ok_or_else(error)?;
    let check = |path: PathBuf| async move {
        let metadata = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|_| error())?;
        if !metadata.is_file() || metadata.len() != size {
            return Err(error());
        }
        Ok(())
    };
    match check(from.clone()).await {
        Ok(()) => tokio::fs::rename(&from, &to).await.map_err(|_| error())?,
        Err(_) => check(to.clone()).await?,
    }
    artifact["id"] = final_id.into();
    artifact["path"] = to.to_string_lossy().into_owned().into();
    artifact
        .as_object_mut()
        .ok_or_else(error)?
        .remove("uploadedAttachmentId");
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{
        browser::PreviewOptions,
        driver::{BrowserCommand, BrowserDriver, BrowserEvent, TabKey},
        html::{HtmlOptions, MemoryHtmlRecovery},
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Mutex, RwLock},
    };
    use t3_mcp::{
        AccessStore, Capability, InvocationScope, Modes, ThreadCaller, ThreadShell,
        access::{authorize, tool_rule},
        decode::decode_input,
    };
    struct Store(RwLock<ThreadShell>);
    #[async_trait]
    impl AccessStore for Store {
        async fn thread_shell(
            &self,
            id: &t3_contracts::base_schemas::ThreadId,
        ) -> t3_mcp::Result<Option<ThreadShell>> {
            Ok((id.0 == "thread").then(|| self.0.read().unwrap().clone()))
        }
    }
    #[derive(Default)]
    struct Driver {
        commands: Mutex<Vec<Value>>,
        pages: Mutex<BTreeMap<TabKey, Value>>,
    }
    #[async_trait]
    impl BrowserDriver for Driver {
        async fn call(
            &self,
            command: BrowserCommand,
            _timeout: std::time::Duration,
        ) -> crate::Result<Value> {
            self.commands
                .lock()
                .unwrap()
                .push(serde_json::to_value(&command).unwrap());
            match command {
                BrowserCommand::Open { snapshot, .. } => {
                    let url = match &snapshot.nav_status {
                        PreviewNavStatus::Loading { url, .. } => Some(url.as_str()),
                        _ => None,
                    };
                    let value = json!({"url":url,"title":null,"loading":false,"dialog":null,"fileChooser":null,"downloads":[]});
                    self.pages
                        .lock()
                        .unwrap()
                        .insert(TabKey::of(&snapshot), value.clone());
                    Ok(value)
                }
                BrowserCommand::Operate {
                    key,
                    operation,
                    input,
                    ..
                } => {
                    let mut pages = self.pages.lock().unwrap();
                    let page = pages.get_mut(&key).ok_or(Error::TabNotFound)?;
                    if operation == "navigate" {
                        page["url"] = input["url"].clone();
                    }
                    Ok(page.clone())
                }
                _ => Ok(json!({})),
            }
        }
        fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<BrowserEvent> {
            tokio::sync::mpsc::unbounded_channel().1
        }
    }
    async fn fixture(
        mode: RuntimeMode,
    ) -> (tempfile::TempDir, McpAdapter, Arc<Store>, Arc<Driver>) {
        let root = tempfile::tempdir().unwrap();
        let driver = Arc::new(Driver::default());
        let preview =
            PreviewService::new(driver.clone(), PreviewOptions::new(root.path().into())).await;
        let store = Arc::new(Store(RwLock::new(ThreadShell {
            id: "thread".into(),
            modes: Modes {
                runtime_mode: mode,
                interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Default,
            },
            deleted: false,
            archived: false,
            active: true,
            provider_instance_id: Some("codex".into()),
            background_subagent_providers: vec![],
            link_origin: None,
        })));
        let attachments_dir = root.path().join("attachments");
        let html = HtmlService::new(
            HtmlOptions {
                attachments_dir: attachments_dir.clone(),
                state_dir: root.path().join("state"),
                secrets_dir: root.path().join("secrets"),
                recovery: Arc::new(MemoryHtmlRecovery::default()),
            },
            None,
        );
        (
            root,
            McpAdapter {
                preview,
                html,
                access: store.clone(),
                attachments_dir,
            },
            store,
            driver,
        )
    }
    async fn context(store: Arc<Store>, tool: &str, input: &Value) -> CallContext {
        let scope = InvocationScope {
            environment_id: "environment".into(),
            request_namespace: "preview-tests".into(),
            issued_at: 0,
            capabilities: BTreeSet::from([Capability::Preview]),
            thread: Some(ThreadCaller {
                thread_id: "thread".into(),
                provider_session_id: "session".into(),
                provider_instance_id: "codex".into(),
                subagent: false,
            }),
            client: None,
            linked_environments: Some(false),
        };
        authorize(
            store,
            scope,
            tool_rule(tool, input).unwrap(),
            tool,
            input,
            false,
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn path_sources_require_the_calling_threads_live_full_access_for_both_html_tools() {
        for tool in ["html_preview", "html_render"] {
            for mode in [
                RuntimeMode::ApprovalRequired,
                RuntimeMode::AutoAcceptEdits,
                RuntimeMode::Auto,
                RuntimeMode::FullAccess,
            ] {
                let (_root, adapter, store, _driver) = fixture(mode).await;
                let input = json!({"path":"C:/isolated/page.html"});
                let call = context(store.clone(), tool, &input).await;
                let result = adapter.source(&call, &input).await;
                if mode == RuntimeMode::FullAccess {
                    assert!(
                        matches!(result, Ok(HtmlSource::Path { .. })),
                        "{tool}: {result:?}"
                    );
                } else {
                    assert!(
                        matches!(result,Err(McpError::Failure {ref code,..}) if code=="capability_denied")
                    );
                }
                assert!(matches!(
                    adapter
                        .source(&call, &json!({"html":"<p>inline</p>"}))
                        .await,
                    Ok(HtmlSource::Html { .. })
                ));
                for ambiguous in [
                    json!({}),
                    json!({"path":"C:/isolated/page.html","html":"<p>both</p>"}),
                ] {
                    assert!(
                        matches!(adapter.source(&call,&ambiguous).await,Err(McpError::Failure {ref code,..}) if code=="invalid_request")
                    );
                }
            }
        }
        let (_root, adapter, store, _driver) = fixture(RuntimeMode::FullAccess).await;
        let input = json!({"path":"C:/isolated/page.html"});
        let call = context(store.clone(), "html_render", &input).await;
        store.0.write().unwrap().modes.runtime_mode = RuntimeMode::ApprovalRequired;
        assert!(
            matches!(adapter.source(&call,&input).await,Err(McpError::Failure {ref code,..}) if code=="capability_denied")
        );
        {
            let mut shell = store.0.write().unwrap();
            shell.modes.runtime_mode = RuntimeMode::FullAccess;
            shell.deleted = true;
        }
        assert!(
            matches!(adapter.source(&call,&input).await,Err(McpError::Failure {ref code,..}) if code=="thread_not_found")
        );
    }
    #[tokio::test]
    async fn preview_open_prefers_target_over_url_and_preserves_url_only_navigation() {
        use t3_mcp::services::PreviewService as _;
        for (input, expected, uses_target) in [
            (
                json!({"url":"localhost:5173/app","show":false}),
                "http://localhost:5173/app",
                false,
            ),
            (
                json!({"url":"javascript:ignored","target":{"kind":"environment-port","port":3000,"path":"/login"},"show":false}),
                "http://localhost:3000/login",
                true,
            ),
            (
                json!({"target":{"kind":"environment-port","port":8443,"protocol":"https","path":"/path"},"show":false}),
                "https://localhost:8443/path",
                true,
            ),
        ] {
            let (_root, adapter, store, driver) = fixture(RuntimeMode::FullAccess).await;
            let call = context(store, "preview_open", &input).await;
            let result = adapter
                .preview_open(call, decode_input("preview_open", input).unwrap())
                .await
                .unwrap();
            assert_eq!(result.url.as_deref(), Some(expected));
            let commands = driver.commands.lock().unwrap();
            let open = commands.iter().find(|c| c["command"] == "open").unwrap();
            assert_eq!(
                open["snapshot"]["navStatus"]["_tag"],
                if uses_target { "Idle" } else { "Loading" }
            );
            let navigate = commands.iter().find(|c| c["operation"] == "navigate");
            assert_eq!(navigate.is_some(), uses_target);
            if let Some(navigate) = navigate {
                assert_eq!(navigate["input"]["url"], expected);
            }
        }
    }
    #[tokio::test]
    async fn recording_claims_reject_unowned_ids_traversal_extensions_and_size_mismatches() {
        let root = tempfile::tempdir().unwrap();
        let thread = "thread".into();
        for pending in [
            "pending-../escape-webm",
            "pending-not-a-uuid-webm",
            "pending-00000000-0000-0000-0000-000000000000-../webm",
            "thread-00000000-0000-0000-0000-000000000000-webm",
        ] {
            let error = claim_recording(
                root.path(),
                &thread,
                json!({"uploadedAttachmentId":pending,"sizeBytes":3}),
            )
            .await
            .unwrap_err();
            assert_eq!(error.tag(), "PreviewAutomationRecordingTransferError");
        }
        let missing = claim_recording(root.path(), &thread, json!({"sizeBytes":3}))
            .await
            .unwrap_err();
        assert_eq!(
            missing.tag(),
            "PreviewAutomationRecordingDesktopUpdateRequiredError"
        );
        let pending = format!("pending-{}-webm", uuid::Uuid::new_v4());
        let source = root.path().join(format!("{pending}.webm"));
        std::fs::write(&source, b"abc").unwrap();
        for size in [0, 4, 50 * 1024 * 1024 + 1] {
            assert_eq!(
                claim_recording(
                    root.path(),
                    &thread,
                    json!({"uploadedAttachmentId":pending,"sizeBytes":size})
                )
                .await
                .unwrap_err()
                .tag(),
                "PreviewAutomationRecordingTransferError"
            );
            assert!(source.is_file());
        }
        let claimed = claim_recording(
            root.path(),
            &thread,
            json!({"uploadedAttachmentId":pending,"sizeBytes":3}),
        )
        .await
        .unwrap();
        assert!(!source.exists());
        assert!(std::path::Path::new(claimed["path"].as_str().unwrap()).is_file());
    }
    #[test]
    fn error_tags_reasons_and_html_failure_codes_survive_the_adapter() {
        for (error, expected) in [
            (Error::TabNotFound, "PreviewAutomationTabNotFoundError"),
            (
                Error::Timeout("test".into()),
                "PreviewAutomationTimeoutError",
            ),
            (Error::ControlRequired, "PreviewControlRequiredError"),
            (
                Error::Disconnected,
                "PreviewAutomationClientDisconnectedError",
            ),
        ] {
            assert!(matches!(map_error(error),McpError::Domain {ref tag,..} if tag==expected));
        }
        let McpError::Domain { tag, fields, .. } =
            map_error(Error::Control(PreviewAutomationControlReason::HumanControl))
        else {
            panic!()
        };
        assert_eq!(tag, "PreviewAutomationControlInterruptedError");
        assert_eq!(fields["reason"], "humanControl");
        assert!(
            matches!(map_error(Error::Mcp(McpError::failure("capability_denied","revoked"))),McpError::Failure {ref code,..} if code=="capability_denied")
        );
        for tag in [
            "HtmlRenderFileError",
            "HtmlRenderFileTooLargeError",
            "HtmlRenderImagesNotFoundError",
            "HtmlRenderImageTooLargeError",
            "HtmlRenderPageTooLargeError",
            "HtmlRenderViewportHeightError",
        ] {
            assert!(
                matches!(html_error(Error::operation(tag,"test")),McpError::Failure {ref code,..} if code=="invalid_request")
            );
        }
        assert!(
            matches!(html_error(Error::operation("HtmlRenderBrowserError","test")),McpError::Failure {ref code,..} if code=="orchestration_error")
        );
        let raw = "javascript:private";
        let McpError::Domain { tag, fields, .. } =
            map_error(crate::urls::normalize_preview_url(raw).unwrap_err())
        else {
            panic!()
        };
        assert_eq!(tag, "PreviewInvalidUrlError");
        assert_eq!(fields["reason"], "unsupported-protocol");
        assert_eq!(fields["protocol"], "javascript:");
        assert_eq!(fields["inputLength"], raw.encode_utf16().count());
    }
}
