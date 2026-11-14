use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use t3_contracts::{agent_sessions::AgentSessionSource, provider_instance::ProviderInstanceId};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionMessage {
    pub role: String,
    pub text: String,
    pub created_at: String,
}
#[derive(Debug, Clone)]
pub struct SessionThread {
    pub source: AgentSessionSource,
    pub provider_instance_id: ProviderInstanceId,
    pub provider_session_id: String,
    pub title: String,
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<SessionMessage>,
    pub omitted_message_count: usize,
}
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().into(),
        Value::Array(a) => a
            .iter()
            .filter(|v| {
                matches!(
                    v["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|v| v["text"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
fn nonempty(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn timestamp(v: &Value, fallback: &str) -> String {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|s| {
            s.to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_else(|| fallback.into())
}
fn hidden(v: &Value) -> bool {
    v["isSidechain"] == true || v["isMeta"] == true || v["isCompactSummary"] == true
}
fn active_pi(records: &[Value]) -> Vec<&Value> {
    let entries: Vec<_> = records.iter().filter(|r| r["type"] != "session").collect();
    let Some(leaf) = entries.last() else {
        return entries;
    };
    let Some(mut id) = leaf["id"].as_str() else {
        return entries;
    };
    let by_id: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|r| r["id"].as_str().map(|id| (id, *r)))
        .collect();
    let mut branch = vec![];
    for _ in 0..entries.len() {
        let Some(entry) = by_id.get(id) else { break };
        branch.push(*entry);
        let Some(parent) = entry["parentId"].as_str().filter(|s| !s.is_empty()) else {
            break;
        };
        id = parent;
    }
    branch.reverse();
    branch
}
pub fn derive_title(source: &AgentSessionSource, text: &str) -> Option<String> {
    let mut request = text;
    if *source == AgentSessionSource::Codex {
        loop {
            let current = request.trim_start();
            let mut removed = None;
            for tag in [
                "recommended_plugins",
                "environment_context",
                "user_instructions",
                "INSTRUCTIONS",
                "in-app-browser-context",
            ] {
                if current.starts_with(&format!("<{tag}>"))
                    || current.starts_with(&format!("<{tag} "))
                {
                    let close = format!("</{tag}>");
                    if let Some(i) = current.find(&close) {
                        removed = Some(&current[i + close.len()..]);
                        break;
                    }
                }
            }
            if removed.is_none() && current.starts_with("# AGENTS.md instructions") {
                removed = Some(current.split_once('\n').map(|(_, r)| r).unwrap_or(""));
            }
            if removed.is_none() {
                removed = current.strip_prefix("## My request for Codex:");
            }
            let Some(next) = removed else { break };
            request = next;
        }
    }
    let title = request
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect::<String>();
    (!title.trim().is_empty()).then(|| title.trim().into())
}
pub fn parse_agent_session_transcript(
    source: AgentSessionSource,
    instance: ProviderInstanceId,
    fallback_id: &str,
    fallback_timestamp: &str,
    contents: &str,
) -> Option<SessionThread> {
    let lines = contents.strip_suffix('\n').unwrap_or(contents).split('\n');
    let mut records = vec![];
    for (index, line) in lines.enumerate() {
        if index >= 100_000 {
            return None;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if valid_record(&value) {
            records.push(value);
        }
    }
    parse_session_records(
        source,
        instance,
        fallback_id,
        fallback_timestamp,
        &records,
        0,
    )
}
pub fn parse_session_records(
    source: AgentSessionSource,
    instance: ProviderInstanceId,
    fallback_id: &str,
    fallback_timestamp: &str,
    records: &[Value],
    dropped: usize,
) -> Option<SessionThread> {
    let mut session = if source == AgentSessionSource::Codex {
        String::new()
    } else if source == AgentSessionSource::Pi {
        records
            .iter()
            .find(|r| r["type"] == "session")
            .and_then(|r| nonempty(&r["id"]))
            .unwrap_or_else(|| fallback_id.into())
    } else {
        fallback_id.into()
    };
    let mut title = None;
    let mut custom_title = None;
    let mut model = None;
    if source == AgentSessionSource::ClaudeAgent {
        for r in records.iter().filter(|r| !hidden(r)) {
            if let Some(id) = nonempty(&r["sessionId"]) {
                session = id;
            }
            if let Some(t) = nonempty(&r["aiTitle"]) {
                title = Some(t);
            }
            if r.get("customTitle").is_some() {
                custom_title = nonempty(&r["customTitle"]);
            }
            if let Some(m) = nonempty(&r["message"]["model"]).filter(|m| m != "<synthetic>") {
                model = Some(m);
            }
        }
    }
    if source == AgentSessionSource::Pi {
        title = records
            .iter()
            .rev()
            .find(|r| r["type"] == "session_info")
            .and_then(|r| nonempty(&r["name"]));
    }
    let mut suppressed = BTreeSet::new();
    if source == AgentSessionSource::Codex {
        let mut canonical = BTreeSet::new();
        let mut response = vec![];
        let finish = |canonical: &mut BTreeSet<String>,
                      response: &mut Vec<(usize, String, String)>,
                      suppressed: &mut BTreeSet<usize>| {
            let turns: BTreeSet<_> = response
                .iter()
                .filter(|(_, _, text)| canonical.contains(text))
                .map(|(_, id, _)| id.clone())
                .collect();
            suppressed.extend(
                response
                    .iter()
                    .filter(|(_, id, _)| turns.contains(id))
                    .map(|(i, _, _)| *i),
            );
            canonical.clear();
            response.clear();
        };
        for (i, r) in records.iter().enumerate() {
            if r["type"] == "response_item"
                && r["payload"]["type"] == "message"
                && r["payload"]["role"] == "assistant"
            {
                finish(&mut canonical, &mut response, &mut suppressed);
            }
            if r["type"] == "event_msg"
                && r["payload"]["type"] == "user_message"
                && let Some(t) = nonempty(&r["payload"]["message"])
            {
                canonical.insert(t);
            }
            if r["type"] == "response_item"
                && r["payload"]["type"] == "message"
                && r["payload"]["role"] == "user"
                && let Some(turn) =
                    nonempty(&r["payload"]["internal_chat_message_metadata_passthrough"]["turn_id"])
            {
                let t = text(&r["payload"]["content"]);
                if !t.is_empty() {
                    response.push((i, turn, t));
                }
            }
        }
        finish(&mut canonical, &mut response, &mut suppressed);
    }
    let entries = if source == AgentSessionSource::Pi {
        active_pi(records)
    } else {
        records.iter().collect()
    };
    let mut messages: VecDeque<(usize, SessionMessage, bool)> = VecDeque::new();
    let mut first_user = None;
    let mut derived = None;
    let mut count = 0usize;
    for (i, r) in entries.into_iter().enumerate() {
        let (role, message, is_response) = match source {
            AgentSessionSource::Pi => {
                if r["type"] != "message" {
                    continue;
                }
                (
                    r["message"]["role"].as_str(),
                    text(&r["message"]["content"]),
                    false,
                )
            }
            AgentSessionSource::ClaudeAgent => {
                if hidden(r) {
                    continue;
                }
                (r["type"].as_str(), text(&r["message"]["content"]), false)
            }
            AgentSessionSource::Codex => {
                if r["type"] == "session_meta" {
                    if session.is_empty() {
                        session = nonempty(&r["payload"]["id"])
                            .or_else(|| nonempty(&r["payload"]["session_id"]))
                            .unwrap_or_default();
                    }
                    continue;
                }
                if r["type"] == "turn_context" {
                    if let Some(m) = nonempty(&r["payload"]["model"]) {
                        model = Some(m);
                    }
                    continue;
                }
                if r["type"] == "event_msg" && r["payload"]["type"] == "user_message" {
                    let t = r["payload"]["message"].as_str().unwrap_or("").to_owned();
                    if t.trim().is_empty() {
                        continue;
                    }
                    if let Some(index) = messages.iter().rposition(|(_, m, response)| {
                        m.role == "assistant" || *response && m.text.trim() == t.trim()
                    }) && messages[index].1.role != "assistant"
                    {
                        let removed = messages.remove(index)?;
                        count = count.saturating_sub(1);
                        if first_user.as_ref().is_some_and(|(id, _)| *id == removed.0) {
                            first_user = None;
                        }
                    }
                    (Some("user"), t, false)
                } else {
                    if r["type"] != "response_item" || r["payload"]["type"] != "message" {
                        continue;
                    }
                    let role = r["payload"]["role"].as_str();
                    let t = text(&r["payload"]["content"]);
                    if role == Some("user") {
                        if suppressed.contains(&i) {
                            continue;
                        }
                        if messages
                            .iter()
                            .rev()
                            .take_while(|(_, m, _)| m.role != "assistant")
                            .any(|(_, m, response)| {
                                m.role == "user" && !*response && m.text.trim() == t.trim()
                            })
                        {
                            continue;
                        }
                    }
                    (role, t, role == Some("user"))
                }
            }
        };
        let Some(role) = role.filter(|role| matches!(*role, "user" | "assistant")) else {
            continue;
        };
        if message.is_empty() {
            continue;
        }
        let m = SessionMessage {
            role: role.into(),
            text: message,
            created_at: timestamp(&r["timestamp"], fallback_timestamp),
        };
        if role == "user" {
            if first_user.is_none() {
                first_user = Some((i, m.clone()));
            }
            if derived.is_none() {
                derived = derive_title(&source, &m.text);
            }
        }
        count += 1;
        messages.push_back((i, m, is_response));
        if messages.len() > 200 {
            messages.pop_front();
        }
    }
    if session.trim().is_empty() {
        return None;
    }
    let (first_id, first) = first_user?;
    if !messages.iter().any(|(id, _, _)| *id == first_id) {
        while messages.len() > 199 {
            messages.pop_front();
        }
        messages.push_front((first_id, first, false));
    }
    let messages: Vec<_> = messages.into_iter().map(|(_, m, _)| m).collect();
    Some(SessionThread {
        source,
        provider_instance_id: instance,
        provider_session_id: session,
        title: custom_title
            .or(title)
            .or(derived)
            .unwrap_or_else(|| "Imported thread".into()),
        model,
        created_at: messages
            .first()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| fallback_timestamp.into()),
        updated_at: fallback_timestamp.into(),
        omitted_message_count: count.saturating_add(dropped).saturating_sub(messages.len()),
        messages,
    })
}
pub(crate) fn valid_record(v: &Value) -> bool {
    let Some(object) = v.as_object() else {
        return false;
    };
    for key in [
        "type",
        "id",
        "name",
        "timestamp",
        "cwd",
        "sessionId",
        "aiTitle",
        "customTitle",
    ] {
        if object.get(key).is_some_and(|v| !v.is_string()) {
            return false;
        }
    }
    if object
        .get("parentId")
        .is_some_and(|v| !v.is_string() && !v.is_null())
    {
        return false;
    }
    for key in ["isSidechain", "isMeta", "isCompactSummary"] {
        if object.get(key).is_some_and(|v| !v.is_boolean()) {
            return false;
        }
    }
    if let Some(message) = object.get("message") {
        let Some(m) = message.as_object() else {
            return false;
        };
        for key in ["role", "model"] {
            if m.get(key).is_some_and(|v| !v.is_string()) {
                return false;
            }
        }
        if let Some(content) = m.get("content")
            && !(content.is_string() || valid_blocks(content))
        {
            return false;
        }
    }
    if let Some(payload) = object.get("payload") {
        let Some(p) = payload.as_object() else {
            return false;
        };
        for key in [
            "id",
            "session_id",
            "type",
            "role",
            "message",
            "model",
            "cwd",
        ] {
            if p.get(key).is_some_and(|v| !v.is_string()) {
                return false;
            }
        }
        if p.get("content").is_some_and(|v| !valid_blocks(v)) {
            return false;
        }
    }
    true
}
fn valid_blocks(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        a.iter().all(|v| {
            v.is_object()
                && ["type", "text"]
                    .into_iter()
                    .all(|k| v.get(k).is_none_or(Value::is_string))
        })
    })
}
