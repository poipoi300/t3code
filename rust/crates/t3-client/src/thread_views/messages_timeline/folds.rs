use super::*;

#[derive(Debug, Clone)]
pub(super) struct Fold {
    pub run_id: String,
    pub anchor: String,
    pub at: String,
    pub hidden: BTreeSet<String>,
    pub members: Vec<String>,
    pub label: String,
    pub attempt_id: Option<String>,
    pub deferred: Vec<String>,
}
pub(super) fn terminals(entries: &[Entry]) -> BTreeSet<String> {
    let mut by_response = BTreeMap::new();
    let mut unkeyed = 0;
    for e in entries.iter().filter(|e| e.kind() == "message") {
        if e.message()["role"] == "user" {
            unkeyed += 1;
        } else if e.message()["role"] == "assistant" {
            let key = optional_id(&e.message()["runId"])
                .map_or_else(|| format!("unkeyed:{unkeyed}"), |r| format!("turn:{r}"));
            by_response.insert(key, s(&e.message()["id"]).to_owned());
        }
    }
    by_response.into_values().collect()
}
pub(super) fn retained_replies(entries: &[Entry]) -> BTreeSet<String> {
    let mut retained = BTreeSet::new();
    let mut last: BTreeMap<String, &Entry> = BTreeMap::new();
    for e in entries {
        if e.kind() == "message"
            && e.item()["type"] == "assistant_message"
            && e.item()["phase"] == "final_answer"
        {
            retained.insert(e.id().to_owned());
        }
        if e.attempt()["status"] != "superseded"
            || (e.kind() == "message" && e.message()["role"] == "user")
            || (e.kind() == "work"
                && matches!(s(&e.w()["itemType"]), "system_notice" | "notification"))
            || !matches!(e.kind(), "message" | "work")
        {
            continue;
        }
        let id = s(&e.attempt()["id"]).to_owned();
        let previous_end = last
            .get(&id)
            .filter(|e| e.kind() == "work")
            .and_then(|e| e.item()["deferredWork"]["lastOrdinal"].as_f64());
        if previous_end
            .zip(e.item()["ordinal"].as_f64())
            .is_some_and(|(end, n)| end > n)
        {
            continue;
        }
        last.insert(id, e);
    }
    for e in last.values() {
        if e.kind() == "message"
            && e.message()["streaming"] != true
            && e.item()["type"] == "assistant_message"
            && e.item().get("phase").is_none()
            && e.item()["status"] == "completed"
            && crate::time::epoch_ms(s(&e.item()["completedAt"]))
                .zip(crate::time::epoch_ms(s(&e.attempt()["completedAt"])))
                .is_some_and(|(a, b)| a < b)
        {
            retained.insert(e.id().to_owned());
        }
    }
    retained
}
pub(super) fn live_subagent(e: &Entry) -> bool {
    e.item()["type"] == "subagent"
        && matches!(s(&e.item()["status"]), "pending" | "running" | "waiting")
}
pub(super) fn live_cards(entries: &[Entry]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut start = 0;
    while start < entries.len() {
        if entries[start].kind() != "event" || entries[start].item()["type"] != "subagent" {
            start += 1;
            continue;
        }
        let first = &entries[start];
        let mut end = start + 1;
        while end < entries.len()
            && entries[end].kind() == "event"
            && entries[end].item()["type"] == "subagent"
            && entries[end].item()["runId"] == first.item()["runId"]
            && entries[end].item()["providerTurnId"] == first.item()["providerTurnId"]
        {
            end += 1;
        }
        if entries[start..end].iter().any(live_subagent) {
            ids.extend(entries[start..end].iter().map(|e| e.id().to_owned()));
        }
        start = end;
    }
    ids
}
pub(super) fn failed_item(e: &Entry) -> Option<&Value> {
    (matches!(e.kind(), "work" | "event")
        && e.item()["type"] == "error"
        && e.item()["status"] == "failed"
        && e.item()["parentItemId"].is_null())
    .then(|| e.item())
}
pub(super) fn failed_runs(
    entries: &[Entry],
    input: &MessagesTimelineRowsInput,
) -> BTreeSet<String> {
    let mut ids: BTreeSet<_> = entries
        .iter()
        .filter_map(failed_item)
        .filter_map(|i| optional_id(&i["runId"]))
        .collect();
    if let Some(run) = input.latest_run.as_ref().filter(|r| r.status == "failed") {
        ids.insert(run.run_id.clone());
    }
    ids
}
pub(super) fn itemized_runs(entries: &[Entry], failed: &BTreeSet<String>) -> BTreeSet<String> {
    let mut usage = BTreeSet::new();
    let mut other = BTreeSet::new();
    for item in entries.iter().filter_map(failed_item) {
        if let Some(id) = optional_id(&item["runId"]) {
            if item["failure"]["class"] == "usage_limit" {
                usage.insert(id);
            } else {
                other.insert(id);
            }
        }
    }
    failed
        .iter()
        .filter(|r| !usage.contains(*r) || other.contains(*r))
        .cloned()
        .collect()
}
pub(super) fn superseded_folds(
    entries: &[Entry],
    failed: &BTreeSet<String>,
    live: &BTreeSet<String>,
    retained: &BTreeSet<String>,
) -> Vec<Fold> {
    let retained_attempts: BTreeSet<_> = entries
        .iter()
        .filter(|e| retained.contains(e.id()))
        .map(|e| s(&e.attempt()["id"]))
        .collect();
    let mut groups: Vec<(String, Vec<&Entry>)> = vec![];
    let mut group_indexes = BTreeMap::<String, usize>::new();
    for e in entries {
        let id = s(&e.attempt()["id"]);
        if e.attempt()["status"] != "superseded"
            || retained_attempts.contains(id)
            || failed.contains(s(&e.attempt()["runId"]))
            || (e.kind() == "message" && e.message()["role"] == "user")
            || (e.kind() == "event" && e.item()["type"] == "handoff")
            || matches!(e.kind(), "html-render" | "mcp-app" | "sent-message")
            || e.persistent()
            || live.contains(e.id())
            || (e.kind() == "work" && e.w()["itemType"] == "system_notice")
        {
            continue;
        }
        if let Some(index) = group_indexes.get(id) {
            groups[*index].1.push(e);
        } else {
            group_indexes.insert(id.into(), groups.len());
            groups.push((id.into(), vec![e]));
        }
    }
    groups
        .into_iter()
        .map(|(id, entries)| {
            let first = entries[0];
            Fold {
                run_id: s(&first.attempt()["runId"]).into(),
                anchor: first.id().into(),
                at: first.at().into(),
                hidden: entries.iter().map(|e| e.id().into()).collect(),
                members: vec![],
                label: "Superseded attempt".into(),
                attempt_id: Some(id),
                deferred: entries
                    .iter()
                    .filter(|e| e.kind() == "work" && !e.item()["deferredWork"].is_null())
                    .filter_map(|e| optional_id(&e.projected()["sourceItemId"]))
                    .collect(),
            }
        })
        .collect()
}
fn fold_run(e: &Entry, runless: Option<&str>) -> Option<String> {
    if e.kind() == "work" && e.w()["itemType"] == "system_notice" {
        return None;
    }
    if (e.kind() == "message" && e.message()["role"] == "assistant") || e.kind() == "work" {
        return entry_run_id(e).or_else(|| runless.map(str::to_owned));
    }
    if e.kind() == "event" && (e.persistent() || e.item()["type"] == "subagent") {
        return optional_id(&e.item()["runId"]).or_else(|| runless.map(str::to_owned));
    }
    None
}
#[derive(Default)]
struct Group<'a> {
    entries: Vec<&'a Entry>,
    terminal: Option<&'a Entry>,
    streaming: bool,
    start: Option<&'a str>,
    anchor: &'a str,
    steers: Vec<(usize, &'a Entry)>,
}

