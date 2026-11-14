use super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    state::sharing::{SharedRows, SharedThreadProjection},
    types::ThreadKey,
};
use futures::{FutureExt, future::LocalBoxFuture};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{
    base_schemas::RuntimeRequestId,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_policy::{ProviderApprovalDecision, ProviderRequestKind},
    rpc::WsOrchestrationV2DispatchCommandRpc,
};

pub fn request_key(id: &IdOrIndex<RuntimeRequestId>) -> String {
    match id {
        IdOrIndex::Id(id) => format!("id:{}", id.0),
        IdOrIndex::Index(i) => format!("index:{i}"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingApproval {
    pub request: Rc<OrchestrationV2RuntimeRequest>,
    pub item: Option<Rc<OrchestrationV2TurnItem>>,
    pub request_kind: ProviderRequestKind,
    pub live: bool,
    pub position: usize,
}
pub type ApprovalMap = BTreeMap<String, Rc<PendingApproval>>;
pub fn ordered(map: &ApprovalMap) -> Vec<Rc<PendingApproval>> {
    let mut rows: Vec<_> = map.values().cloned().collect();
    rows.sort_by_key(|r| r.position);
    rows
}

#[derive(Default)]
pub struct PendingApprovals {
    requests: Option<SharedRows<OrchestrationV2RuntimeRequest>>,
    items: Option<SharedRows<OrchestrationV2TurnItem>>,
    rows: Rc<ApprovalMap>,
}
impl PendingApprovals {
    pub fn project(&mut self, projection: &SharedThreadProjection) -> Rc<ApprovalMap> {
        if self
            .requests
            .as_ref()
            .is_some_and(|r| Rc::ptr_eq(r, &projection.runtime_requests))
            && self
                .items
                .as_ref()
                .is_some_and(|r| Rc::ptr_eq(r, &projection.turn_items))
        {
            return self.rows.clone();
        }
        let mut items = BTreeMap::new();
        for item in projection.turn_items.iter() {
            if let OrchestrationV2TurnItem::ApprovalRequest(a) = item.as_ref() {
                items.insert(request_key(&a.request_id), item.clone());
            }
        }
        let rows: ApprovalMap = projection
            .runtime_requests
            .iter()
            .enumerate()
            .filter_map(|(position, request)| {
                let RuntimeRequestKind::ProviderRequestKind(kind) = &request.kind else {
                    return None;
                };
                if request.status != RuntimeRequestStatus::Pending {
                    return None;
                }
                let id = request_key(&request.id);
                let row = PendingApproval {
                    request: request.clone(),
                    item: items.get(&id).cloned(),
                    request_kind: **kind,
                    live: matches!(
                        request.response_capability,
                        RuntimeRequestResponseCapability::Live(_)
                    ),
                    position,
                };
                let row = self
                    .rows
                    .get(&id)
                    .filter(|old| old.as_ref() == &row)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(row));
                Some((id, row))
            })
            .collect();
        if rows.len() != self.rows.len()
            || rows
                .iter()
                .any(|(id, r)| self.rows.get(id).is_none_or(|old| !Rc::ptr_eq(old, r)))
        {
            self.rows = Rc::new(rows);
        }
        self.requests = Some(projection.runtime_requests.clone());
        self.items = Some(projection.turn_items.clone());
        self.rows.clone()
    }
}

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn pending_approvals(&self, key: &ThreadKey) -> Rc<ApprovalMap> {
        let Some(projection) = self.runtime.store().shared_thread(key) else {
            self.state.approvals.borrow_mut().remove(key);
            return Rc::default();
        };
        self.state
            .approvals
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .project(&projection)
    }
    pub fn can_operate(&self, key: &ThreadKey) -> bool {
        if self.unsupported(&key.environment, "orchestration.dispatchCommand") {
            return false;
        }
        let command =
            OrchestrationV2Command::RuntimeRequestRespond(Box::new(CommandRuntimeRequestRespond {
                command_id: t3_contracts::base_schemas::CommandId("availability".into()),
                thread_id: key.thread.clone(),
                request_id: RuntimeRequestId("availability".into()),
                decision: Some(Some(ProviderApprovalDecision::Decline)),
                feedback: None,
                answers: None,
                attachments_by_question_id: None,
            }));
        self.endpoint(&key.environment)
            .is_ok_and(|e| e.available::<WsOrchestrationV2DispatchCommandRpc>(&command))
    }
    pub fn approvals_available(&self, key: &ThreadKey) -> bool {
        self.can_operate(key)
            && self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .and_then(|e| e.threads.get(&key.thread))
                .is_some_and(|t| t.status == crate::types::SyncStatus::Live)
    }
    fn approval_command(
        &self,
        key: &ThreadKey,
        request_id: RuntimeRequestId,
        decision: ProviderApprovalDecision,
    ) -> Result<OrchestrationV2Command, ClientError> {
        Ok(OrchestrationV2Command::RuntimeRequestRespond(Box::new(
            CommandRuntimeRequestRespond {
                command_id: crate::operations::allocate_command_id(&self.platform, None)?,
                thread_id: key.thread.clone(),
                request_id,
                decision: Some(Some(decision)),
                feedback: None,
                answers: None,
                attachments_by_question_id: None,
            },
        )))
    }
    pub fn respond(
        &self,
        key: ThreadKey,
        id: IdOrIndex<RuntimeRequestId>,
        decision: ProviderApprovalDecision,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            let endpoint = service.endpoint(&key.environment)?;
            let generation = service.session_generation(&key.environment);
            let identity = (key.clone(), request_key(&id));
            if !service
                .state
                .answering
                .borrow_mut()
                .insert(identity.clone())
            {
                return Err(Self::refused(
                    "This approval request is already being answered.",
                ));
            }
            let _answering = Answering(service.state.clone(), identity);
            if !service
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .and_then(|e| e.threads.get(&key.thread))
                .is_some_and(|t| t.status == crate::types::SyncStatus::Live)
            {
                return Err(ClientError::Disconnected);
            }
            let pending = service.pending_approvals(&key);
            let approval = pending
                .get(&request_key(&id))
                .ok_or_else(|| Self::refused("This approval request is no longer pending."))?;
            if !approval.live {
                return Err(Self::refused(
                    "The agent process for this approval is no longer running.",
                ));
            }
            if let Some(OrchestrationV2TurnItem::ApprovalRequest(item)) = approval.item.as_deref()
                && let Some(options) = &item.options
                && options
                    .as_ref()
                    .is_none_or(|options| !options.iter().any(|option| option.decision == decision))
            {
                return Err(Self::refused(
                    "This approval option is no longer available.",
                ));
            }
            let IdOrIndex::Id(id) = id else {
                return Err(Self::refused(
                    "This approval request has no resolved identity.",
                ));
            };
            let command = service.approval_command(&key, id, decision)?;
            let result = endpoint
                .command::<WsOrchestrationV2DispatchCommandRpc>(command)
                .await
                .map_err(|e| e.into_client_error());
            if let Err(error) = &result {
                service.remember_unsupported(
                    &key.environment,
                    "orchestration.dispatchCommand",
                    generation,
                    error,
                );
            }
            result?;
            Ok(())
        }
        .boxed_local()
    }
}
struct Answering(Rc<super::ThreadOpsState>, (ThreadKey, String));
impl Drop for Answering {
    fn drop(&mut self) {
        self.0.answering.borrow_mut().remove(&self.1);
        self.0.actions.notify();
    }
}
