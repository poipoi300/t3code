use super::execution::{id_text, work_active};
use std::collections::BTreeMap;
use t3_contracts::{
    model::{ProviderOptionSelection, ProviderOptionSelectionValue, ProviderOptionSelections},
    model_selection::ModelSelection,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    wire::WireDateTime,
};

pub fn is_terminal_subagent_status(s: &OrchestrationV2TurnItemStatus) -> bool {
    matches!(
        s,
        OrchestrationV2TurnItemStatus::Completed
            | OrchestrationV2TurnItemStatus::Failed
            | OrchestrationV2TurnItemStatus::Cancelled
            | OrchestrationV2TurnItemStatus::Interrupted
    )
}
pub fn is_live_subagent_turn_item(i: &OrchestrationV2TurnItem) -> bool {
    matches!(i, OrchestrationV2TurnItem::Subagent(i) if work_active(&i.status))
}
pub fn waiting_on_subagents_label(count: usize) -> String {
    if count == 1 {
        "Waiting on a subagent".into()
    } else {
        format!("Waiting on {count} subagents")
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSubagentKind {
    Subagent,
    Workflow,
    WorkflowAgent,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentUsage {
    pub total_tokens: u64,
    pub tool_uses: Option<u64>,
    pub duration_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSubagent {
    pub id: String,
    pub kind: RuntimeSubagentKind,
    pub title: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub status: OrchestrationV2TurnItemStatus,
    pub activation_count: u64,
    pub usage: Option<SubagentUsage>,
    pub progress: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub parent_agent_id: Option<String>,
    pub agent_index: Option<u64>,
    pub phase_index: Option<u64>,
    pub phase_title: Option<String>,
    pub attempt: Option<u64>,
    pub workflow_name: Option<String>,
    pub phases: Vec<OrchestrationV2WorkflowPhase>,
    pub run_handles: Option<OrchestrationV2WorkflowRunHandles>,
    pub child_thread_id: Option<String>,
    pub first_seen_at: WireDateTime,
    pub started_at: Option<WireDateTime>,
    pub completed_at: Option<WireDateTime>,
    pub updated_at: WireDateTime,
}
fn prompt_title(prompt: &str) -> String {
    if prompt.encode_utf16().count() <= 80 {
        return prompt.into();
    }
    let mut units = 0;
    let prefix: String = prompt
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 77
        })
        .collect();
    format!("{prefix}...")
}
fn usage(total: Option<u64>, tools: Option<u64>, duration: Option<u64>) -> Option<SubagentUsage> {
    total.map(|total_tokens| SubagentUsage {
        total_tokens,
        tool_uses: tools,
        duration_ms: duration,
    })
}

pub fn projected_subagents_to_runtime(
    agents: &[OrchestrationV2Subagent],
    epoch_iso: impl Fn(u64) -> WireDateTime,
) -> Vec<RuntimeSubagent> {
    agents
        .iter()
        .flat_map(|agent| {
            let workflow = agent.workflow.as_ref().and_then(Option::as_ref);
            let coordinator = RuntimeSubagent {
                id: id_text(&agent.id),
                kind: if workflow.is_some() {
                    RuntimeSubagentKind::Workflow
                } else {
                    RuntimeSubagentKind::Subagent
                },
                title: agent
                    .title
                    .clone()
                    .unwrap_or_else(|| prompt_title(&agent.prompt)),
                model: agent.model.clone(),
                effort: None,
                speed: None,
                status: agent.status.clone(),
                activation_count: 1,
                usage: workflow.and_then(|w| {
                    usage(
                        w.total_tokens.flatten(),
                        w.tool_calls.flatten(),
                        w.duration_ms.flatten(),
                    )
                }),
                progress: agent.progress.clone().flatten(),
                result: agent.result.clone(),
                error: (agent.status == OrchestrationV2TurnItemStatus::Failed)
                    .then(|| agent.result.clone())
                    .flatten(),
                parent_agent_id: None,
                agent_index: None,
                phase_index: None,
                phase_title: None,
                attempt: None,
                workflow_name: workflow
                    .and_then(|w| w.name.as_ref()?.as_ref())
                    .map(|s| s.0.clone()),
                phases: workflow.map(|w| w.phases.clone()).unwrap_or_default(),
                run_handles: workflow.and_then(|w| w.run_handles.clone().flatten()),
                child_thread_id: agent.child_thread_id.as_ref().map(id_text),
                first_seen_at: agent
                    .started_at
                    .as_ref()
                    .unwrap_or(&agent.updated_at)
                    .clone(),
                started_at: agent.started_at.clone(),
                completed_at: agent.completed_at.clone(),
                updated_at: agent.updated_at.clone(),
            };
            let mut expanded = vec![coordinator.clone()];
            if let Some(workflow) = workflow {
                for member in &workflow.agents {
                    let status = if matches!(
                        member.state,
                        WorkflowAgentState::Queued | WorkflowAgentState::Running
                    ) && is_terminal_subagent_status(&coordinator.status)
                    {
                        coordinator.status.clone()
                    } else {
                        match member.state {
                            WorkflowAgentState::Queued => OrchestrationV2TurnItemStatus::Pending,
                            WorkflowAgentState::Running => OrchestrationV2TurnItemStatus::Running,
                            WorkflowAgentState::Completed => {
                                OrchestrationV2TurnItemStatus::Completed
                            }
                            WorkflowAgentState::Failed => OrchestrationV2TurnItemStatus::Failed,
                        }
                    };
                    let started = member.started_at.flatten().map(&epoch_iso);
                    let completed = is_terminal_subagent_status(&status)
                        .then(|| {
                            member
                                .completed_at
                                .flatten()
                                .map(&epoch_iso)
                                .or_else(|| {
                                    member
                                        .started_at
                                        .flatten()
                                        .zip(member.duration_ms.flatten())
                                        .and_then(|(s, d)| s.checked_add(d))
                                        .map(&epoch_iso)
                                })
                                .or_else(|| coordinator.completed_at.clone())
                        })
                        .flatten();
                    let failed = status == OrchestrationV2TurnItemStatus::Failed;
                    expanded.push(RuntimeSubagent {
                        id: format!("{}:agent:{}", coordinator.id, member.index),
                        kind: RuntimeSubagentKind::WorkflowAgent,
                        title: member.label.0.clone(),
                        model: member
                            .model
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.clone()),
                        effort: member
                            .effort
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.clone()),
                        // An omitted member speed inherits the coordinator's value, as object spread does.
                        speed: member
                            .speed
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.clone())
                            .or_else(|| coordinator.speed.clone()),
                        status,
                        activation_count: member.attempt.flatten().unwrap_or(1),
                        usage: usage(
                            member.total_tokens.flatten(),
                            member.tool_calls.flatten(),
                            member.duration_ms.flatten(),
                        ),
                        progress: member.prompt.clone().flatten(),
                        result: (!failed).then(|| member.result.clone().flatten()).flatten(),
                        error: failed.then(|| member.result.clone().flatten()).flatten(),
                        parent_agent_id: Some(coordinator.id.clone()),
                        agent_index: Some(member.index),
                        phase_index: member.phase_index.flatten(),
                        phase_title: member
                            .phase_title
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.clone()),
                        attempt: member.attempt.flatten(),
                        phases: vec![],
                        child_thread_id: member
                            .child_thread_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.clone()),
                        first_seen_at: member
                            .queued_at
                            .flatten()
                            .map(&epoch_iso)
                            .or_else(|| started.clone())
                            .unwrap_or_else(|| coordinator.first_seen_at.clone()),
                        updated_at: completed
                            .as_ref()
                            .or(started.as_ref())
                            .unwrap_or(&coordinator.first_seen_at)
                            .clone(),
                        started_at: started,
                        completed_at: completed,
                        ..coordinator.clone()
                    });
                }
            }
            expanded
        })
        .collect()
}

