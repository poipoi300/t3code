//! Pending request selectors borrow authoritative contracts rather than reconstructing wire data.
use std::collections::BTreeMap;
use t3_contracts::{
    base_schemas::RuntimeRequestId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};
pub struct PendingApproval<'a> {
    pub request: &'a OrchestrationV2RuntimeRequest,
    pub item: Option<&'a TurnItemApprovalRequest>,
    pub live: bool,
    pub supports_feedback: bool,
}
pub struct PendingInput<'a> {
    pub request: &'a OrchestrationV2RuntimeRequest,
    pub item: &'a TurnItemUserInputRequest,
    pub dismissible: bool,
}
pub struct PendingRequests<'a> {
    pub approvals: Vec<PendingApproval<'a>>,
    pub user_inputs: Vec<PendingInput<'a>>,
}
pub struct ChildThreadApproval<'a> {
    pub thread: crate::types::ThreadKey,
    pub title: String,
    pub approval: PendingApproval<'a>,
}
pub fn child_thread_approvals<'a>(
    children: &[&std::rc::Rc<super::threads::shell::ThreadShellView>],
    mut projection: impl FnMut(&crate::types::ThreadKey) -> Option<&'a OrchestrationV2ThreadProjection>,
) -> Vec<ChildThreadApproval<'a>> {
    let mut approvals = vec![];
    for child in children {
        if !child.has_pending_approvals() {
            continue;
        }
        let Some(p) = projection(&child.key) else {
            continue;
        };
        approvals.extend(
            derive_pending(&p.runtime_requests, &p.turn_items)
                .approvals
                .into_iter()
                .map(|approval| ChildThreadApproval {
                    thread: child.key.clone(),
                    title: child.source.title.clone(),
                    approval,
                }),
        );
    }
    approvals
}
pub async fn send_approval_feedback<F, Fut>(
    approval: &PendingApproval<'_>,
    feedback: &str,
    mut respond: F,
) -> bool
where
    F: FnMut(
        IdOrIndex<RuntimeRequestId>,
        t3_contracts::provider_policy::ProviderApprovalDecision,
        Option<String>,
    ) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let feedback = t3_contracts::wire::trim_js(feedback);
    if !approval.live || !approval.supports_feedback || feedback.is_empty() {
        return false;
    }
    respond(
        approval.request.id.clone(),
        t3_contracts::provider_policy::ProviderApprovalDecision::Decline,
        Some(feedback.into()),
    )
    .await
}
pub async fn decline_pending_approvals<F, Fut>(
    approvals: &[PendingApproval<'_>],
    mut respond: F,
) -> bool
where
    F: FnMut(
        IdOrIndex<RuntimeRequestId>,
        t3_contracts::provider_policy::ProviderApprovalDecision,
        Option<String>,
    ) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for approval in approvals {
        if approval.live
            && !respond(
                approval.request.id.clone(),
                t3_contracts::provider_policy::ProviderApprovalDecision::Decline,
                None,
            )
            .await
        {
            return false;
        }
    }
    true
}

pub const FULL_ACCESS_OVERRIDE_QUESTION: &str = "Why am I being asked this?";
pub fn full_access_override_explanation(ask_rule: bool) -> &'static str {
    if ask_rule {
        "Full access skips approvals, but an ask rule in the agent's own settings matches this action, so it still asks. Remove or narrow that rule to stop these prompts."
    } else {
        "Full access skips approvals, but the agent still stops for a few safety checks of its own that no setting can skip, such as a delete whose path it can't verify. T3 Code can't turn these off, so check the request and approve it if it's what you expect."
    }
}

