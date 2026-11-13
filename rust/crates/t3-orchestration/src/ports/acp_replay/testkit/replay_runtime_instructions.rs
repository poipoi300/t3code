use crate::testing::replay::{ReplayEntry, ReplayTranscript};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn key(value: Option<&Value>) -> String {
    value
        .map(|value| format!("json:{value}"))
        .unwrap_or_else(|| "undefined".into())
}
fn text(part: &Value) -> Option<&str> {
    (part["type"] == "text")
        .then(|| part["text"].as_str())
        .flatten()
}
struct PendingPrompt {
    session: String,
    notes: Option<String>,
    compact: bool,
    compactions_at_send: u64,
}

pub fn materialize_replay_transcript_runtime_instructions(
    mut transcript: ReplayTranscript,
    driver: &str,
    model: &str,
    build: impl Fn(&str, &str) -> String,
) -> ReplayTranscript {
    let harness = match driver {
        "cursor" => "Cursor",
        "grok" => "Grok",
        "kiro" => "Kiro",
        "acpRegistry" => "acpRegistry",
        "fx" => "fx",
        _ => return transcript,
    };
    let instructions = build(harness, model);
    let mut session_models = BTreeMap::<String, String>::new();
    let mut delivered = BTreeMap::<String, String>::new();
    let mut counts = BTreeMap::<String, u64>::new();
    let mut completed = BTreeSet::<(String, String)>::new();
    let mut open = BTreeMap::<String, BTreeSet<String>>::new();
    let mut pending = None::<PendingPrompt>;
    let mut kiro_compact = None::<String>;
    fn compact(
        session: &str,
        id: Option<&str>,
        delivered: &mut BTreeMap<String, String>,
        counts: &mut BTreeMap<String, u64>,
        completed: &mut BTreeSet<(String, String)>,
    ) {
        if let Some(id) = id
            && !completed.insert((session.into(), id.into()))
        {
            return;
        }
        delivered.remove(session);
        *counts.entry(session.into()).or_default() += 1;
    }
    for entry in &mut transcript.entries {
        match entry {
            ReplayEntry::EmitInbound(entry) => {
                let frame = &entry.frame;
                if frame["kind"] == "response"
                    && frame["method"] == "session/prompt"
                    && let Some(prompt) = pending.take()
                {
                    let success = frame.get("error").is_none()
                        && !matches!(
                            frame["result"]["stopReason"].as_str(),
                            Some("cancelled" | "refusal" | "error")
                        );
                    if success
                        && open
                            .get(&prompt.session)
                            .is_some_and(|open| !open.is_empty())
                    {
                        open.remove(&prompt.session);
                        compact(
                            &prompt.session,
                            None,
                            &mut delivered,
                            &mut counts,
                            &mut completed,
                        );
                    }
                    if success && prompt.compact {
                        compact(
                            &prompt.session,
                            None,
                            &mut delivered,
                            &mut counts,
                            &mut completed,
                        );
                    } else if success
                        && counts.get(&prompt.session).copied().unwrap_or_default()
                            == prompt.compactions_at_send
                        && let Some(notes) = prompt.notes
                    {
                        delivered.insert(prompt.session, notes);
                    }
                }
                if frame["kind"] == "response"
                    && frame["method"] == "_kiro/session/compact"
                    && let Some(session) = kiro_compact.take()
                    && frame["result"]["success"] == true
                {
                    compact(&session, None, &mut delivered, &mut counts, &mut completed);
                }
                let update = &frame["params"]["update"];
                if frame["method"] == "session/update"
                    && update["sessionUpdate"] == "compaction_update"
                {
                    let session = key(frame["params"].get("sessionId"));
                    let id = key(update.get("compactionId"));
                    if update["status"] == "in_progress" {
                        open.entry(session.clone()).or_default().insert(id);
                    } else if let Some(open) = open.get_mut(&session) {
                        open.remove(&id);
                    }
                    if update["status"] == "completed" {
                        compact(
                            &session,
                            update["compactionId"].as_str(),
                            &mut delivered,
                            &mut counts,
                            &mut completed,
                        );
                    }
                }
            }
            ReplayEntry::ExpectOutbound(entry) => {
                let frame = &mut entry.frame;
                if driver == "cursor"
                    && frame["type"] == "run.start"
                    && let Some(message) = frame["message"].as_str()
                {
                    if !message.ends_with(&instructions) {
                        frame["message"] = json!(format!("{message}\n\n{instructions}"));
                    }
                    continue;
                }
                if matches!(
                    frame["method"].as_str(),
                    Some("session/close" | "initialize")
                ) {
                    delivered.clear();
                    completed.clear();
                    open.clear();
                    pending = None;
                    kiro_compact = None;
                    continue;
                }
                let selected = match frame["method"].as_str() {
                    Some("session/set_config_option") if frame["params"]["configId"] == "model" => {
                        frame["params"]["value"].as_str()
                    }
                    Some("session/set_model") => frame["params"]["modelId"].as_str(),
                    _ => None,
                };
                if let Some(selected) = selected {
                    session_models.insert(key(frame["params"].get("sessionId")), selected.into());
                    continue;
                }
                if frame["method"] == "_kiro/session/compact"
                    && frame["params"].get("sessionId").is_some()
                {
                    kiro_compact = Some(key(frame["params"].get("sessionId")));
                    continue;
                }
                if frame["method"] != "session/prompt" {
                    continue;
                }
                let session = key(frame["params"].get("sessionId"));
                let Some(prompt) = frame["params"]["prompt"].as_array() else {
                    continue;
                };
                let compactions_at_send = counts.get(&session).copied().unwrap_or_default();
                if let Some(first) = prompt.first().and_then(text).map(str::trim)
                    && first.starts_with('/')
                {
                    let compact = driver == "grok"
                        && prompt.len() == 1
                        && first.strip_prefix("/compact").is_some_and(|rest| {
                            rest.is_empty() || rest.starts_with(char::is_whitespace)
                        });
                    pending = Some(PendingPrompt {
                        session,
                        notes: None,
                        compact,
                        compactions_at_send,
                    });
                    continue;
                }
                let expected = session_models
                    .get(&session)
                    .map(|model| build(harness, model))
                    .unwrap_or_else(|| instructions.clone());
                let last = prompt.last().and_then(text);
                let recorded =
                    last.is_some_and(|last| last == expected || last.starts_with("<runtime_info>"));
                let resend = !recorded && delivered.get(&session) != Some(&expected);
                pending = Some(PendingPrompt {
                    session,
                    notes: (recorded || resend).then(|| expected.clone()),
                    compact: false,
                    compactions_at_send,
                });
                if resend {
                    frame["params"]["prompt"]
                        .as_array_mut()
                        .expect("fixture structure validated above")
                        .push(json!({"type":"text","text":expected}));
                }
            }
            ReplayEntry::RuntimeExit(_) => {}
        }
    }
    transcript
}