pub fn workflow_member_model_selection(
    member: &RuntimeSubagent,
    instance_id: ProviderInstanceId,
) -> Option<ModelSelection> {
    let model = member.model.as_ref().filter(|s| !s.is_empty())?.clone();
    let mut options = vec![];
    if let Some(effort) = &member.effort
        && !effort.is_empty()
    {
        options.push(ProviderOptionSelection {
            id: "effort".into(),
            value: ProviderOptionSelectionValue::String(effort.clone()),
        });
    }
    if let Some(speed) = &member.speed
        && !speed.is_empty()
    {
        options.push(ProviderOptionSelection {
            id: "fastMode".into(),
            value: ProviderOptionSelectionValue::Boolean(speed == "fast"),
        });
    }
    Some(ModelSelection {
        instance_id,
        model,
        options: Some(ProviderOptionSelections(options)),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowPhaseState {
    Pending,
    Running,
    Done,
}
pub struct WorkflowPhase<'a> {
    pub index: u64,
    pub title: String,
    pub members: Vec<&'a RuntimeSubagent>,
    pub state: WorkflowPhaseState,
    pub settled_count: usize,
}
pub struct SubagentWorkflowGroup<'a> {
    pub workflow: &'a RuntimeSubagent,
    pub phases: Vec<WorkflowPhase<'a>>,
    pub unphased_members: Vec<&'a RuntimeSubagent>,
}
pub fn derive_workflow_groups(
    agents: &[RuntimeSubagent],
    millis: impl Fn(&WireDateTime) -> i64,
) -> Vec<SubagentWorkflowGroup<'_>> {
    let mut workflows: Vec<_> = agents
        .iter()
        .filter(|a| a.kind == RuntimeSubagentKind::Workflow)
        .collect();
    workflows.sort_by(|a, b| {
        millis(&a.first_seen_at)
            .cmp(&millis(&b.first_seen_at))
            .then(a.id.cmp(&b.id))
    });
    workflows
        .into_iter()
        .map(|workflow| {
            let members: Vec<_> = agents
                .iter()
                .filter(|a| {
                    a.kind != RuntimeSubagentKind::Workflow
                        && a.parent_agent_id.as_deref() == Some(&workflow.id)
                })
                .collect();
            let mut titles = BTreeMap::new();
            for member in &members {
                if let Some(index) = member.phase_index {
                    let title = member
                        .phase_title
                        .clone()
                        .or_else(|| titles.get(&index).cloned())
                        .unwrap_or_else(|| format!("Phase {}", index + 1));
                    titles.insert(index, title);
                }
            }
            for phase in &workflow.phases {
                titles.insert(phase.index, phase.title.0.clone());
            }
            let order =
                |rows: &mut Vec<&RuntimeSubagent>| rows.sort_by_key(|a| a.agent_index.unwrap_or(0));
            let phases = titles
                .into_iter()
                .map(|(index, title)| {
                    let mut rows: Vec<_> = members
                        .iter()
                        .copied()
                        .filter(|m| m.phase_index == Some(index))
                        .collect();
                    order(&mut rows);
                    let settled_count = rows
                        .iter()
                        .filter(|m| is_terminal_subagent_status(&m.status))
                        .count();
                    let state = if rows.is_empty() {
                        WorkflowPhaseState::Pending
                    } else if rows.iter().any(|m| {
                        work_active(&m.status) || m.status == OrchestrationV2TurnItemStatus::Idle
                    }) {
                        WorkflowPhaseState::Running
                    } else {
                        WorkflowPhaseState::Done
                    };
                    WorkflowPhase {
                        index,
                        title,
                        members: rows,
                        state,
                        settled_count,
                    }
                })
                .collect();
            let mut unphased_members = members
                .into_iter()
                .filter(|m| m.phase_index.is_none())
                .collect();
            order(&mut unphased_members);
            SubagentWorkflowGroup {
                workflow,
                phases,
                unphased_members,
            }
        })
        .collect()
}

