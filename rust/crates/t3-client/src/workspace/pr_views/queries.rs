use super::super::{
    pr_list::{compare_recency, in_list_state},
    pull_requests::*,
};
use crate::time::epoch_ms as timestamp;
use crate::{
    ClientError,
    types::{EnvironmentId, Generation},
    view_logic::{JsTrim, is_js_whitespace},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use t3_contracts::pull_request::*;

impl QueryKind {
    pub fn accepts_result(self, value: &Value) -> bool {
        use t3_contracts::rpc::*;
        fn accepts<M: t3_contracts::rpc::RpcMethod>(value: &Value) -> bool {
            serde_json::from_value::<M::Success>(value.clone()).is_ok()
        }
        match self {
            Self::List => accepts::<WsPullRequestsListRpc>(value),
            Self::ListStats => accepts::<WsPullRequestsListStatsRpc>(value),
            Self::Summary | Self::LinkedSummary => accepts::<WsPullRequestsSummaryRpc>(value),
            Self::Stack => accepts::<WsPullRequestsStackRpc>(value),
            Self::Activity => accepts::<WsPullRequestsActivityRpc>(value),
            Self::Detail => accepts::<WsPullRequestsDetailRpc>(value),
            Self::Preview | Self::PreviewCard => accepts::<WsPullRequestsPreviewRpc>(value),
            Self::LabelCandidates => accepts::<WsPullRequestsLabelCandidatesRpc>(value),
            Self::ReviewerCandidates | Self::ReviewerSearch => {
                accepts::<WsPullRequestsReviewerCandidatesRpc>(value)
            }
            Self::ReviewGuide => accepts::<WsPullRequestsReviewGuideRpc>(value),
            Self::ChangeBreakdown => accepts::<WsPullRequestsChangeBreakdownRpc>(value),
            Self::Checks => accepts::<WsPullRequestsChecksRpc>(value),
            Self::FilterOptions => accepts::<WsPullRequestsFilterOptionsRpc>(value),
            Self::FilesViewed => accepts::<WsPullRequestsFilesViewedRpc>(value),
            Self::LinkedThreads | Self::LinkedThreadsUncached => {
                accepts::<WsPullRequestsLinkedThreadsRpc>(value)
            }
            Self::ThreadComments => accepts::<WsPullRequestsThreadCommentsRpc>(value),
            Self::DiffFileContents => accepts::<WsPullRequestsDiffFileContentsRpc>(value),
            Self::Diff => serde_json::from_value::<PullRequestDiffResult>(value.clone()).is_ok(),
        }
    }
    pub const ALL: [Self; 22] = [
        Self::List,
        Self::ListStats,
        Self::Summary,
        Self::Stack,
        Self::Activity,
        Self::Detail,
        Self::Preview,
        Self::LabelCandidates,
        Self::ReviewerCandidates,
        Self::ReviewGuide,
        Self::ChangeBreakdown,
        Self::Diff,
        Self::Checks,
        Self::FilterOptions,
        Self::FilesViewed,
        Self::LinkedThreads,
        Self::ThreadComments,
        Self::DiffFileContents,
        Self::LinkedSummary,
        Self::PreviewCard,
        Self::ReviewerSearch,
        Self::LinkedThreadsUncached,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::ListStats => "list-stats",
            Self::Summary => "summary",
            Self::Stack => "stack",
            Self::Activity => "activity",
            Self::Detail => "detail",
            Self::Preview => "preview",
            Self::LabelCandidates => "label-candidates",
            Self::ReviewerCandidates => "reviewer-candidates",
            Self::ReviewGuide => "review-guide",
            Self::ChangeBreakdown => "change-breakdown",
            Self::Diff => "diff",
            Self::Checks => "checks",
            Self::FilterOptions => "filter-options",
            Self::FilesViewed => "files-viewed",
            Self::LinkedThreads => "linked-threads",
            Self::ThreadComments => "thread-comments",
            Self::DiffFileContents => "diff-file-contents",
            Self::LinkedSummary => "linked-summary",
            Self::PreviewCard => "preview-card",
            Self::ReviewerSearch => "reviewer-search",
            Self::LinkedThreadsUncached => "linked-threads-uncached",
        }
    }
    pub fn rpc_tag(self) -> Option<&'static str> {
        Some(match self {
            Self::List => "pullRequests.list",
            Self::ListStats => "pullRequests.listStats",
            Self::Summary | Self::LinkedSummary => "pullRequests.summary",
            Self::Stack => "pullRequests.stack",
            Self::Activity => "pullRequests.activity",
            Self::Detail => "pullRequests.detail",
            Self::Preview | Self::PreviewCard => "pullRequests.preview",
            Self::LabelCandidates => "pullRequests.labelCandidates",
            Self::ReviewerCandidates | Self::ReviewerSearch => "pullRequests.reviewerCandidates",
            Self::ReviewGuide => "pullRequests.reviewGuide",
            Self::ChangeBreakdown => "pullRequests.changeBreakdown",
            Self::Checks => "pullRequests.checks",
            Self::FilterOptions => "pullRequests.filterOptions",
            Self::FilesViewed => "pullRequests.filesViewed",
            Self::LinkedThreads | Self::LinkedThreadsUncached => "pullRequests.linkedThreads",
            Self::ThreadComments => "pullRequests.threadComments",
            Self::DiffFileContents => "pullRequests.diffFileContents",
            Self::Diff => return None,
        })
    }
    pub fn idle_ttl_ms(self) -> u64 {
        match self {
            Self::LinkedThreadsUncached => 0,
            Self::LinkedSummary | Self::Stack => 5_000,
            _ => 300_000,
        }
    }
    pub fn refreshes_with_server(self, input: &Value) -> bool {
        match self {
            Self::List => input.get("cursors").is_none(),
            Self::ListStats
            | Self::Summary
            | Self::LinkedSummary
            | Self::Stack
            | Self::Activity
            | Self::Detail
            | Self::Preview
            | Self::PreviewCard
            | Self::ReviewGuide
            | Self::ChangeBreakdown
            | Self::Checks
            | Self::LinkedThreads => true,
            _ => false,
        }
    }
    pub fn refresh_interval_ms(self, linked_signals_supported: bool) -> Option<u64> {
        match self {
            Self::LinkedSummary => Some(60_000),
            Self::LinkedThreads if !linked_signals_supported => Some(10_000),
            _ => None,
        }
    }
}

