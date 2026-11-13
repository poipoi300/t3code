#![cfg(feature = "acp-adapter")]

use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_orchestration::{
    error::{Error, Result},
    ports::acp_replay::testkit::{
        provider_replay_fixtures::{fixture_cases, native::*},
        provider_replay_harness::ReplayFixtureHostFactory,
    },
    provider::{ProviderEvent, ProviderEventKind, TerminalOutcome},
};

pub const ROWS: &[(&str, &str, bool)] = &[
    ("R1", "opencode2_nested_background", true),
    ("R1-compat", "opencode2_background", true),
    ("R2c-compat", "opencode2_resume_after_restart", false),
    ("R2c-compat", "opencode2_interrupt", false),
    ("R3", "opencode2_simple", false),
    ("R3", "opencode2_tool_call", false),
];
fn flag(flag: Option<bool>) -> &'static str {
    match flag {
        Some(true) => "true",
        Some(false) => "false",
        None => "undefined",
    }
}
fn snapshot_failures(
    require_settled: bool,
    succeeded: bool,
    snapshots: impl IntoIterator<Item = (String, Option<bool>)>,
) -> (Vec<String>, String) {
    let mut last = Vec::<(String, Option<bool>)>::new();
    let mut ever_pending = false;
    for (id, pending) in snapshots {
        ever_pending |= pending == Some(true);
        if let Some((_, old)) = last.iter_mut().find(|(key, _)| key == &id) {
            *old = pending;
        } else {
            last.push((id, pending));
        }
    }
    let mut failures = Vec::new();
    if !succeeded {
        failures.push("fixture output assertions failed".into());
    }
    if require_settled {
        if !ever_pending {
            failures.push("no pendingBackgroundWork=true snapshot was published".into());
        }
        for (id, pending) in &last {
            if *pending != Some(false) {
                failures.push(format!(
                    "{id} ended on pendingBackgroundWork={}",
                    flag(*pending)
                ));
            }
        }
    } else if last.iter().any(|(_, pending)| *pending == Some(true)) {
        failures.push("a native thread ended with pending work".into());
    }
    let finals = last
        .iter()
        .map(|(id, pending)| format!("{id}={}", flag(*pending)))
        .collect::<Vec<_>>()
        .join(" ");
    (failures, finals)
}
fn tasks(value: &serde_json::Value) -> String {
    format!(
        "[{}]",
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|task| task["taskId"].as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn describe(event: &ProviderEvent) -> Result<Option<String>> {
    Ok(match &event.kind {
        ProviderEventKind::Thread {
            provider_thread,
            pending_background_work,
            ..
        } => {
            let thread = serde_json::to_value(provider_thread)?;
            Some(format!(
                "provider_thread.updated native={} pendingBackgroundWork={} pendingBackgroundTasks={}",
                thread["nativeThreadRef"]["nativeId"]
                    .as_str()
                    .unwrap_or("?"),
                flag(*pending_background_work),
                tasks(&thread["pendingBackgroundTasks"])
            ))
        }
        ProviderEventKind::Turn { provider_turn, .. } => {
            let turn = serde_json::to_value(provider_turn)?;
            Some(format!(
                "provider_turn.updated native={} status={}",
                turn["nativeTurnRef"]["nativeId"].as_str().unwrap_or("?"),
                turn["status"].as_str().unwrap_or("?")
            ))
        }
        ProviderEventKind::Subagent(subagent) => {
            let value = serde_json::to_value(subagent)?;
            Some(format!(
                "subagent.updated title={} status={}",
                value["title"].as_str().unwrap_or("?"),
                value["status"].as_str().unwrap_or("?")
            ))
        }
        ProviderEventKind::Terminal { outcome, .. } => Some(format!(
            "turn.terminal status={}",
            match outcome {
                TerminalOutcome::Completed { .. } => "completed",
                TerminalOutcome::Interrupted { .. } => "interrupted",
                TerminalOutcome::Cancelled { .. } => "cancelled",
                TerminalOutcome::Failed { .. } => "failed",
            }
        )),
        _ => None,
    })
}

pub async fn run_pending_snapshot_evidence(
    factory: &dyn FixtureAdapterFactory,
    host: &dyn ReplayFixtureHostFactory,
    transcript_path: Option<&Path>,
) -> Result<()> {
    for (id, name, settled) in ROWS {
        let (fixture, variant) = fixture_cases("opencode")?
            .into_iter()
            .find(|(fixture, _)| fixture.name == *name)
            .ok_or(Error::Unsupported("OpenCode2 evidence fixture"))?;
        let events = Arc::new(Mutex::new(Vec::<ProviderEvent>::new()));
        let captured = events.clone();
        let result = run_fixture_case(
            &fixture,
            variant,
            factory,
            host,
            FixtureObservations {
                provider_events: Some(Arc::new(move |event| {
                    captured
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(event.clone())
                })),
                ..Default::default()
            },
        )
        .await;
        let events = events.lock().unwrap_or_else(|e| e.into_inner());
        let mut lines = vec![format!("== {id} {name} (recorded fixture) ==")];
        for (index, event) in events.iter().enumerate() {
            if let Some(description) = describe(event)? {
                lines.push(format!("  adapter[{index}] {description}"));
            }
        }
        let snapshots = events.iter().filter_map(|event| match &event.kind {
            ProviderEventKind::Thread {
                provider_thread,
                pending_background_work,
                ..
            } => Some((
                provider_thread
                    .native_thread_ref
                    .as_ref()
                    .and_then(|reference| reference.native_id.as_ref())
                    .map_or("?", |id| id.0.as_str())
                    .to_owned(),
                *pending_background_work,
            )),
            _ => None,
        });
        let (failures, finals) = snapshot_failures(*settled, result.is_ok(), snapshots);
        lines.push(format!("  final snapshot per native thread: {finals}"));
        if let Ok(result) = &result {
            for event in &result.domain_events {
                let value = serde_json::to_value(event)?;
                if value["type"] == "provider-thread.updated" {
                    lines.push(format!(
                        "  domain provider-thread.updated native={} pendingBackgroundTasks={}",
                        value["payload"]["nativeThreadRef"]["nativeId"]
                            .as_str()
                            .unwrap_or("?"),
                        tasks(&value["payload"]["pendingBackgroundTasks"])
                    ));
                }
            }
        }
        lines.push(format!(
            "  {id} {name}: {}",
            if failures.is_empty() {
                "PASS".into()
            } else {
                format!("FAIL ({})", failures.join("; "))
            }
        ));
        if let Some(path) = transcript_path {
            use std::io::Write;
            writeln!(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?,
                "{}",
                lines.join("\n")
            )?;
        }
        if !failures.is_empty() {
            return Err(Error::Service {
                operation: "OpenCode2 pending snapshots",
                detail: failures.join("; "),
            });
        }
    }
    Ok(())
}

#[test]
fn pending_snapshot_evidence_keeps_native_thread_identity_and_observed_flags() {
    let (failures, finals) = snapshot_failures(
        true,
        true,
        [
            ("child".into(), Some(true)),
            ("root".into(), Some(false)),
            ("child".into(), Some(false)),
        ],
    );
    assert!(failures.is_empty());
    assert_eq!(finals, "child=false root=false");
    assert_eq!(
        snapshot_failures(true, true, [("root".into(), None)]).0,
        [
            "no pendingBackgroundWork=true snapshot was published",
            "root ended on pendingBackgroundWork=undefined"
        ]
    );
    assert_eq!(
        snapshot_failures(false, true, [("child".into(), Some(true))]).0,
        ["a native thread ended with pending work"]
    );
    assert!(
        snapshot_failures(false, true, [("root".into(), None)])
            .0
            .is_empty()
    );
    assert_eq!(
        snapshot_failures(false, false, []).0,
        ["fixture output assertions failed"]
    );
}
