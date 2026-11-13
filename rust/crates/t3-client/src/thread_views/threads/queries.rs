use crate::{
    ClientError,
    platform::Platform,
    services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, SyncStatus},
};
use serde::Serialize;
use std::collections::BTreeMap;
use t3_contracts::orchestration_v2::*;

struct GenerationQueries<T> {
    reads: BTreeMap<(EnvironmentId, u64, String), Resource<T>>,
    idle_ttl_ms: u64,
}
impl<T> GenerationQueries<T> {
    fn new(idle_ttl_ms: u64) -> Self {
        Self {
            reads: BTreeMap::new(),
            idle_ttl_ms,
        }
    }
    fn query(
        &mut self,
        environment: EnvironmentId,
        generation: u64,
        session: Generation,
        input: &impl Serialize,
    ) -> Result<Option<Resource<T>>, ClientError> {
        if generation == 0 {
            return Ok(None);
        }
        self.reads.retain(|(id, old_generation, _), resource| {
            let keep = id != &environment || *old_generation == generation;
            if !keep {
                resource.cancel();
            }
            keep
        });
        let arguments = serde_json::to_string(input)?;
        let resource = self
            .reads
            .entry((environment, generation, arguments))
            .or_insert_with(|| {
                Resource::new(
                    ResourcePolicy {
                        stale_ms: u64::MAX,
                        idle_ttl_ms: self.idle_ttl_ms,
                        refresh_ms: None,
                        completed_ttl_ms: None,
                    },
                    session,
                )
            });
        resource.replace_session(session);
        Ok(Some(resource.clone()))
    }
    fn collect(&mut self, now: u64) {
        self.reads.retain(|_, resource| !resource.expired(now));
    }
    fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.reads.retain(|(id, _, _), resource| {
            if id == environment {
                resource.cancel();
                false
            } else {
                true
            }
        });
    }
}
pub struct ThreadQueries {
    counts: GenerationQueries<OrchestrationV2OlderSettledThreadsPage>,
    pages: GenerationQueries<OrchestrationV2OlderSettledThreadsPage>,
    search: GenerationQueries<OrchestrationV2SearchThreadsResult>,
}
impl Default for ThreadQueries {
    fn default() -> Self {
        Self {
            counts: GenerationQueries::new(0),
            pages: GenerationQueries::new(0),
            search: GenerationQueries::new(60_000),
        }
    }
}
impl ThreadQueries {
    pub fn counts(
        &mut self,
        environment: EnvironmentId,
        generation: u64,
        session: Generation,
        before: &t3_contracts::wire::WireDateTime,
        key: &str,
    ) -> Result<Option<Resource<OrchestrationV2OlderSettledThreadsPage>>, ClientError> {
        self.counts
            .query(environment, generation, session, &(before, key))
    }
    pub fn page(
        &mut self,
        environment: EnvironmentId,
        generation: u64,
        session: Generation,
        input: &OrchestrationV2GetOlderSettledThreadsInput,
    ) -> Result<Option<Resource<OrchestrationV2OlderSettledThreadsPage>>, ClientError> {
        self.pages.query(environment, generation, session, input)
    }
    pub fn search(
        &mut self,
        environment: EnvironmentId,
        generation: u64,
        session: Generation,
        input: &OrchestrationV2SearchThreadsInput,
    ) -> Result<Option<Resource<OrchestrationV2SearchThreadsResult>>, ClientError> {
        self.search.query(environment, generation, session, input)
    }
    pub fn collect(&mut self, now: u64) {
        self.counts.collect(now);
        self.pages.collect(now);
        self.search.collect(now);
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.counts.remove_environment(environment);
        self.pages.remove_environment(environment);
        self.search.remove_environment(environment);
    }
}
pub fn search_waiting_for_shell(generation: u64, status: SyncStatus) -> bool {
    generation == 0 && status == SyncStatus::Synchronizing
}
pub async fn read_older_settled_counts<
    P: Platform + ?Sized,
    F: std::future::Future<Output = Result<OrchestrationV2OlderSettledThreadsPage, ClientError>>,
>(
    resource: &Resource<OrchestrationV2OlderSettledThreadsPage>,
    platform: &P,
    before: t3_contracts::wire::WireDateTime,
    read: impl FnOnce(OrchestrationV2GetOlderSettledThreadsInput) -> F,
) -> Result<(), ClientError> {
    resource
        .run(platform, |_| async move {
            platform
                .sleep(super::older_settled::COUNT_REREAD_DELAY_MS)
                .await;
            read(OrchestrationV2GetOlderSettledThreadsInput {
                before: OrchestrationV2OlderSettledCursor {
                    settled_at: before,
                    thread_id: None,
                },
                project_ids: None,
                limit: 0,
            })
            .await
        })
        .await
}
