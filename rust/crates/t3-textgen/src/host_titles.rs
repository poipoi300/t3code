use std::sync::Arc;

use async_trait::async_trait;
use t3_contracts::base_schemas::ProjectId;

use crate::{
    CancellationToken,
    generation::{GenerationContext, GenerationError, Operation, RoutingState, TextGeneration},
    prompts::ThreadTitleInput,
    title_context::{TitleLinkResolver, resolve_title_links},
};

#[async_trait]
pub trait TitleRoutingSource: Send + Sync {
    /// Return current instance, model-usage and project-enablement state in settings order.
    async fn snapshot(
        &self,
        project_id: Option<&ProjectId>,
        cancellation: CancellationToken,
    ) -> Result<RoutingState, GenerationError>;
}

pub struct GeneratedTitle {
    pub title: String,
    pub needs_refinement: bool,
}

pub struct HostTitleGenerator {
    generation: Arc<TextGeneration>,
    routing: Arc<dyn TitleRoutingSource>,
    links: Option<Arc<dyn TitleLinkResolver>>,
}

impl HostTitleGenerator {
    pub fn new(generation: Arc<TextGeneration>, routing: Arc<dyn TitleRoutingSource>) -> Self {
        Self {
            generation,
            routing,
            links: None,
        }
    }

    pub fn with_links(mut self, links: Arc<dyn TitleLinkResolver>) -> Self {
        self.links = Some(links);
        self
    }

    pub async fn generate(
        &self,
        mut context: GenerationContext,
        mut input: ThreadTitleInput,
        cancellation: CancellationToken,
    ) -> Result<GeneratedTitle, GenerationError> {
        if cancellation.is_cancelled() {
            return Err(GenerationError::Cancelled);
        }
        let child = cancellation.child_token();
        let _guard = child.clone().drop_guard();
        let work = async {
            let routing = self
                .routing
                .snapshot(context.project_id.as_ref(), child.clone())
                .await?;
            context.model_selection =
                routing.resolve(&context.model_selection, Operation::GenerateThreadTitle)?;
            if input.linked_context.is_none()
                && let Some(links) = &self.links
            {
                input.linked_context = resolve_title_links(
                    &input.message,
                    &context.cwd,
                    links.as_ref(),
                    child.clone(),
                )
                .await;
            }
            let (title, needs_refinement) = self
                .generation
                .generate_thread_title(context, input, None, child.clone())
                .await?;
            Ok(GeneratedTitle {
                title,
                needs_refinement,
            })
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(GenerationError::Cancelled),
            result = work => result,
        }
    }
}
