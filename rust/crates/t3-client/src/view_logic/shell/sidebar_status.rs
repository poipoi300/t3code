use crate::thread_views::shelves::RuntimeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Approval,
    Input,
    Request,
    Preparing,
    Working,
    Waiting,
    Failed,
    Limited,
    Ready,
}
#[derive(Debug, Clone, Default)]
pub struct StatusInput<'a> {
    pub approvals: bool,
    pub user_input: bool,
    pub subagent_request: Option<&'a str>,
    pub guest_messages: usize,
    pub runtime: Option<RuntimeState>,
    pub usage_limit: bool,
    pub blocked_on_thread_wait: bool,
    pub latest_run_failed: bool,
    pub latest_run_completed_at: Option<&'a str>,
    pub last_visited_at: Option<&'a str>,
    pub background_holds_completion: bool,
    pub plan_mode: bool,
    pub latest_run_settled: bool,
    pub actionable_plan: bool,
}
pub fn resolve_status(input: &StatusInput<'_>) -> Status {
    if input.approvals || input.subagent_request == Some("approval") {
        return Status::Approval;
    }
    if input.user_input || input.subagent_request == Some("user_input") {
        return Status::Input;
    }
    if input.guest_messages > 0 {
        return Status::Request;
    }
    match input.runtime {
        Some(RuntimeState::Running) if input.blocked_on_thread_wait => Status::Waiting,
        Some(RuntimeState::Preparing) => Status::Preparing,
        Some(
            RuntimeState::Queued
            | RuntimeState::Starting
            | RuntimeState::Running
            | RuntimeState::Waiting,
        ) => Status::Working,
        Some(RuntimeState::Idle) => Status::Waiting,
        Some(RuntimeState::Failed) if input.usage_limit => Status::Limited,
        Some(RuntimeState::Failed) => Status::Failed,
        _ => Status::Ready,
    }
}
pub fn should_recede(
    status: Status,
    unread: bool,
    woke: bool,
    active: bool,
    selected: bool,
) -> bool {
    if active || selected || matches!(status, Status::Input | Status::Request) {
        return false;
    }
    match status {
        Status::Preparing | Status::Working | Status::Waiting => true,
        Status::Ready | Status::Approval => !unread && !woke,
        _ => false,
    }
}
pub fn has_unseen_completion(
    input: &StatusInput<'_>,
    parse: &impl Fn(&str) -> Option<i64>,
) -> bool {
    let Some(completed) = input.latest_run_completed_at.and_then(parse) else {
        return false;
    };
    let Some(visited) = input.last_visited_at.filter(|s| !s.is_empty()) else {
        return false;
    };
    parse(visited).is_none_or(|visit| completed > visit)
}
pub fn has_unseen_failure(input: &StatusInput<'_>, parse: &impl Fn(&str) -> Option<i64>) -> bool {
    if !input.latest_run_failed {
        return false;
    }
    let Some(visited) = input.last_visited_at.filter(|s| !s.is_empty()) else {
        return true;
    };
    input
        .latest_run_completed_at
        .and_then(parse)
        .zip(parse(visited))
        .is_none_or(|(completed, visited)| completed > visited)
}
pub fn window_title_status(input: &StatusInput<'_>) -> Option<&'static str> {
    let input = StatusInput {
        guest_messages: 0,
        ..input.clone()
    };
    match resolve_status(&input) {
        Status::Approval => Some("Needs approval"),
        Status::Input => Some("Needs your input"),
        Status::Preparing | Status::Working => Some("Working"),
        Status::Waiting if input.runtime == Some(RuntimeState::Running) => Some("Working"),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    PendingApproval,
    AwaitingInput,
    GuestRequest,
    Working,
    Connecting,
    Waiting,
    Failed,
    PlanReady,
    Completed,
}
impl Pill {
    pub fn label(self) -> &'static str {
        match self {
            Self::PendingApproval => "Pending Approval",
            Self::AwaitingInput => "Awaiting Input",
            Self::GuestRequest => "Guest Request",
            Self::Working => "Working",
            Self::Connecting => "Connecting",
            Self::Waiting => "Waiting",
            Self::Failed => "Failed",
            Self::PlanReady => "Plan Ready",
            Self::Completed => "Completed",
        }
    }
    pub fn pulse(self) -> bool {
        matches!(self, Self::Working | Self::Connecting)
    }
}
pub fn resolve_status_pill(
    input: &StatusInput<'_>,
    parse: &impl Fn(&str) -> Option<i64>,
) -> Option<Pill> {
    if input.approvals || input.subagent_request == Some("approval") {
        return Some(Pill::PendingApproval);
    }
    if input.user_input || input.subagent_request == Some("user_input") {
        return Some(Pill::AwaitingInput);
    }
    if input.guest_messages > 0 {
        return Some(Pill::GuestRequest);
    }
    if (input.runtime == Some(RuntimeState::Running) && !input.blocked_on_thread_wait)
        || input.runtime == Some(RuntimeState::Waiting)
    {
        return Some(Pill::Working);
    }
    if matches!(
        input.runtime,
        Some(RuntimeState::Preparing | RuntimeState::Starting | RuntimeState::Queued)
    ) {
        return Some(Pill::Connecting);
    }
    if (input.runtime == Some(RuntimeState::Running) && input.blocked_on_thread_wait)
        || input.background_holds_completion
    {
        return Some(Pill::Waiting);
    }
    if has_unseen_failure(input, parse) {
        return Some(Pill::Failed);
    }
    if input.plan_mode && input.latest_run_settled && input.actionable_plan {
        return Some(Pill::PlanReady);
    }
    has_unseen_completion(input, parse).then_some(Pill::Completed)
}
pub fn folder_dot(
    threads: &[StatusInput<'_>],
    parse: &impl Fn(&str) -> Option<i64>,
) -> Option<&'static str> {
    let statuses = threads.iter().map(resolve_status).collect::<Vec<_>>();
    if statuses.iter().any(|s| {
        matches!(
            s,
            Status::Input | Status::Approval | Status::Request | Status::Limited
        )
    }) {
        return Some("attention");
    }
    if statuses.contains(&Status::Failed) {
        return Some("failed");
    }
    if threads
        .iter()
        .zip(&statuses)
        .any(|(t, s)| *s == Status::Ready && has_unseen_completion(t, parse))
    {
        return Some("unread");
    }
    statuses
        .iter()
        .any(|s| matches!(s, Status::Working | Status::Preparing))
        .then_some("working")
}
/// Outer `None` is an older server omitting visits; inner `None` is an explicit mark-unread.
pub fn resolve_last_visited_at<'a>(
    server: Option<Option<&'a str>>,
    local: Option<&'a str>,
) -> Option<&'a str> {
    server.unwrap_or(local)
}
