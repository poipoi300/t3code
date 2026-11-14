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
                    .ok_or_else(|| {
                        McpError::failure("thread_not_found", "The calling thread was not found.")
                    })?;
                if shell.modes.runtime_mode != RuntimeMode::FullAccess
                    || context.limits.runtime_mode != RuntimeMode::FullAccess
                {
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
