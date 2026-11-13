//! Wire-shaped demo data: what a server would send. Decoded by `t3-contracts`, never by the UI.

use serde_json::{Value, json};

pub const DEMO_PAIRING_URL: &str = "https://demo.t3.codes/pair#token=demo-pairing-token";
pub const FIRST_THREAD: &str = "thread-shared-ui";

const PROJECTS: [(&str, &str, &str); 2] = [
    ("project-t3code", "t3code", "/Users/demo/code/t3code"),
    ("project-nsh", "nsh-spikes", "/Users/demo/code/nsh-spikes"),
];

const THREADS: [(&str, &str, &str); 9] = [
    (
        FIRST_THREAD,
        "project-t3code",
        "Build the shared Rust UI slice",
    ),
    (
        "thread-timeline-perf",
        "project-t3code",
        "Timeline virtualisation with long code blocks",
    ),
    (
        "thread-composer",
        "project-t3code",
        "Rich composer: chips, IME and paste",
    ),
    (
        "thread-pairing",
        "project-t3code",
        "Pairing flow on phone widths",
    ),
    (
        "thread-terminal",
        "project-t3code",
        "Terminal emulator fidelity",
    ),
    (
        "thread-theme",
        "project-t3code",
        "Port theme tokens from index.css",
    ),
    ("thread-uia", "project-nsh", "UIA Text pattern for Windows"),
    (
        "thread-scrollbar",
        "project-nsh",
        "Scrollbar keeps the GPUI frame loop alive",
    ),
    (
        "thread-selection",
        "project-nsh",
        "Select all across recycled rows",
    ),
];

pub fn timestamp(seconds: u64) -> String {
    let minutes = seconds / 60;
    format!(
        "2026-11-13T{:02}:{:02}:{:02}.000Z",
        12 + (minutes / 60) % 12,
        minutes % 60,
        seconds % 60
    )
}

fn thread_json(id: &str, project: &str, title: &str, updated: &str) -> Value {
    json!({
        "id": id, "projectId": project, "title": title,
        "providerInstanceId": "codex",
        "modelSelection": {"instanceId": "codex", "model": "gpt-5.4"},
        "runtimeMode": "full-access", "interactionMode": "default",
        "branch": null, "worktreePath": null, "activeProviderThreadId": null,
        "lineage": {"rootThreadId": id, "parentThreadId": null, "relationshipToParent": null},
        "forkedFrom": null, "createdBy": "user", "creationSource": "web",
        "createdAt": updated, "updatedAt": updated,
        "archivedAt": null, "settledOverride": null, "settledAt": null,
        "lastVisitedAt": null, "deletedAt": null
    })
}

pub fn shell() -> Value {
    let threads: Vec<Value> = THREADS
        .iter()
        .enumerate()
        .map(|(index, (id, project, title))| {
            let mut thread = thread_json(id, project, title, &timestamp(3600 - index as u64 * 240));
            if let Some(object) = thread.as_object_mut() {
                object.extend(
                    json!({
                        "latestRunId": null, "activeRunId": null, "status": "idle",
                        "pendingRuntimeRequest": null, "latestVisibleMessage": null,
                        "latestUserMessageAt": null, "hasActionableProposedPlan": false,
                        "itemCount": 0, "visibleItemCount": 0
                    })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                );
            }
            thread
        })
        .collect();
    let projects: Vec<Value> = PROJECTS
        .iter()
        .map(|(id, title, root)| {
            json!({
                "id": id, "title": title, "workspaceRoot": root, "repositoryIdentity": null,
                "defaultModelSelection": null, "scripts": [],
                "createdAt": "2026-11-01T09:00:00.000Z", "updatedAt": "2026-11-13T09:00:00.000Z"
            })
        })
        .collect();
    json!({
        "schemaVersion": 1, "snapshotSequence": 1,
        "threads": threads, "archivedThreads": [], "projects": projects
    })
}

