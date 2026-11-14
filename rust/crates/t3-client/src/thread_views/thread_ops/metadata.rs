use super::ThreadOperations;
use crate::{
    ClientError,
    composition::QueryLease,
    platform::Platform,
    rpc::RpcEndpoint,
    services::resources::ResourcePolicy,
    types::{EnvironmentId, ThreadKey},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::collections::BTreeMap;
use t3_contracts::{
    base_schemas::ThreadId,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    rpc::{WsOrchestrationV2DispatchCommandRpc, WsOrchestrationV2GetThreadLinkMetadataRpc},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadLinkLookup {
    pub settled: bool,
    pub metadata: BTreeMap<crate::types::ThreadId, OrchestrationV2ThreadLinkMetadata>,
    pub error: Option<String>,
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn watch_thread_link_metadata(
        &self,
        environment: EnvironmentId,
        threads: Vec<crate::types::ThreadId>,
    ) -> LocalBoxStream<'static, ThreadLinkLookup> {
        let lease = self
            .runtime
            .query::<WsOrchestrationV2GetThreadLinkMetadataRpc>(
                &environment,
                OrchestrationV2GetThreadTitlesInput {
                    thread_ids: threads.into_iter().map(|id| ThreadId(id.0)).collect(),
                },
                ResourcePolicy::default(),
            );
        lease
            .resource()
            .observe(self.platform.monotonic_ms())
            .map(move |_| {
                let state = lease.resource().snapshot();
                ThreadLinkLookup {
                    settled: !state.loading && (state.value.is_some() || state.error.is_some()),
                    metadata: state
                        .value
                        .as_ref()
                        .map(|value| {
                            value
                                .iter()
                                .map(|(id, meta)| {
                                    (crate::types::ThreadId(id.0.clone()), meta.clone())
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    error: state.error,
                }
            })
            .boxed_local()
    }
    pub fn thread_link_metadata(
        &self,
        environment: &EnvironmentId,
        input: OrchestrationV2GetThreadTitlesInput,
    ) -> Option<QueryLease<BTreeMap<ThreadId, OrchestrationV2ThreadLinkMetadata>>> {
        Some(
            self.runtime
                .query::<WsOrchestrationV2GetThreadLinkMetadataRpc>(
                    environment,
                    input,
                    ResourcePolicy::default(),
                ),
        )
    }
    pub fn retry_workspace_preparation(
        &self,
        key: ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.retry_preparation(key, None)
    }
    pub fn retry_workspace_preparation_run(
        &self,
        key: ThreadKey,
        run: t3_contracts::base_schemas::RunId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.retry_preparation(key, Some(run))
    }
    fn retry_preparation(
        &self,
        key: ThreadKey,
        run: Option<t3_contracts::base_schemas::RunId>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            let id = {
                let snapshot = service.runtime.store().snapshot();
                let projection = snapshot.environments.get(&key.environment).and_then(|e| e.threads.get(&key.thread)).filter(|t|t.status==crate::types::SyncStatus::Live).and_then(|t| t.projection.as_ref()).ok_or(ClientError::Disconnected)?;
                let ids = crate::thread_views::threads::turn_item_presentation::workspace_preparation_retry_run_ids(&projection.runs, &projection.turn_items);
                match run {
                    Some(run) if ids.contains(&IdOrIndex::Id(run.clone()))=>run,
                    None=>{let [IdOrIndex::Id(id)] = ids.as_slice() else { return Err(Self::refused("Select the workspace preparation run to retry.")); }; id.clone()},
                    _=>return Err(Self::refused("This workspace preparation is no longer available to retry.")),
                }
            };
            let command: OrchestrationV2Command = serde_json::from_value(serde_json::json!({"type":"prepared-run.retry","commandId":crate::operations::allocate_command_id(&service.platform,None)?,"threadId":key.thread,"runId":id}))?;
            service.endpoint(&key.environment)?.command::<WsOrchestrationV2DispatchCommandRpc>(command).await.map_err(|e| e.into_client_error())?; Ok(())
        }.boxed_local()
    }
}
