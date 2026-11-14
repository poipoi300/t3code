use super::*;
use crate::{view_logic::JsTrim, work_log::*};

fn expanded_row(id: &str, at: &str, entries: &[&WorkLogEntry]) -> Value {
    json!({"kind":"work","id":format!("{id}:details"),"createdAt":at,"groupedEntries":entries,"isExpandedToolGroup":true})
}
fn images(rows: &mut Vec<Value>, id: &str, at: &str, entries: &[&WorkLogEntry]) {
    let paths =
        work_entries_viewed_image_paths(&entries.iter().map(|e| (*e).clone()).collect::<Vec<_>>());
    if !paths.is_empty() {
        rows.push(json!({"kind":"viewed-images","id":format!("{id}:viewed-images"),"createdAt":at,"paths":paths}));
    }
}
fn is_active(e: &Entry, input: &MessagesTimelineRowsInput, run: Option<&str>) -> bool {
    input.is_working
        && e.w()["toolLifecycleStatus"] == "inProgress"
        && optional_id(&e.w()["runId"])
            .map_or(input.runless_work_active, |r| Some(r.as_str()) == run)
}
fn active_activity(w: &WorkLogEntry) -> bool {
    w.tool_lifecycle_status.as_deref() == Some("inProgress")
        || (w.tool_lifecycle_status.is_none()
            && (w.source_activity_kind.as_deref() == Some("task.progress")
                || session::work_log_entry_is_tool_like(w)))
}
fn exceptional(e: &Entry, input: &MessagesTimelineRowsInput) -> bool {
    special_work(e, input)
        || (e.w()["itemType"] == "error" && e.w()["toolLifecycleStatus"] == "failed")
}
fn special_work(e: &Entry, input: &MessagesTimelineRowsInput) -> bool {
    helpers::pending(e, input)
        || e.w()["tone"] == "error"
        || e.w()["sourceActivityKind"] == "runtime.error"
        || matches!(
            s(&e.w()["itemType"]),
            "system_notice" | "notification" | "todo_list"
        )
        || e.w().get("questionAnswer").is_some()
}
fn plain_work(e: &Entry) -> Value {
    json!({"kind":"work","id":e.id(),"createdAt":e.at(),"groupedEntries":[e.w()],"isExpandedToolGroup":false})
}
fn idle_reason(message: &Value) -> Option<&str> {
    message["idleCompaction"].as_str()
}
fn compact_command(message: &Value) -> bool {
    message["role"] == "user"
        && s(&message["text"])
            .js_trim()
            .eq_ignore_ascii_case("/compact")
        && message["attachments"]
            .as_array()
            .is_none_or(|a| a.is_empty())
}
fn idle_label(reason: &str) -> &str {
    match reason {
        "onReturn" => "Compacting…",
        "beforeCacheExpires" => "Compacting before the cache expires…",
        _ => "Compacting context",
    }
}
fn idle_title(reason: &str) -> Option<&str> {
    match reason {
        "onReturn" => Some("Context compacted before sending"),
        "beforeCacheExpires" => Some("Context automatically compacted before cache expiry"),
        _ => None,
    }
}
fn resume_age(answer: &Value) -> Option<String> {
    let answers = answer["answers"].as_object()?;
    for (id, chosen) in answers {
        if chosen != "Compact and continue"
            && !chosen
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a == "Compact and continue"))
        {
            continue;
        }
        let text = answer["questionTextById"][id].as_str().unwrap_or(id);
        let Some(rest) = text.strip_prefix("This session is ") else {
            continue;
        };
        let Some((age, rest)) = rest.split_once(" old and uses ") else {
            continue;
        };
        let Some(tokens) = rest.strip_suffix(" tokens. Compact it before continuing?") else {
            continue;
        };
        let number = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
        let age_valid = age.strip_suffix('m').is_some_and(|age| {
            age.split_once("h ")
                .map_or_else(|| number(age), |(h, m)| number(h) && number(m))
        });
        let mut groups = tokens.split(',');
        let first = groups.next().unwrap_or_default();
        let tokens_valid = (1..=3).contains(&first.len())
            && number(first)
            && groups.all(|g| g.len() == 3 && number(g));
        if age_valid && tokens_valid {
            return Some(age.into());
        }
    }
    None
}
fn format_tokens(n: f64) -> String {
    let rounded = (n + 0.5).floor();
    if rounded.abs() < 1000. {
        return format!("{rounded:.0}");
    }
    let trim = |n: f64| {
        let p = if n.abs() < 10. {
            2
        } else if n.abs() < 100. {
            1
        } else {
            0
        };
        let text = format!("{n:.p$}");
        if text
            .split_once('.')
            .is_some_and(|(_, f)| f.bytes().all(|b| b == b'0'))
        {
            text.split('.').next().unwrap_or_default().to_owned()
        } else {
            text
        }
    };
    let mut scaled = n / 1000.;
    let mut unit = 0;
    while trim(scaled).parse::<f64>().unwrap_or_default().abs() >= 1000. && unit < 3 {
        scaled /= 1000.;
        unit += 1;
    }
    format!("{}{}", trim(scaled), ["K", "M", "B", "T"][unit])
}
fn token_counts(item: &Value) -> Value {
    if item["type"] != "compaction" || item["status"] == "running" {
        return Value::Null;
    }
    item["beforeTokenCount"]
        .as_f64()
        .zip(item["afterTokenCount"].as_f64())
        .map_or(
            Value::Null,
            |(a, b)| json!({"before":format_tokens(a),"after":format_tokens(b)}),
        )
}
fn deferred(e: &Entry) -> Option<String> {
    (e.kind() == "work" && !e.item()["deferredWork"].is_null())
        .then(|| optional_id(&e.projected()["sourceItemId"]))
        .flatten()
}

