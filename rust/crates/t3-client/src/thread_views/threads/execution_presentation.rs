use super::{
    execution::{self, RuntimeSummary},
    pending_work::{background_work_holds_completion, task_id},
};
use t3_contracts::{
    base_schemas::{ThreadId, TurnItemId},
    model::{ProviderOptionDescriptor, ProviderOptionSelectionValue},
    model_selection::ModelSelection,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    server::{ServerProvider, ServerProviderModel},
    wire::WireDateTime,
};

pub const REASONING_EFFORT_OPTION_IDS: &[&str] = &[
    "reasoningEffort",
    "effort",
    "reasoning",
    "variant",
    "reasoning_effort",
    "thinking_level",
];
pub fn format_model_selection_effort(
    selection: &ModelSelection,
    models: &[ServerProviderModel],
    reported: Option<&ModelSelection>,
) -> Option<String> {
    let descriptors = models
        .iter()
        .find(|m| m.slug.0 == selection.model)?
        .capabilities
        .as_ref()?
        .option_descriptors
        .as_ref()?
        .as_ref()?;
    let value = |id: &str| {
        selection
            .options
            .as_ref()
            .and_then(|o| o.0.iter().find(|o| o.id == id))
            .map(|o| &o.value)
    };
    let legacy = matches!(value("effort"),Some(ProviderOptionSelectionValue::String(s)) if s == "ultracode")
        && descriptors
            .iter()
            .any(|d| matches!(d,ProviderOptionDescriptor::Boolean(d) if d.id == "ultracode"));
    let reported =
        reported.filter(|r| r.instance_id == selection.instance_id && r.model == selection.model);
    for &id in REASONING_EFFORT_OPTION_IDS {
        let Some(ProviderOptionDescriptor::Select(d)) = descriptors
            .iter()
            .find(|d| matches!(d,ProviderOptionDescriptor::Select(d) if d.id == id))
        else {
            continue;
        };
        let reported_value = reported
            .and_then(|r| r.options.as_ref())
            .and_then(|o| o.0.iter().find(|o| o.id == id))
            .map(|o| &o.value);
        let stated = value(id).or(reported_value);
        if stated.is_none() && id != "variant" {
            continue;
        }
        if matches!(stated, Some(ProviderOptionSelectionValue::Boolean(_))) {
            continue;
        }
        let current = match stated {
            Some(ProviderOptionSelectionValue::String(_)) if legacy && id == "effort" => {
                Some("xhigh")
            }
            Some(ProviderOptionSelectionValue::String(s)) => Some(s.as_str()),
            _ => None,
        };
        let current = current.or_else(|| {
            (id != "variant")
                .then(|| {
                    d.options
                        .iter()
                        .find(|o| o.is_default == Some(Some(true)))
                        .map(|o| o.id.as_str())
                })
                .flatten()
        });
        if let Some(label)=d.options.iter().find(|o| Some(o.id.as_str()) == current).map(|o| o.label.clone()).or_else(|| {
            if value(id).is_none() && matches!(reported_value,Some(ProviderOptionSelectionValue::String(s)) if s == "default") {Some("Default".into())} else if id == "variant" {Some("Unknown".into())} else {None}
        }) && !label.is_empty() {return Some(label);}
    }
    None
}
pub fn format_subagent_effort(
    model_slug: Option<&str>,
    selection: Option<&ModelSelection>,
    provider: Option<&ServerProvider>,
) -> Option<String> {
    let (slug, selection, provider) = (model_slug?, selection?, provider?);
    let resolved = crate::view_logic::chat_view::resolve_selectable_model(
        &provider.driver.0,
        Some(&selection.model),
        &provider.models,
    )
    .map_or(selection.model.trim(), |m| &m.slug.0);
    if resolved != slug {
        return None;
    }
    if provider.models.iter().any(|m| m.slug.0 == slug) {
        let mut selection = selection.clone();
        selection.model = slug.into();
        return format_model_selection_effort(&selection, &provider.models, None);
    }
    REASONING_EFFORT_OPTION_IDS.iter().find_map(|id| {
        selection
            .options
            .as_ref()?
            .0
            .iter()
            .find(|o| o.id == *id)
            .and_then(|o| match &o.value {
                ProviderOptionSelectionValue::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
    })
}

pub fn format_provider_subagent_status(
    status: Option<&execution::ProviderSubagentStatus>,
    now: i64,
    parse: impl Fn(&WireDateTime) -> Option<i64>,
    duration: impl Fn(i64) -> String,
) -> String {
    let Some(s) = status else {
        return "Starting".into();
    };
    let label = match s.status {
        ExecutionNodeStatus::Idle => "Idle",
        ExecutionNodeStatus::Pending | ExecutionNodeStatus::Running => "Working",
        ExecutionNodeStatus::Waiting => "Waiting",
        ExecutionNodeStatus::Completed => "Completed",
        ExecutionNodeStatus::Interrupted => "Interrupted",
        ExecutionNodeStatus::Failed => "Failed",
        ExecutionNodeStatus::Cancelled | ExecutionNodeStatus::RolledBack => "Cancelled",
    };
    let live = matches!(
        s.status,
        ExecutionNodeStatus::Pending | ExecutionNodeStatus::Running | ExecutionNodeStatus::Waiting
    );
    if !live && s.status != ExecutionNodeStatus::Completed {
        return label.into();
    }
    let times = s.started_at.as_ref().and_then(&parse).zip(if live {
        Some(now)
    } else {
        s.completed_at.as_ref().and_then(&parse)
    });
    let Some((start, end)) = times else {
        return label.into();
    };
    let elapsed = duration(((end - start).div_euclid(1000) * 1000).max(1000));
    if live {
        format!("{label} {elapsed}")
    } else {
        format!("{label} in {elapsed}")
    }
}

pub struct ProviderGoalPresentation {
    pub title: &'static str,
    pub objective: String,
    pub usage: Option<String>,
    pub can_resume: bool,
}
fn goal_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let thousands = (tokens as f64 / 1000.0).round() as u64;
    if thousands < 1000 {
        return format!("{thousands}k");
    }
    let millions = format!("{:.1}", tokens as f64 / 1_000_000.0);
    format!("{}m", millions.strip_suffix(".0").unwrap_or(&millions))
}
pub fn present_provider_goal(
    goal: &OrchestrationV2ProviderGoal,
    working: bool,
    duration: impl Fn(u64) -> String,
) -> ProviderGoalPresentation {
    let mut usage = vec![];
    if let Some(tokens) = goal.tokens_used.flatten().filter(|n| *n > 0) {
        usage.push(match goal.token_budget.flatten() {
            Some(budget) => format!("{} / {} tokens", goal_tokens(tokens), goal_tokens(budget)),
            None => format!("{} tokens", goal_tokens(tokens)),
        });
    }
    if let Some(seconds) = goal.time_used_seconds.flatten().filter(|s| *s >= 60) {
        usage.push(duration(seconds * 1000));
    }
    if let Some(checks) = goal.checks.flatten().filter(|c| *c > 0) {
        usage.push(format!(
            "{checks} {}",
            if checks == 1 { "check" } else { "checks" }
        ));
    }
    ProviderGoalPresentation {
        title: match goal.status {
            OrchestrationV2ProviderGoalStatus::Active if !working => "Goal set",
            OrchestrationV2ProviderGoalStatus::Active => "Pursuing goal",
            OrchestrationV2ProviderGoalStatus::Paused => "Goal paused",
            OrchestrationV2ProviderGoalStatus::Blocked => "Goal blocked",
            OrchestrationV2ProviderGoalStatus::UsageLimited => "Goal hit a usage limit",
            OrchestrationV2ProviderGoalStatus::BudgetLimited => "Goal reached its token budget",
            OrchestrationV2ProviderGoalStatus::Complete => "Goal complete",
        },
        objective: goal.objective.0.clone(),
        usage: (!usage.is_empty()).then(|| usage.join(" · ")),
        can_resume: !matches!(
            goal.status,
            OrchestrationV2ProviderGoalStatus::Active | OrchestrationV2ProviderGoalStatus::Complete
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundWorkItem {
    pub task_id: String,
    pub kind: &'static str,
    pub label: String,
    pub child_thread_id: Option<ThreadId>,
}
pub struct BackgroundWorkPresentation {
    pub title: String,
    pub items: Vec<BackgroundWorkItem>,
    pub waiting: bool,
}
pub fn present_pending_background_work(
    tasks: &[OrchestrationV2PendingBackgroundTask],
    command_display: impl Fn(&str) -> String,
) -> Option<BackgroundWorkPresentation> {
    if tasks.is_empty() {
        return None;
    }
    let waiting = background_work_holds_completion(tasks);
    let mut items: Vec<_> = tasks
        .iter()
        .map(|task| {
            let (kind, description, child) = match task {
                OrchestrationV2PendingBackgroundTask::Subagent(t) => (
                    "subagent",
                    &t.description,
                    t.child_thread_id.clone().flatten(),
                ),
                OrchestrationV2PendingBackgroundTask::Command(t) => {
                    ("command", &t.description, None)
                }
                OrchestrationV2PendingBackgroundTask::Monitor(t) => {
                    ("monitor", &t.description, None)
                }
                OrchestrationV2PendingBackgroundTask::BackgroundTask(t) => {
                    ("background task", &t.description, None)
                }
            };
            let label = description
                .as_ref()
                .and_then(Option::as_ref)
                .map(|d| d.0.trim())
                .map(|s| {
                    if kind == "subagent" {
                        super::subagent_display::format_subagent_display_title(s)
                            .trim()
                            .into()
                    } else if kind == "command" || kind == "monitor" {
                        command_display(s)
                    } else {
                        s.into()
                    }
                })
                .filter(|s: &String| !s.is_empty())
                .unwrap_or_else(|| kind.into());
            BackgroundWorkItem {
                task_id: task_id(task).into(),
                kind,
                label,
                child_thread_id: child,
            }
        })
        .collect();
    let rank = |kind| match kind {
        "subagent" => 0,
        "command" => 1,
        "monitor" => 2,
        _ => 3,
    };
    items.sort_by_key(|i| rank(i.kind));
    let title = if items.len() == 1 {
        let only = &items[0];
        if only.label != only.kind {
            format!(
                "{} · {}",
                if waiting { "Waiting" } else { "Running" },
                only.label
            )
        } else {
            format!(
                "{} a {}",
                if waiting { "Waiting on" } else { "Running" },
                only.kind
            )
        }
    } else {
        let mut groups = vec![];
        for kind in ["subagent", "command", "monitor", "background task"] {
            let count = items.iter().filter(|i| i.kind == kind).count();
            if count > 0 {
                groups.push(format!(
                    "{count} {kind}{}",
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
        let joined = if groups.len() > 1 {
            format!(
                "{} and {}",
                groups[..groups.len() - 1].join(", "),
                groups[groups.len() - 1]
            )
        } else {
            groups.join("")
        };
        format!(
            "{} {joined}",
            if waiting { "Waiting on" } else { "Running" }
        )
    };
    Some(BackgroundWorkPresentation {
        title,
        items,
        waiting,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimitNoticeRows {
    pub item_ids: Vec<IdOrIndex<TurnItemId>>,
    pub provider_notice_item_id: Option<IdOrIndex<TurnItemId>>,
}
pub fn is_provider_limit_notice(text: &str) -> bool {
    let text = t3_contracts::wire::trim_js(text).to_lowercase();
    let text = text.strip_suffix('.').unwrap_or(&text);
    let (notice, reset) = text
        .split_once(" · resets ")
        .map_or((text, None), |(a, b)| (a, Some(b)));
    let limit = ["you've hit your ", "you’ve hit your "]
        .iter()
        .filter_map(|prefix| notice.strip_prefix(prefix))
        .any(|rest| {
            matches!(
                rest,
                "limit" | "5-hour limit" | "session limit" | "weekly limit" | "usage limit"
            )
        });
    if !limit
        && !matches!(
            notice,
            "you're out of extra usage"
                | "you’re out of extra usage"
                | "claude ai usage limit reached"
        )
    {
        return false;
    }
    let Some(reset) = reset else {
        return true;
    };
    let clock = if let Some((clock, zone)) = reset.split_once(" (") {
        let Some(zone) = zone.strip_suffix(')') else {
            return false;
        };
        if !zone.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
        }) {
            return false;
        }
        clock
    } else {
        reset
    };
    let Some(clock) = clock
        .strip_suffix("am")
        .or_else(|| clock.strip_suffix("pm"))
    else {
        return false;
    };
    let (hour, minute) = clock
        .split_once(':')
        .map_or((clock, None), |(h, m)| (h, Some(m)));
    !hour.starts_with('0')
        && hour.parse::<u8>().is_ok_and(|h| (1..=12).contains(&h))
        && minute.is_none_or(|m| {
            m.len() == 2
                && m.bytes().all(|b| b.is_ascii_digit())
                && m.parse::<u8>().is_ok_and(|m| m <= 59)
        })
}
pub fn derive_usage_limit_notice_rows(
    p: &OrchestrationV2ThreadProjection,
    parse: &impl Fn(&WireDateTime) -> i64,
) -> Option<UsageLimitNoticeRows> {
    let runtime = execution::derive_thread_structure_runtime(p, parse)?;
    if runtime.status != Some(OrchestrationV2RunStatus::Failed)
        || runtime.last_error_class != Some(OrchestrationV2ProviderFailureClass::UsageLimit)
    {
        return None;
    }
    let run_id = execution::derive_latest_thread_run(p, parse)?.run_id;
    let run = p.runs.iter().find(|r| r.id == run_id)?;
    let failure = execution::latest_root_provider_failure(Some(run), &p.turn_items, parse);
    let mut item_ids = vec![];
    let mut turn = None;
    for item in &p.turn_items {
        if let OrchestrationV2TurnItem::Error(i) = item
            && i.run_id.as_ref() == Some(&run.id)
            && i.node_id == run.root_node_id
            && i.status == OrchestrationV2TurnItemStatus::Failed
            && i.failure.class == OrchestrationV2ProviderFailureClass::UsageLimit
        {
            item_ids.push(i.id.clone());
            if failure.is_some_and(|f| std::ptr::eq(f, &i.failure)) {
                turn = i.provider_turn_id.as_ref();
            }
        }
    }
    let mut notice = None;
    if let Some(turn) = turn {
        for item in &p.turn_items {
            if item_field!(item, run_id).as_ref() != Some(&run.id)
                || item_field!(item, provider_turn_id).as_ref() != Some(turn)
            {
                continue;
            }
            match item {
                OrchestrationV2TurnItem::SystemNotice(i)
                    if i.node_id == run.root_node_id
                        && i.parent_item_id.is_none()
                        && i.native_item_ref.as_ref().is_some_and(|r| {
                            r.driver.0 == "claudeAgent"
                                && r.native_id.as_ref().is_some_and(|id| {
                                    id.0.starts_with(&format!(
                                        "usage-limit:{}:",
                                        execution::id_text(turn)
                                    ))
                                })
                        }) =>
                {
                    item_ids.push(i.id.clone())
                }
                OrchestrationV2TurnItem::AssistantMessage(i)
                    if i.parent_item_id.is_none()
                        && notice
                            .is_none_or(|n: &TurnItemAssistantMessage| i.ordinal > n.ordinal) =>
                {
                    notice = Some(i.as_ref())
                }
                _ => {}
            }
        }
    }
    Some(UsageLimitNoticeRows {
        item_ids,
        provider_notice_item_id: notice.map(|i| i.id.clone()),
    })
}
pub fn without_usage_limit_notice_rows<'a>(
    rows: &'a [OrchestrationV2ProjectedTurnItem],
    notice: Option<&UsageLimitNoticeRows>,
    provider_limit_notice: impl Fn(&str) -> bool,
) -> Vec<&'a OrchestrationV2ProjectedTurnItem> {
    rows.iter().filter(|row| notice.is_none_or(|n| !n.item_ids.contains(item_field!(&row.item,id)) && !matches!(&row.item,OrchestrationV2TurnItem::AssistantMessage(i) if Some(&i.id) == n.provider_notice_item_id.as_ref() && provider_limit_notice(i.text.trim())))).collect()
}

pub fn notification_child_thread_id(
    source: &OrchestrationV2NotificationSource,
) -> Option<&ThreadId> {
    match source {
        OrchestrationV2NotificationSource::SubagentNotificationSource(s) => {
            s.child_thread_id.as_ref().and_then(Option::as_ref)
        }
        OrchestrationV2NotificationSource::Member1(s) => {
            s.child_thread_id.as_ref().and_then(Option::as_ref)
        }
        _ => None,
    }
}

pub fn runtime_can_archive(runtime: Option<&RuntimeSummary>) -> bool {
    runtime.is_none_or(|r| {
        if r.status == Some(OrchestrationV2RunStatus::Queued) {
            r.active_run_id.is_none()
        } else {
            !r.status
                .as_ref()
                .is_some_and(execution::interruptible_status)
        }
    })
}