fn base_item(thread: &str, ordinal: u64, kind: &str, status: &str, at: &str) -> Value {
    json!({
        "id": format!("{thread}-item-{ordinal}"), "type": kind, "threadId": thread,
        "runId": null, "nodeId": null, "providerThreadId": null, "providerTurnId": null,
        "nativeItemRef": null, "parentItemId": null, "ordinal": ordinal, "status": status,
        "title": null, "startedAt": at, "completedAt": at, "updatedAt": at
    })
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    base
}

pub fn user_message(thread: &str, ordinal: u64, text: &str, at: &str) -> Value {
    merge(
        base_item(thread, ordinal, "user_message", "completed", at),
        json!({
            "createdBy": "user", "creationSource": "web",
            "messageId": format!("{thread}-message-{ordinal}"),
            "inputIntent": "turn_start", "text": text, "attachments": []
        }),
    )
}

pub fn assistant_message(
    thread: &str,
    ordinal: u64,
    text: &str,
    streaming: bool,
    at: &str,
) -> Value {
    let status = if streaming { "running" } else { "completed" };
    merge(
        base_item(thread, ordinal, "assistant_message", status, at),
        json!({
            "messageId": format!("{thread}-message-{ordinal}"),
            "text": text, "streaming": streaming
        }),
    )
}

fn row(thread: &str, position: u64, item: Value) -> Value {
    json!({
        "position": position, "visibility": "local", "sourceThreadId": thread,
        "sourceItemId": item["id"], "item": item
    })
}

/// What each thread holds: the messages `rust/shells/reference/ui-reference.e2e.ts` sends to
/// the dev replay agent and the replies its recordings play, so the reference captures of the
/// web app show the same conversation (minus the work log rows the slice does not draw yet).
pub fn turns(thread: &str) -> Vec<(bool, String)> {
    if thread == FIRST_THREAD {
        return vec![
            (true, SHOWCASE_PROMPT.to_owned()),
            (false, SHOWCASE_INTRO.to_owned()),
            (false, SHOWCASE_DONE.to_owned()),
        ];
    }
    let title = THREADS
        .iter()
        .find(|(id, _, _)| *id == thread)
        .map_or("this thread", |(_, _, title)| title);
    vec![
        (true, format!("Let's work on {title}.")),
        (false, "first fixture turn complete".to_owned()),
        (true, FOLLOW_UP.to_owned()),
        (false, "second fixture turn complete".to_owned()),
    ]
}

/// [`turns`] as the visible rows a thread opens with.
pub fn conversation(thread: &str) -> Vec<Value> {
    turns(thread)
        .iter()
        .enumerate()
        .map(|(index, (user, text))| {
            let ordinal = index as u64 + 1;
            let at = timestamp(60 * ordinal);
            let item = if *user {
                user_message(thread, ordinal, text, &at)
            } else {
                assistant_message(thread, ordinal, text, false, &at)
            };
            row(thread, index as u64, item)
        })
        .collect()
}

/// The projection a thread opens with, holding `rows`.
pub fn projection(thread: &str, rows: Vec<Value>) -> Value {
    let (project, title) = THREADS
        .iter()
        .find(|(id, _, _)| *id == thread)
        .map_or(("project-t3code", "Thread"), |(_, project, title)| {
            (*project, *title)
        });
    let items: Vec<Value> = rows.iter().map(|row| row["item"].clone()).collect();
    json!({
        "thread": thread_json(thread, project, title, &timestamp(3600)),
        "runs": [], "attempts": [], "nodes": [], "subagents": [], "providerSessions": [],
        "providerThreads": [], "providerTurns": [], "runtimeRequests": [], "messages": [],
        "plans": [], "turnItems": items, "checkpointScopes": [], "checkpoints": [],
        "contextHandoffs": [], "contextTransfers": [], "visibleTurnItems": rows,
        "updatedAt": timestamp(3600)
    })
}