pub fn is_workflow_script_message(
    role: &ConversationMessageRole,
    text: &str,
    marked: Option<bool>,
) -> bool {
    if *role != ConversationMessageRole::User {
        return false;
    }
    if let Some(marked) = marked {
        return marked;
    }
    let mut text = t3_contracts::wire::trim_js(text);
    loop {
        if let Some(rest) = text.strip_prefix("//") {
            text = rest
                .split_once('\n')
                .map_or("", |(_, rest)| t3_contracts::wire::trim_js(rest));
        } else if let Some(rest) = text.strip_prefix("/*") {
            let Some((_, rest)) = rest.split_once("*/") else {
                return false;
            };
            text = t3_contracts::wire::trim_js(rest);
        } else {
            break;
        }
    }
    fn token<'a>(text: &'a str, word: &str) -> Option<&'a str> {
        text.strip_prefix(word)
            .filter(|rest| {
                rest.chars()
                    .next()
                    .is_some_and(crate::view_logic::is_js_whitespace)
            })
            .map(t3_contracts::wire::trim_js)
    }
    token(text, "export")
        .and_then(|s| token(s, "const"))
        .and_then(|s| s.strip_prefix("meta"))
        .is_some_and(|s| {
            s.chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
        })
}

pub struct WorkflowOpening<'a> {
    pub message: &'a OrchestrationV2ConversationMessage,
    pub coordinator: Option<&'a OrchestrationV2Subagent>,
    pub agents: Vec<RuntimeSubagent>,
}
pub fn workflow_opening_parent_thread_id(
    p: &OrchestrationV2ThreadProjection,
) -> Option<&t3_contracts::base_schemas::ThreadId> {
    if p.thread.creation_source != OrchestrationV2CreationSource::Provider
        || p.thread.lineage.relationship_to_parent
            != Some(AppThreadLineageRelationshipToParent::Subagent)
    {
        return None;
    }
    let first = p
        .messages
        .iter()
        .find(|m| m.role == ConversationMessageRole::User)?;
    is_workflow_script_message(&first.role, &first.text, first.workflow_script.flatten())
        .then_some(p.thread.lineage.parent_thread_id.as_ref())
        .flatten()
}
pub fn resolve_workflow_opening<'a>(
    p: &'a OrchestrationV2ThreadProjection,
    parent: Option<&'a [OrchestrationV2Subagent]>,
    epoch_iso: impl Fn(u64) -> WireDateTime,
) -> Option<WorkflowOpening<'a>> {
    if p.thread.creation_source != OrchestrationV2CreationSource::Provider
        || p.thread.lineage.relationship_to_parent
            != Some(AppThreadLineageRelationshipToParent::Subagent)
    {
        return None;
    }
    let message = p
        .messages
        .iter()
        .find(|m| m.role == ConversationMessageRole::User)?;
    if message.workflow_script == Some(Some(false)) {
        return None;
    }
    let coordinator = parent.and_then(|agents| {
        agents
            .iter()
            .find(|a| a.child_thread_id.as_ref() == Some(&p.thread.id))
    });
    if !coordinator.is_some_and(|c| c.workflow.as_ref().is_some_and(Option::is_some))
        && !is_workflow_script_message(
            &message.role,
            &message.text,
            message.workflow_script.flatten(),
        )
    {
        return None;
    }
    let agents = coordinator
        .map(|c| projected_subagents_to_runtime(std::slice::from_ref(c), epoch_iso))
        .unwrap_or_default();
    Some(WorkflowOpening {
        message,
        coordinator,
        agents,
    })
}
pub fn workflow_opening_headline(
    opening: &WorkflowOpening<'_>,
) -> (String, Option<String>, Option<String>) {
    let description = opening.coordinator.and_then(|c| c.title.clone());
    let title = opening
        .agents
        .first()
        .and_then(|a| a.workflow_name.clone())
        .or_else(|| description.clone())
        .unwrap_or_else(|| "Workflow".into());
    let members: Vec<_> = opening
        .agents
        .iter()
        .filter(|a| a.parent_agent_id.is_some())
        .collect();
    let mut counts = [0; 4];
    for member in &members {
        counts[match member.status {
            OrchestrationV2TurnItemStatus::Completed => 1,
            OrchestrationV2TurnItemStatus::Failed => 2,
            OrchestrationV2TurnItemStatus::Cancelled
            | OrchestrationV2TurnItemStatus::Interrupted => 3,
            _ => 0,
        }] += 1;
    }
    let summary = (!members.is_empty()).then(|| {
        format!(
            "{} agent{} · {}",
            members.len(),
            if members.len() == 1 { "" } else { "s" },
            ["working", "done", "failed", "stopped"]
                .iter()
                .zip(counts)
                .filter(|(_, c)| *c > 0)
                .map(|(name, count)| format!("{count} {name}"))
                .collect::<Vec<_>>()
                .join(" · ")
        )
    });
    let description = description.filter(|d| d != &title);
    (title, description, summary)
}
