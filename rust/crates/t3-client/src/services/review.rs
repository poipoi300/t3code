use super::{
    resources::{LocalLimiter, Resource},
    runtime::Services,
};
use crate::{ClientError, platform::Platform, rpc::RpcEndpoint, types::EnvironmentId};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{review::*, rpc::WsReviewGetDiffPreviewRpc};

#[derive(Clone)]
pub struct ReviewService {
    patch_reads: LocalLimiter,
    revisions: Rc<RefCell<BTreeMap<(EnvironmentId, String), String>>>,
    patches: Rc<RefCell<BTreeMap<(EnvironmentId, String), PatchEntry>>>,
}
struct PatchEntry {
    preview: Option<String>,
    resource: Resource<ReviewDiffPreviewSource>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviewPatchInput {
    pub request: ReviewDiffPreviewInput,
    pub cache_key: String,
    pub preview: Option<ReviewDiffPreviewInput>,
}
impl Default for ReviewService {
    fn default() -> Self {
        Self {
            patch_reads: LocalLimiter::new(4),
            revisions: Default::default(),
            patches: Default::default(),
        }
    }
}
impl ReviewService {
    pub async fn preview(
        &self,
        services: &Services,
        environment: EnvironmentId,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ReviewDiffPreviewInput,
        on_answer: impl FnOnce(&ReviewDiffPreviewResult),
    ) -> Result<(), ClientError> {
        let resource = services.query::<WsReviewGetDiffPreviewRpc>(environment.clone(), &input)?;
        let key = (environment.clone(), serde_json::to_string(&input)?);
        let complete = resource
            .snapshot()
            .value
            .is_some_and(|value| value.incomplete != Some(true));
        resource
            .clone()
            .run(platform, |token| async move {
                if !complete
                    && input.source_kind.is_some()
                    && input.file.is_none()
                    && input.commit_sha.as_ref().and_then(Option::as_ref).is_none()
                    && input.tracked_only != Some(true)
                {
                    let mut tracked = input.clone();
                    tracked.tracked_only = Some(true);
                    let answer = endpoint.call::<WsReviewGetDiffPreviewRpc>(tracked).await?;
                    if !resource.is_current(token) {
                        return Err(ClientError::Cancelled);
                    }
                    if answer.incomplete != Some(true) {
                        self.answered(services, &environment, key, &answer);
                        on_answer(&answer);
                        return Ok(answer);
                    }
                    resource.emit(token, answer);
                    platform.yield_to_host().await;
                }
                let answer = endpoint.call::<WsReviewGetDiffPreviewRpc>(input).await?;
                if !resource.is_current(token) {
                    return Err(ClientError::Cancelled);
                }
                self.answered(services, &environment, key, &answer);
                on_answer(&answer);
                Ok(answer)
            })
            .await
    }
    fn answered(
        &self,
        services: &Services,
        environment: &EnvironmentId,
        key: (EnvironmentId, String),
        answer: &ReviewDiffPreviewResult,
    ) {
        let previous = self
            .revisions
            .borrow_mut()
            .insert(key.clone(), answer.generated_at.0.clone());
        if previous.as_deref() != Some(&answer.generated_at.0) {
            services.invalidate(environment, "review.diffFilePatch");
            let patches: Vec<_> = self
                .patches
                .borrow()
                .iter()
                .filter(|((id, _), entry)| {
                    id == environment && entry.preview.as_ref() == Some(&key.1)
                })
                .map(|(_, entry)| entry.resource.clone())
                .collect();
            for patch in patches {
                patch.invalidate();
            }
        }
    }
    pub fn patch_resource(
        &self,
        environment: EnvironmentId,
        generation: crate::types::Generation,
        input: &ReviewPatchInput,
    ) -> Result<Resource<ReviewDiffPreviewSource>, ClientError> {
        let key = (environment, serde_json::to_string(input)?);
        let mut patches = self.patches.borrow_mut();
        let entry = patches.entry(key).or_insert(PatchEntry {
            preview: input
                .preview
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            resource: Resource::new(
                super::resources::ResourcePolicy {
                    stale_ms: 300_000,
                    ..Default::default()
                },
                generation,
            ),
        });
        entry.resource.replace_session(generation);
        Ok(entry.resource.clone())
    }
    pub fn replace_session(
        &self,
        environment: &EnvironmentId,
        generation: crate::types::Generation,
    ) {
        for ((id, _), entry) in self.patches.borrow().iter() {
            if id == environment {
                entry.resource.replace_session(generation);
            }
        }
    }
    pub fn disconnect(&self, environment: &EnvironmentId) {
        for ((id, _), entry) in self.patches.borrow().iter() {
            if id == environment {
                entry.resource.cancel();
            }
        }
    }
    pub fn collect(&self, now: u64) {
        self.patches
            .borrow_mut()
            .retain(|_, entry| !entry.resource.expired(now));
    }
    pub async fn patch(
        &self,
        resource: &Resource<ReviewDiffPreviewSource>,
        endpoint: &impl RpcEndpoint,
        platform: &(impl Platform + ?Sized),
        input: ReviewDiffPreviewInput,
    ) -> Result<(), ClientError> {
        let file = input
            .file
            .as_ref()
            .ok_or_else(|| ClientError::InvalidPayload("file patch requires a file".into()))?;
        let kind = serde_json::to_value(&file.source_kind)?;
        resource
            .run(platform, |_| async {
                let _permit = self.patch_reads.acquire().await;
                let answer = endpoint.call::<WsReviewGetDiffPreviewRpc>(input).await?;
                answer
                    .sources
                    .into_iter()
                    .find(|source| serde_json::to_value(&source.kind).ok().as_ref() == Some(&kind))
                    .ok_or_else(|| {
                        ClientError::InvalidPayload(
                            "Diff no longer available. Refresh the comparison.".into(),
                        )
                    })
            })
            .await
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        self.revisions
            .borrow_mut()
            .retain(|(id, _), _| id != environment);
        self.patches.borrow_mut().retain(|(id, _), entry| {
            if id != environment {
                return true;
            }
            entry.resource.close();
            false
        });
    }
}