pub fn reply_to(text: &str) -> String {
    format!(
        "Done. I sent \"{}\" through the client core and streamed this reply into the last row.\n\n\
         - The store published one thread change per chunk.\n\
         - Only this row's memo changed; the other rows kept their `Rc`.\n\
         - The same code ran on desktop, web and Android.",
        text.trim()
    )
}

/// The reply split into word-sized chunks, as a provider streams it.
pub fn chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if character == ' ' || character == '\n' {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

pub const SHOWCASE_PROMPT: &str = "Give me a short tour.";
pub const FOLLOW_UP: &str = "Show me the selector that keeps streaming cheap.";

/// `apps/server/scripts/dev-replay/showcase_transcript.ndjson`'s two assistant messages.
pub const SHOWCASE_INTRO: &str =
    "The project has a single math module. I will add a `clamp` helper beside `sum`.";
pub const SHOWCASE_DONE: &str = "## Done

I added `clamp` next to `sum` and the tests pass.

- Reads the manifest first
- Asks before it writes
- Streams a terminal result

```ts
clamp(12, 0, 10); // 10
clamp(-3, 0, 10); // 0
```

| Step | Tool | Result |
| --- | --- | --- |
| Read | `read` | 1 file |
| Edit | `edit` | +4 lines |
| Test | `execute` | 12 passed |

This turn is a replay. It costs nothing, so send it again as often as you need.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The assistant messages a replay transcript plays, in order.
    fn replayed(path: &str) -> Vec<String> {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let text = std::fs::read_to_string(repo.join(path)).expect("the replay transcript");
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|frame| {
                let update = &frame["frame"]["params"]["update"];
                (update["sessionUpdate"] == "agent_message_chunk")
                    .then(|| update["content"]["text"].as_str().map(str::to_owned))
                    .flatten()
            })
            .collect()
    }

    #[test]
    fn the_reference_scenario_seeds_the_same_projects_threads_and_messages() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../shells/reference/ui-reference.e2e.ts");
        let scenario = std::fs::read_to_string(path).expect("the reference scenario");
        let quoted = |text: &str| format!("\"{text}\"");
        for (_, title, _) in PROJECTS {
            assert!(scenario.contains(&quoted(title)), "project {title}");
        }
        for (index, (_, project, title)) in THREADS.iter().enumerate() {
            let project = PROJECTS
                .iter()
                .find(|(id, _, _)| id == project)
                .map(|(_, name, _)| *name)
                .expect("project");
            let row = format!("[{}, {}],", quoted(project), quoted(title));
            assert!(scenario.contains(&row), "thread {index} {title}");
        }
        assert!(scenario.contains(&quoted(SHOWCASE_PROMPT)));
        assert!(scenario.contains(&quoted(FOLLOW_UP)));
        assert!(scenario.contains("`Let's work on ${title}.`"));
        assert_eq!(
            turns("thread-terminal")[0].1,
            "Let's work on Terminal emulator fidelity."
        );
    }

    #[test]
    fn replies_are_what_the_replay_recordings_play() {
        let showcase = replayed("apps/server/scripts/dev-replay/showcase_transcript.ndjson");
        assert_eq!(
            showcase,
            [
                format!(
                    "{SHOWCASE_INTRO}

"
                ),
                SHOWCASE_DONE.to_owned()
            ]
        );
        let multi = replayed(
            "apps/server/src/orchestration-v2/testkit/fixtures/multi_turn/registry_transcript.ndjson",
        );
        let other = turns("thread-terminal");
        assert_eq!(multi, [other[1].1.clone(), other[3].1.clone()]);
    }

    #[test]
    fn chunks_reassemble_to_the_reply() {
        let reply = reply_to("hello");
        assert_eq!(chunks(&reply).concat(), reply);
        assert!(chunks(&reply).len() > 10);
    }
}
