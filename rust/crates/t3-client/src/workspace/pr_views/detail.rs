use super::super::pull_requests::PullRequestKey;
use super::{markdown::bound_utf16, queries::is_stale_reply};
use crate::time::epoch_ms as timestamp;
use crate::{
    ClientError,
    platform::{
        Storage, StorageArea, StorageCondition, StorageKey, StorageTransaction, StorageWrite,
    },
    types::EnvironmentId,
};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{git::VcsRef, pull_request::*};

pub fn allows_single_merge(
    stack_actions: bool,
    has_stack: bool,
    pending: bool,
    error: Option<&str>,
) -> bool {
    !stack_actions || (!has_stack && !pending && error.is_none())
}
pub fn resolve_merge_method(
    allowed: &[PullRequestMergeMethod],
    current: Option<&PullRequestMergeMethod>,
    project_default: Option<&PullRequestMergeMethod>,
    last: &PullRequestMergeMethod,
) -> PullRequestMergeMethod {
    [current, project_default, Some(last)]
        .into_iter()
        .flatten()
        .find(|m| allowed.contains(m))
        .cloned()
        .or_else(|| allowed.first().cloned())
        .unwrap_or(PullRequestMergeMethod::Merge)
}
pub fn merge_method_label(method: &PullRequestMergeMethod) -> &'static str {
    match method {
        PullRequestMergeMethod::Merge => "Merge",
        PullRequestMergeMethod::Squash => "Squash and merge",
        PullRequestMergeMethod::Rebase => "Rebase and merge",
        PullRequestMergeMethod::FastForward => "Fast-forward merge",
    }
}
pub struct PrimaryControlInput<'a> {
    pub state: &'a PullRequestState,
    pub is_draft: bool,
    pub mergeability: &'a PullRequestMergeability,
    pub checks: Option<&'a PullRequestChecksState>,
    pub auto_merge_enabled: Option<bool>,
    pub has_merge_method: bool,
    pub can_merge: bool,
    pub can_mark_ready: bool,
    pub can_enable_auto_merge: bool,
}
pub fn primary_control(input: PrimaryControlInput<'_>) -> Option<&'static str> {
    if *input.state == PullRequestState::Merged {
        return Some("merged");
    }
    if *input.state == PullRequestState::Closed {
        return Some("closed");
    }
    if *input.mergeability == PullRequestMergeability::Conflicting {
        return Some("resolve");
    }
    if input.is_draft {
        return input.can_mark_ready.then_some("ready");
    }
    if input.auto_merge_enabled == Some(true) {
        return Some("auto-merge-armed");
    }
    if !input.has_merge_method {
        return None;
    }
    if input.auto_merge_enabled == Some(false)
        && input.checks.is_some()
        && input.checks != Some(&PullRequestChecksState::Passing)
        && input.can_enable_auto_merge
    {
        return Some("enable-auto-merge");
    }
    input.can_merge.then_some("merge")
}
pub fn should_refresh_activity(previous: Option<(&str, &str)>, next: (&str, &str)) -> bool {
    previous.is_some_and(|previous| previous.0 == next.0 && previous.1 != next.1)
}
pub fn merge_thread_comments(
    base: &[PullRequestThreadComment],
    loaded: &[PullRequestThreadComment],
) -> Vec<PullRequestThreadComment> {
    let mut seen: BTreeSet<_> = base.iter().map(|c| &c.id).collect();
    base.iter()
        .chain(loaded.iter().filter(|c| seen.insert(&c.id)))
        .cloned()
        .collect()
}
pub fn edit_thread_comment(comments: &mut [PullRequestThreadComment], id: &str, body: &str) {
    for comment in comments.iter_mut().filter(|c| c.id.0 == id) {
        comment.body = body.into();
    }
}
pub fn is_stacked_base(base: &str, refs: &[VcsRef]) -> bool {
    let Some(default) = refs.iter().find(|r| r.is_default) else {
        return false;
    };
    if default.is_remote != Some(Some(true)) {
        return default.name.0 != base;
    }
    let remote = default
        .remote_name
        .as_ref()
        .and_then(Option::as_ref)
        .map(|v| v.0.as_str())
        .unwrap_or_else(|| default.name.0.split('/').next().unwrap_or_default());
    default
        .name
        .0
        .strip_prefix(&format!("{remote}/"))
        .unwrap_or(&default.name.0)
        != base
}
pub fn allowed_merge_methods(detail: &PullRequestDetail) -> Vec<PullRequestMergeMethod> {
    detail
        .capabilities
        .merge_methods
        .iter()
        .filter(|method| match method {
            PullRequestMergeMethod::Merge => detail.merge_capabilities.merge,
            PullRequestMergeMethod::Squash => detail.merge_capabilities.squash,
            PullRequestMergeMethod::Rebase => detail.merge_capabilities.rebase,
            PullRequestMergeMethod::FastForward => {
                detail.merge_capabilities.fast_forward == Some(Some(true))
            }
        })
        .cloned()
        .collect()
}
pub fn can_perform_action(detail: &PullRequestDetail, action: &PullRequestAction) -> bool {
    detail.capabilities.actions.contains(action)
        && detail.viewer_permissions.actions.contains(action)
}
pub fn can_delete_branch(detail: &PullRequestDetail) -> bool {
    detail.state != PullRequestState::Open
        && detail.capabilities.delete_branch == Some(Some(true))
        && can_perform_action(detail, &PullRequestAction::DeleteBranch)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksState {
    None,
    Pending,
    Failing,
    Passing,
}
pub fn classify_checks(checks: &[PullRequestCheck]) -> ChecksState {
    if checks.is_empty() {
        ChecksState::None
    } else if checks.iter().any(|c| {
        matches!(
            c.status,
            PullRequestCheckStatus::Failure | PullRequestCheckStatus::Cancelled
        )
    }) {
        ChecksState::Failing
    } else if checks.iter().any(|c| {
        matches!(
            c.status,
            PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired
        )
    }) {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}
pub fn describe_checks(checks: &[PullRequestCheck]) -> String {
    if checks.is_empty() {
        return "No checks reported".into();
    }
    let total = checks.len();
    let count = |status| checks.iter().filter(|c| c.status == status).count();
    let failed = count(PullRequestCheckStatus::Failure) + count(PullRequestCheckStatus::Cancelled);
    let pending = count(PullRequestCheckStatus::Pending);
    let action = count(PullRequestCheckStatus::ActionRequired);
    let passed = count(PullRequestCheckStatus::Success);
    let mut parts = Vec::new();
    if pending > 0 {
        parts.push(format!("{pending} of {total} running"));
    }
    if action > 0 {
        parts.push(format!("{action} of {total} awaiting action"));
    }
    if failed > 0 {
        parts.push(if parts.is_empty() {
            format!("{failed} of {total} failing")
        } else {
            format!("{failed} failed")
        });
    }
    if parts.is_empty() {
        return if passed == total {
            "All checks passed".into()
        } else {
            format!("{passed} of {total} passing")
        };
    }
    parts.join(" · ")
}
pub fn group_checks(
    checks: &[PullRequestCheck],
) -> (
    Vec<&PullRequestCheck>,
    Vec<&PullRequestCheck>,
    Vec<&PullRequestCheck>,
) {
    (
        checks
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    PullRequestCheckStatus::Failure
                        | PullRequestCheckStatus::Cancelled
                        | PullRequestCheckStatus::ActionRequired
                )
            })
            .collect(),
        checks
            .iter()
            .filter(|c| c.status == PullRequestCheckStatus::Pending)
            .collect(),
        checks
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    PullRequestCheckStatus::Success
                        | PullRequestCheckStatus::Skipped
                        | PullRequestCheckStatus::Neutral
                )
            })
            .collect(),
    )
}
pub fn thread_panel_action(detail: &PullRequestDetail) -> Option<&'static str> {
    if detail.state != PullRequestState::Open {
        return None;
    }
    if detail.mergeability == PullRequestMergeability::Conflicting {
        return Some("resolve");
    }
    if detail.is_draft {
        return can_perform_action(detail, &PullRequestAction::Ready).then_some("ready");
    }
    match classify_checks(&detail.checks) {
        ChecksState::Failing => Some("fix"),
        ChecksState::Pending => None,
        _ => (can_perform_action(detail, &PullRequestAction::Merge)
            && !allowed_merge_methods(detail).is_empty())
        .then_some("merge"),
    }
}
pub fn base_freshness(
    detail: &PullRequestDetail,
) -> Option<(Option<u64>, Vec<PullRequestUpdateMethod>)> {
    if detail.state != PullRequestState::Open
        || detail.base_comparison != Some(Some(PullRequestBaseComparison::Behind))
        || detail.mergeability != PullRequestMergeability::Mergeable
    {
        return None;
    }
    let offered = detail
        .capabilities
        .update_methods
        .as_ref()
        .and_then(Option::as_ref)
        .map_or(&[][..], Vec::as_slice);
    let allowed = detail
        .viewer_permissions
        .update_methods
        .as_ref()
        .and_then(Option::as_ref)
        .map_or(&[][..], Vec::as_slice);
    Some((
        detail.behind_by.flatten(),
        offered
            .iter()
            .filter(|m| allowed.contains(m))
            .cloned()
            .collect(),
    ))
}
pub fn action_needs_host_refresh(action: &PullRequestAction) -> bool {
    matches!(
        action,
        PullRequestAction::UpdateBranch | PullRequestAction::ApproveWorkflows
    )
}
pub fn action_menu_has_group(draft_toggle: bool, auto_merge: bool, merge_methods: bool) -> bool {
    draft_toggle || auto_merge || merge_methods
}
pub fn handoff_labels(in_thread: bool) -> (&'static str, &'static str, &'static str) {
    if in_thread {
        ("Fix", "Fix in this thread", "Fix findings in this thread")
    } else {
        ("Fix in a thread", "Fix", "Fix findings in a thread")
    }
}
pub fn order_comments(comments: &mut [PullRequestComment], newest_first: bool) {
    if newest_first {
        comments.reverse();
    }
}
pub fn is_comment_finished(
    comment: &PullRequestComment,
    thread: Option<&ConversationThread<'_>>,
) -> bool {
    thread.is_some_and(ConversationThread::is_resolved)
        || review_outcome(comment.review_state.as_deref()) == Some(ReviewOutcome::Dismissed)
}
pub fn is_not_found(error: &PullRequestOperationError) -> bool {
    error.reason.as_ref().and_then(Option::as_ref)
        == Some(&PullRequestOperationErrorReason::NotFound)
}
pub fn checkout_failure_description(
    error: Option<&t3_contracts::git::GitCommandError>,
    raw: &str,
) -> String {
    let message = error.map(|error| {
        error
            .reason
            .as_ref()
            .and_then(|reason| serde_json::to_value(reason).ok())
            .and_then(|value| value.as_str().map(str::to_owned))
            .map_or_else(
                || error.detail.clone(),
                |reason| format!("{} ({reason})", error.detail),
            )
    });
    readable_failure(
        message.as_deref().unwrap_or(raw),
        "Check that the branch still exists and is not checked out elsewhere, then try again.",
    )
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOutcome {
    Approved,
    ChangesRequested,
    Dismissed,
}
pub fn review_outcome(state: Option<&str>) -> Option<ReviewOutcome> {
    match state?.trim().to_lowercase().replace('_', "-").as_str() {
        "approved" => Some(ReviewOutcome::Approved),
        "changes-requested" => Some(ReviewOutcome::ChangesRequested),
        "dismissed" => Some(ReviewOutcome::Dismissed),
        _ => None,
    }
}
pub fn newest_commit_at(commits: &[PullRequestCommit]) -> Option<&str> {
    let mut newest = None;
    let mut newest_at = i64::MIN;
    for commit in commits {
        if let Some(at) = timestamp(&commit.committed_date.0)
            && at > newest_at
        {
            newest = Some(commit.committed_date.0.as_str());
            newest_at = at;
        }
    }
    newest
}
pub fn verdict_stale(at: &str, newest: Option<&str>) -> bool {
    timestamp(at)
        .zip(newest.and_then(timestamp))
        .is_some_and(|(verdict, commit)| verdict < commit)
}
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewOutcomeEntry {
    pub key: String,
    pub actor: Option<PullRequestActor>,
    pub outcome: ReviewOutcome,
    pub at: String,
    pub stale: bool,
}
pub fn latest_review_outcomes(
    comments: &[PullRequestComment],
    commits: &[PullRequestCommit],
) -> Vec<ReviewOutcomeEntry> {
    let newest = newest_commit_at(commits);
    let mut result: Vec<ReviewOutcomeEntry> = Vec::new();
    for comment in comments {
        let Some(outcome) = review_outcome(comment.review_state.as_deref()) else {
            continue;
        };
        let key = comment
            .author
            .as_ref()
            .map_or_else(|| format!("ghost:{}", comment.id.0), |a| a.login.0.clone());
        let next = ReviewOutcomeEntry {
            key: key.clone(),
            actor: comment.author.clone(),
            outcome,
            at: comment.created_at.0.clone(),
            stale: verdict_stale(&comment.created_at.0, newest),
        };
        if let Some(current) = result.iter_mut().find(|r| r.key == key) {
            if timestamp(&current.at)
                .zip(timestamp(&next.at))
                .is_some_and(|(a, b)| a > b)
            {
                continue;
            }
            *current = next;
        } else {
            result.push(next);
        }
    }
    result.retain(|r| r.outcome != ReviewOutcome::Dismissed);
    result
}
fn without_html_comments(body: &str, unfinished: bool) -> String {
    let mut output = String::new();
    let mut tail = body;
    while let Some(start) = tail.find("<!--") {
        let Some(end) = tail[start + 4..].find("-->").map(|end| start + 4 + end + 3) else {
            output.push_str(if unfinished { &tail[..start] } else { tail });
            return output;
        };
        output.push_str(&tail[..start]);
        tail = &tail[end..];
    }
    output.push_str(tail);
    output
}
pub fn visible_body(body: &str) -> Option<&str> {
    (!without_html_comments(body, false).trim().is_empty()).then(|| body.trim())
}

pub enum ConversationThread<'a> {
    Review(&'a PullRequestReviewThread),
    Overview(&'a PullRequestOverviewThread),
}
impl ConversationThread<'_> {
    pub fn id(&self) -> &str {
        match self {
            Self::Review(t) => &t.id.0,
            Self::Overview(t) => &t.id.0,
        }
    }
    fn comments(&self) -> &[PullRequestThreadComment] {
        match self {
            Self::Review(t) => &t.comments,
            Self::Overview(t) => &t.comments,
        }
    }
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Review(t) => Some(&t.path.0),
            Self::Overview(_) => None,
        }
    }
    pub fn line(&self) -> Option<u64> {
        match self {
            Self::Review(t) => t.line,
            Self::Overview(_) => None,
        }
    }
    pub fn is_resolved(&self) -> bool {
        match self {
            Self::Review(t) => t.is_resolved,
            Self::Overview(t) => t.is_resolved,
        }
    }
    fn comment_count(&self) -> Option<u64> {
        match self {
            Self::Review(t) => t.comment_count.flatten(),
            Self::Overview(t) => t.comment_count.flatten(),
        }
    }
    fn has_cursor(&self) -> bool {
        match self {
            Self::Review(t) => t.next_comments_cursor.is_some(),
            Self::Overview(t) => t.next_comments_cursor.is_some(),
        }
    }
}
pub struct ConversationItem<'a> {
    pub comments: Vec<PullRequestComment>,
    pub thread: Option<ConversationThread<'a>>,
    pub more_replies: u64,
    pub has_more_replies: bool,
    pub url: Option<String>,
}
pub fn conversation_items<'a>(
    comments: &[PullRequestComment],
    threads: Vec<ConversationThread<'a>>,
) -> Vec<ConversationItem<'a>> {
    let by_id: BTreeMap<_, _> = comments.iter().map(|c| (&c.id, c)).collect();
    let mut claimed = BTreeSet::new();
    let mut items = Vec::new();
    for thread in threads {
        let mut ordered: Vec<_> = thread
            .comments()
            .iter()
            .map(|c| {
                by_id.get(&c.id).map_or_else(
                    || PullRequestComment {
                        id: c.id.clone(),
                        kind: PullRequestCommentKind::ReviewComment,
                        author: c.author.clone(),
                        body: c.body.clone(),
                        created_at: c.created_at.clone(),
                        edited_at: c.edited_at.clone(),
                        url: c.url.clone(),
                        path: thread.path().map(str::to_owned),
                        review_state: None,
                        reactions: c.reactions.clone(),
                        can_edit: c.can_edit,
                    },
                    |c| (*c).clone(),
                )
            })
            .collect();
        ordered.sort_by_key(|comment| timestamp(&comment.created_at.0));
        if ordered.is_empty() {
            continue;
        }
        for c in &ordered {
            claimed.insert(c.id.clone());
        }
        let more = thread
            .comment_count()
            .unwrap_or(ordered.len() as u64)
            .saturating_sub(ordered.len() as u64);
        let has_more = more > 0 || thread.has_cursor();
        let url = ordered.iter().find_map(|c| c.url.clone());
        items.push(ConversationItem {
            comments: ordered,
            thread: Some(thread),
            more_replies: more,
            has_more_replies: has_more,
            url,
        });
    }
    items.sort_by(|a, b| {
        timestamp(&a.comments[0].created_at.0).cmp(&timestamp(&b.comments[0].created_at.0))
    });
    let mut threads = items.into_iter().peekable();
    let mut result = Vec::new();
    for comment in comments {
        if claimed.contains(&comment.id) {
            continue;
        }
        while threads.peek().is_some_and(|t| {
            timestamp(&t.comments[0].created_at.0)
                .zip(timestamp(&comment.created_at.0))
                .is_some_and(|(a, b)| a < b)
        }) {
            if let Some(thread) = threads.next() {
                result.push(thread);
            }
        }
        result.push(ConversationItem {
            comments: vec![comment.clone()],
            thread: None,
            more_replies: 0,
            has_more_replies: false,
            url: comment.url.clone(),
        });
    }
    result.extend(threads);
    result
}
pub fn filter_conversations(items: &mut Vec<ConversationItem<'_>>, bots: bool, resolved: bool) {
    items.retain(|item| {
        item.comments.iter().any(|c| {
            let outcome = review_outcome(c.review_state.as_deref());
            (bots
                || !c
                    .author
                    .as_ref()
                    .is_some_and(|a| a.is_bot == Some(Some(true)))
                || matches!(
                    outcome,
                    Some(ReviewOutcome::Approved | ReviewOutcome::ChangesRequested)
                ))
                && (resolved
                    || !(item
                        .thread
                        .as_ref()
                        .is_some_and(ConversationThread::is_resolved)
                        || outcome == Some(ReviewOutcome::Dismissed)))
        })
    });
}
pub fn page_conversations<'a, 'b>(
    items: &'b [ConversationItem<'a>],
    count: usize,
) -> (&'b [ConversationItem<'a>], usize) {
    let hidden = items.len().saturating_sub(count);
    (&items[hidden..], hidden)
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEvent {
    pub id: String,
    pub at: String,
    pub kind: &'static str,
    pub title: String,
    pub body: Option<String>,
    pub markdown: bool,
    pub url: Option<String>,
    pub actor: Option<PullRequestActor>,
    pub commit_authors: Vec<PullRequestActor>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub path: Option<String>,
    pub review_state: Option<String>,
    pub reactions: Vec<PullRequestReaction>,
    pub comment: Option<PullRequestComment>,
    pub thread: Option<TimelineThread>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineThread {
    pub id: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub can_resolve: Option<bool>,
    pub replies: Vec<TimelineEvent>,
    pub more_replies: u64,
    pub has_more_replies: bool,
    pub url: Option<String>,
}
fn comment_event(comment: &PullRequestComment) -> TimelineEvent {
    let review = comment.kind == PullRequestCommentKind::Review;
    TimelineEvent {
        id: comment.id.0.clone(),
        at: comment.created_at.0.clone(),
        kind: if review { "review" } else { "comment" },
        title: if review { "reviewed" } else { "commented" }.into(),
        body: visible_body(&comment.body).map(str::to_owned),
        markdown: true,
        url: comment.url.clone(),
        actor: comment.author.clone(),
        commit_authors: Vec::new(),
        additions: None,
        deletions: None,
        path: comment.path.clone(),
        review_state: comment.review_state.clone(),
        reactions: comment.reactions.clone().flatten().unwrap_or_default(),
        comment: Some(comment.clone()),
        thread: None,
    }
}
pub fn terminology(provider: &str) -> (&'static str, &'static str) {
    if provider == "gitlab" {
        ("merge request", "MR")
    } else {
        ("pull request", "PR")
    }
}
fn simple_event(id: &str, at: &str, kind: &'static str, title: String) -> TimelineEvent {
    TimelineEvent {
        id: id.into(),
        at: at.into(),
        kind,
        title,
        body: None,
        markdown: false,
        url: None,
        actor: None,
        commit_authors: Vec::new(),
        additions: None,
        deletions: None,
        path: None,
        review_state: None,
        reactions: Vec::new(),
        comment: None,
        thread: None,
    }
}
pub fn build_timeline(detail: &PullRequestDetailView) -> Vec<TimelineEvent> {
    let term = terminology(&detail.provider.0).0;
    let mut opened = simple_event(
        "created",
        &detail.created_at.0,
        "opened",
        format!("opened this {term}"),
    );
    opened.actor = detail.author.clone();
    let mut events = vec![opened];
    for commit in &detail.commits {
        let mut event = simple_event(
            &commit.oid.0,
            &commit.committed_date.0,
            "commit",
            format!(
                "Commit {}",
                commit.oid.0.chars().take(7).collect::<String>()
            ),
        );
        event.body = (!commit.message_headline.is_empty()).then(|| commit.message_headline.clone());
        event.commit_authors = commit.authors.clone().flatten().unwrap_or_default();
        event.actor = event.commit_authors.first().cloned();
        event.additions = commit.additions.flatten();
        event.deletions = commit.deletions.flatten();
        events.push(event);
    }
    let threads = detail
        .review_threads
        .iter()
        .map(ConversationThread::Review)
        .chain(
            detail
                .overview_threads
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
                .map(ConversationThread::Overview),
        )
        .collect();
    for item in conversation_items(&detail.comments, threads) {
        let mut event = comment_event(&item.comments[0]);
        if let Some(thread) = &item.thread {
            let (is_outdated, can_resolve) = match thread {
                ConversationThread::Review(t) => (t.is_outdated, t.can_resolve.flatten()),
                ConversationThread::Overview(t) => (t.is_outdated, t.can_resolve.flatten()),
            };
            event.thread = Some(TimelineThread {
                id: thread.id().into(),
                path: thread.path().map(str::to_owned),
                line: thread.line(),
                is_resolved: thread.is_resolved(),
                is_outdated,
                can_resolve,
                replies: item.comments[1..].iter().map(comment_event).collect(),
                more_replies: item.more_replies,
                has_more_replies: item.has_more_replies,
                url: item.url,
            });
        }
        events.push(event);
    }
    let term_title = format!("{}{}", term[..1].to_uppercase(), &term[1..]);
    if let Some(at) = &detail.merged_at {
        events.push(simple_event(
            "merged",
            &at.0,
            "merged",
            format!("{term_title} merged"),
        ));
    } else if let Some(at) = &detail.closed_at {
        events.push(simple_event(
            "closed",
            &at.0,
            "closed",
            format!("{term_title} closed"),
        ));
    }
    events.sort_by(|a, b| {
        timestamp(&b.at)
            .unwrap_or(0)
            .cmp(&timestamp(&a.at).unwrap_or(0))
    });
    events
}
pub enum TimelineRow<'a> {
    Event(&'a TimelineEvent),
    Comments(Vec<&'a TimelineEvent>),
}
pub fn group_timeline(events: &[TimelineEvent]) -> Vec<TimelineRow<'_>> {
    let mut rows = Vec::new();
    for event in events {
        if matches!(event.kind, "comment" | "review")
            && event.thread.is_none()
            && review_outcome(event.review_state.as_deref()).is_none()
        {
            if let Some(TimelineRow::Comments(comments)) = rows.last_mut() {
                comments.push(event);
            } else {
                rows.push(TimelineRow::Comments(vec![event]));
            }
        } else {
            rows.push(TimelineRow::Event(event));
        }
    }
    rows
}
pub fn readable_failure(raw: &str, hint: &str) -> String {
    let mut detail = raw.trim();
    if let Some(tail) = detail.strip_prefix("Pull request operation ")
        && let Some((_, message)) = tail.split_once(" failed:")
    {
        detail = message.trim();
    }
    let lower = detail.to_lowercase();
    let words: Vec<_> = lower.trim_end_matches('.').split_whitespace().collect();
    let noise = detail.is_empty()
        || lower == "unknown error"
        || lower == "unknown error."
        || matches!(words.as_slice(), ["exited" | "exit", "with", "code" | "status", number] if number.bytes().all(|b| b.is_ascii_digit()))
        || words.last() == Some(&"failed")
            && words[..words.len() - 1].iter().all(|word| {
                [
                    "github",
                    "gitlab",
                    "forgejo",
                    "gitcafe",
                    "bitbucket",
                    "azure",
                    "devops",
                    "cursor",
                    "cli",
                    "api",
                    "command",
                ]
                .contains(word)
            });
    if noise {
        hint.into()
    } else {
        bound_utf16(detail, 320, "…")
    }
}
pub fn displayed_detail<'a>(
    live: Option<&'a PullRequestDetail>,
    cached: Option<&'a PullRequestDetail>,
    reference: &PullRequestRef,
) -> Option<&'a PullRequestDetail> {
    if live.is_some() {
        return live;
    }
    let cached = cached?;
    if cached.project_id != reference.project_id
        || !cached
            .repository
            .0
            .eq_ignore_ascii_case(&reference.repository.0)
        || cached.number != reference.number
    {
        return None;
    }
    if let Some(host) = reference.host.as_ref().and_then(Option::as_ref) {
        let url = url::Url::parse(&cached.url.0).ok()?;
        let authority = url.authority().rsplit('@').next()?.to_lowercase();
        if authority != host.0.to_lowercase() {
            return None;
        }
    }
    Some(cached)
}
fn snapshot_key(
    environment: &EnvironmentId,
    reference: &PullRequestRef,
) -> Result<StorageKey, ClientError> {
    let key = PullRequestKey::new(environment.clone(), reference);
    Ok(StorageKey::new(
        "pull-request-detail-snapshot",
        Some(environment.clone()),
        serde_json::to_string(&(
            &key.project.project,
            &key.host,
            &key.repository,
            key.number,
            &key.expected_account_id,
        ))?,
    ))
}
pub async fn read_detail_snapshot(
    storage: &impl Storage,
    environment: &EnvironmentId,
    reference: &PullRequestRef,
) -> Option<PullRequestDetail> {
    read_detail_snapshot_with(storage, environment, reference, |detail| {
        displayed_detail(None, Some(detail), reference).is_some()
    })
    .await
}
pub async fn read_detail_snapshot_with(
    storage: &impl Storage,
    environment: &EnvironmentId,
    reference: &PullRequestRef,
    valid: impl Fn(&PullRequestDetail) -> bool,
) -> Option<PullRequestDetail> {
    let key = snapshot_key(environment, reference).ok()?;
    let mut bytes = storage.get(StorageArea::Ordinary, key).await.ok()?;
    if bytes.is_none() && reference.host.is_some() {
        let mut hostless = reference.clone();
        hostless.host = None;
        bytes = storage
            .get(
                StorageArea::Ordinary,
                snapshot_key(environment, &hostless).ok()?,
            )
            .await
            .ok()?;
    }
    let detail: PullRequestDetail = serde_json::from_slice(&bytes?).ok()?;
    valid(&detail).then_some(detail)
}
pub async fn write_detail_snapshot(
    storage: &impl Storage,
    environment: &EnvironmentId,
    reference: &PullRequestRef,
    detail: &PullRequestDetail,
) -> Result<(), ClientError> {
    let key = snapshot_key(environment, reference)?;
    let index_key = StorageKey::new("pull-request-detail-snapshot-index", None, "recentDetails");
    loop {
        let previous = storage
            .get(StorageArea::Ordinary, index_key.clone())
            .await?;
        let mut recent: Vec<StorageKey> = previous
            .as_deref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_default();
        recent.retain(|previous| previous != &key);
        recent.push(key.clone());
        let mut writes: Vec<_> = recent
            .drain(..recent.len().saturating_sub(20))
            .map(|key| StorageWrite { key, value: None })
            .collect();
        writes.push(StorageWrite {
            key: index_key.clone(),
            value: Some(serde_json::to_vec(&recent)?),
        });
        writes.push(StorageWrite {
            key: key.clone(),
            value: Some(serde_json::to_vec(detail)?),
        });
        if storage
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: index_key.clone(),
                    expected: previous,
                }],
                ordinary: writes,
                secure: Vec::new(),
            })
            .await?
        {
            return Ok(());
        }
    }
}
pub fn summary_is_stale(
    detail: &PullRequestDetail,
    summary: &PullRequestSummary,
) -> Result<bool, ClientError> {
    Ok(is_stale_reply(
        &serde_json::to_value(detail)?,
        &serde_json::to_value(summary)?,
    ))
}

