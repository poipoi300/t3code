use crate::{CancellationToken, prompts::*, text::*};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::wire::trim_js;
use t3_contracts::{
    base_schemas::ProjectId, chat_attachment::ChatAttachment, model_selection::ModelSelection,
    pull_request_review_guide::PullRequestReviewGuideStop,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    GenerateCommitMessage,
    GeneratePrContent,
    GenerateBranchName,
    GenerateThreadTitle,
    GenerateReviewGuide,
    GenerateTranscriptionPostProcessing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GenerationError {
    #[error("Text generation was cancelled.")]
    Cancelled,
    #[error("Text generation took too long.")]
    Timeout,
    #[error("The model returned invalid structured output.")]
    InvalidOutput,
    #[error("The model returned the placeholder title \"title\".")]
    PlaceholderTitle,
    #[error("The model did not return text.")]
    EmptyOutput,
    #[error("The model refused the generation request.")]
    Refusal,
    #[error("Text generation failed.")]
    Provider,
    #[error("The text generator is disabled.")]
    Disabled,
    #[error("The model reached its output limit.")]
    OutputLimit,
    #[error("No coding agent is ready to investigate the checkout read-only.")]
    InvestigationUnsupported,
    #[error("The model API rejected the API key.")]
    RejectedKey,
    #[error("The model API is rate-limiting requests.")]
    RateLimit,
    #[error("The model or endpoint is unavailable.")]
    Unavailable,
    #[error("The model API returned a reply T3 Code couldn't read.")]
    UnreadableReply,
    #[error("Enter the API key again: a saved key is only sent to its saved API and address.")]
    KeyRequired,
    #[error("The text generator endpoint is not configured on this server.")]
    EndpointRefused,
}

#[derive(Clone)]
pub struct GenerationContext {
    pub cwd: PathBuf,
    pub model_selection: ModelSelection,
    pub project_id: Option<ProjectId>,
}

// Deliberately no Debug: prompts and replies may contain private messages, code or credentials.
pub struct GenerationRequest {
    pub operation: Operation,
    pub cwd: PathBuf,
    pub model_selection: ModelSelection,
    pub prompt: String,
    pub output_schema: Value,
    pub attachments: Vec<ChatAttachment>,
    pub investigation_timeout: Option<Duration>,
    /// The checkout's empty Git tree, used for child Git commands during review.
    pub review_git_attr_source: Option<String>,
}

#[async_trait]
pub trait TextRunner: Send + Sync {
    /// Dropping this future or cancelling the token must stop provider-owned work.
    async fn run(
        &self,
        request: &GenerationRequest,
        cancellation: CancellationToken,
    ) -> Result<String, GenerationError>;
}

#[derive(Clone)]
pub struct ProviderTarget {
    pub instance_id: String,
    pub driver: String,
    pub enabled: bool,
    pub project_enabled: bool,
    pub installed: bool,
    pub unauthenticated: bool,
    pub probe_error: bool,
    pub available: bool,
    pub text_capable: bool,
    pub text_generator: bool,
    pub unsupported_models: Vec<String>,
    pub exhausted_models: Vec<String>,
    pub exhausted: bool,
    pub custom_models: Vec<String>,
}

pub struct RoutingState {
    /// Configured instances retain the order in the settings document, never hash-map order.
    pub providers: Vec<ProviderTarget>,
    pub default_selection: Option<ModelSelection>,
}
impl RoutingState {
    pub fn resolve(
        &self,
        selection: &ModelSelection,
        operation: Operation,
    ) -> Result<ModelSelection, GenerationError> {
        let current = self
            .providers
            .iter()
            .find(|p| p.instance_id == selection.instance_id.0.as_str());
        let review = operation == Operation::GenerateReviewGuide;
        if let Some(p) = current
            && p.text_generator
            && !review
        {
            return if p.enabled {
                Ok(selection.clone())
            } else {
                Err(GenerationError::Disabled)
            };
        }
        let cannot_investigate = current.is_some_and(|p| p.text_generator) && review;
        let unusable = cannot_investigate
            || current.is_some_and(|p| {
                p.enabled
                    && (!p.text_capable
                        || p.unsupported_models.contains(&selection.model)
                        || (p.probe_error && (!p.installed || p.unauthenticated)))
            });
        let exhausted =
            current.is_some_and(|p| p.exhausted || p.exhausted_models.contains(&selection.model));
        if unusable || exhausted {
            const DRIVERS: [&str; 8] = [
                "codex",
                "claudeAgent",
                "cursor",
                "grok",
                "muse",
                "pi",
                "opencode",
                "antigravity",
            ];
            let defaults = DRIVERS
                .iter()
                .filter_map(|driver| self.providers.iter().find(|p| p.instance_id == *driver));
            let configured = self
                .providers
                .iter()
                .filter(|p| DRIVERS.contains(&p.driver.as_str()));
            for p in defaults.chain(configured) {
                if p.instance_id == selection.instance_id.0.as_str()
                    || !p.enabled
                    || !p.project_enabled
                    || !p.installed
                    || p.unauthenticated
                    || p.probe_error
                    || !p.available
                    || !p.text_capable
                    || p.text_generator
                    || p.exhausted
                    || review && !matches!(p.driver.as_str(), "codex" | "claudeAgent")
                {
                    continue;
                }
                let custom = self.default_selection.as_ref().filter(|s| {
                    s.instance_id.0.as_str() == p.instance_id && p.custom_models.contains(&s.model)
                });
                let defaults = t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER
                    .iter()
                    .chain(t3_contracts::model::DEFAULT_MODEL_BY_PROVIDER.iter());
                let model = defaults
                    .clone()
                    .find(|(driver, _)| *driver == p.driver)
                    .map_or(
                        t3_contracts::model::DEFAULT_TEXT_GENERATION_MODEL,
                        |(_, model)| *model,
                    );
                if let Some(custom) = custom
                    && !p.exhausted_models.contains(&custom.model)
                {
                    return Ok(custom.clone());
                }
                if !p.exhausted_models.iter().any(|m| m == model) {
                    return Ok(ModelSelection {
                        instance_id: serde_json::from_value(json!(p.instance_id))
                            .map_err(|_| GenerationError::Provider)?,
                        model: model.into(),
                        options: None,
                    });
                }
            }
        }
        if cannot_investigate {
            Err(GenerationError::InvestigationUnsupported)
        } else {
            Ok(selection.clone())
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "input", rename_all = "camelCase")]
pub enum GenerationInput {
    Commit(CommitInput),
    Pr(PrInput),
    Branch(BranchInput),
    ThreadTitle(ThreadTitleInput),
    Review(ReviewInput),
    Transcription { prompt: String },
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Generated {
    Commit {
        subject: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    Pr {
        title: String,
        body: String,
    },
    Branch {
        branch: String,
    },
    ThreadTitle {
        title: String,
        #[serde(rename = "needsRefinement", skip_serializing_if = "Option::is_none")]
        needs_refinement: Option<bool>,
    },
    Review {
        stops: Vec<PullRequestReviewGuideStop>,
    },
    Transcription {
        transcription: String,
    },
}

fn object_schema(fields: &[(&str, Value)]) -> Value {
    let properties = fields
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    json!({"type":"object","properties":properties,"required":fields.iter().map(|(key,_)|*key).collect::<Vec<_>>(),"additionalProperties":false})
}
pub fn output_schema(input: &GenerationInput) -> Value {
    let string = json!({"type":"string"});
    match input {
        GenerationInput::Commit(input) => {
            let mut fields = vec![("subject", string.clone()), ("body", string.clone())];
            if input.include_branch {
                fields.push(("branch", string));
            }
            object_schema(&fields)
        }
        GenerationInput::Pr(_) => object_schema(&[("title", string.clone()), ("body", string)]),
        GenerationInput::Branch(_) => object_schema(&[("branch", string)]),
        GenerationInput::ThreadTitle(_) => object_schema(&[
            ("title", string),
            ("needsRefinement", json!({"type":"boolean"})),
        ]),
        GenerationInput::Transcription { .. } => object_schema(&[("transcription", string)]),
        GenerationInput::Review(_) => {
            let integer = json!({"type":"integer"});
            let strings = json!({"type":"array","items":string});
            let excerpt = object_schema(&[
                ("path", string.clone()),
                ("side", json!({"type":"string","enum":["new","old"]})),
                ("start", integer.clone()),
                ("end", integer.clone()),
                ("caption", string.clone()),
            ]);
            let file = object_schema(&[("path", string.clone()), ("note", string.clone())]);
            let note = object_schema(&[
                ("path", string.clone()),
                ("line", integer),
                ("title", string.clone()),
                ("body", string.clone()),
            ]);
            let attention = object_schema(&[
                (
                    "kind",
                    json!({"type":"string","enum":["tradeoff","contract","product","risk","accessibility"]}),
                ),
                ("note", string.clone()),
            ]);
            let stop = object_schema(&[
                ("title", string.clone()),
                ("summary", string),
                ("attention", json!({"anyOf":[attention,{"type":"null"}]})),
                ("excerpts", json!({"type":"array","items":excerpt})),
                ("files", json!({"type":"array","items":file})),
                ("notes", json!({"type":"array","items":note})),
                ("questions", strings),
            ]);
            object_schema(&[("stops", json!({"type":"array","items":stop}))])
        }
    }
}

impl GenerationInput {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Commit(_) => Operation::GenerateCommitMessage,
            Self::Pr(_) => Operation::GeneratePrContent,
            Self::Branch(_) => Operation::GenerateBranchName,
            Self::ThreadTitle(_) => Operation::GenerateThreadTitle,
            Self::Review(_) => Operation::GenerateReviewGuide,
            Self::Transcription { .. } => Operation::GenerateTranscriptionPostProcessing,
        }
    }
    pub fn prompt(&self) -> String {
        match self {
            Self::Commit(v) => commit_prompt(v),
            Self::Pr(v) => pr_prompt(v),
            Self::Branch(v) => branch_prompt(v),
            Self::ThreadTitle(v) => thread_title_prompt(v),
            Self::Review(v) => review_prompt(v),
            Self::Transcription { prompt } => prompt.clone(),
        }
    }
    fn attachments(&self) -> Vec<ChatAttachment> {
        match self {
            Self::Branch(v) => v.attachments.clone(),
            Self::ThreadTitle(v) => v.attachments.clone(),
            _ => Vec::new(),
        }
    }
}

pub fn decode_output(input: &GenerationInput, raw: &str) -> Result<Generated, GenerationError> {
    let value: Value = serde_json::from_str(extract_json_object(raw))
        .map_err(|_| GenerationError::InvalidOutput)?;
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .ok_or(GenerationError::InvalidOutput)
    };
    Ok(match input {
        GenerationInput::Commit(input) => {
            let branch = if input.include_branch {
                Some(sanitize_feature_branch(string("branch")?))
            } else {
                None
            };
            Generated::Commit {
                subject: sanitize_commit_subject(string("subject")?),
                body: trim_js(string("body")?).to_owned(),
                branch,
            }
        }
        GenerationInput::Pr(_) => Generated::Pr {
            title: sanitize_pr_title(string("title")?),
            body: trim_js(string("body")?).to_owned(),
        },
        GenerationInput::Branch(input) => {
            let branch = format_generated_branch(string("branch")?, input.naming.as_ref())?;
            Generated::Branch { branch }
        }
        GenerationInput::ThreadTitle(_) => {
            let title = sanitize_thread_title(string("title")?);
            if title.eq_ignore_ascii_case("title") {
                return Err(GenerationError::PlaceholderTitle);
            }
            let refinement = match value.get("needsRefinement") {
                None => false,
                Some(Value::Bool(b)) => *b,
                _ => return Err(GenerationError::InvalidOutput),
            };
            Generated::ThreadTitle {
                title,
                needs_refinement: refinement.then_some(true),
            }
        }
        GenerationInput::Review(_) => Generated::Review {
            stops: serde_json::from_value(
                value
                    .get("stops")
                    .cloned()
                    .ok_or(GenerationError::InvalidOutput)?,
            )
            .map_err(|_| GenerationError::InvalidOutput)?,
        },
        GenerationInput::Transcription { .. } => Generated::Transcription {
            transcription: string("transcription")?.to_owned(),
        },
    })
}

pub struct TextGeneration {
    runner: Arc<dyn TextRunner>,
}
impl TextGeneration {
    pub fn new(runner: Arc<dyn TextRunner>) -> Self {
        Self { runner }
    }
    pub async fn generate(
        &self,
        context: GenerationContext,
        input: GenerationInput,
        routing: Option<&RoutingState>,
        cancellation: CancellationToken,
    ) -> Result<Generated, GenerationError> {
        if cancellation.is_cancelled() {
            return Err(GenerationError::Cancelled);
        }
        let selection = if let Some(routing) = routing {
            routing.resolve(&context.model_selection, input.operation())?
        } else {
            context.model_selection
        };
        let investigation =
            matches!(input, GenerationInput::Review(_)).then_some(Duration::from_secs(900));
        let review_git_attr_source = match &input {
            GenerationInput::Review(review) => review.git_attr_source.clone(),
            _ => None,
        };
        let request = GenerationRequest {
            operation: input.operation(),
            cwd: context.cwd,
            model_selection: selection,
            prompt: input.prompt(),
            output_schema: output_schema(&input),
            attachments: input.attachments(),
            investigation_timeout: investigation,
            review_git_attr_source,
        };
        let child = cancellation.child_token();
        struct CancelOnDrop(CancellationToken);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _guard = CancelOnDrop(child.clone());
        let reply = tokio::select! {biased;
            _=cancellation.cancelled()=>return Err(GenerationError::Cancelled),
            reply=tokio::time::timeout(investigation.unwrap_or(Duration::from_secs(180)),self.runner.run(&request,child))=>reply.map_err(|_|GenerationError::Timeout)??
        };
        decode_output(&input, &reply)
    }

    pub async fn generate_thread_title(
        &self,
        context: GenerationContext,
        input: ThreadTitleInput,
        routing: Option<&RoutingState>,
        cancellation: CancellationToken,
    ) -> Result<(String, bool), GenerationError> {
        match self
            .generate(
                context,
                GenerationInput::ThreadTitle(input),
                routing,
                cancellation,
            )
            .await?
        {
            Generated::ThreadTitle {
                title,
                needs_refinement,
            } => Ok((title, needs_refinement.unwrap_or(false))),
            _ => Err(GenerationError::InvalidOutput),
        }
    }
}
