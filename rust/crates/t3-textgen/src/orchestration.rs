//! Optional adapter for the runtime's host-owned title-generation port.

use std::sync::Arc;

use async_trait::async_trait;
use t3_contracts::{
    base_schemas::ThreadId,
    orchestration_v2::{HistoricalMessageRole, OrchestrationV2HistoricalMessage},
};
use t3_orchestration::{
    api::{TitleGenerationInput, TitleGenerator},
    error::{Error, Result},
};

use crate::{
    CancellationToken,
    generation::{GenerationContext, GenerationError},
    host_titles::HostTitleGenerator,
    prompts::{ForkSource, ThreadTitleInput},
    title_context::{TitleMessage, TitleRole, format_context},
};

pub struct LegacyTitleContext {
    pub generation: GenerationContext,
    pub project_title: Option<String>,
    pub previous_title: Option<String>,
    pub fork_source: Option<ForkSource>,
    pub instructions: Option<String>,
}

#[async_trait]
pub trait LegacyTitleContextSource: Send + Sync {
    /// The legacy seam omits cwd, project and settings; resolve those from the host's thread store.
    async fn resolve(
        &self,
        thread_id: ThreadId,
        cancellation: CancellationToken,
    ) -> std::result::Result<LegacyTitleContext, GenerationError>;
}

pub struct OrchestrationTitleGenerator {
    titles: HostTitleGenerator,
    legacy: Option<Arc<dyn LegacyTitleContextSource>>,
}

impl OrchestrationTitleGenerator {
    pub fn new(titles: HostTitleGenerator) -> Self {
        Self {
            titles,
            legacy: None,
        }
    }

    pub fn with_legacy_context(mut self, source: Arc<dyn LegacyTitleContextSource>) -> Self {
        self.legacy = Some(source);
        self
    }
}

fn generation_error(error: GenerationError) -> Error {
    if error == GenerationError::Cancelled {
        Error::Cancelled
    } else {
        Error::Service {
            operation: "TextGeneration.generateThreadTitle",
            detail: error.to_string(),
        }
    }
}

#[async_trait]
impl TitleGenerator for OrchestrationTitleGenerator {
    async fn generate_with_input(
        &self,
        input: TitleGenerationInput,
        cancellation: CancellationToken,
    ) -> Result<String> {
        self.titles
            .generate(
                GenerationContext {
                    cwd: input.cwd,
                    model_selection: input.model_selection,
                    project_id: Some(input.project_id),
                },
                ThreadTitleInput {
                    message: input.message,
                    project_title: Some(input.project_title),
                    previous_title: input.previous_title,
                    fork_source: input.fork_source.map(|source| ForkSource {
                        title: source.title,
                        history: source.history,
                    }),
                    instructions: Some(input.instructions),
                    attachments: input.attachments,
                    linked_context: None,
                },
                cancellation,
            )
            .await
            .map(|generated| generated.title)
            .map_err(generation_error)
    }

    async fn generate(
        &self,
        thread_id: ThreadId,
        messages: Vec<OrchestrationV2HistoricalMessage>,
        cancellation: CancellationToken,
    ) -> Result<String> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let source = self.legacy.as_ref().ok_or(Error::Unsupported(
            "TitleGenerator.generate requires LegacyTitleContextSource",
        ))?;
        let child = cancellation.child_token();
        let _guard = child.clone().drop_guard();
        let metadata = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::Cancelled),
            result = source.resolve(thread_id, child.clone()) => result.map_err(generation_error)?,
        };
        let messages = messages
            .into_iter()
            .map(|message| TitleMessage {
                role: match message.role {
                    HistoricalMessageRole::User => TitleRole::User,
                    HistoricalMessageRole::Assistant => TitleRole::Assistant,
                },
                text: message.text,
                attachments: Vec::new(),
            })
            .collect::<Vec<_>>();
        let context = format_context(&messages);
        self.titles
            .generate(
                metadata.generation,
                ThreadTitleInput {
                    message: context.message,
                    attachments: context.attachments,
                    project_title: metadata.project_title,
                    previous_title: metadata.previous_title,
                    fork_source: metadata.fork_source,
                    instructions: metadata.instructions,
                    linked_context: None,
                },
                cancellation,
            )
            .await
            .map(|generated| generated.title)
            .map_err(generation_error)
    }
}