/// Repository breadcrumb for the web forge PR URL shapes. Preserve the enterprise origin
/// and nested namespaces, and reject unrelated paths rather than inventing a link.
pub fn repository_link(raw: &str, number: u64) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let (prefix, last) = path.rsplit_once('/')?;
    if last.parse::<u64>().ok()? != number {
        return None;
    }
    let repo = [
        "/-/merge_requests",
        "/pullrequest",
        "/pull",
        "/pulls",
        "/pull-requests",
        "/merge_requests",
    ]
    .iter()
    .find_map(|suffix| prefix.strip_suffix(suffix))?
    .to_owned();
    if repo.trim_matches('/').is_empty() {
        return None;
    }
    url.set_path(&repo);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
}

/// Build the linked-threads key from the forge URL for this checkout and PR.
pub fn linked_thread_reference(
    detail: &PullRequestDetail,
    reference: &PullRequestRef,
) -> Option<PullRequestRef> {
    let (parsed, _) = crate::state::pull_requests::parsed_key(&detail.url.0)?;
    let url = url::Url::parse(&detail.url.0).ok()?;
    let requested_host = reference.host.as_ref().and_then(Option::as_ref);
    if !url.username().is_empty()
        || url.password().is_some()
        || requested_host.is_some_and(|host| {
            !host.0.eq_ignore_ascii_case(&parsed.host.0)
                && !host.0.eq_ignore_ascii_case(url.authority())
        })
        || reference.project_id != detail.project_id
        || reference.number != parsed.number
        || detail.number != parsed.number
        || !detail
            .repository
            .0
            .eq_ignore_ascii_case(&reference.repository.0)
    {
        return None;
    }
    Some(PullRequestRef {
        host: Some(Some(parsed.host)),
        repository: parsed.repository,
        ..reference.clone()
    })
}