type Rows = std::rc::Rc<[std::rc::Rc<OrchestrationV2ProjectedTurnItem>]>;
type Requests = std::rc::Rc<[std::rc::Rc<OrchestrationV2RuntimeRequest>]>;
struct QuestionOverlay {
    input: std::rc::Weak<OrchestrationV2ProjectedTurnItem>,
    request: std::rc::Weak<OrchestrationV2RuntimeRequest>,
    output: std::rc::Rc<OrchestrationV2ProjectedTurnItem>,
}
#[derive(Default)]
pub struct QuestionHistoryProjector {
    rows: Option<Rows>,
    requests: Option<Requests>,
    result: Option<Rows>,
    by_id: BTreeMap<String, std::rc::Rc<OrchestrationV2RuntimeRequest>>,
    cache: BTreeMap<usize, QuestionOverlay>,
}
fn request_key(id: &IdOrIndex<RuntimeRequestId>) -> String {
    match id {
        IdOrIndex::Id(id) => format!("id:{}", id.0),
        IdOrIndex::Index(index) => format!("index:{index}"),
    }
}
impl QuestionHistoryProjector {
    fn project_row(
        &mut self,
        row: &std::rc::Rc<OrchestrationV2ProjectedTurnItem>,
    ) -> std::rc::Rc<OrchestrationV2ProjectedTurnItem> {
        use std::rc::Rc;
        let OrchestrationV2TurnItem::UserInputRequest(item) = &row.item else {
            return row.clone();
        };
        if item.question_answer.as_ref().is_some_and(Option::is_some) {
            return row.clone();
        }
        let Some(request) = self.by_id.get(&request_key(&item.request_id)) else {
            return row.clone();
        };
        if request.status != RuntimeRequestStatus::Resolved {
            return row.clone();
        }
        let Some(answers) = request.answers.as_ref().and_then(Option::as_ref) else {
            return row.clone();
        };
        let key = Rc::as_ptr(row) as usize;
        if let Some(cached) = self.cache.get(&key)
            && cached
                .input
                .upgrade()
                .is_some_and(|old| Rc::ptr_eq(&old, row))
            && cached
                .request
                .upgrade()
                .is_some_and(|old| Rc::ptr_eq(&old, request))
        {
            return cached.output.clone();
        }
        let mut output = (**row).clone();
        if let OrchestrationV2TurnItem::UserInputRequest(item) = &mut output.item {
            item.question_answer = Some(Some(
                t3_contracts::provider_policy::UserInputAttachmentAnswerPayload {
                    request_id: match &request.id {
                        IdOrIndex::Id(id) => id.0.clone(),
                        IdOrIndex::Index(index) => index.to_string(),
                    },
                    answers: answers.clone(),
                    attachments_by_question_id: BTreeMap::new(),
                    question_text_by_id: Some(Some(
                        item.questions
                            .iter()
                            .map(|q| (q.id.0.clone(), q.question.0.clone()))
                            .collect(),
                    )),
                },
            ));
        }
        let output = Rc::new(output);
        self.cache.insert(
            key,
            QuestionOverlay {
                input: Rc::downgrade(row),
                request: Rc::downgrade(request),
                output: output.clone(),
            },
        );
        output
    }
    pub fn project(
        &mut self,
        rows: Rows,
        requests: Requests,
        replaced_index: Option<usize>,
    ) -> Rows {
        use std::rc::Rc;
        let same_requests = self
            .requests
            .as_ref()
            .is_some_and(|r| Rc::ptr_eq(r, &requests));
        if same_requests
            && self.rows.as_ref().is_some_and(|r| Rc::ptr_eq(r, &rows))
            && let Some(result) = &self.result
        {
            return result.clone();
        }
        if !same_requests {
            self.by_id = requests
                .iter()
                .map(|r| (request_key(&r.id), r.clone()))
                .collect();
        }
        let unoverlaid = same_requests
            && self
                .rows
                .as_ref()
                .zip(self.result.as_ref())
                .is_some_and(|(a, b)| Rc::ptr_eq(a, b));
        let result = if unoverlaid
            && replaced_index
                .and_then(|i| rows.get(i))
                .is_some_and(|row| Rc::ptr_eq(row, &self.project_row(row)))
        {
            rows.clone()
        } else {
            let mapped: Vec<_> = rows.iter().map(|row| self.project_row(row)).collect();
            if mapped
                .iter()
                .zip(rows.iter())
                .all(|(a, b)| Rc::ptr_eq(a, b))
            {
                rows.clone()
            } else {
                mapped.into()
            }
        };
        self.cache.retain(|_, entry| entry.input.strong_count() > 0);
        self.rows = Some(rows);
        self.requests = Some(requests);
        self.result = Some(result.clone());
        result
    }
}
pub fn derive_pending<'a>(
    requests: &'a [OrchestrationV2RuntimeRequest],
    items: &'a [OrchestrationV2TurnItem],
) -> PendingRequests<'a> {
    let mut approvals = vec![];
    let mut user_inputs = vec![];
    for request in requests {
        if request.status != RuntimeRequestStatus::Pending {
            continue;
        }
        if let RuntimeRequestKind::Member2(kind) = &request.kind {
            if **kind == RuntimeRequestKindMember2::UserInput
                && let Some(item) = items.iter().rev().find_map(|item| match item {
                    OrchestrationV2TurnItem::UserInputRequest(item)
                        if item.request_id == request.id =>
                    {
                        Some(item.as_ref())
                    }
                    _ => None,
                })
            {
                user_inputs.push(PendingInput {
                    request,
                    item,
                    dismissible: item.response_mode.as_ref().is_some_and(Option::is_some)
                        || matches!(
                            request.response_capability,
                            RuntimeRequestResponseCapability::Message(_)
                        ),
                });
            }
            continue;
        }
        let item = items.iter().rev().find_map(|item| match item {
            OrchestrationV2TurnItem::ApprovalRequest(item) if item.request_id == request.id => {
                Some(item.as_ref())
            }
            _ => None,
        });
        let (live, supports_feedback) = match &request.response_capability {
            RuntimeRequestResponseCapability::Live(capability) => {
                (true, capability.supports_feedback == Some(Some(true)))
            }
            _ => (false, false),
        };
        approvals.push(PendingApproval {
            request,
            item,
            live,
            supports_feedback,
        });
    }
    PendingRequests {
        approvals,
        user_inputs,
    }
}
pub fn seed_answers(
    questions: &[super::questions::Question],
    drafts: &mut BTreeMap<String, super::questions::DraftAnswer>,
) {
    for question in questions {
        if !question.allow_custom_answer {
            continue;
        }
        if let Some(answer) = &question.initial_answer {
            drafts
                .entry(question.id.clone())
                .or_insert_with(|| super::questions::DraftAnswer {
                    custom_answer: Some(answer.clone()),
                    ..Default::default()
                });
        }
    }
}
pub fn user_input_draft_key(
    environment: &str,
    thread: Option<&str>,
    request: Option<&str>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(environment, thread, request))
}
pub fn prune_resolved<T>(
    drafts: &mut BTreeMap<String, T>,
    environment: &str,
    thread: &str,
    pending: &[String],
) -> Result<(), serde_json::Error> {
    let prefix = serde_json::to_string(&(environment, thread))?;
    let prefix = &prefix[..prefix.len() - 1];
    let pending: Vec<_> = pending
        .iter()
        .map(|r| user_input_draft_key(environment, Some(thread), Some(r)))
        .collect::<Result<_, _>>()?;
    drafts.retain(|key, _| !key.starts_with(prefix) || pending.contains(key));
    Ok(())
}
pub use crate::persistence::encode_uri_component;
pub fn resolved_question_draft_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    prefix: &str,
    pending: &[String],
) -> Result<Vec<&'a str>, serde_json::Error> {
    let prefixes: Vec<_> = pending
        .iter()
        .map(|request| {
            let json = serde_json::to_string(&[request])?;
            Ok::<_, serde_json::Error>(format!(
                "{prefix}{}",
                encode_uri_component(&format!("{},", &json[..json.len() - 1]))
            ))
        })
        .collect::<Result<_, _>>()?;
    Ok(keys
        .into_iter()
        .filter(|key| key.starts_with(prefix) && !prefixes.iter().any(|p| key.starts_with(p)))
        .collect())
}
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRequests {
    pub pending: Vec<IdOrIndex<RuntimeRequestId>>,
    pub unanswered: Vec<IdOrIndex<RuntimeRequestId>>,
}
#[derive(Default)]
pub struct ResolvedAnnouncer {
    announced: Option<Vec<IdOrIndex<RuntimeRequestId>>>,
}
impl ResolvedAnnouncer {
    pub fn observe(
        &mut self,
        requests: &[OrchestrationV2RuntimeRequest],
    ) -> Option<ResolvedRequests> {
        let pending: Vec<_> = requests
            .iter()
            .filter(|r| r.status == RuntimeRequestStatus::Pending)
            .map(|r| r.id.clone())
            .collect();
        let resolved = self
            .announced
            .as_ref()
            .is_none_or(|prior| prior.iter().any(|id| !pending.contains(id)));
        self.announced = Some(pending.clone());
        if !resolved {
            return None;
        }
        let unanswered = requests
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    RuntimeRequestStatus::Cancelled | RuntimeRequestStatus::Expired
                )
            })
            .map(|r| r.id.clone())
            .collect();
        Some(ResolvedRequests {
            pending,
            unanswered,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScopedResolvedRequests {
    pub thread: crate::types::ThreadKey,
    pub requests: ResolvedRequests,
}
#[derive(Default)]
struct ResolutionSlot {
    pending: BTreeMap<crate::types::ThreadKey, ResolvedRequests>,
    waker: Option<std::task::Waker>,
}
type ResolutionListeners =
    std::cell::RefCell<Vec<std::rc::Weak<std::cell::RefCell<ResolutionSlot>>>>;
#[derive(Clone, Default)]
pub struct RequestResolutionHub {
    listeners: std::rc::Rc<ResolutionListeners>,
}
pub struct RequestResolutionSubscription {
    slot: std::rc::Rc<std::cell::RefCell<ResolutionSlot>>,
    owner: std::rc::Weak<ResolutionListeners>,
}
impl Drop for RequestResolutionSubscription {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let slot = std::rc::Rc::downgrade(&self.slot);
            owner
                .borrow_mut()
                .retain(|listener| !listener.ptr_eq(&slot));
        }
    }
}
impl futures::Stream for RequestResolutionSubscription {
    type Item = ScopedResolvedRequests;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut slot = self.slot.borrow_mut();
        if let Some((thread, requests)) = slot.pending.pop_first() {
            return std::task::Poll::Ready(Some(ScopedResolvedRequests { thread, requests }));
        }
        if self.owner.upgrade().is_none() {
            return std::task::Poll::Ready(None);
        }
        slot.waker = Some(cx.waker().clone());
        std::task::Poll::Pending
    }
}
impl RequestResolutionHub {
    pub fn subscribe(&self) -> RequestResolutionSubscription {
        let slot = std::rc::Rc::new(std::cell::RefCell::new(ResolutionSlot::default()));
        self.listeners
            .borrow_mut()
            .push(std::rc::Rc::downgrade(&slot));
        RequestResolutionSubscription {
            slot,
            owner: std::rc::Rc::downgrade(&self.listeners),
        }
    }
    pub fn publish(&self, event: ScopedResolvedRequests) {
        let mut wake = vec![];
        self.listeners.borrow_mut().retain(|listener| {
            let Some(listener) = listener.upgrade() else {
                return false;
            };
            let mut slot = listener.borrow_mut();
            slot.pending
                .insert(event.thread.clone(), event.requests.clone());
            if let Some(waker) = slot.waker.take() {
                wake.push(waker);
            }
            true
        });
        for waker in wake {
            waker.wake();
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    fn request(id: &str, status: &str, kind: &str) -> OrchestrationV2RuntimeRequest {
        serde_json::from_value(json!({"id":id,"nodeId":"n","providerTurnId":null,"nativeRequestRef":null,"kind":kind,"status":status,"responseCapability":{"type":"live","providerSessionId":"s","supportsFeedback":true},"createdAt":"2026-01-01T00:00:00.000Z","resolvedAt":null})).unwrap()
    }
    #[test]
    fn requests_without_cards_and_ignored_internal_requests() {
        let requests = [
            request("a", "pending", "command"),
            request("q", "pending", "user_input"),
            request("auth", "pending", "auth_refresh"),
            request("tool", "pending", "dynamic_tool_call"),
            request("old", "resolved", "file-change"),
        ];
        let pending = derive_pending(&requests, &[]);
        assert_eq!(pending.approvals.len(), 1);
        assert!(pending.approvals[0].supports_feedback);
        assert!(pending.user_inputs.is_empty());
    }
    #[test]
    fn announcer_first_and_resolution_only() {
        let mut a = ResolvedAnnouncer::default();
        assert!(a.observe(&[]).is_some());
        assert!(a.observe(&[request("a", "pending", "command")]).is_none());
        let r = a.observe(&[request("a", "expired", "command")]).unwrap();
        assert!(r.pending.is_empty());
        assert_eq!(r.unanswered.len(), 1);
        assert!(a.observe(&[request("a", "expired", "command")]).is_none());
    }
    #[test]
    fn draft_keys_isolate_environments_threads_and_escaped_requests() {
        let key = user_input_draft_key("e", Some("t"), Some("r")).unwrap();
        let other = user_input_draft_key("e", Some("t2"), Some("r")).unwrap();
        let mut drafts = BTreeMap::from([(key.clone(), 1), (other.clone(), 2)]);
        prune_resolved(&mut drafts, "e", "t", &[]).unwrap();
        assert!(!drafts.contains_key(&key));
        assert!(drafts.contains_key(&other));
        let prefix = "question:";
        let request = "r\"😀".to_string();
        let kept = format!(
            "{prefix}{}",
            encode_uri_component(&serde_json::to_string(&[request.as_str(), "q"]).unwrap())
        );
        let removed = format!("{prefix}{}", encode_uri_component("[\"other\",\"q\"]"));
        assert_eq!(
            resolved_question_draft_keys(
                [kept.as_str(), removed.as_str(), "elsewhere"],
                prefix,
                &[request]
            )
            .unwrap(),
            vec![removed.as_str()]
        );
    }
}