pub fn persistence_key(kind: QueryKind, input: &impl Serialize) -> Result<String, ClientError> {
    Ok(serde_json::to_string(&(
        format!("environment-data:pull-requests:{}", kind.label()),
        input,
    ))?)
}
pub fn is_stale_reply(shown: &Value, reply: &Value) -> bool {
    if shown["state"] == "merged" && reply["state"] != "merged" {
        return true;
    }
    if let (Some(shown), Some(reply)) = (
        shown["updatedAt"].as_str().and_then(timestamp),
        reply["updatedAt"].as_str().and_then(timestamp),
    ) && shown != reply
    {
        return reply < shown;
    }
    reply["observedAt"].as_f64().unwrap_or(f64::NEG_INFINITY)
        < shown["observedAt"].as_f64().unwrap_or(f64::NEG_INFINITY)
}
pub fn same_reference(reader: &Value, acted: &PullRequestRef) -> bool {
    let host = acted
        .host
        .as_ref()
        .and_then(Option::as_ref)
        .map(|h| h.0.to_lowercase());
    reader["projectId"] == json!(acted.project_id)
        && reader["host"].as_str().map(str::to_lowercase) == host
        && reader["repository"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(&acted.repository.0))
        && reader["number"].as_u64() == Some(acted.number)
        && reader.get("expectedAccountId").filter(|v| !v.is_null())
            == serde_json::to_value(&acted.expected_account_id)
                .ok()
                .as_ref()
                .filter(|v| !v.is_null())
}
fn list_concerns(input: &Value, reference: &PullRequestRef) -> bool {
    reference.expected_account_id.is_none()
        && input
            .get("projectId")
            .is_none_or(|v| v == &json!(reference.project_id))
        && input.get("projectIds").is_none_or(|v| {
            v.as_array()
                .is_some_and(|v| v.contains(&json!(reference.project_id)))
        })
        && input["host"].as_str().is_none_or(|h| {
            reference
                .host
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(|r| h.eq_ignore_ascii_case(&r.0))
        })
}

