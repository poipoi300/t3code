use crate::{
    api::*,
    error::{Error, Result},
};
use serde_json::Value;

pub fn should_prepare_legacy_import_handoff(
    history_origin: Option<&str>,
    has_native_history: bool,
    has_completed_run: bool,
    legacy_import_item_count: usize,
) -> bool {
    history_origin == Some("v1_import")
        && !has_native_history
        && !has_completed_run
        && legacy_import_item_count > 0
}

pub fn derived_id(prefix: &str, parts: &[&str]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = prefix.to_owned();
    for part in parts {
        result.push(':');
        for byte in part.bytes() {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                result.push(char::from(byte));
            } else {
                result.push('%');
                result.push(char::from(HEX[usize::from(byte >> 4)]));
                result.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
    }
    result
}

pub fn thread_has_unfinished_work(projection: &OrchestrationV2ThreadProjection) -> bool {
    projection.runs.iter().any(|run| {
        !matches!(
            run.status,
            OrchestrationV2RunStatus::Completed
                | OrchestrationV2RunStatus::Failed
                | OrchestrationV2RunStatus::Cancelled
                | OrchestrationV2RunStatus::Interrupted
                | OrchestrationV2RunStatus::RolledBack
        )
    }) || projection
        .runtime_requests
        .iter()
        .any(|request| request.status == RuntimeRequestStatus::Pending)
}

pub fn command_thread_id(command: &OrchestrationV2ServerCommand) -> Result<ThreadId> {
    let value = serde_json::to_value(command)?;
    let key = match value["type"].as_str() {
        Some("thread.fork" | "thread.merge_back") => "targetThreadId",
        Some(
            "delegated_task.request"
            | "delegated_task.remote.request"
            | "delegated_task.remote.complete"
            | "delegated_task.remote.stop"
            | "delegated_task.wake-policy"
            | "delegated_task.completion-delivery.acknowledge"
            | "delegated_task.completion-delivery.dispose"
            | "thread.created.record",
        ) => "parentThreadId",
        _ => "threadId",
    };
    Ok(serde_json::from_value(value[key].clone())?)
}

pub fn command_identity(
    command: &OrchestrationV2ServerCommand,
) -> Result<(CommandId, String, ThreadId)> {
    let value = serde_json::to_value(command)?;
    Ok((
        serde_json::from_value(value["commandId"].clone())?,
        value["type"]
            .as_str()
            .ok_or(Error::Decode {
                entity: "command",
                detail: "missing type".into(),
            })?
            .to_owned(),
        command_thread_id(command)?,
    ))
}

// Local shim: the frozen error enum cannot carry the source's tagged wrappers and nested causes.
pub fn rejection(command: &Value, reason: impl Into<String>) -> Error {
    Error::Rejected {
        command_id: CommandId(command["commandId"].as_str().unwrap_or_default().to_owned()),
        reason: reason.into(),
    }
}

// Dates are compared numerically; their original strings remain the persisted values.
pub fn epoch_millis(value: &str) -> Option<i64> {
    fn number(value: &str, range: std::ops::Range<usize>) -> Option<i64> {
        let digits = value.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    }
    let bytes = value.as_bytes();
    let expanded = matches!(bytes.first(), Some(b'+' | b'-'));
    let shift = if expanded { 3 } else { 0 };
    let year = if expanded {
        let magnitude = number(value, 1..7)?;
        if bytes[0] == b'-' {
            if magnitude == 0 {
                return None;
            }
            -magnitude
        } else {
            magnitude
        }
    } else {
        number(value, 0..4)?
    };
    let month = number(value, 5 + shift..7 + shift)?;
    let day = number(value, 8 + shift..10 + shift)?;
    if bytes.get(4 + shift) != Some(&b'-')
        || bytes.get(7 + shift) != Some(&b'-')
        || !(1..=12).contains(&month)
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let yoe = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * shifted_month + 2) / 5 + day - 1;
    let date = (era * 146097 + doe - 719468) * 86_400_000;
    if value.len() == 10 + shift {
        return (date.abs() <= 8_640_000_000_000_000).then_some(date);
    }
    if !matches!(bytes.get(10 + shift), Some(b'T' | b't' | b' '))
        || bytes.get(13 + shift) != Some(&b':')
        || bytes.get(16 + shift) != Some(&b':')
    {
        return None;
    }
    let hour = number(value, 11 + shift..13 + shift)?;
    let minute = number(value, 14 + shift..16 + shift)?;
    let second = number(value, 17 + shift..19 + shift)?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }
    let mut end = 19 + shift;
    let mut millis = 0;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            if end - start < 3 {
                millis = millis * 10 + i64::from(bytes[end] - b'0');
            }
            end += 1;
        }
        if end == start {
            return None;
        }
        for _ in end - start..3 {
            millis *= 10;
        }
    }
    let offset = match bytes.get(end) {
        Some(b'Z' | b'z') if end + 1 == bytes.len() => 0,
        Some(b'+' | b'-') => {
            let offset_hour = number(value, end + 1..end + 3)?;
            let colon = usize::from(bytes.get(end + 3) == Some(&b':'));
            let offset_minute = number(value, end + 3 + colon..end + 5 + colon)?;
            if end + 5 + colon != bytes.len() || offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            (offset_hour * 60 + offset_minute) * 60_000 * if bytes[end] == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let millis = date + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis - offset;
    (millis.abs() <= 8_640_000_000_000_000).then_some(millis)
}

pub fn iso_from_epoch_millis(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000) + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let time = millis.rem_euclid(86_400_000);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{year:+07}")
    };
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedPullRequestKey {
    pub key: t3_contracts::thread_pull_request::ThreadPullRequestKey,
    pub authority: Option<String>,
    pub url: Option<String>,
}
impl PreparedPullRequestKey {
    pub fn from_value(value: &Value) -> Result<Self> {
        Ok(Self {
            key: serde_json::from_value(value.clone())?,
            authority: value["authority"].as_str().map(str::to_owned),
            url: value["url"].as_str().map(str::to_owned),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PreparedPullRequestIdentities {
    pub qualified_keys: Vec<(
        PreparedPullRequestKey,
        t3_contracts::thread_pull_request::ThreadPullRequestKey,
    )>,
    pub keys: Vec<(
        t3_contracts::thread_pull_request::ThreadPullRequestKey,
        t3_contracts::thread_pull_request::ThreadPullRequestKey,
    )>,
    pub legacy_keys: Vec<(
        t3_contracts::thread_pull_request::ThreadLinkedPullRequest,
        t3_contracts::thread_pull_request::ThreadPullRequestKey,
    )>,
    pub canonical_urls: Vec<(String, String)>,
}

impl PreparedPullRequestIdentities {
    pub fn observe_key(
        &mut self,
        raw: &Value,
        canonical: t3_contracts::thread_pull_request::ThreadPullRequestKey,
    ) -> Result<()> {
        self.qualified_keys
            .push((PreparedPullRequestKey::from_value(raw)?, canonical));
        Ok(())
    }
    pub fn key(&self, value: &Value) -> Result<Value> {
        let raw = PreparedPullRequestKey::from_value(value)?;
        let qualified = self
            .qualified_keys
            .iter()
            .filter(|(key, _)| key == &raw)
            .map(|(_, key)| key)
            .collect::<Vec<_>>();
        let bare = self
            .keys
            .iter()
            .filter(|(key, _)| key == &raw.key && raw.authority.is_none() && raw.url.is_none())
            .map(|(_, key)| key)
            .collect::<Vec<_>>();
        let matches = if qualified.is_empty() {
            bare
        } else {
            qualified
        };
        let normalized=matches.first().ok_or(Error::Unsupported("pull-request key requires the host source-control registry observation, including authority and URL"))?;
        if matches.iter().any(|key| key != normalized) {
            return Err(Error::Decode {
                entity: "source-control identity observations",
                detail: "Conflicting canonical keys for one raw key, authority and URL.".into(),
            });
        }
        Ok(t3_contracts::stored::to_value(normalized)?)
    }
    pub fn legacy(&self, value: &Value) -> Result<Value> {
        let raw = serde_json::from_value::<
            t3_contracts::thread_pull_request::ThreadLinkedPullRequest,
        >(value.clone())?;
        let key = self
            .legacy_keys
            .iter()
            .find(|(link, _)| link == &raw)
            .map(|(_, key)| key)
            .ok_or(Error::Unsupported(
                "legacy pull-request key requires the host source-control registry observation",
            ))?;
        Ok(t3_contracts::stored::to_value(key)?)
    }
    pub fn url(&self, url: &str) -> Result<String> {
        self.canonical_urls
            .iter()
            .find(|(raw, _)| raw == url)
            .map(|(_, canonical)| canonical.clone())
            .ok_or(Error::Unsupported(
                "pull-request URL requires canonical host registry observation",
            ))
    }
}

pub fn with_pull_request_watch(mut link: Value, watch: Option<Value>) -> Result<Value> {
    let fields = link.as_object_mut().ok_or(Error::Stale)?;
    let previous = fields.shift_remove("watch");
    let ended = fields.shift_remove("endedWatch");
    if let Some(watch) = watch {
        fields.insert("watch".into(), watch);
    } else if let Some(ended) = previous.or(ended) {
        fields.insert("endedWatch".into(), ended);
    }
    Ok(link)
}

pub fn pull_request_links_of(
    thread: &Value,
    identities: &PreparedPullRequestIdentities,
) -> Result<Vec<Value>> {
    use serde_json::json;
    let mut links = thread["pullRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if thread.get("pullRequests").is_none() && !thread["linkedPullRequest"].is_null() {
        let legacy = &thread["linkedPullRequest"];
        let key = identities.legacy(legacy)?;
        let mut link = key;
        link["url"] = legacy["url"].clone();
        link["source"] = json!("manual");
        link["linkedAt"] = json!("1970-01-01T00:00:00.000Z");
        link["snapshot"] = Value::Null;
        link["stack"] = Value::Null;
        links.push(link);
    }
    Ok(links)
}

pub fn pull_request_mutation(
    mut thread: Value,
    command: &Value,
    now: &IsoDateTime,
    identities: &PreparedPullRequestIdentities,
) -> Result<Value> {
    use serde_json::json;
    let kind = command["type"].as_str().ok_or(Error::Stale)?;
    let mut links = pull_request_links_of(&thread, identities)?;
    if kind == "thread.pull-request.sync" {
        thread["branchPullRequest"] = command["branchPullRequest"].clone();
        if let Some(linked) = command.get("linkedPullRequest") {
            let old = if thread["linkedPullRequest"].is_null() {
                None
            } else {
                Some(identities.legacy(&thread["linkedPullRequest"])?)
            };
            let new = if linked.is_null() {
                None
            } else {
                Some(identities.legacy(linked)?)
            };
            let mut filtered = Vec::new();
            for link in &links {
                let key = identities.key(link)?;
                if Some(&key) != old.as_ref() && Some(&key) != new.as_ref() {
                    filtered.push(link.clone());
                }
            }
            if let Some(key) = new {
                let mut link = key.clone();
                link["url"] = json!(identities.url(linked["url"].as_str().ok_or(Error::Stale)?)?);
                link["source"] = json!("manual");
                link["linkedAt"] = json!(now);
                link["snapshot"] = Value::Null;
                link["stack"] = Value::Null;
                if let Some(existing) = links
                    .iter()
                    .find(|link| identities.key(link).ok().as_ref() == Some(&key))
                {
                    for field in ["watch", "endedWatch", "repositoryWebUrl"] {
                        if let Some(value) = existing.get(field) {
                            link[field] = value.clone();
                        }
                    }
                }
                filtered.push(link);
            }
            thread["linkedPullRequest"] = linked.clone();
            thread["pullRequests"] = json!(filtered);
        }
        return Ok(thread);
    }
    let key = identities.key(command)?;
    let mut index = None;
    for (offset, link) in links.iter().enumerate() {
        if identities.key(link)? == key {
            index = Some(offset);
            break;
        }
    }
    match kind {
        "thread.pull-request.link" => {
            if let Some(index) = index
                && !(links[index]["source"] == "stack-dismissed"
                    && command["source"] != "stack"
                    && command["source"] != "stack-dismissed")
            {
                return Ok(thread);
            }
            let mut link = index.map(|index| links.remove(index)).unwrap_or_else(|| {
                let mut link = key.clone();
                link["linkedAt"] = json!(now);
                link["snapshot"] = Value::Null;
                link["stack"] = Value::Null;
                link
            });
            link["source"] = command["source"].clone();
            link["url"] = json!(identities.url(command["url"].as_str().ok_or(Error::Stale)?)?);
            if let Some(url) = command.get("repositoryWebUrl") {
                link["repositoryWebUrl"] = url.clone();
            }
            if command["source"] == "manual"
                && !thread["branchPullRequest"].is_null()
                && !links.iter().any(|link| link["source"] != "stack-dismissed")
            {
                let branch = &thread["branchPullRequest"];
                let branch_key = identities.legacy(branch)?;
                if branch_key != key
                    && !links
                        .iter()
                        .any(|link| identities.key(link).ok().as_ref() == Some(&branch_key))
                {
                    let mut branch_link = branch_key;
                    branch_link["url"] =
                        json!(identities.url(branch["url"].as_str().ok_or(Error::Stale)?)?);
                    branch_link["source"] = json!("manual");
                    branch_link["linkedAt"] = json!(now);
                    branch_link["snapshot"] = Value::Null;
                    branch_link["stack"] = Value::Null;
                    links.insert(0, branch_link);
                }
            }
            links.push(link);
            thread["updatedAt"] = json!(now);
        }
        "thread.pull-request.unlink" => {
            let Some(index) = index else {
                return Ok(thread);
            };
            let stack = links[index]["source"] == "stack"
                || !links[index]["stack"].is_null()
                || links.iter().any(|link| {
                    link["host"].as_str().map(str::to_lowercase)
                        == key["host"].as_str().map(str::to_owned)
                        && link["repository"].as_str().map(str::to_lowercase)
                            == key["repository"].as_str().map(str::to_owned)
                        && link["stack"]["layers"].as_array().is_some_and(|layers| {
                            layers.iter().any(|layer| layer["number"] == key["number"])
                        })
                });
            if stack {
                links[index] = with_pull_request_watch(links[index].clone(), None)?;
                links[index]["source"] = json!("stack-dismissed");
            } else {
                links.remove(index);
            }
            thread["updatedAt"] = json!(now);
        }
        "thread.pull-request-link.sync" => {
            let Some(index) = index else {
                return Ok(thread);
            };
            links[index]["snapshot"] = command["snapshot"].clone();
            links[index]["stack"] = command["stack"].clone();
        }
        "thread.pull-request.watch" | "thread.pull-request-watch.sync" => {
            if kind == "thread.pull-request.watch"
                && command["watching"] == true
                && thread["lineage"]["relationshipToParent"] == "subagent"
                && thread["creationSource"] == "provider"
            {
                return Err(rejection(
                    command,
                    "This subagent is run by its provider and cannot take messages. Message the parent thread instead.",
                ));
            }
            if kind == "thread.pull-request.watch"
                && command["watching"] == true
                && command.get("link").is_some()
                && index.is_none_or(|index| links[index]["source"] == "stack-dismissed")
            {
                let dismissed = index.map(|index| links.remove(index));
                let mut link = key;
                link["url"] =
                    json!(identities.url(command["link"]["url"].as_str().ok_or(Error::Stale)?)?);
                link["source"] = command["link"]["source"].clone();
                link["linkedAt"] = json!(now);
                link["snapshot"] = Value::Null;
                link["stack"] = Value::Null;
                if let Some(url) = command["link"].get("repositoryWebUrl").or_else(|| {
                    dismissed
                        .as_ref()
                        .and_then(|link| link.get("repositoryWebUrl"))
                }) {
                    link["repositoryWebUrl"] = url.clone();
                }
                if let Some(ended) = dismissed.as_ref().and_then(|link| link.get("endedWatch")) {
                    link["endedWatch"] = ended.clone();
                }
                index = Some(links.len());
                links.push(link);
            }
            let Some(index) = index.filter(|index| links[*index]["source"] != "stack-dismissed")
            else {
                return Ok(thread);
            };
            let existing = links[index].get("watch").cloned();
            let watch = if kind == "thread.pull-request-watch.sync" {
                if existing
                    .as_ref()
                    .is_none_or(|watch| watch["startedAt"] != command["startedAt"])
                {
                    return Ok(thread);
                }
                if command["watch"].is_null() {
                    None
                } else {
                    let mut watch = command["watch"].clone();
                    if command.get("checksPendingBefore").is_some()
                        && existing
                            .as_ref()
                            .is_none_or(|watch| watch.get("checksPending").is_none())
                    {
                        watch
                            .as_object_mut()
                            .ok_or(Error::Stale)?
                            .shift_remove("checksPending");
                    }
                    Some(watch)
                }
            } else if command["watching"] == false {
                None
            } else if existing.is_some() {
                existing
            } else {
                let ended = links[index].get("endedWatch");
                let mut watch = json!({"startedAt":now,"headSha":null,"failedChecks":[],"passed":false,"passedChecks":[],"conflicting":false,"awaitingBaseline":"all","remarksThrough":now,"remarkIds":[],"wakes":0});
                if let Some(ended) = ended {
                    for field in [
                        "headSha",
                        "failedChecks",
                        "passed",
                        "passedChecks",
                        "conflicting",
                        "awaitingBaseline",
                        "owedChecks",
                        "awaitingApproval",
                        "approvalAnnounced",
                        "checksAwaitedSince",
                    ] {
                        if let Some(value) = ended.get(field) {
                            watch[field] = value.clone();
                        }
                    }
                    if ended["checksPending"] == true {
                        watch["checksPending"] = json!(true);
                    }
                }
                Some(watch)
            };
            if kind == "thread.pull-request-watch.sync"
                && watch.is_none()
                && let Some(ended) = command.get("endedWatch")
            {
                links[index]["watch"] = ended.clone();
            }
            links[index] = with_pull_request_watch(links[index].clone(), watch)?;
            if kind == "thread.pull-request.watch" {
                thread["updatedAt"] = json!(now);
            }
        }
        _ => return Err(Error::Unsupported("pull-request mutation")),
    }
    if matches!(
        kind,
        "thread.pull-request.link" | "thread.pull-request.unlink" | "thread.pull-request-link.sync"
    ) && !thread["linkedPullRequest"].is_null()
    {
        let legacy = identities.legacy(&thread["linkedPullRequest"])?;
        if !links.iter().any(|link| {
            link["source"] != "stack-dismissed"
                && identities.key(link).ok().as_ref() == Some(&legacy)
        }) {
            thread["linkedPullRequest"] = Value::Null;
        }
    }
    thread["pullRequests"] = json!(links);
    Ok(thread)
}

pub fn record_thread_open(previous: &Value, at: i64, picked_query: &str) -> Value {
    let mut picks: Vec<_> = previous["picks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|pick| {
            pick["at"]
                .as_f64()
                .is_some_and(|earlier| at as f64 - earlier <= 21.0 * 86_400_000.0)
        })
        .cloned()
        .collect();
    if !picked_query.is_empty() {
        picks.retain(|pick| pick["query"] != picked_query);
        picks.insert(0, serde_json::json!({"query":picked_query,"at":at}));
        picks.truncate(3);
    }
    let age = previous["openedAt"]
        .as_f64()
        .map(|opened| at as f64 - opened);
    if age.is_some_and(|age| age < 600_000.0) {
        let mut previous = previous.clone();
        previous["picks"] = serde_json::json!(picks);
        return previous;
    }
    let score = age.map_or(1.0, |age| {
        previous["score"].as_f64().unwrap_or(0.0)
            * 2.0f64.powf(-age.max(0.0) / (14.0 * 86_400_000.0))
            + 1.0
    });
    serde_json::json!({"openedAt":at,"score":score,"picks":picks})
}

// Local shim until the shared Unicode search fold (NFC/NFKD and Mark removal) is available.
pub fn picked_search_query(query: &str) -> Result<String> {
    if !query.is_ascii() {
        return Err(Error::Unsupported(
            "thread-open search folding requires unicode-normalization",
        ));
    }
    let query = query
        .trim()
        .trim_matches(['"', '\'', '`'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    Ok(query.chars().take(32).collect::<String>().trim().to_owned())
}

pub struct DelegatedTaskProgress<'a> {
    pub state: &'static str,
    pub result_run: Option<&'a OrchestrationV2Run>,
}

pub fn delegated_task_progress(
    projection: &OrchestrationV2ThreadProjection,
) -> Result<DelegatedTaskProgress<'_>> {
    let value = t3_contracts::stored::to_value(projection)?;
    let monitor: Vec<_> = value["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["notification"]["source"]["kind"] == "monitor")
        .map(|message| message["runId"].clone())
        .collect();
    let work: Vec<_> = projection
        .runs
        .iter()
        .filter(|run| {
            run.status != OrchestrationV2RunStatus::RolledBack
                && !monitor.contains(&t3_contracts::stored::to_value(&run.id).unwrap_or_default())
        })
        .collect();
    let terminal = |run: &OrchestrationV2Run| {
        matches!(
            run.status,
            OrchestrationV2RunStatus::Completed
                | OrchestrationV2RunStatus::Failed
                | OrchestrationV2RunStatus::Cancelled
                | OrchestrationV2RunStatus::Interrupted
                | OrchestrationV2RunStatus::RolledBack
        )
    };
    let active = work.iter().any(|run| {
        !terminal(run)
            && !(run.status == OrchestrationV2RunStatus::Queued
                && run.queue_held == Some(Some(true)))
    });
    let children = value["subagents"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|task| {
            matches!(
                task["status"].as_str(),
                Some("pending" | "running" | "waiting")
            ) || task["status"] != "idle"
                && matches!(
                    task["completionDelivery"]["state"].as_str(),
                    Some("pending" | "claimed")
                )
        })
        || value["providerThreads"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|thread| {
                thread["pendingBackgroundTasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|task| {
                        task["awaited"] == true
                            || matches!(task["kind"].as_str(), Some("subagent" | "background_task"))
                    })
            });
    let result_run = work
        .into_iter()
        .filter(|run| terminal(run) && (run.started_at.is_some() || run.ordinal == 1))
        .reduce(|latest, run| {
            if super::model_switch::run_ran_after(run, latest) {
                run
            } else {
                latest
            }
        });
    Ok(DelegatedTaskProgress {
        state: if active || result_run.is_none() {
            "working"
        } else if children {
            "waiting_for_children"
        } else {
            "result_available"
        },
        result_run,
    })
}

pub type RestartContinuations = Vec<(ThreadId, Vec<RunId>)>;

pub fn idle_orphan_tasks(
    parent: &OrchestrationV2ThreadProjection,
    children: &[OrchestrationV2ThreadProjection],
    continuations: Option<&RestartContinuations>,
    now: &IsoDateTime,
) -> Result<Vec<OrchestrationV2Subagent>> {
    let parent_id = t3_contracts::stored::to_value(&parent.thread.id)?;
    let mut cancelled = Vec::new();
    for task in &parent.subagents {
        let task_value = t3_contracts::stored::to_value(task)?;
        if task_value["origin"] != "app_owned"
            || !matches!(
                task_value["status"].as_str(),
                Some("pending" | "running" | "waiting")
            )
        {
            continue;
        }
        let Some(child) = children.iter().find(|child| {
            t3_contracts::stored::to_value(&child.thread.id)
                .ok()
                .as_ref()
                == Some(&task_value["childThreadId"])
        }) else {
            continue;
        };
        let value = t3_contracts::stored::to_value(child)?;
        if value["thread"]["lineage"]["parentThreadId"] != parent_id
            || value["thread"]["lineage"]["relationshipToParent"] != "subagent"
            || value["thread"]["forkedFrom"]["type"] == "node"
                && value["thread"]["forkedFrom"]["nodeId"] == task_value["id"]
            || value["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|run| run["delegatedTaskId"] == task_value["id"])
        {
            continue;
        }
        if child.runs.iter().any(|run| {
            run.status == OrchestrationV2RunStatus::Queued
                || super::command_policy::is_active_run(run)
        }) || value["providerThreads"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|owner| {
                owner["status"] == "active"
                    || owner["pendingBackgroundTasks"]
                        .as_array()
                        .is_some_and(|tasks| !tasks.is_empty())
            })
            || value["runtimeRequests"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|request| request["status"] == "pending")
            || value["turnItems"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("command_execution" | "dynamic_tool" | "subagent")
                    ) && matches!(
                        item["status"].as_str(),
                        Some("pending" | "running" | "waiting")
                    )
                })
        {
            continue;
        }
        let pending = continuations
            .and_then(|rows| {
                rows.iter()
                    .find(|(id, _)| serde_json::json!(id) == value["thread"]["id"])
                    .map(|(_, ids)| ids)
            })
            .ok_or(Error::Unsupported(
                "idle orphan cancellation requires restart continuation observations",
            ))?;
        if !pending.is_empty() {
            continue;
        }
        let mut task = task_value;
        task["status"] = serde_json::json!("cancelled");
        task["completedAt"] = serde_json::json!(now);
        task["updatedAt"] = serde_json::json!(now);
        task["completionDelivery"] = serde_json::json!({"state":"disposed","observedByRunId":null});
        cancelled.push(serde_json::from_value(task)?);
    }
    Ok(cancelled)
}

pub fn wake_work_started_at(
    projection: &OrchestrationV2ThreadProjection,
    trigger: &Value,
    delivery: &str,
) -> Result<Option<IsoDateTime>> {
    let value = t3_contracts::stored::to_value(projection)?;
    let wake = !trigger["notification"].is_null()
        || !trigger["delegatedCompletion"].is_null()
        || !trigger["restartContinuationOfRunId"].is_null()
        || trigger["createdBy"] == "agent"
            && trigger["creationSource"] == "mcp"
            && !trigger["senderThreadId"].is_null()
            && (delivery == "automatic-queue"
                || delivery == "immediate"
                    && value["subagents"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|task| {
                            task["origin"] == "app_owned"
                                && task["childThreadId"] == trigger["senderThreadId"]
                        }));
    if !wake {
        return Ok(None);
    }
    let previous = projection
        .runs
        .iter()
        .filter(|run| run.started_at.is_some())
        .max_by_key(|run| {
            (
                run.started_at.as_ref().and_then(|at| epoch_millis(&at.0)),
                run.ordinal,
            )
        });
    previous
        .map(|run| {
            let value = t3_contracts::stored::to_value(run)?;
            let date = ["workStartedAt", "startedAt", "requestedAt"]
                .iter()
                .find_map(|key| value[key].as_str())
                .ok_or(Error::Stale)?;
            Ok(t3_contracts::wire::WireDateTime(date.into()))
        })
        .transpose()
}

pub fn stale_question_wake(
    parent: &OrchestrationV2ThreadProjection,
    message: &OrchestrationV2ConversationMessage,
    child: Option<&OrchestrationV2ThreadProjection>,
) -> Result<bool> {
    let message = t3_contracts::stored::to_value(message)?;
    let source = &message["notification"]["source"];
    if source["kind"] != "delegated_task" || message["notification"]["outcome"] != "updated" {
        return Ok(false);
    }
    let ids = source["taskIds"].as_array().cloned().unwrap_or_default();
    let task = parent.subagents.iter().find(|task| {
        t3_contracts::stored::to_value(&task.id)
            .ok()
            .is_some_and(|id| ids.contains(&id))
    });
    let Some(task) = task else {
        return Ok(false);
    };
    let task = t3_contracts::stored::to_value(task)?;
    if task["childThreadId"].is_null()
        || matches!(
            task["status"].as_str(),
            Some("completed" | "failed" | "cancelled" | "interrupted")
        )
    {
        return Ok(true);
    }
    let Some(child) = child else {
        return Ok(false);
    };
    if t3_contracts::stored::to_value(&child.thread.id)? != task["childThreadId"] {
        return Ok(false);
    }
    let parent_id = t3_contracts::stored::to_value(&parent.thread.id)?;
    Ok(!child.runtime_requests.iter().any(|request| {
        let request = t3_contracts::stored::to_value(request).unwrap_or_default();
        request["kind"] == "user_input"
            && request["status"] == "pending"
            && message["id"]
                == format!(
                    "message:delegated-task-question:{}:{}",
                    parent_id.as_str().unwrap_or_default(),
                    request["id"].as_str().unwrap_or_default()
                )
    }))
}

pub enum PullRequestWatchWakeState {
    Unchanged,
    Stale,
    Updated(Value),
}

fn monitor_stamps(message: &Value) -> Vec<&Value> {
    let source = &message["notification"]["source"];
    if source["kind"] != "monitor" || source["pullRequest"].is_null() {
        return Vec::new();
    }
    std::iter::once(&source["pullRequest"])
        .chain(source["otherPullRequests"].as_array().into_iter().flatten())
        .collect()
}
fn snapshot_ends_watch(snapshot: &Value, started: &Value) -> bool {
    snapshot["state"] == "merged"
        || snapshot["state"] == "closed"
            && snapshot["syncedAt"]
                .as_str()
                .and_then(epoch_millis)
                .zip(started.as_str().and_then(epoch_millis))
                .is_some_and(|(synced, started)| synced >= started)
}

pub fn current_pull_request_watch_wake(
    projection: &OrchestrationV2ThreadProjection,
    message: &OrchestrationV2ConversationMessage,
    identities: &PreparedPullRequestIdentities,
) -> Result<PullRequestWatchWakeState> {
    use crate::ports::thread_project::pull_request_watch::{
        PullRequestWatchWake, combine_pull_request_watch_wakes,
    };
    let mut message = t3_contracts::stored::to_value(message)?;
    let stamps = monitor_stamps(&message);
    if stamps.is_empty() {
        return Ok(PullRequestWatchWakeState::Unchanged);
    }
    let links = pull_request_links_of(
        &t3_contracts::stored::to_value(&projection.thread)?,
        identities,
    )?;
    let all_messages = t3_contracts::stored::to_value(&projection.messages)?;
    let mut current = Vec::new();
    for stamp in &stamps {
        let mut link = None;
        for candidate in links
            .iter()
            .filter(|link| link["source"] != "stack-dismissed")
        {
            if identities.key(candidate)? == identities.key(stamp)? {
                link = Some(candidate);
                break;
            }
        }
        let stale = if let Some(link) = link {
            let watch = &link["watch"];
            if stamp["watchEnded"] == true {
                let latest = if watch.is_null() {
                    &link["endedWatch"]
                } else {
                    watch
                };
                latest["startedAt"] != stamp["watchStartedAt"]
            } else if watch.is_null() {
                let mut ended_with_notice = false;
                for notice in all_messages
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(monitor_stamps)
                {
                    if notice["watchEnded"] == true
                        && notice["watchStartedAt"] == stamp["watchStartedAt"]
                        && identities.key(notice)? == identities.key(stamp)?
                    {
                        ended_with_notice = true;
                        break;
                    }
                }
                !ended_with_notice
                    || link["endedWatch"]["startedAt"] != stamp["watchStartedAt"]
                    || snapshot_ends_watch(&link["snapshot"], &stamp["watchStartedAt"])
            } else {
                watch["startedAt"] != stamp["watchStartedAt"]
                    || !stamp["headSha"].is_null() && watch["headSha"] != stamp["headSha"]
                    || snapshot_ends_watch(&link["snapshot"], &watch["startedAt"])
            }
        } else {
            true
        };
        if !stale {
            current.push((*stamp).clone());
        }
    }
    if current.is_empty() {
        return Ok(PullRequestWatchWakeState::Stale);
    }
    if current.len() == stamps.len() || current.iter().any(|stamp| stamp["section"].is_null()) {
        return Ok(PullRequestWatchWakeState::Unchanged);
    }
    let sections=current.iter().map(|stamp| {
        let section=&stamp["section"];
        Ok(PullRequestWatchWake{text:section["text"].as_str().ok_or(Error::Stale)?.into(),notification:serde_json::from_value(serde_json::json!({"source":{"kind":"monitor"},"outcome":section["outcome"],"summary":section["summary"]}))?})
    }).collect::<Result<Vec<_>>>()?;
    let combined = combine_pull_request_watch_wakes(sections[0].clone(), &sections[1..])?;
    let mut first = current[0].clone();
    if current.len() == 1 {
        first
            .as_object_mut()
            .ok_or(Error::Stale)?
            .shift_remove("section");
    }
    let mut notification = t3_contracts::stored::to_value(&combined.notification)?;
    notification["source"] = serde_json::json!({"kind":"monitor","pullRequest":first});
    if current.len() > 1 {
        notification["source"]["otherPullRequests"] = serde_json::json!(&current[1..]);
    }
    message["text"] = serde_json::json!(combined.text);
    message["notification"] = notification;
    Ok(PullRequestWatchWakeState::Updated(message))
}

pub fn metadata_update_continuation(
    command: &OrchestrationV2ServerCommand,
) -> Result<Option<OrchestrationV2ServerCommand>> {
    let value = t3_contracts::stored::to_value(command)?;
    if value["type"] != "thread.metadata.update" || value.get("continuation").is_none() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(
        serde_json::json!({"type":"message.dispatch","commandId":value["commandId"],"threadId":value["threadId"],"messageId":value["continuation"]["messageId"],"text":value["continuation"]["text"],"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"mcp"}),
    )?))
}

pub fn follows_current_conversation(message: &Value) -> bool {
    !message["delegatedCompletion"].is_null()
        || message["followsThreadSelection"] == true
        || message["notification"]["source"]["kind"] == "monitor"
            && !(message["createdBy"] == "agent"
                && message["creationSource"] == "provider"
                && !message["notification"].is_null()
                && message["delegatedCompletion"].is_null())
}

pub fn dispatch_follows_current(command: &Value) -> bool {
    follows_current_conversation(command)
        || !command["scheduledTaskId"].is_null() && command.get("modelSelection").is_none()
}

pub fn queued_run_model_selection(
    projection: &OrchestrationV2ThreadProjection,
    run: &OrchestrationV2Run,
) -> Result<t3_contracts::model_selection::ModelSelection> {
    let follows = projection
        .messages
        .iter()
        .find(|message| message.id == run.user_message_id)
        .map(t3_contracts::stored::to_value)
        .transpose()?
        .is_some_and(|message| follows_current_conversation(&message));
    Ok(if follows {
        &projection.thread.model_selection
    } else {
        &run.model_selection
    }
    .clone())
}

pub fn queue_tail_model_selection(
    projection: &OrchestrationV2ThreadProjection,
) -> Result<t3_contracts::model_selection::ModelSelection> {
    for run in super::queued_run_order::queued_runs_in_delivery_order(projection)
        .into_iter()
        .rev()
    {
        let message = projection
            .messages
            .iter()
            .find(|message| message.id == run.user_message_id)
            .map(t3_contracts::stored::to_value)
            .transpose()?;
        if !message.as_ref().is_some_and(follows_current_conversation) {
            return Ok(run.model_selection.clone());
        }
    }
    Ok(projection.thread.model_selection.clone())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveThreadListEntry {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub title: t3_contracts::base_schemas::TrimmedNonEmptyString,
    pub lineage: OrchestrationV2AppThreadLineage,
    pub settled_override: Option<AppThreadSettledOverride>,
    pub settled_at: Option<IsoDateTime>,
    pub snoozed_until: Option<Option<IsoDateTime>>,
    pub updated_at: IsoDateTime,
}

fn terminal_task_run(status: &OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Cancelled
            | OrchestrationV2RunStatus::RolledBack
    )
}

pub fn delegated_result_pending(
    child: &OrchestrationV2ThreadProjection,
    task: Option<&NodeId>,
    restarts: &[RunId],
) -> Result<bool> {
    use crate::ports::runs_ingestion::restart_background_note::{
        delegated_task_awaits_work, delegated_task_result_run_from, delegated_task_runs,
    };
    let pending_after =
        |runs: &[OrchestrationV2Run], result: &OrchestrationV2Run| -> Result<bool> {
            for run in runs {
                if (run.id == result.id || super::model_switch::run_ran_after(run, result))
                    && restarts.contains(
                        &crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                            &run.id,
                        )?,
                    )
                {
                    return Ok(true);
                }
            }
            Ok(false)
        };
    if let Some(task) = task {
        let runs = delegated_task_runs(&child.runs, &child.messages, task)?;
        let Some(run) = delegated_task_result_run_from(&runs) else {
            return Ok(true);
        };
        if !terminal_task_run(&run.status)
            || delegated_task_awaits_work(&runs, &child.subagents, &child.provider_threads)
        {
            return Ok(true);
        }
        return pending_after(&runs, run);
    }
    let progress = delegated_task_progress(child)?;
    if progress.state != "result_available" {
        return Ok(true);
    }
    progress
        .result_run
        .map_or(Ok(false), |run| pending_after(&child.runs, run))
}

pub fn delegated_cancellation_marker(
    before: &OrchestrationV2ThreadProjection,
    after: &OrchestrationV2ThreadProjection,
    task: &OrchestrationV2Subagent,
    now: &IsoDateTime,
) -> Result<Option<OrchestrationV2Subagent>> {
    use crate::ports::runs_ingestion::restart_background_note::{
        delegated_task_awaits_work, delegated_task_runs, run_outcome,
    };
    let value = t3_contracts::stored::to_value(task)?;
    if value["origin"] != "app_owned"
        || value["childThreadId"] != t3_contracts::stored::to_value(&after.thread.id)?
        || !matches!(
            value["status"].as_str(),
            Some("pending" | "running" | "waiting")
        )
    {
        return Ok(None);
    }
    let id = crate::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&task.id)?;
    let latest = |child: &OrchestrationV2ThreadProjection| -> Result<Option<OrchestrationV2Run>> {
        Ok(delegated_task_runs(&child.runs, &[], &id)?
            .into_iter()
            .max_by_key(|run| run.ordinal))
    };
    let after_turn = |child: &OrchestrationV2ThreadProjection| {
        let mut child = child.clone();
        for run in &mut child.runs {
            if run.status == OrchestrationV2RunStatus::Waiting {
                run.status = OrchestrationV2RunStatus::Completed;
            }
        }
        child
    };
    let unsettled = if let Some(run) = latest(before)? {
        !terminal_task_run(&run_outcome(&run))
            || delegated_task_awaits_work(
                &delegated_task_runs(&before.runs, &before.messages, &id)?,
                &before.subagents,
                &before.provider_threads,
            )
    } else {
        let thread = t3_contracts::stored::to_value(&before.thread)?;
        thread["forkedFrom"]["type"] == "node"
            && thread["forkedFrom"]["nodeId"] == value["id"]
            && delegated_task_progress(&after_turn(before))?.state != "result_available"
    };
    if !unsettled {
        return Ok(None);
    }
    let messages = t3_contracts::stored::to_value(&after.messages)?;
    let monitors: Vec<_> = messages
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["notification"]["source"]["kind"] == "monitor")
        .map(|message| message["runId"].clone())
        .collect();
    let mut selected = latest(after)?;
    if selected.is_none() {
        for run in &after.runs {
            if super::command_policy::is_active_run(run)
                && !monitors.contains(&t3_contracts::stored::to_value(&run.id)?)
                && selected
                    .as_ref()
                    .is_none_or(|current| run.ordinal > current.ordinal)
            {
                selected = Some(run.clone());
            }
        }
    }
    if selected.is_none() {
        selected = delegated_task_progress(&after_turn(after))?
            .result_run
            .cloned();
    }
    let Some(run) = selected else { return Ok(None) };
    let mut marker = value;
    marker["cancelledRunId"] = t3_contracts::stored::to_value(&run.id)?;
    marker["updatedAt"] = serde_json::json!(now);
    Ok(Some(serde_json::from_value(marker)?))
}