/// Resolves the legacy client-side `pullRequestMergeMethodOverrides` map when a project
/// has no scoped server `pullRequestMergeMethod` default configured.
pub fn resolve_legacy_merge_method_override(
    client: &t3_contracts::settings::ClientSettingsSchema,
    projects: &[crate::workspace::projects::EnvironmentProject],
    environment_id: &EnvironmentId,
    project_id: &t3_contracts::base_schemas::ProjectId,
) -> Option<PullRequestMergeMethod> {
    use crate::services::settings::scope::logical_project_scope_key;
    use crate::workspace::projects::{
        GroupingMode, GroupingSettings, PhysicalProjectKey, logical_project_key,
    };
    use t3_contracts::settings::SidebarProjectGroupingMode;

    if client.pull_request_merge_method_overrides.is_empty() {
        return None;
    }
    let target = projects
        .iter()
        .find(|p| &p.environment == environment_id && &p.project.id == project_id)?;
    let to_mode = |mode: &SidebarProjectGroupingMode| match mode {
        SidebarProjectGroupingMode::Repository => GroupingMode::Repository,
        SidebarProjectGroupingMode::RepositoryPath => GroupingMode::RepositoryPath,
        SidebarProjectGroupingMode::Separate => GroupingMode::Separate,
    };
    let settings = GroupingSettings {
        mode: to_mode(&client.sidebar_project_grouping_mode),
        overrides: client
            .sidebar_project_grouping_overrides
            .iter()
            .filter_map(|(key, mode)| {
                let (env, path) = key.0.split_once(':')?;
                Some((
                    PhysicalProjectKey::new(EnvironmentId(env.into()), path),
                    to_mode(mode),
                ))
            })
            .collect(),
    };
    let key = logical_project_scope_key(&logical_project_key(target, &settings, false));
    client
        .pull_request_merge_method_overrides
        .iter()
        .find(|(candidate, _)| candidate.0 == key)
        .map(|(_, method)| method.clone())
}
/// Object IDs for the exact file versions displayed in a whole-PR patch.
/// Use the panel parser's file ranges and decoded paths, including rename destinations.
pub fn diff_file_revisions(patch: &str) -> std::collections::BTreeMap<String, String> {
    crate::workspace::diff_panel::files::parse_patch_files(patch)
        .into_iter()
        .filter_map(|file| {
            let index = patch[file.range.clone()]
                .lines()
                .take_while(|line| !line.starts_with("@@"))
                .find_map(|line| line.strip_prefix("index "))?;
            let (_, new) = index.split_whitespace().next()?.split_once("..")?;
            if new.is_empty() || !new.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            Some((file.path().to_owned(), new.to_owned()))
        })
        .collect()
}
#[cfg(test)]
mod review_revision_tests {
    #[test]
    fn revisions_follow_paths_and_ignore_hunk_content() {
        let patch = "diff --git a/before b/after\nsimilarity index 85%\nrename from before\nrename to after\nindex abc..def 100644\n--- a/before\n+++ b/after\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/other b/other\n--- a/other\n+++ b/other\n@@ -1 +1 @@\n-x\n+index abc..999\n";
        assert_eq!(
            super::diff_file_revisions(patch),
            [("after".into(), "def".into())].into()
        );
    }
}