pub(super) fn derive(input: &MessagesTimelineRowsInput, raw: &[Entry]) -> Vec<Value> {
    let entries = helpers::shown_entries(input, raw);
    let embedded: BTreeSet<_> = entries
        .iter()
        .filter(|e| matches!(e.kind(), "html-render" | "mcp-app"))
        .filter_map(entry_run_id)
        .collect();
    let summaries: BTreeMap<_, _> = input
        .turn_diff_summaries
        .iter()
        .filter_map(|s| optional_id(&s["assistantMessageId"]).map(|id| (id, s)))
        .collect();
    let run = unsettled(input);
    let revert = if input.supports_conversation_rollback {
        let checkpoints: Vec<_> = input
            .turn_diff_summaries
            .iter()
            .filter_map(|s| serde_json::from_value::<session::CheckpointSummary>(s.clone()).ok())
            .collect();
        let run = run
            .as_ref()
            .and_then(|r| serde_json::from_value(json!(r)).ok());
        session::derive_revert_turn_count_by_user_message_id(
            &input.timeline_entries,
            &checkpoints,
            input.thread_start_checkpoint_available,
            input.available_revert_turn_counts.as_ref(),
            run.as_ref(),
        )
    } else {
        BTreeMap::new()
    };
    let terminal = folds::terminals(&entries);
    let failed = folds::failed_runs(&entries, input);
    let itemized = folds::itemized_runs(&entries, &failed);
    let live = folds::live_cards(&entries);
    let retained = folds::retained_replies(&entries);
    let superseded = folds::superseded_folds(&entries, &failed, &live, &retained);
    let superseded_items: BTreeSet<_> = superseded
        .iter()
        .flat_map(|f| f.deferred.iter().cloned())
        .collect();
    let active = active_runs(&entries, input);
    let run_active = |id: Option<String>| {
        id.map_or(input.is_working && input.runless_work_active, |r| {
            active.contains(&r)
        })
    };
    let folds = folds::turn_folds(&entries, input);
    let mut collapsed = BTreeSet::new();
    let mut collapsed_members = BTreeSet::new();
    let mut collapsed_superseded = BTreeSet::new();
    for f in &folds {
        if !input.expanded_run_ids.contains(&f.run_id) {
            collapsed.extend(f.hidden.iter().cloned());
            collapsed_members.extend(f.members.iter().cloned());
        }
    }
    for f in &superseded {
        if !f
            .attempt_id
            .as_ref()
            .is_some_and(|id| input.expanded_attempt_ids.contains(id))
        {
            collapsed_superseded.extend(f.hidden.iter().cloned());
        }
    }
    for e in entries.iter().filter(|e| helpers::pending(e, input)) {
        collapsed.remove(e.id());
        collapsed_superseded.remove(e.id());
    }
    let header = if input.is_working {
        boundary(&entries).map_or(0, |i| i + 1)
    } else {
        entries.len()
    };
    let folds_by_anchor: BTreeMap<_, _> = folds.iter().map(|f| (f.anchor.as_str(), f)).collect();
    let superseded_by_anchor: BTreeMap<_, _> =
        superseded.iter().map(|f| (f.anchor.as_str(), f)).collect();
    let entries_by_id: BTreeMap<_, _> = entries.iter().map(|e| (e.id(), e)).collect();
    let anchored = |e: &Entry| {
        folds_by_anchor.contains_key(e.id()) || superseded_by_anchor.contains_key(e.id())
    };
    let mut active_tools = vec![];
    if input.is_working && (run.is_some() || input.runless_work_active) {
        let mut tail_attempt = None;
        for e in entries.iter().skip(header).rev() {
            if e.kind() != "work"
                || special_work(e, input)
                || !run_active(entry_run_id(e))
                || e.compaction()
                || collapsed.contains(e.id())
                || collapsed_superseded.contains(e.id())
                || anchored(e)
            {
                break;
            }
            let attempt = optional_id(&e.attempt()["id"]);
            if let Some(previous) = &tail_attempt {
                if previous != &attempt {
                    break;
                }
            } else {
                tail_attempt = Some(attempt);
            }
            active_tools.push(e);
        }
    }
    active_tools.reverse();
    let visible_tools: Vec<_> = active_tools
        .iter()
        .copied()
        .filter(|e| {
            e.work
                .as_ref()
                .is_some_and(|w| helpers::work_entry_is_visible_in_group(w, true))
        })
        .collect();
    let anchor = active_tools.first().copied();
    let latest = visible_tools.last().copied();
    let latest_running = visible_tools
        .iter()
        .rev()
        .copied()
        .find(|e| e.work.as_ref().is_some_and(active_activity));
    let keeps_live = latest_running.is_some()
        || (latest.is_some_and(|e| {
            e.work
                .as_ref()
                .is_some_and(session::work_entry_indicates_tool_success)
        }) && input.pending_approval_request_ids.is_empty());
    let latest_failed = latest_running.is_none()
        && latest.is_some_and(|e| {
            e.w()["toolLifecycleStatus"] != "declined"
                && e.work
                    .as_ref()
                    .is_some_and(session::work_entry_display_indicates_tool_failure)
        });
    let thought =
        visible_tools.iter().rev().copied().find(|e| {
            e.w()["itemType"] == "reasoning" && !s(&e.w()["detail"]).js_trim().is_empty()
        });
    let active_row=anchor.zip(latest).filter(|_|!latest_failed).map(|(anchor,latest)|{
        let id=group_id(anchor.id());
        let mut row=json!({"kind":"work-live","id":if keeps_live{"live-activity-row".into()}else{format!("work-live:{}",anchor.id())},"createdAt":anchor.at(),"entry":latest_running.unwrap_or(latest).w(),"groupedEntries":visible_tools.iter().map(|e|e.w()).collect::<Vec<_>>(),"groupId":id,"expanded":input.expanded_work_group_ids.contains(&id),"active":keeps_live});
        if let Some(thought)=thought{row["thought"]=thought.w().clone();}row
    });
    let active_ids: BTreeSet<_> = if active_row.is_some() || latest_failed {
        active_tools.iter().map(|e| e.id()).collect()
    } else {
        BTreeSet::new()
    };
    let placement = latest.map(|e| e.id());
    let mut compaction_runs = BTreeMap::new();
    let mut compacted = BTreeSet::new();
    let mut pending_age = BTreeMap::new();
    let mut ages = BTreeMap::new();
    for e in &entries {
        if e.kind() == "message"
            && compact_command(e.message())
            && let Some(run) = optional_id(&e.message()["runId"])
        {
            compaction_runs.insert(run, idle_reason(e.message()).map(str::to_owned));
        } else if e.kind() == "work"
            && let Some(run) = entry_run_id(e)
        {
            if e.compaction() {
                compacted.insert(run.clone());
                if let Some(age) = pending_age.remove(&run) {
                    ages.insert(e.id().to_owned(), age);
                }
            }
            if let Some(age) = resume_age(&e.w()["questionAnswer"]) {
                pending_age.insert(run, age);
            }
        }
    }
    let active_compaction = entries
        .iter()
        .find(|e| e.compaction() && is_active(e, input, run.as_deref()));
    let active_run_compaction = input.is_working
        && run
            .as_ref()
            .is_some_and(|r| compaction_runs.contains_key(r));
    let compaction_finished = active_run_compaction
        && run.as_ref().is_some_and(|r| compacted.contains(r))
        && active_compaction.is_none();
    let only_compaction_finished = compaction_finished
        && !entries.iter().any(|e| {
            entry_run_id(e) == run
                && !e.compaction()
                && !(e.kind() == "message" && compact_command(e.message()))
        });
    let running_compaction = run
        .as_ref()
        .filter(|_| active_run_compaction && !compaction_finished);
    let on_return = running_compaction
        .is_some_and(|r| compaction_runs.get(r).and_then(Option::as_deref) == Some("onReturn"));
    let resume =
        active_compaction.filter(|e| entry_run_id(e).is_some() && ages.contains_key(e.id()));
    let active_compaction_run = if on_return {
        running_compaction.cloned()
    } else {
        resume.and_then(entry_run_id)
    };
    let has_active_compaction =
        active_compaction.is_some() || (active_run_compaction && !compaction_finished);
    let ended = input
        .latest_run
        .as_ref()
        .filter(|r| r.turn_ended == Some(true))
        .map(|r| &r.run_id);
    let zen_landed = input.tool_call_visibility == "zen"
        && ended.is_some_and(|r| {
            entries
                .iter()
                .rposition(|e| entry_run_id(e).as_ref() == Some(r))
                .is_some_and(|i| i >= header)
        });
    let working = || {
        if only_compaction_finished
            || zen_landed
            || (running_compaction.is_some() && active_compaction_run.is_none())
        {
            return None;
        }
        let at = if on_return {
            None
        } else {
            resume.map(|e| e.at().to_owned())
        }
        .or_else(|| input.active_turn_started_at.clone());
        let mut row = json!({"kind":"working","id":"working-indicator-row","createdAt":at});
        if active_compaction_run.is_some() {
            row["compacting"] = json!(true);
        }
        Some(row)
    };
    let mut rows = vec![];
    let mut has_activity = false;
    let mut index = 0;
    while index < entries.len() {
        let e = &entries[index];
        if input.is_working
            && index == header
            && let Some(row) = working()
        {
            rows.push(row);
        }
        if Some(e.id()) == placement
            && let Some(row) = &active_row
        {
            rows.push(row.clone());
            has_activity |= keeps_live;
            let work: Vec<_> = visible_tools
                .iter()
                .filter_map(|e| e.work.as_ref())
                .collect();
            let id = s(&row["groupId"]);
            if row["expanded"] == true {
                rows.push(expanded_row(id, s(&row["createdAt"]), &work));
            }
            images(&mut rows, id, s(&row["createdAt"]), &work);
        }
        index += 1;
        if e.kind() == "event"
            && (e.item()["type"] == "run_interrupt_request"
                || (e.item()["type"] == "run_interrupt_result"
                    && optional_id(&e.item()["runId"]).is_some_and(|r| {
                        compaction_runs.contains_key(&r) && compacted.contains(&r)
                    })))
        {
            continue;
        }
        if let Some(f) = folds_by_anchor.get(e.id()) {
            let deferred: Vec<_> = f
                .members
                .iter()
                .filter_map(|id| entries_by_id.get(id.as_str()).copied())
                .filter_map(deferred)
                .filter(|id| !superseded_items.contains(id))
                .collect();
            let mut row = json!({"kind":"turn-fold","id":format!("turn-fold:{}",f.run_id),"createdAt":f.at,"runId":f.run_id,"label":f.label,"expanded":input.expanded_run_ids.contains(&f.run_id)});
            if !deferred.is_empty() {
                row["deferredWorkItemIds"] = json!(deferred);
            }
            rows.push(row);
        }
        if collapsed.contains(e.id()) {
            continue;
        }
        if let Some(f) = superseded_by_anchor.get(e.id()) {
            let id = f.attempt_id.as_deref().unwrap_or_default();
            rows.push(json!({"kind":"attempt-fold","deferredWorkItemIds":f.deferred,"id":format!("attempt-fold:{id}"),"createdAt":f.at,"runId":f.run_id,"attemptId":id,"label":f.label,"expanded":input.expanded_attempt_ids.contains(id)}));
        }
        if collapsed_superseded.contains(e.id()) || active_ids.contains(e.id()) {
            continue;
        }
        if e.compaction() {
            let active = is_active(e, input, run.as_deref());
            let id = entry_run_id(e);
            if active && active_compaction_run.is_some() && id == active_compaction_run {
                continue;
            }
            let item = &e.w()["structuredPayload"];
            let reason = id
                .as_ref()
                .and_then(|id| compaction_runs.get(id))
                .and_then(Option::as_deref);
            let title = reason.and_then(idle_title).map(str::to_owned).or_else(|| {
                ages.get(e.id())
                    .map(|age| format!("Context compacted after {age} idle"))
            });
            let failed = item["type"] == "compaction" && item["status"] == "failed";
            let mut row = json!({"kind":"context-compaction","id":e.id(),"createdAt":e.at(),"label":if active && reason.is_some(){idle_label(reason.unwrap_or_default())}else{s(&e.w()["label"])},"tokenCounts":token_counts(item),"active":active});
            if let Some(title) = title {
                row["title"] = json!(title);
            }
            if failed {
                row["failed"] = json!(true);
                if let Some(failure) = item["summary"]
                    .as_str()
                    .map(str::js_trim)
                    .filter(|s| !s.is_empty())
                {
                    row["failure"] = json!(failure);
                }
            }
            rows.push(row);
            continue;
        }
        if e.kind() == "work"
            && entry_run_id(e).is_some_and(|r| compacted.contains(&r))
            && resume_age(&e.w()["questionAnswer"]).is_some()
        {
            continue;
        }
        if e.kind() == "work" {
            if exceptional(e, input) || entry_run_id(e).is_some_and(|r| itemized.contains(&r)) {
                rows.push(plain_work(e));
                continue;
            }
            let mut grouped = vec![e];
            while let Some(next) = entries.get(index) {
                if next.kind() != "work"
                    || next.compaction()
                    || exceptional(next, input)
                    || active_ids.contains(next.id())
                    || collapsed.contains(next.id())
                    || collapsed_superseded.contains(next.id())
                    || anchored(next)
                    || next.w()["runId"] != e.w()["runId"]
                    || next.attempt()["id"] != e.attempt()["id"]
                {
                    break;
                }
                grouped.push(next);
                index += 1;
            }
            let visible: Vec<_> = grouped
                .into_iter()
                .filter(|e| {
                    e.work.as_ref().is_some_and(|w| {
                        helpers::work_entry_is_visible_in_group(
                            w,
                            is_active(e, input, run.as_deref()),
                        )
                    })
                })
                .collect();
            if visible.is_empty() {
                continue;
            }
            let work: Vec<_> = visible.iter().filter_map(|e| e.work.as_ref()).collect();
            let owned = helpers::summary_entries(&work);
            let latest_active = visible
                .iter()
                .rev()
                .find(|e| is_active(e, input, run.as_deref()));
            let id = group_id(e.id());
            let expanded = input.expanded_work_group_ids.contains(&id);
            if let Some(latest) = latest_active {
                rows.push(json!({"kind":"work-live","id":format!("work-live:{}",e.id()),"createdAt":e.at(),"entry":latest.w(),"groupedEntries":work,"groupId":id,"expanded":expanded,"active":true}));
                has_activity = true;
                if expanded {
                    rows.push(expanded_row(&id, e.at(), &work));
                }
            } else if work.len() == 1 && session::work_log_entry_is_tool_like(work[0]) {
                let label = if helpers::tool_action(work[0]) == "edit" {
                    summarize_tool_group(&owned).summary
                } else {
                    helpers::single_tool_label(work[0])
                };
                rows.push(json!({"kind":"work","id":e.id(),"createdAt":e.at(),"groupedEntries":work,"isExpandedToolGroup":false,"displayLabel":label}));
            } else {
                let primary = work
                    .iter()
                    .find(|w| w.presentation.contains_key("toolSource"));
                let source = primary.and_then(|w| w.presentation.get("toolSource"));
                let key = source.and_then(|s| s["key"].as_str());
                let primary_icon = key
                    .and_then(|key| {
                        work.iter()
                            .find(|w| {
                                w.presentation
                                    .get("toolSource")
                                    .is_some_and(|s| s["key"] == key)
                                    && w.presentation.contains_key("toolIcon")
                            })
                            .and_then(|w| w.presentation.get("toolIcon"))
                    })
                    .or_else(|| source.and_then(|s| s.get("icon")));
                let surface = primary
                    .and_then(|w| w.presentation.get("toolSurface"))
                    .or_else(|| {
                        work.iter()
                            .rev()
                            .find_map(|w| w.presentation.get("toolSurface"))
                    });
                let icon = primary_icon.or_else(|| {
                    work.iter()
                        .rev()
                        .find_map(|w| w.presentation.get("toolIcon"))
                });
                let latest_tool = work
                    .iter()
                    .rev()
                    .find(|w| session::work_log_entry_is_tool_like(w));
                let summary = if work.len() == 1 && !session::work_log_entry_is_tool_like(work[0]) {
                    work[0].label.clone()
                } else {
                    summarize_tool_group(&owned).summary
                };
                let mut row = json!({"kind":"work-toggle","id":format!("work-toggle:{}",e.id()),"createdAt":e.at(),"runId":e.w()["runId"],"groupId":id,"hiddenCount":work.len(),"expanded":expanded,"summary":summary,"summaryKind":tool_group_summary_kind(&owned),"hasFailure":latest_tool.is_some_and(|w|session::work_entry_display_indicates_tool_failure(w))});
                if let Some(surface) = surface {
                    row["toolSurface"] = surface.clone();
                }
                if let Some(icon) = icon {
                    row["toolIcon"] = icon.clone();
                }
                rows.push(row);
                if expanded {
                    rows.push(expanded_row(&id, e.at(), &work));
                }
            }
            images(&mut rows, &id, e.at(), &work);
            continue;
        }
        if matches!(
            e.kind(),
            "proposed-plan" | "html-render" | "sent-message" | "mcp-app"
        ) {
            let mut row = e.value.clone();
            if let Some(obj) = row.as_object_mut() {
                for key in ["attempt", "runId", "beforeEntryId"] {
                    obj.remove(key);
                }
            }
            rows.push(row);
            continue;
        }
        if e.kind() == "event" {
            if let Some(previous) = rows.last_mut().filter(|p| {
                p["kind"] == "event"
                    && p["projectedItem"]["item"]["type"] == "subagent"
                    && e.item()["type"] == "subagent"
                    && p["projectedItem"]["item"]["runId"] == e.item()["runId"]
                    && p["projectedItem"]["item"]["providerTurnId"] == e.item()["providerTurnId"]
            }) {
                let mut subagents = previous["subagents"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![previous["projectedItem"].clone()]);
                subagents.push(e.projected().clone());
                previous["subagents"] = json!(subagents);
            } else {
                rows.push(json!({"kind":"event","id":e.id(),"createdAt":e.at(),"projectedItem":e.projected()}));
            }
            continue;
        }
        if e.kind() != "message" {
            continue;
        }
        let message = e.message();
        let id = optional_id(&message["runId"]);
        if compact_command(message)
            && let Some(id) = id
        {
            if !compacted.contains(&id) && active_compaction_run.as_ref() != Some(&id) {
                let pending = running_compaction == Some(&id);
                let reason = compaction_runs.get(&id).and_then(Option::as_deref);
                rows.push(json!({"kind":"context-compaction","id":format!("compaction:{id}"),"createdAt":e.at(),"label":if pending{reason.map_or("Compacting context",idle_label)}else{"Context compaction"},"tokenCounts":null,"active":pending}));
            }
            continue;
        }
        let assistant = message["role"] == "assistant";
        let in_progress = assistant && run_active(id.clone());
        let meta = assistant && terminal.contains(s(&message["id"])) && !in_progress;
        let mut row = json!({"kind":"message","id":e.id(),"createdAt":e.at(),"message":message,"showAssistantMeta":meta,"suppressEmptyResponse":assistant && id.is_some_and(|r|embedded.contains(&r)),"showAssistantCopyButton":meta,"assistantCopyStreaming":message["streaming"]==true || in_progress});
        if !e.projected().is_null() {
            row["projectedItem"] = e.projected().clone();
        }
        if assistant && let Some(summary) = summaries.get(s(&message["id"])) {
            row["assistantTurnDiffSummary"] = (*summary).clone();
        }
        if message["role"] == "user"
            && let Some(count) = revert.get(s(&message["id"]))
        {
            row["revertTurnCount"] = json!(count);
        }
        rows.push(row);
    }
    let setup = input.worktree_setup.as_ref().filter(|s| !s.is_null());
    let handed_off = setup.is_some_and(|s| {
        s["stages"].as_array().is_some_and(|a| {
            a.iter()
                .any(|s| s["id"] == "agent" && s["status"] == "done")
        })
    }) && input
        .latest_run
        .as_ref()
        .is_some_and(|r| r.started_at.is_some());
    let owns_setup = !handed_off
        && setup.is_some_and(|s| matches!(s["phase"].as_str(), Some("running" | "done")));
    if let Some(setup) = setup.filter(|s| !handed_off || s["phase"] != "running") {
        let row = json!({"kind":"worktree-setup","id":"worktree-setup-row","createdAt":setup["startedAt"],"snapshot":setup,"embedded":handed_off});
        let working = owns_setup
            .then(|| rows.iter().position(|r| r["kind"] == "working"))
            .flatten();
        if let Some(i) = working {
            rows.insert(i + 1, row);
        } else {
            let i = rows
                .iter()
                .position(|r| r["kind"] == "message" && r["message"]["role"] == "user")
                .map_or(rows.len(), |i| i + 1);
            if owns_setup {
                rows.insert(i,json!({"kind":"working","id":"working-indicator-row","createdAt":setup["startedAt"]}));
                rows.insert(i + 1, row);
            } else {
                rows.insert(i, row);
            }
        }
    }
    if input.is_working
        && !rows.iter().any(|r| r["kind"] == "working")
        && header == entries.len()
        && let Some(row) = working()
    {
        rows.push(row);
    }
    if input.is_working
        && !input
            .latest_run
            .as_ref()
            .is_some_and(|r| r.turn_ended == Some(true))
        && !owns_setup
        && !has_active_compaction
        && !only_compaction_finished
        && (latest_failed || (!has_activity && input.pending_approval_request_ids.is_empty()))
    {
        let mut row = json!({"kind":"thinking","id":"live-activity-row","createdAt":input.active_turn_started_at});
        if let Some(anchor) = anchor.filter(|_| latest_failed) {
            let id = group_id(anchor.id());
            let expanded = input.expanded_work_group_ids.contains(&id);
            row["groupId"] = json!(id);
            row["expanded"] = json!(expanded);
            rows.push(row);
            let work: Vec<_> = visible_tools
                .iter()
                .filter_map(|e| e.work.as_ref())
                .collect();
            if expanded {
                rows.push(expanded_row(&id, anchor.at(), &work));
            }
            images(&mut rows, &id, anchor.at(), &work);
        } else {
            let waiting = run.as_ref().map_or(0, |run| {
                raw.iter()
                    .rev()
                    .take_while(|e| {
                        e.kind() == "event"
                            && e.item()["type"] == "subagent"
                            && e.item()["runId"] == *run
                    })
                    .filter(|e| folds::live_subagent(e))
                    .count()
            });
            if waiting > 0 {
                row["label"] = json!(if waiting == 1 {
                    "Waiting on a subagent".into()
                } else {
                    format!("Waiting on {waiting} subagents")
                });
            }
            rows.push(row);
        }
    }
    let mut rows = attach_trailing(attach_created(rows, &entries, &collapsed_members));
    let work = |r: &Value| {
        matches!(
            s(&r["kind"]),
            "work" | "work-toggle" | "work-live" | "thinking"
        )
    };
    for i in 0..rows.len() {
        if work(&rows[i]) && rows.get(i + 1).is_some_and(work) {
            rows[i]["continuesWorkLog"] = json!(true);
            continue;
        }
        if !own_message(&rows[i]) {
            continue;
        }
        let prev = i
            .checked_sub(1)
            .and_then(|i| rows.get(i))
            .filter(|r| own_message(r));
        let next = rows.get(i + 1).is_some_and(own_message);
        let grouped = prev.is_some();
        let continues = prev
            .is_some_and(|p| message_steer(&p["message"]) && message_steer(&rows[i]["message"]));
        if next || grouped {
            rows[i]["groupedWithNext"] = json!(next);
            rows[i]["groupedWithPrevious"] = json!(grouped);
            rows[i]["continuesSteer"] = json!(continues);
        }
    }
    rows
}
fn message_steer(m: &Value) -> bool {
    matches!(s(&m["inputIntent"]), "steer" | "promoted_queued_to_steer")
}
fn own_message(r: &Value) -> bool {
    r["kind"] == "message"
        && r["message"]["role"] == "user"
        && r["message"]["createdBy"] != "agent"
        && r["message"].get("scheduledTaskId").is_none()
        && r["message"]["context"].get("guest").is_none()
        && !(s(&r["message"]["text"]).starts_with("[Triggered by schedule task: ")
            && s(&r["message"]["id"]).starts_with("scheduled-task-message:"))
}
fn attach_created(rows: Vec<Value>, entries: &[Entry], collapsed: &BTreeSet<String>) -> Vec<Value> {
    let terminal: BTreeMap<_, _> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r["kind"] == "message" && r["showAssistantMeta"] == true)
        .filter_map(|(i, r)| optional_id(&r["message"]["runId"]).map(|id| (id, i)))
        .collect();
    let mut created: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in entries {
        if matches!(e.kind(), "work" | "event")
            && e.item()["type"] == "thread_created"
            && let Some(run) = optional_id(&e.item()["runId"])
        {
            created.entry(run).or_default().push(json!({"kind":"event","id":e.id(),"createdAt":e.at(),"projectedItem":e.projected()}));
        }
    }
    let mut result = vec![];
    for (i, row) in rows.into_iter().enumerate() {
        if row["kind"] == "event"
            && row["projectedItem"]["item"]["type"] == "thread_created"
            && optional_id(&row["projectedItem"]["item"]["runId"])
                .and_then(|id| terminal.get(&id))
                .is_some_and(|terminal| collapsed.contains(s(&row["id"])) || i > *terminal)
        {
            continue;
        }
        let summaries = if row["kind"] == "message" && row["showAssistantMeta"] == true {
            optional_id(&row["message"]["runId"]).and_then(|id| created.get(&id))
        } else {
            None
        };
        result.push(row);
        if let Some(summaries) = summaries {
            for e in summaries {
                let mut e = e.clone();
                e["id"] = json!(format!("summary:{}", s(&e["id"])));
                e["resourceSummary"] = json!(true);
                result.push(e);
            }
        }
    }
    result
}
fn attach_trailing(rows: Vec<Value>) -> Vec<Value> {
    let mut without = BTreeSet::new();
    let mut metas = BTreeMap::new();
    for (i, row) in rows.iter().enumerate() {
        let Some(run) = optional_id(&row["message"]["runId"]) else {
            continue;
        };
        if row["kind"] != "message"
            || row["message"]["role"] != "assistant"
            || row["showAssistantMeta"] != true
        {
            continue;
        }
        let mut last = None;
        let mut has_group = false;
        for (index, candidate) in rows.iter().enumerate().skip(i + 1) {
            if candidate["kind"] == "message" {
                break;
            }
            if (candidate["kind"] == "work-toggle" && candidate["runId"] == run)
                || (candidate["kind"] == "event"
                    && candidate["resourceSummary"] == true
                    && candidate["projectedItem"]["item"]["runId"] == run)
            {
                has_group = true;
                last = Some(index);
                continue;
            }
            if candidate["kind"] == "work"
                && let Some(entries) = candidate["groupedEntries"]
                    .as_array()
                    .filter(|a| a.iter().any(|e| e["runId"] == run))
            {
                if candidate["isExpandedToolGroup"] != true
                    && entries.iter().any(|e| {
                        serde_json::from_value::<WorkLogEntry>(e.clone())
                            .is_ok_and(|w| session::work_log_entry_is_tool_like(&w))
                            || (e["projectedItem"]["item"]["type"] == "error"
                                && e["projectedItem"]["item"]["status"] == "failed")
                    })
                {
                    has_group = true;
                }
                if has_group {
                    last = Some(index);
                }
            }
        }
        if let Some(last) = last {
            without.insert(s(&row["id"]).to_owned());
            let mut meta = json!({"kind":"assistant-meta","id":format!("assistant-meta:{}",s(&row["message"]["id"])),"createdAt":rows[last]["createdAt"],"message":row["message"],"showAssistantCopyButton":row["showAssistantCopyButton"],"assistantCopyStreaming":row["assistantCopyStreaming"]});
            if let Some(p) = row.get("projectedItem") {
                meta["projectedItem"] = p.clone();
            }
            metas.insert(last, meta);
        }
    }
    let mut result = vec![];
    for (i, mut row) in rows.into_iter().enumerate() {
        if row["kind"] == "message" && without.contains(s(&row["id"])) {
            row["showAssistantMeta"] = json!(false);
            row["showAssistantCopyButton"] = json!(false);
        }
        result.push(row);
        if let Some(meta) = metas.remove(&i) {
            result.push(meta);
        }
    }
    result
}
