//! Presentation from TimelineV2EventRow.logic.ts, V2LifecycleRow.logic.ts and handoff.ts.

use std::rc::Rc;

use t3_contracts::model_selection::ModelSelection;
use t3_contracts::orchestration_v2::{
    OrchestrationV2ModelSwitch, OrchestrationV2NotificationOutcome as Outcome,
    OrchestrationV2NotificationSource as NotificationSource, OrchestrationV2Run,
    OrchestrationV2Subagent, OrchestrationV2TurnItemStatus as Status, TurnItemHandoff,
    TurnItemNotification,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_contracts::server::{ServerConfig, ServerProvider, ServerProviderModel};
use t3_contracts::wire::WireDateTime;

use crate::time::epoch_ms;

pub fn resolved_id<T: AsRef<str>>(id: &IdOrIndex<T>) -> Option<&str> {
    match id {
        IdOrIndex::Id(id) => Some(id.as_ref()),
        IdOrIndex::Index(_) => None,
    }
}

pub fn active(status: &Status) -> bool {
    matches!(status, Status::Pending | Status::Running | Status::Waiting)
}

pub fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Pending => "Queued",
        Status::Running => "Running",
        Status::Waiting => "Waiting",
        Status::Idle => "Idle · resumable",
        Status::Completed => "Completed",
        Status::Failed => "Failed",
        Status::Cancelled | Status::Interrupted => "Stopped",
    }
}

pub fn subagent_resumed_since(
    status: &Status,
    started_at: Option<&str>,
    notified_at: &str,
) -> bool {
    active(status)
        || started_at
            .and_then(epoch_ms)
            .zip(epoch_ms(notified_at))
            .is_some_and(|(start, notified)| start > notified)
}

pub struct NotificationEvent {
    pub status: Option<Status>,
    pub label: &'static str,
}

pub struct SubagentNotification {
    pub agent: Rc<OrchestrationV2Subagent>,
    pub child_thread_id: t3_contracts::base_schemas::ThreadId,
    pub event: Option<NotificationEvent>,
}