pub(super) fn turn_folds(entries: &[Entry], input: &MessagesTimelineRowsInput) -> Vec<Fold> {
    if input.tool_call_visibility == "zen" {
        return vec![];
    }
    let terminal = terminals(entries);
    let live = live_cards(entries);
    let retained = retained_replies(entries);
    let mut unfolded = active_runs(entries, input);
    unfolded.extend(failed_runs(entries, input));
    let mut interrupted = BTreeSet::new();
    let mut prompts = BTreeMap::new();
    for (i, e) in entries.iter().enumerate() {
        if starts_response(e) {
            let id = if e.kind() == "message" {
                optional_id(&e.message()["runId"])
            } else {
                entry_run_id(e)
            };
            if let Some(id) = id {
                prompts.entry(id).or_insert(i);
            }
        }
        if e.kind() == "event"
            && matches!(
                s(&e.item()["type"]),
                "run_interrupt_request" | "run_interrupt_result"
            )
            && let Some(id) = optional_id(&e.item()["runId"])
        {
            interrupted.insert(id);
        }
    }
    let mut groups: Vec<(String, Group<'_>)> = vec![];
    let mut group_indexes = BTreeMap::<String, usize>::new();
    let mut runless_failed = BTreeSet::new();
    let mut runless = None;
    let mut response = None;
    let mut pending = None;
    for (index, e) in entries.iter().enumerate() {
        if starts_response(e) {
            pending = entries.get(index + 1).map(|n| (e.at(), n.id()));
            let id = if e.kind() == "message" {
                optional_id(&e.message()["runId"])
            } else {
                entry_run_id(e)
            };
            runless = id.is_none().then(|| format!("runless:{}", e.id()));
            response = id.or_else(|| runless.clone());
            continue;
        }
        if steer(e) {
            if let Some(id) = optional_id(&e.message()["runId"]).or_else(|| response.clone())
                && let Some((_, g)) = group_indexes.get(&id).map(|index| &mut groups[*index])
                && !g.entries.is_empty()
            {
                if g.steers
                    .last()
                    .is_some_and(|(start, _)| *start == g.entries.len())
                {
                    g.steers.pop();
                }
                g.steers.push((g.entries.len(), e));
            }
            continue;
        }
        let Some(own) = fold_run(e, runless.as_deref()) else {
            continue;
        };
        let run = if response.is_some() && prompts.get(&own).is_some_and(|n| *n > index) {
            response.clone().unwrap_or(own)
        } else {
            own
        };
        if Some(&run) == runless.as_ref() && failed_item(e).is_some() {
            runless_failed.insert(run.clone());
        }
        let group_index = group_indexes.get(&run).copied().unwrap_or_else(|| {
            let (start, anchor) = pending
                .take()
                .map_or((None, e.id()), |(at, id)| (Some(at), id));
            group_indexes.insert(run.clone(), groups.len());
            groups.push((
                run,
                Group {
                    start,
                    anchor,
                    ..Default::default()
                },
            ));
            groups.len() - 1
        });
        let g = &mut groups[group_index].1;
        g.entries.push(e);
        if e.kind() == "message" {
            if terminal.contains(s(&e.message()["id"])) {
                g.terminal = Some(e);
            }
            if e.message()["streaming"] == true {
                g.streaming = true;
            }
        }
    }
    let mut folds = vec![];
    for (run, g) in groups {
        if unfolded.contains(&run)
            || interrupted.contains(&run)
            || runless_failed.contains(&run)
            || (input.is_working && input.runless_work_active && Some(&run) == runless.as_ref())
            || g.streaming
        {
            continue;
        }
        let terminal_index = g
            .terminal
            .and_then(|t| g.entries.iter().position(|e| e.id() == t.id()))
            .unwrap_or(g.entries.len());
        let visible_compaction = g
            .entries
            .iter()
            .rev()
            .find(|e| e.compaction())
            .map(|e| e.id());
        let mut hidden = BTreeSet::new();
        for (i, e) in g.entries.iter().enumerate() {
            if g.terminal.is_some_and(|t| e.id() == t.id()) {
                continue;
            }
            let trailing = e.work.as_ref().is_some_and(|w| {
                w.tool_lifecycle_status.as_deref() != Some("inProgress")
                    && !session::work_entry_display_indicates_tool_failure(w)
            });
            if !e.compaction() && i > terminal_index && !trailing {
                continue;
            }
            if Some(e.id()) == visible_compaction
                || e.persistent()
                || live.contains(e.id())
                || retained.contains(e.id())
                || (e.kind() == "work"
                    && (e.w()["itemType"] == "notification"
                        || e.w().get("questionAnswer").is_some()))
            {
                continue;
            }
            hidden.insert(e.id().to_owned());
        }
        let hides_work = |segment: &[&Entry]| {
            segment
                .iter()
                .any(|e| hidden.contains(e.id()) && !e.compaction())
        };
        if !hides_work(&g.entries) {
            continue;
        }
        let first = g.entries[0];
        let last = g.entries[g.entries.len() - 1];
        let latest = input.latest_run.as_ref().filter(|r| r.run_id == run);
        let ms = latest
            .and_then(|r| r.started_at.as_deref().zip(r.completed_at.as_deref()))
            .map_or_else(
                || {
                    elapsed(
                        g.start.unwrap_or(first.at()),
                        max_iso(g.terminal.map(|e| e.end()), Some(last.end()))
                            .unwrap_or(last.end()),
                    )
                },
                |(a, b)| elapsed(a, b),
            );
        let duration = ms.map(session::timestamps::format_duration);
        let label = if latest.is_some_and(|r| r.status == "interrupted") {
            duration.map_or_else(
                || "You stopped this response".into(),
                |d| format!("You stopped after {d}"),
            )
        } else {
            duration.map_or_else(|| "Worked".into(), |d| format!("Worked for {d}"))
        };
        let end = g.steers.first().map_or(g.entries.len(), |(i, _)| *i);
        let segment = &g.entries[..end];
        folds.push(Fold {
            run_id: run.clone(),
            anchor: g.anchor.into(),
            at: g.start.unwrap_or(first.at()).into(),
            hidden: segment
                .iter()
                .filter(|e| hidden.contains(e.id()))
                .map(|e| e.id().into())
                .collect(),
            members: segment.iter().map(|e| e.id().into()).collect(),
            label,
            attempt_id: None,
            deferred: vec![],
        });
        for (i, (start, steer)) in g.steers.iter().enumerate() {
            let end = g.steers.get(i + 1).map_or(g.entries.len(), |(i, _)| *i);
            let segment = &g.entries[*start..end];
            if segment.is_empty() || !hides_work(segment) {
                continue;
            }
            let end_at = segment
                .iter()
                .fold(None, |at, e| max_iso(at, Some(e.end())))
                .unwrap_or(steer.at());
            let label = elapsed(steer.at(), end_at).map_or_else(
                || "Continued".into(),
                |ms| format!("Continued for {}", session::timestamps::format_duration(ms)),
            );
            folds.push(Fold {
                run_id: format!("{run}:steer:{}", steer.id()),
                anchor: segment[0].id().into(),
                at: steer.at().into(),
                hidden: segment
                    .iter()
                    .filter(|e| hidden.contains(e.id()))
                    .map(|e| e.id().into())
                    .collect(),
                members: segment.iter().map(|e| e.id().into()).collect(),
                label,
                attempt_id: None,
                deferred: vec![],
            });
        }
    }
    folds
}