fn copy_fields(value: &mut Value, reply: &Value, fields: &[&str]) {
    for field in fields {
        if let Some(next) = reply.get(field) {
            value[*field] = next.clone();
        }
    }
}
fn query_draft_filter(query: &str) -> Option<PullRequestListFiltersDraft> {
    let mut tail = query;
    let mut draft = None;
    while !tail.is_empty() {
        tail = tail.trim_start_matches(is_js_whitespace);
        if tail.is_empty() {
            break;
        }
        let mut length = 0;
        while length < tail.len() {
            let rest = &tail[length..];
            let Some(ch) = rest.chars().next() else { break };
            if is_js_whitespace(ch) {
                break;
            }
            if ch == '"' {
                let Some(end) = rest[1..].find('"') else {
                    break;
                };
                length += end + 2;
            } else {
                length += ch.len_utf8();
            }
        }
        if length == 0 {
            tail = &tail[tail.chars().next()?.len_utf8()..];
            continue;
        }
        let token = &tail[..length];
        tail = &tail[length..];
        let negated = token.starts_with('-');
        let token = token.strip_prefix('-').unwrap_or(token);
        let Some((name, value)) = token.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("draft") {
            continue;
        }
        let value = value.replace('"', "");
        let value = value.js_trim();
        let only = if value.eq_ignore_ascii_case("true") {
            !negated
        } else if value.eq_ignore_ascii_case("false") {
            negated
        } else {
            continue;
        };
        draft = Some(if only {
            PullRequestListFiltersDraft::Only
        } else {
            PullRequestListFiltersDraft::Hide
        });
    }
    draft
}
fn list_accepts(
    input: &PullRequestListInput,
    state: &PullRequestState,
    draft: Option<bool>,
) -> bool {
    let query = input
        .query
        .as_ref()
        .and_then(Option::as_ref)
        .map_or("", |v| v.0.as_str());
    let draft_filter = query_draft_filter(query).or_else(|| {
        input
            .filters
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|f| f.draft.clone().flatten())
    });
    in_list_state(state, &input.state)
        && match draft_filter {
            Some(PullRequestListFiltersDraft::Only) => draft != Some(false),
            Some(PullRequestListFiltersDraft::Hide) => draft != Some(true),
            None => true,
        }
}
#[derive(Debug, Clone, PartialEq)]
pub enum ListPatch {
    Unchanged,
    Refresh,
    Patched(Vec<PullRequestListEntry>),
}
pub fn patch_list_entries(
    input: &PullRequestListInput,
    entries: &[PullRequestListEntry],
    reference: &PullRequestRef,
    reply: &PullRequestSummary,
) -> Result<ListPatch, ClientError> {
    let reply_value = serde_json::to_value(reply)?;
    let mut found = false;
    let mut changed = false;
    let mut patched = Vec::new();
    for entry in entries {
        let mut value = serde_json::to_value(entry)?;
        if !same_reference(&value, reference) {
            patched.push(entry.clone());
            continue;
        }
        found = true;
        if is_stale_reply(&value, &reply_value) {
            patched.push(entry.clone());
            continue;
        }
        changed = true;
        copy_fields(
            &mut value,
            &reply_value,
            &[
                "title",
                "url",
                "author",
                "headBranch",
                "baseBranch",
                "state",
                "isDraft",
                "mergeability",
                "updatedAt",
            ],
        );
        let next: PullRequestListEntry = serde_json::from_value(value)?;
        if list_accepts(input, &next.state, Some(next.is_draft)) {
            patched.push(next);
        }
    }
    if !found {
        return Ok(
            if input.cursors.is_none()
                && list_accepts(input, &reply.state, reply.is_draft.flatten())
            {
                ListPatch::Refresh
            } else {
                ListPatch::Unchanged
            },
        );
    }
    if !changed {
        return Ok(ListPatch::Unchanged);
    }
    patched.sort_by(compare_recency);
    Ok(ListPatch::Patched(patched))
}