pub fn subagent_notification(
    notice: &TurnItemNotification,
    notified_at: &str,
    agents: &[Rc<OrchestrationV2Subagent>],
) -> Option<SubagentNotification> {
    let (child, tasks) = match &notice.source {
        NotificationSource::Member1(source) => (
            source.child_thread_id.as_ref().and_then(Option::as_ref),
            Some(source.task_ids.as_slice()),
        ),
        NotificationSource::Member2(source) => (
            source.child_thread_id.as_ref().and_then(Option::as_ref),
            None,
        ),
        NotificationSource::SubagentNotificationSource(source) => (
            source.child_thread_id.as_ref().and_then(Option::as_ref),
            None,
        ),
        _ => return None,
    };
    let child = child?;
    let agent = agents
        .iter()
        .find(|a| match tasks {
            Some(tasks) => {
                resolved_id(&a.id).is_some_and(|id| tasks.iter().any(|task| task.0 == id))
            }
            None => a.child_thread_id.as_ref().and_then(resolved_id) == Some(child.0.as_str()),
        })?
        .clone();
    let (status, label) = match notice.outcome {
        Outcome::Completed => (Some(Status::Completed), "Finished"),
        Outcome::Failed => (Some(Status::Failed), "Failed"),
        Outcome::Cancelled => (Some(Status::Cancelled), "Stopped"),
        Outcome::Updated => (None, "Updated"),
        Outcome::Unknown => (None, "Finished"),
    };
    let resumed = status.is_some()
        && subagent_resumed_since(
            &agent.status,
            agent.started_at.as_ref().map(|s| s.0.as_str()),
            notified_at,
        );
    Some(SubagentNotification {
        agent,
        child_thread_id: child.clone(),
        event: (!resumed).then_some(NotificationEvent { status, label }),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTiming {
    pub status: Status,
    pub started_at: Option<WireDateTime>,
    pub completed_at: Option<WireDateTime>,
}

pub fn subagent_group_timing(agents: &[AgentTiming]) -> AgentTiming {
    let started_at = agents
        .iter()
        .filter_map(|a| {
            a.started_at
                .as_ref()
                .and_then(|s| epoch_ms(&s.0).map(|ms| (ms, s)))
        })
        .min_by_key(|(ms, _)| *ms)
        .map(|(_, s)| s.clone());
    let live = agents.iter().any(|a| active(&a.status));
    let completed_at = (!live && agents.iter().all(|a| a.completed_at.is_some()))
        .then(|| {
            agents
                .iter()
                .filter_map(|a| {
                    a.completed_at
                        .as_ref()
                        .and_then(|s| epoch_ms(&s.0).map(|ms| (ms, s)))
                })
                .max_by_key(|(ms, _)| *ms)
                .map(|(_, s)| s.clone())
        })
        .flatten();
    AgentTiming {
        status: if live {
            Status::Running
        } else {
            Status::Completed
        },
        started_at,
        completed_at,
    }
}

/// AgentElapsed.tsx's noncompact label; the caller supplies the host-adjusted clock.
pub fn subagent_elapsed(agent: &AgentTiming, now_ms: i64) -> Option<String> {
    let start = epoch_ms(&agent.started_at.as_ref()?.0)?;
    let end = if active(&agent.status) {
        now_ms
    } else {
        epoch_ms(&agent.completed_at.as_ref()?.0)?
    };
    let seconds = end.saturating_sub(start).max(0) / 1000;
    Some(if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60)
    })
}

/// V2LifecycleRow's single-line preview; progress appends inspect only a bounded prefix.
pub fn subagent_timeline_detail(
    status: &Status,
    result: Option<&str>,
    progress: Option<&str>,
) -> Option<String> {
    let (first, second) = if super::threads::subagent_display::terminal_status(status) {
        (result, progress)
    } else {
        (progress, result)
    };
    fn prefix(s: &str) -> &str {
        let end = s.char_indices().nth(8192).map_or(s.len(), |(at, _)| at);
        t3_contracts::wire::trim_js(&s[..end])
    }
    let first = first.map(prefix);
    let second = second.map(prefix);
    let raw = first
        .filter(|s| !t3_contracts::wire::trim_js(s).is_empty())
        .or_else(|| second.filter(|s| !t3_contracts::wire::trim_js(s).is_empty()))?;
    let raw = t3_contracts::wire::trim_js(raw);
    if raw
        .get(..28)
        .is_some_and(|s| s.eq_ignore_ascii_case("Child task ended with status"))
        && raw
            .get(28..)
            .and_then(|s| s.chars().next())
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    {
        return None;
    }
    let plain = plain_detail(raw);
    (!plain.is_empty()).then_some(plain)
}

fn plain_detail(source: &str) -> String {
    let mut links = String::new();
    let mut rest = source;
    while let Some(at) = rest.find('[') {
        links.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(close) = rest.find(']') else {
            links.push_str(rest);
            rest = "";
            break;
        };
        if close > 1 && rest[close + 1..].starts_with('(') {
            let Some(end) = rest[close + 2..].find(')') else {
                links.push_str(rest);
                rest = "";
                break;
            };
            links.push_str(&rest[1..close]);
            rest = &rest[close + 2 + end + 1..];
        } else {
            links.push_str(&rest[..close + 1]);
            rest = &rest[close + 1..];
        }
    }
    links.push_str(rest);
    links
        .replace('`', "")
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .map(|line| {
            let line = line.trim_start_matches([' ', '\t']);
            line.strip_prefix(['-', '*'])
                .filter(|s| s.starts_with([' ', '\t']))
                .map_or(line, |s| s.trim_start_matches([' ', '\t']))
        })
        .collect::<Vec<_>>()
        .join(" ")
        .split(crate::view_logic::is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandoffEndpoint {
    pub instance_id: ProviderInstanceId,
    pub model: Option<String>,
}

/// providerIconUtils.ts getTriggerDisplayModelName and provider-core/modelShortName.ts.
pub fn display_model_name(model: &ServerProviderModel, driver: Option<&str>) -> String {
    let name = &model.name.0;
    let short = model
        .short_name
        .as_ref()
        .and_then(Option::as_ref)
        .map(|s| s.0.as_str())
        .filter(|s| !s.is_empty());
    let label = short
        .map(str::to_owned)
        .unwrap_or_else(|| model_short_name(name, driver));
    let qualifier = model
        .sub_provider
        .as_ref()
        .and_then(Option::as_ref)
        .map(|s| s.0.trim())
        .filter(|s| !s.is_empty());
    let Some(qualifier) = qualifier else {
        return label;
    };
    let Some(prefix) = label.get(..qualifier.len()) else {
        return label;
    };
    if prefix.to_lowercase() != qualifier.to_lowercase() {
        return label;
    }
    let suffix = &label[qualifier.len()..];
    let trimmed = suffix.trim_start();
    let stripped = if trimmed.starts_with(['.', ':', '/', '-']) {
        trimmed[1..].trim()
    } else if suffix.starts_with(char::is_whitespace) {
        trimmed.trim()
    } else {
        return label;
    };
    if stripped.is_empty() {
        label
    } else {
        stripped.to_owned()
    }
}

fn model_short_name(name: &str, driver: Option<&str>) -> String {
    let native: &[&str] = match driver {
        Some("codex") => &["GPT-", "GPT "],
        Some("claudeAgent") => &["Claude "],
        Some("antigravity") => &["Gemini "],
        Some("cursor" | "grok") => &["Grok "],
        Some("muse") => &["Muse "],
        _ => &[],
    };
    let unprefixed = name
        .split_once(": ")
        .filter(|(vendor, rest)| {
            let bytes = vendor.as_bytes();
            bytes.first().is_some_and(u8::is_ascii_uppercase)
                && bytes.len() <= 25
                && bytes
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.&- ".contains(c))
                && !rest.starts_with(crate::view_logic::is_js_whitespace)
                && !rest.is_empty()
        })
        .map_or(name, |(_, rest)| rest);
    let family = ["Claude ", "GPT-", "GPT ", "Gemini ", "Muse ", "Grok "]
        .into_iter()
        .find(|family| unprefixed.starts_with(family));
    if let Some(family) = family {
        let rest = unprefixed[family.len()..].trim();
        let words = model_words(rest);
        if (native.contains(&family) && !words.is_empty())
            || (!native.contains(&family) && names_model(&words))
        {
            return if rest.contains(crate::view_logic::is_js_whitespace) {
                rest.to_owned()
            } else {
                rest.replace('-', " ")
            };
        }
    }
    if names_model(&model_words(unprefixed)) {
        unprefixed.into()
    } else {
        name.into()
    }
}

fn model_words(mut text: &str) -> Vec<String> {
    let mut clean = String::new();
    while let Some(start) = text.find('(') {
        let Some(end) = text[start..].find(')') else {
            break;
        };
        clean.push_str(&text[..start]);
        clean.push(' ');
        text = &text[start + end + 1..];
    }
    clean.push_str(text);
    clean
        .split(|c| c == '-' || crate::view_logic::is_js_whitespace(c))
        .filter(|s| !s.is_empty() && !model_version(s))
        .map(str::to_owned)
        .collect()
}

fn model_version(word: &str) -> bool {
    let word = word.strip_prefix(['v', 'V']).unwrap_or(word);
    let word = if word.ends_with(|c: char| c.is_ascii_alphabetic()) {
        &word[..word.len() - 1]
    } else {
        word
    };
    !word.is_empty()
        && word
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
}

fn names_model(words: &[String]) -> bool {
    const VARIANTS: &[&str] = &[
        "pro",
        "flash",
        "mini",
        "max",
        "lite",
        "nano",
        "ultra",
        "plus",
        "fast",
        "turbo",
        "preview",
        "latest",
        "thinking",
        "reasoning",
        "build",
        "code",
        "image",
        "vision",
        "audio",
        "live",
        "custom",
        "tools",
        "exp",
        "experimental",
        "chat",
        "tts",
        "stt",
        "non",
        "transcribe",
    ];
    words
        .iter()
        .any(|word| !VARIANTS.contains(&word.to_lowercase().as_str()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandoffEndpoints {
    pub from: Vec<HandoffEndpoint>,
    pub to: HandoffEndpoint,
}

pub struct ProviderVisual {
    pub driver: String,
    pub display_name: String,
    pub initials: String,
    pub accent: Option<String>,
    pub show_badge: bool,
}

pub fn provider_visual(config: &ServerConfig, id: &str) -> Option<ProviderVisual> {
    use crate::services::providers::{
        InstanceBadge, normalize_provider_accent, provider_instance_display_name,
        provider_instance_initials, should_show_instance_badge,
    };
    let providers = &config.providers.0;
    let provider = providers.iter().find(|p| p.instance_id.0 == id)?;
    let settings = |p: &ServerProvider| config.settings.provider_instances.get(&p.instance_id);
    fn accent<'a>(config: &'a ServerConfig, p: &'a ServerProvider) -> Option<&'a str> {
        config
            .settings
            .provider_instances
            .get(&p.instance_id)
            .and_then(|s| s.accent_color.as_ref().and_then(Option::as_deref))
            .or_else(|| {
                p.accent_color
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.0.as_str())
            })
    }
    let badges: Vec<_> = providers
        .iter()
        .map(|p| InstanceBadge {
            driver: &p.driver.0,
            accent: accent(config, p),
            enabled: p.enabled,
            acp_agent: settings(p).and_then(|s| s.config.as_ref()).and_then(|c| {
                (c["source"] != "local")
                    .then(|| c["agentId"].as_str())
                    .flatten()
            }),
        })
        .collect();
    let badge = providers
        .iter()
        .position(|p| p.instance_id == provider.instance_id)?;
    let display_name = settings(provider)
        .and_then(|s| s.display_name.as_ref().and_then(Option::as_deref))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| provider_instance_display_name(provider));
    Some(ProviderVisual {
        driver: provider.driver.0.clone(),
        initials: provider_instance_initials(&display_name),
        display_name,
        accent: normalize_provider_accent(accent(config, provider)).map(str::to_owned),
        show_badge: should_show_instance_badge(&badges[badge], &badges),
    })
}

pub fn resolve_handoff_endpoints(
    item: &TurnItemHandoff,
    runs: &[Rc<OrchestrationV2Run>],
) -> Option<HandoffEndpoints> {
    let target = item
        .run_id
        .as_ref()
        .and_then(|id| runs.iter().find(|r| r.id == *id));
    let instance_id = ProviderInstanceId(resolved_id(&item.to_provider_instance_id)?.into());
    let model = item.to_model.as_ref().and_then(|s| s.clone()).or_else(|| {
        target
            .filter(|r| r.provider_instance_id == item.to_provider_instance_id)
            .map(|r| r.model_selection.model.clone())
    });
    let from = match item
        .from_model_selections
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|s| !s.is_empty())
    {
        Some(stamped) => stamped
            .iter()
            .map(|s| HandoffEndpoint {
                instance_id: s.instance_id.clone(),
                model: Some(s.model.clone()),
            })
            .collect(),
        None => item
            .from_provider_instance_ids
            .iter()
            .map(|id| {
                let instance_id = ProviderInstanceId(resolved_id(id)?.into());
                let model = runs
                    .iter()
                    .filter(|r| {
                        r.provider_instance_id == *id
                            && target.is_none_or(|t| r.ordinal < t.ordinal)
                    })
                    .fold(None, |latest: Option<&Rc<OrchestrationV2Run>>, run| {
                        Some(
                            latest
                                .filter(|latest| latest.ordinal >= run.ordinal)
                                .unwrap_or(run),
                        )
                    })
                    .map(|r| r.model_selection.model.clone());
                Some(HandoffEndpoint { instance_id, model })
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some(HandoffEndpoints {
        from,
        to: HandoffEndpoint { instance_id, model },
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelSwitchChange {
    pub model_changed: bool,
    pub effort: Option<(String, String)>,
}

pub fn resolve_model_switch_change(
    change: &OrchestrationV2ModelSwitch,
    providers: &[ServerProvider],
) -> Option<ModelSwitchChange> {
    let from = resolve_effort(&change.r#from, providers);
    let to = resolve_effort(&change.to, providers);
    let effort = from
        .zip(to)
        .filter(|(a, b)| a.0 != b.0)
        .map(|(a, b)| (a.1, b.1));
    let model_changed = change.r#from.model != change.to.model;
    (model_changed || effort.is_some()).then_some(ModelSwitchChange {
        model_changed,
        effort,
    })
}

fn resolve_effort(
    selection: &ModelSelection,
    providers: &[ServerProvider],
) -> Option<(String, String)> {
    let options = serde_json::to_value(&selection.options).ok()?;
    let stated = |id: &str| {
        options.as_array()?.iter().find(|o| o["id"] == id)?["value"]
            .as_str()
            .map(str::to_owned)
    };
    let model = providers
        .iter()
        .find(|p| p.instance_id == selection.instance_id)
        .and_then(|p| p.models.iter().find(|m| m.slug.0 == selection.model));
    let Some(model) = model else {
        return [
            "reasoningEffort",
            "effort",
            "reasoning",
            "variant",
            "reasoning_effort",
            "thinking_level",
            "thinking",
        ]
        .into_iter()
        .find_map(stated)
        .map(|s| (s.clone(), s));
    };
    let model = serde_json::to_value(model).ok()?;
    let descriptor = model["capabilities"]["optionDescriptors"]
        .as_array()?
        .iter()
        .find(|d| {
            d["type"] == "select"
                && (d["role"] == "reasoning"
                    || matches!(
                        d["id"].as_str(),
                        Some(
                            "reasoningEffort"
                                | "effort"
                                | "reasoning"
                                | "variant"
                                | "reasoning_effort"
                                | "thinking_level"
                                | "thinking"
                        )
                    ))
        })?;
    let options = descriptor["options"].as_array()?;
    let value = stated(descriptor["id"].as_str()?).or_else(|| {
        if descriptor["id"] == "variant" {
            return None;
        }
        if let Some(current) = descriptor["currentValue"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            return Some(current.to_owned());
        }
        options.iter().find(|o| o["isDefault"] == true)?["id"]
            .as_str()
            .map(str::to_owned)
    })?;
    let label = options
        .iter()
        .find(|o| o["id"] == value)
        .and_then(|o| o["label"].as_str())
        .unwrap_or(&value)
        .to_owned();
    Some((value, label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const BEFORE: &str = "2026-10-09T12:05:00.000Z";
    const NOTICE: &str = "2026-10-09T12:21:00.000Z";
    const AFTER: &str = "2026-10-09T12:48:00.000Z";

    fn agent(status: Status, start: Option<&str>, end: Option<&str>) -> AgentTiming {
        AgentTiming {
            status,
            started_at: start.map(|s| WireDateTime(s.into())),
            completed_at: end.map(|s| WireDateTime(s.into())),
        }
    }

    #[test]
    fn resumed_notifications_follow_all_three_typescript_cases_and_invalid_dates() {
        for (status, start, notified, expected) in [
            (
                Status::Running,
                Some(AFTER),
                "2026-10-09T12:50:00.000Z",
                true,
            ),
            (Status::Completed, Some(AFTER), NOTICE, true),
            (Status::Failed, Some(BEFORE), NOTICE, false),
            (Status::Idle, None, NOTICE, false),
            (Status::Completed, Some(AFTER), "invalid", false),
            (Status::Completed, Some(NOTICE), NOTICE, false),
        ] {
            assert_eq!(subagent_resumed_since(&status, start, notified), expected);
        }
    }

    #[test]
    fn group_timing_uses_earliest_start_latest_end_and_unknown_end() {
        let a = agent(Status::Completed, Some(BEFORE), Some(NOTICE));
        let b = agent(Status::Failed, Some(NOTICE), Some(AFTER));
        assert_eq!(
            subagent_group_timing(&[b.clone(), a.clone()]),
            agent(Status::Completed, Some(BEFORE), Some(AFTER))
        );
        for unfinished in [
            agent(Status::Running, Some(NOTICE), Some(AFTER)),
            agent(Status::Idle, Some(NOTICE), None),
        ] {
            assert!(
                subagent_group_timing(&[a.clone(), unfinished])
                    .completed_at
                    .is_none()
            );
        }
        assert_eq!(
            subagent_group_timing(&[]),
            agent(Status::Completed, None, None)
        );
        assert_eq!(subagent_elapsed(&a, 0).as_deref(), Some("16m 00s"));
        assert_eq!(
            subagent_elapsed(
                &agent(Status::Running, Some(BEFORE), None),
                epoch_ms(AFTER).expect("date")
            )
            .as_deref(),
            Some("43m 00s")
        );
        assert_eq!(
            subagent_elapsed(&agent(Status::Idle, Some(BEFORE), None), 0),
            None
        );
    }

    #[test]
    fn notifications_select_the_reported_task_in_a_reused_child_thread() {
        let agent = |id: &str, status: &str, start: &str| {
            Rc::new(serde_json::from_value(json!({
            "id":id,"threadId":"thread","runId":null,"parentNodeId":"root","origin":"app_owned","createdBy":"agent",
            "driver":"codex","providerInstanceId":"codex","providerThreadId":null,"childThreadId":"child","nativeTaskRef":null,
            "prompt":"Review","title":null,"model":null,"status":status,"result":null,"startedAt":start,"completedAt":AFTER,"updatedAt":AFTER
        })).expect("canonical subagent"))
        };
        let agents = vec![
            agent("old", "failed", BEFORE),
            agent("new", "completed", AFTER),
        ];
        let mut notice = serde_json::to_value(handoff(json!({}))).expect("common item fields");
        notice["source"] =
            json!({"kind":"delegated_task","taskIds":["new"],"childThreadId":"child"});
        notice["outcome"] = json!("failed");
        notice["summary"] = json!("Failed");
        let notice = serde_json::from_value(notice).expect("notification");
        let selected = subagent_notification(&notice, NOTICE, &agents).expect("reported task");
        assert!(Rc::ptr_eq(&selected.agent, &agents[1]));
        assert!(
            selected.event.is_none(),
            "a newer settled activation supersedes the old failure"
        );
        assert_eq!(selected.child_thread_id.0, "child");
        assert!(
            subagent_notification(&notice, NOTICE, &agents[..1]).is_none(),
            "never substitute another task in the same child"
        );
    }

    #[test]
    fn detail_priority_strips_only_the_sources_plain_markup_and_generic_end() {
        assert_eq!(
            subagent_timeline_detail(&Status::Completed, Some("a\r- b\u{2028}* c"), None),
            Some("a b c".into())
        );
        assert_eq!(
            subagent_timeline_detail(
                &Status::Running,
                Some("result"),
                Some("- `progress` [file](/repo)\n* checked")
            ),
            Some("progress file checked".into())
        );
        assert_eq!(
            subagent_timeline_detail(&Status::Completed, Some("result"), Some("progress")),
            Some("result".into())
        );
        assert_eq!(
            subagent_timeline_detail(&Status::Idle, Some("result"), Some("progress")),
            Some("progress".into())
        );
        assert_eq!(
            subagent_timeline_detail(
                &Status::Failed,
                Some("child task ended with status failed"),
                Some("progress")
            ),
            None
        );
        assert_eq!(
            subagent_timeline_detail(
                &Status::Failed,
                Some("Child task ended with statuses"),
                None
            ),
            Some("Child task ended with statuses".into())
        );
        let huge = "progress ".repeat(50_000);
        let detail =
            subagent_timeline_detail(&Status::Running, None, Some(&huge)).expect("bounded detail");
        assert!(detail.len() <= 8192);
    }

    fn handoff(extra: Value) -> TurnItemHandoff {
        let mut value = json!({"id":"handoff","threadId":"thread","runId":"target","nodeId":null,
            "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":1,"status":"completed","title":null,"startedAt":null,"completedAt":null,
            "updatedAt":NOTICE,"contextHandoffId":"context","fromProviderThreadIds":[],
            "toProviderThreadId":"to-thread","fromProviderInstanceIds":["from"],"toProviderInstanceId":"to","strategy":"full_thread_summary"});
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("extra").clone());
        serde_json::from_value(value).expect("canonical handoff")
    }

    fn run(id: &str, ordinal: u64, provider: &str, model: &str) -> Rc<OrchestrationV2Run> {
        Rc::new(serde_json::from_value(json!({"id":id,"threadId":"thread","ordinal":ordinal,
            "providerInstanceId":provider,"modelSelection":{"instanceId":provider,"model":model},
            "providerThreadId":null,"userMessageId":"prompt","rootNodeId":null,"activeAttemptId":null,
            "status":"completed","requestedAt":BEFORE,"startedAt":BEFORE,"completedAt":NOTICE,
            "checkpointId":null,"contextHandoffId":null})).expect("canonical run"))
    }

    #[test]
    fn handoffs_preserve_stamped_models_and_recover_legacy_models_in_unsorted_history() {
        let runs = vec![
            run("later", 4, "from", "wrong"),
            run("old", 1, "from", "old"),
            run("target", 3, "to", "destination"),
            run("source", 2, "from", "source"),
        ];
        let endpoints = resolve_handoff_endpoints(&handoff(json!({})), &runs).expect("resolved");
        assert_eq!(endpoints.from[0].model.as_deref(), Some("source"));
        assert_eq!(endpoints.to.model.as_deref(), Some("destination"));
        let item = handoff(
            json!({"fromModelSelections":[{"instanceId":"from","model":"a"},{"instanceId":"from","model":"b"}],"toModel":"stamped"}),
        );
        let endpoints = resolve_handoff_endpoints(&item, &runs).expect("resolved");
        assert_eq!(
            endpoints
                .from
                .iter()
                .map(|s| s.model.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("a"), Some("b")]
        );
        assert_eq!(endpoints.to.model.as_deref(), Some("stamped"));
    }

    #[test]
    fn handoffs_keep_unknown_models_and_reject_unresolved_wire_indices() {
        let item = handoff(json!({}));
        let endpoints = resolve_handoff_endpoints(&item, &[]).expect("provider identities");
        assert_eq!(endpoints.from[0].instance_id.0, "from");
        assert!(endpoints.from[0].model.is_none());
        assert!(endpoints.to.model.is_none());
        let wrong = vec![run("target", 2, "from", "wrong")];
        assert!(
            resolve_handoff_endpoints(&item, &wrong)
                .expect("resolved")
                .to
                .model
                .is_none()
        );
        let item = handoff(json!({"toProviderInstanceId":42}));
        assert!(resolve_handoff_endpoints(&item, &[]).is_none());
    }

    fn selection(model: &str, options: Value) -> ModelSelection {
        serde_json::from_value(json!({"instanceId":"from","model":model,"options":options}))
            .expect("selection")
    }

    fn provider(mut models: Value) -> ServerProvider {
        for model in models.as_array_mut().expect("models") {
            model["isCustom"] = json!(false);
            if model.get("capabilities").is_none() {
                model["capabilities"] = Value::Null;
            }
        }
        serde_json::from_value(
            json!({"instanceId":"from","driver":"codex","enabled":true,"installed":true,
            "version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":NOTICE,
            "models":models,"slashCommands":[],"skills":[]}),
        )
        .expect("provider")
    }

    #[test]
    fn model_switches_read_catalog_default_and_ignore_invisible_changes() {
        let providers = vec![provider(
            json!([{"slug":"gpt","name":"GPT","capabilities":{"optionDescriptors":[
            {"id":"serviceTier","type":"select","label":"Speed","options":[{"id":"fast","label":"Fast"}]},
            {"id":"thought_level","type":"select","role":"reasoning","label":"Effort","options":[{"id":"medium","label":"Medium","isDefault":true},{"id":"high","label":"High"}]}]}}]),
        )];
        let base = selection("gpt", json!([]));
        let change = |to| OrchestrationV2ModelSwitch {
            r#from: base.clone(),
            to,
        };
        assert_eq!(
            resolve_model_switch_change(
                &change(selection(
                    "gpt",
                    json!([{"id":"thought_level","value":"high"}])
                )),
                &providers
            ),
            Some(ModelSwitchChange {
                model_changed: false,
                effort: Some(("Medium".into(), "High".into()))
            })
        );
        for options in [
            json!([]),
            json!([{"id":"thought_level","value":"medium"}]),
            json!([{"id":"serviceTier","value":"fast"}]),
        ] {
            assert!(
                resolve_model_switch_change(&change(selection("gpt", options)), &providers)
                    .is_none()
            );
        }
        assert_eq!(
            resolve_model_switch_change(&change(selection("mini", json!([]))), &providers),
            Some(ModelSwitchChange {
                model_changed: true,
                effort: None
            })
        );
    }

    #[test]
    fn model_switches_use_stated_efforts_only_without_a_catalog() {
        for id in [
            "reasoningEffort",
            "thinking",
            "reasoning",
            "variant",
            "reasoning_effort",
            "thinking_level",
        ] {
            let change = OrchestrationV2ModelSwitch {
                r#from: selection("gpt", json!([{"id":id,"value":"low"}])),
                to: selection("gpt", json!([{"id":id,"value":"high"}])),
            };
            assert_eq!(
                resolve_model_switch_change(&change, &[])
                    .expect("stated effort")
                    .effort,
                Some(("low".into(), "high".into()))
            );
            assert!(
                resolve_model_switch_change(
                    &change,
                    &[provider(json!([{"slug":"gpt","name":"GPT"}]))]
                )
                .is_none()
            );
        }
    }

    #[test]
    fn trigger_names_port_all_eight_model_short_name_cases_and_qualifiers() {
        for (name, driver, expected) in [
            ("Claude Opus 5.5", Some("claudeAgent"), "Opus 5.5"),
            ("GPT-6.1-Sol", Some("codex"), "6.1 Sol"),
            ("GPT 6.0 Astra", Some("codex"), "6.0 Astra"),
            ("Grok 4 Pro", None, "Grok 4 Pro"),
            ("Claude Opus 5.5", None, "Opus 5.5"),
            ("NVIDIA: Nemotron 3 Super", None, "Nemotron 3 Super"),
            ("GPT-5.5", Some("codex"), "GPT-5.5"),
            ("Gemini 3.1 Pro", Some("antigravity"), "3.1 Pro"),
        ] {
            assert_eq!(model_short_name(name, driver), expected);
        }
        let model = serde_json::from_value(json!({"slug":"slug","name":"Name","shortName":"Vendor: Custom","subProvider":"Vendor","isCustom":false,"capabilities":null})).expect("model");
        assert_eq!(display_model_name(&model, None), "Custom");
    }

    #[test]
    fn account_visuals_resolve_settings_names_accents_and_multiple_instances() {
        let mut config: ServerConfig = serde_json::from_str(
            include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
                .lines()
                .next()
                .expect("config sample"),
        )
        .expect("config");
        config.settings = t3_contracts::settings::ServerSettings::default();
        config.providers.0 = vec![provider(json!([]))];
        assert!(
            !provider_visual(&config, "from")
                .expect("known account")
                .show_badge
        );
        let mut another = provider(json!([]));
        another.instance_id.0 = "other".into();
        config.providers.0.push(another);
        assert!(
            provider_visual(&config, "from")
                .expect("multiple accounts")
                .show_badge
        );
        config.providers.0.truncate(1);
        config.settings.provider_instances.insert(
            ProviderInstanceId("from".into()),
            serde_json::from_value(
                json!({"driver":"codex","displayName":"Work account","accentColor":"#123456"}),
            )
            .expect("settings override"),
        );
        let visual = provider_visual(&config, "from").expect("settings visual");
        assert_eq!(visual.display_name, "Work account");
        assert_eq!(visual.initials, "WA");
        assert_eq!(visual.accent.as_deref(), Some("#123456"));
        assert!(visual.show_badge);
        assert!(provider_visual(&config, "missing").is_none());
    }
}