#[derive(Debug, Clone)]
pub struct QueryRegistration {
    pub key: QueryKey,
    pub input: Value,
    pub state: CachedQuery<Value>,
    pub subscribers: usize,
    pub refresh_requested: bool,
    pub detail_read_version: u64,
    idle_since: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct QueryAnswer {
    pub key: QueryKey,
    pub value: Value,
    pub action_patch: bool,
}
#[derive(Debug, Clone)]
pub struct ActionReplyAnswer {
    pub environment: EnvironmentId,
    pub reference: PullRequestRef,
    pub summary: PullRequestSummary,
}
#[derive(Default)]
pub struct QueryRegistry {
    rows: BTreeMap<QueryKey, QueryRegistration>,
    answers: Vec<QueryAnswer>,
    action_replies: Vec<ActionReplyAnswer>,
    sequence: u64,
    pub persistence: PullRequestQueryCache,
}
pub type ReadsInFlight = BTreeMap<QueryKey, QueryTicket>;
impl QueryRegistry {
    pub fn apply_labels(
        &mut self,
        environment: &EnvironmentId,
        reference: &PullRequestRef,
        labels: &[String],
        applied: bool,
    ) {
        let candidates = self
            .rows
            .iter()
            .find(|(key, row)| {
                &key.environment == environment
                    && key.kind == QueryKind::LabelCandidates
                    && same_reference(&row.input, reference)
            })
            .and_then(|(_, row)| row.state.value.clone());
        for (key, row) in &mut self.rows {
            if &key.environment != environment || !same_reference(&row.input, reference) {
                continue;
            }
            if key.kind == QueryKind::LabelCandidates {
                row.refresh_requested |= row.state.loading;
                row.state.patch(|value| {
                    if let Some(candidates) = value["candidates"].as_array_mut() {
                        for candidate in candidates {
                            if labels.iter().any(|name| candidate["name"] == *name) {
                                candidate["isApplied"] = json!(applied);
                            }
                        }
                    }
                });
            } else if key.kind == QueryKind::Detail
                && matches!(row.input.get("allowStale"), None | Some(Value::Bool(false)))
            {
                row.refresh_requested |= row.state.loading;
                row.state.patch(|value| {
                    if let Some(current) = value["labels"].as_array_mut() {
                        if applied {
                            for name in labels {
                                if current.iter().any(|label| label["name"] == *name) {
                                    continue;
                                }
                                let color = candidates
                                    .as_ref()
                                    .and_then(|v| v["candidates"].as_array())
                                    .and_then(|items| {
                                        items.iter().find(|label| label["name"] == *name)
                                    })
                                    .map(|label| label["color"].clone())
                                    .unwrap_or(Value::Null);
                                current.push(json!({"name": name, "color": color}));
                            }
                        } else {
                            current
                                .retain(|label| !labels.iter().any(|name| label["name"] == *name));
                        }
                    }
                });
            }
        }
    }
    pub fn apply_reviewers(
        &mut self,
        environment: &EnvironmentId,
        reference: &PullRequestRef,
        reviewers: &[PullRequestReviewerRequestInputReviewers],
        requested: bool,
    ) {
        let selected: Vec<Value> = self
            .rows
            .iter()
            .find(|(key, row)| {
                &key.environment == environment
                    && key.kind == QueryKind::ReviewerCandidates
                    && same_reference(&row.input, reference)
            })
            .and_then(|(_, row)| row.state.value.as_ref())
            .and_then(|v| v["candidates"].as_array())
            .into_iter()
            .flatten()
            .filter(|candidate| {
                reviewers.iter().any(|reviewer| {
                    candidate["id"] == json!(reviewer.id)
                        && candidate["kind"] == json!(reviewer.kind)
                })
            })
            .cloned()
            .collect();
        let missing = selected.len() < reviewers.len();
        for (key, row) in &mut self.rows {
            if &key.environment != environment
                || !same_reference(&row.input, reference)
                || !matches!(
                    key.kind,
                    QueryKind::ReviewerCandidates | QueryKind::Detail | QueryKind::Activity
                )
            {
                continue;
            }
            if key.kind == QueryKind::Detail
                && !matches!(row.input.get("allowStale"), None | Some(Value::Bool(false)))
            {
                continue;
            }
            if missing || row.state.loading {
                row.refresh_requested = true;
            }
            row.state.patch(|value| {
                if key.kind == QueryKind::ReviewerCandidates {
                    if let Some(candidates) = value["candidates"].as_array_mut() {
                        for candidate in candidates { if selected.iter().any(|s| s["id"] == candidate["id"] && s["kind"] == candidate["kind"]) { candidate["isRequested"] = json!(requested); } }
                    }
                    return;
                }
                let contributed: Vec<_> = value["comments"].as_array().into_iter().flatten()
                    .filter(|c| matches!(c["kind"].as_str(), Some("review" | "review-comment")))
                    .filter_map(|c| c["author"]["login"].as_str().map(str::to_lowercase)).collect();
                let Some(actors) = value["reviewers"].as_array_mut() else { return };
                if requested {
                    for candidate in &selected {
                        let login = candidate["login"].as_str().unwrap_or_default().to_lowercase();
                        if !actors.iter().any(|actor| actor["login"].as_str().is_some_and(|v| v.to_lowercase() == login)) {
                            actors.push(json!({"login": candidate["login"], "name": candidate["name"], "avatarUrl": candidate["avatarUrl"]}));
                        }
                    }
                } else {
                    actors.retain(|actor| {
                        let login = actor["login"].as_str().unwrap_or_default().to_lowercase();
                        !selected.iter().any(|c| c["login"].as_str().is_some_and(|v| v.to_lowercase() == login))
                            || key.kind == QueryKind::Activity && contributed.contains(&login)
                    });
                }
            });
        }
    }
    pub fn command_succeeded(
        &mut self,
        environment: &EnvironmentId,
        tag: &str,
        input: &Value,
        result: &Value,
        session: Generation,
    ) -> Result<(), ClientError> {
        let reference = if tag == "pullRequests.invalidate" {
            input.get("reference")
        } else {
            Some(input)
        };
        let Some(reference) =
            reference.and_then(|v| serde_json::from_value::<PullRequestRef>(v.clone()).ok())
        else {
            return Ok(());
        };
        match tag {
            "pullRequests.setLabels" => {
                let labels = serde_json::from_value::<Vec<String>>(input["labels"].clone())?;
                self.apply_labels(environment, &reference, &labels, input["applied"] == true);
            }
            "pullRequests.requestReviewers" => {
                let reviewers: Vec<PullRequestReviewerRequestInputReviewers> =
                    serde_json::from_value(input["reviewers"].clone())?;
                self.apply_reviewers(
                    environment,
                    &reference,
                    &reviewers,
                    input["requested"] == true,
                );
            }
            "pullRequests.generateReviewGuide" => {
                for (key, row) in &mut self.rows {
                    if &key.environment == environment
                        && key.kind == QueryKind::ReviewGuide
                        && same_reference(&row.input, &reference)
                    {
                        row.state.invalidate(session);
                        row.refresh_requested = true;
                        row.state.value = Some(result.clone());
                    }
                }
            }
            "pullRequests.update" | "pullRequests.updateComment" | "pullRequests.invalidate" => {
                for (key, row) in &mut self.rows {
                    if &key.environment == environment
                        && same_reference(&row.input, &reference)
                        && match tag {
                            "pullRequests.update" => {
                                matches!(key.kind, QueryKind::Preview | QueryKind::PreviewCard)
                            }
                            "pullRequests.updateComment" => key.kind == QueryKind::Activity,
                            _ => matches!(key.kind, QueryKind::Preview | QueryKind::PreviewCard),
                        }
                    {
                        row.state.invalidate(session);
                        row.refresh_requested = true;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn register(
        &mut self,
        environment: EnvironmentId,
        kind: QueryKind,
        input: &impl Serialize,
    ) -> Result<QueryKey, ClientError> {
        let mut input = serde_json::to_value(input)?;
        if matches!(
            kind,
            QueryKind::Summary
                | QueryKind::LinkedSummary
                | QueryKind::Detail
                | QueryKind::Preview
                | QueryKind::PreviewCard
                | QueryKind::Checks
        ) {
            let reference: PullRequestRef = serde_json::from_value(input)?;
            input = serde_json::to_value(reference)?;
        }
        let key = QueryKey::new(environment, kind, &input)?;
        self.rows
            .entry(key.clone())
            .or_insert_with(|| QueryRegistration {
                key: key.clone(),
                input,
                state: CachedQuery::default(),
                subscribers: 0,
                refresh_requested: false,
                detail_read_version: 0,
                idle_since: None,
            });
        Ok(key)
    }
    pub fn get(&self, key: &QueryKey) -> Option<&QueryRegistration> {
        self.rows.get(key)
    }
    pub fn value<T: DeserializeOwned>(&self, key: &QueryKey) -> Option<T> {
        serde_json::from_value(self.rows.get(key)?.state.value.clone()?).ok()
    }
    pub fn subscribe(&mut self, key: &QueryKey) {
        if let Some(row) = self.rows.get_mut(key) {
            row.subscribers += 1;
            row.idle_since = None;
        }
    }
    pub fn unsubscribe(&mut self, key: &QueryKey, now: u64) {
        if let Some(row) = self.rows.get_mut(key) {
            row.subscribers = row.subscribers.saturating_sub(1);
            if row.subscribers == 0 {
                row.idle_since = Some(now);
            }
        }
    }
    pub fn prune_idle(&mut self, now: u64) {
        self.rows.retain(|key, row| {
            row.idle_since
                .is_none_or(|at| now.saturating_sub(at) < key.kind.idle_ttl_ms())
        });
    }
    pub fn begin(&mut self, key: &QueryKey, session: Generation) -> Option<QueryTicket> {
        let row = self.rows.get_mut(key)?;
        row.refresh_requested = false;
        self.sequence += 1;
        Some(row.state.begin_revision(session, self.sequence))
    }
    pub fn discard(&mut self, key: &QueryKey, ticket: QueryTicket) {
        if let Some(row) = self.rows.get_mut(key)
            && row.state.loading
            && row.state.ticket() == ticket
        {
            row.state.invalidate(ticket.session);
            row.refresh_requested = true;
        }
    }
    pub fn finish(
        &mut self,
        key: &QueryKey,
        ticket: QueryTicket,
        result: Result<Value, String>,
        now: u64,
    ) -> Result<bool, ClientError> {
        let Some(row) = self.rows.get_mut(key) else {
            return Ok(false);
        };
        if !row.state.finish(ticket, result, now) {
            return Ok(false);
        }
        if row.state.error.is_some() {
            return Ok(true);
        }
        let Some(value) = row.state.value.clone() else {
            return Ok(true);
        };
        if key.kind == QueryKind::Detail {
            row.detail_read_version += 1;
        }
        let input = row.input.clone();
        self.save(key, &input, &value, now)?;
        self.answers.push(QueryAnswer {
            key: key.clone(),
            value,
            action_patch: false,
        });
        Ok(true)
    }
    pub fn detail_read_version(&self, key: &QueryKey) -> u64 {
        self.rows.get(key).map_or(0, |row| row.detail_read_version)
    }
    fn save(
        &mut self,
        key: &QueryKey,
        input: &Value,
        value: &Value,
        now: u64,
    ) -> Result<(), ClientError> {
        if key.kind == QueryKind::ListStats {
            self.persistence.save_stats(
                key.environment.clone(),
                &serde_json::from_value(input.clone())?,
                &serde_json::from_value(value.clone())?,
                now,
            )?;
        } else if key.kind.persisted() {
            self.persistence.save(
                key.environment.clone(),
                persistence_key(key.kind, input)?,
                serde_json::to_string(value)?,
                now,
            );
        }
        Ok(())
    }
    pub fn restore(&mut self, key: &QueryKey, now: u64) -> Result<bool, ClientError> {
        let Some(row) = self.rows.get_mut(key) else {
            return Ok(false);
        };
        if row.state.value.is_some() || !key.kind.persisted() {
            return Ok(false);
        }
        let value = if key.kind == QueryKind::ListStats {
            self.persistence
                .load_stats(
                    &key.environment,
                    &serde_json::from_value(row.input.clone())?,
                    now,
                )
                .map(serde_json::to_value)
                .transpose()?
        } else {
            self.persistence.load(
                &key.environment,
                &persistence_key(key.kind, &row.input)?,
                now,
            )
        };
        row.state.value = value.filter(|value| key.kind.accepts_result(value));
        Ok(row.state.value.is_some())
    }
    pub fn reads_in_flight(&self) -> ReadsInFlight {
        self.rows
            .iter()
            .filter(|(_, row)| row.state.loading)
            .map(|(key, row)| (key.clone(), row.state.ticket()))
            .collect()
    }
    pub fn refresh(&mut self, key: &QueryKey, session: Generation) {
        if let Some(row) = self.rows.get_mut(key) {
            row.state.invalidate(session);
            row.refresh_requested = true;
        }
    }
    pub fn server_refresh(
        &mut self,
        environment: &EnvironmentId,
        session: Generation,
        linked_only: bool,
    ) {
        for (key, row) in &mut self.rows {
            if &key.environment == environment
                && row.subscribers > 0
                && if linked_only {
                    key.kind == QueryKind::LinkedThreads
                } else {
                    key.kind.refreshes_with_server(&row.input)
                }
            {
                row.state.invalidate(session);
                row.refresh_requested = true;
            }
        }
    }
    pub fn due_refreshes(&self, now: u64, linked_signals_supported: bool) -> Vec<QueryKey> {
        self.rows
            .iter()
            .filter(|(key, row)| {
                row.subscribers > 0
                    && !row.state.loading
                    && (row.refresh_requested
                        || (key.kind == QueryKind::LinkedSummary
                            && row
                                .state
                                .value
                                .as_ref()
                                .is_some_and(|v| v["state"] != "merged")
                            || key.kind == QueryKind::LinkedThreads)
                            && key
                                .kind
                                .refresh_interval_ms(linked_signals_supported)
                                .is_some_and(|interval| {
                                    row.state
                                        .updated_at
                                        .is_some_and(|at| now.saturating_sub(at) >= interval)
                                }))
            })
            .map(|(key, _)| key.clone())
            .collect()
    }
    pub fn take_answers(&mut self) -> Vec<QueryAnswer> {
        std::mem::take(&mut self.answers)
    }
    pub fn take_action_replies(&mut self) -> Vec<ActionReplyAnswer> {
        std::mem::take(&mut self.action_replies)
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.rows.retain(|key, _| &key.environment != environment);
        self.answers
            .retain(|answer| &answer.key.environment != environment);
        self.action_replies
            .retain(|answer| &answer.environment != environment);
        self.persistence.remove_environment(environment);
    }

    pub fn apply_action_reply(
        &mut self,
        environment: &EnvironmentId,
        reference: &PullRequestRef,
        reply: Option<&PullRequestSummary>,
        before: &ReadsInFlight,
        now: u64,
    ) -> Result<(), ClientError> {
        let reply_value = reply.map(serde_json::to_value).transpose()?;
        let mut saves = Vec::new();
        for (key, row) in &mut self.rows {
            if &key.environment != environment {
                continue;
            }
            let concerns = match key.kind {
                QueryKind::List => list_concerns(&row.input, reference),
                QueryKind::Summary
                | QueryKind::LinkedSummary
                | QueryKind::Detail
                | QueryKind::Preview
                | QueryKind::PreviewCard => same_reference(&row.input, reference),
                _ => false,
            };
            if !concerns {
                continue;
            }
            let unrefreshed = !key.kind.refreshes_with_server(&row.input);
            let fresh_read =
                row.state.loading && !unrefreshed && before.get(key) != Some(&row.state.ticket());
            let reread = |row: &mut QueryRegistration| {
                if !fresh_read {
                    let session = row.state.ticket().session;
                    row.state.invalidate(session);
                    row.refresh_requested = true;
                }
            };
            let Some(reply_value) = &reply_value else {
                if matches!(
                    key.kind,
                    QueryKind::Detail | QueryKind::Preview | QueryKind::PreviewCard
                ) {
                    reread(row);
                }
                continue;
            };
            let Some(shown) = &row.state.value else {
                reread(row);
                continue;
            };
            let mut next = shown.clone();
            let mut lacking = false;
            if key.kind == QueryKind::List {
                let input = serde_json::from_value(row.input.clone())?;
                let value: PullRequestListResult = serde_json::from_value(shown.clone())?;
                match patch_list_entries(
                    &input,
                    &value.entries,
                    reference,
                    reply.ok_or_else(|| ClientError::InvalidPayload("action reply".into()))?,
                )? {
                    ListPatch::Refresh => {
                        reread(row);
                        continue;
                    }
                    ListPatch::Unchanged => {}
                    ListPatch::Patched(entries) => next["entries"] = serde_json::to_value(entries)?,
                }
            } else if !is_stale_reply(shown, reply_value) {
                match key.kind {
                    QueryKind::Summary | QueryKind::LinkedSummary => {
                        let identity = ["projectId", "repository", "number", "observedAt"]
                            .into_iter()
                            .filter_map(|field| shown.get(field).map(|v| (field, v.clone())))
                            .collect::<Vec<_>>();
                        if let (Some(object), Some(reply)) =
                            (next.as_object_mut(), reply_value.as_object())
                        {
                            object.extend(reply.clone());
                        }
                        for (field, value) in identity {
                            next[field] = value;
                        }
                    }
                    QueryKind::Detail => {
                        lacking = shown["state"] == "closed" && reply_value["state"] == "open"
                            || reply_value
                                .get("headSha")
                                .is_some_and(|v| Some(v) != shown.get("headSha"));
                        copy_fields(
                            &mut next,
                            reply_value,
                            &[
                                "title",
                                "url",
                                "author",
                                "state",
                                "isDraft",
                                "mergeability",
                                "additions",
                                "deletions",
                                "changedFiles",
                                "headBranch",
                                "baseBranch",
                                "updatedAt",
                                "mergedAt",
                                "closedAt",
                                "autoMergeEnabled",
                                "autoMergeMethod",
                            ],
                        );
                        if reply_value["autoMergeEnabled"] == false
                            && let Some(object) = next.as_object_mut()
                        {
                            object.remove("autoMergeMethod");
                        }
                    }
                    QueryKind::Preview | QueryKind::PreviewCard => {
                        if shown.get("updatedAt").is_none() && shown.get("observedAt").is_none() {
                            reread(row);
                            continue;
                        }
                        copy_fields(
                            &mut next,
                            reply_value,
                            &[
                                "title",
                                "url",
                                "author",
                                "state",
                                "isDraft",
                                "updatedAt",
                                "observedAt",
                            ],
                        );
                    }
                    _ => {}
                }
            }
            if &next == shown {
                if lacking || unrefreshed && row.state.loading {
                    reread(row);
                }
                continue;
            }
            if !fresh_read && (row.state.loading || lacking || unrefreshed && row.subscribers > 0) {
                reread(row);
            }
            // A post-commit read keeps its ticket; earlier reads are invalidated above.
            row.state.value = Some(next.clone());
            self.answers.push(QueryAnswer {
                key: key.clone(),
                value: next.clone(),
                action_patch: true,
            });
            saves.push((key.clone(), row.input.clone(), next));
        }
        for (key, input, next) in saves {
            self.save(&key, &input, &next, now)?;
        }
        if reference.expected_account_id.is_none()
            && let Some(summary) = reply
        {
            self.action_replies.push(ActionReplyAnswer {
                environment: environment.clone(),
                reference: reference.clone(),
                summary: summary.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLane {
    SerialEnvironment(EnvironmentId),
    SerialViewed(EnvironmentId, String, String, u64),
    SingleFlight(String),
    Unscheduled,
}
pub fn command_lane(environment: &EnvironmentId, tag: &str, input: &Value) -> CommandLane {
    match tag {
        "pullRequests.generateReviewGuide" => CommandLane::Unscheduled,
        "pullRequests.threadComments" => CommandLane::SingleFlight(
            json!([environment, input["threadId"], input["cursor"]]).to_string(),
        ),
        "pullRequests.diffFileContents" => CommandLane::SingleFlight(
            json!([
                environment,
                input["projectId"],
                input["host"].as_str().map(str::to_lowercase),
                input["repository"],
                input["number"],
                input["commit"],
                input["changeType"],
                input["oldPath"],
                input["newPath"]
            ])
            .to_string(),
        ),
        "pullRequests.setFilesViewed" => CommandLane::SerialViewed(
            environment.clone(),
            input["projectId"].as_str().unwrap_or_default().into(),
            input["repository"].as_str().unwrap_or_default().into(),
            input["number"].as_u64().unwrap_or_default(),
        ),
        _ => CommandLane::SerialEnvironment(environment.clone()),
    }
}
pub fn summary_to_vcs_status(
    detail: &PullRequestSummary,
) -> t3_contracts::git::VcsStatusChangeRequest {
    use t3_contracts::git::{VcsStatusChangeRequest, VcsStatusChangeRequestState};
    VcsStatusChangeRequest {
        number: detail.number,
        title: detail.title.clone(),
        url: detail.url.0.clone(),
        base_ref: detail.base_branch.clone(),
        head_ref: detail.head_branch.clone(),
        state: match detail.state {
            PullRequestState::Open => VcsStatusChangeRequestState::Open,
            PullRequestState::Closed => VcsStatusChangeRequestState::Closed,
            PullRequestState::Merged => VcsStatusChangeRequestState::Merged,
        },
        is_draft: (detail.is_draft == Some(Some(true))).then_some(Some(true)),
        updated_at: Some(Some(detail.updated_at.0.clone())),
    }
}

pub fn detail_to_vcs_status(
    detail: &PullRequestDetail,
) -> t3_contracts::git::VcsStatusChangeRequest {
    use t3_contracts::git::{VcsStatusChangeRequest, VcsStatusChangeRequestState};
    VcsStatusChangeRequest {
        number: detail.number,
        title: detail.title.clone(),
        url: detail.url.0.clone(),
        base_ref: detail.base_branch.clone(),
        head_ref: detail.head_branch.clone(),
        state: match detail.state {
            PullRequestState::Open => VcsStatusChangeRequestState::Open,
            PullRequestState::Closed => VcsStatusChangeRequestState::Closed,
            PullRequestState::Merged => VcsStatusChangeRequestState::Merged,
        },
        is_draft: detail.is_draft.then_some(Some(true)),
        updated_at: Some(Some(detail.updated_at.0.clone())),
    }
}
