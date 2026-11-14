#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::services::{config::cacheable_config, terminal::*};
use t3_contracts::{server::ServerConfig, terminal::TerminalAttachStreamEvent};

#[test]
fn history_and_new_readers_skip_cut_control_sequences_while_live_readers_keep_every_byte() {
    for (history, budget) in [
        ("prefix\u{1b}[31mOK", 6),
        ("prefix\u{1b}]0;HIDDEN\u{7}OK", 10),
        ("prefix\u{1b}]0;HIDDEN\u{7}OK", 4),
    ] {
        let mut output = TerminalOutputState::default();
        output.reset(history, budget);
        assert_eq!(output.text(), "OK");
        output.reset("", budget);
        let mut readers = vec![(TerminalOutputCursor::default(), String::new())];
        for ch in history.chars() {
            output.append(&ch.to_string(), budget);
            readers.push((TerminalOutputCursor::default(), String::new()));
            for (cursor, written) in &mut readers {
                let update = output.read(*cursor);
                match &update {
                    TerminalOutputUpdate::Reset { data, .. } => *written = data.clone(),
                    TerminalOutputUpdate::Append { data, .. } => written.push_str(data),
                    TerminalOutputUpdate::None(_) => {}
                }
                *cursor = update.cursor();
            }
        }
        assert_eq!(readers[0].1, history);
        for (_, written) in readers {
            assert!(
                written.contains('\u{1b}') || ["", "O", "OK", "K"].contains(&written.as_str()),
                "unsafe replay: {written:?}"
            );
        }
        assert_eq!(output.text(), "OK");
    }
}

#[test]
fn clear_appends_erasure_and_preserves_mode_history_for_fresh_emulators() {
    let mut generations = AttachGenerations::default();
    let mut buffer = generations.seed();
    let mode_history = "\u{1b}[?1049h\u{1b}[?2004hhello";
    buffer.output.reset(mode_history, 100);
    let cursor = buffer.output.read(TerminalOutputCursor::default()).cursor();
    let event: TerminalAttachStreamEvent = serde_json::from_value(
        json!({"type":"cleared","threadId":"thread","terminalId":"terminal"}),
    )
    .unwrap();
    buffer.apply(&event, 5, &mut generations);
    let erase = "\u{1b}[H\u{1b}[2J\u{1b}[3J";
    assert!(
        matches!(buffer.output.read(cursor), TerminalOutputUpdate::Append { data, .. } if data == erase)
    );
    assert!(
        matches!(buffer.output.read(TerminalOutputCursor::default()), TerminalOutputUpdate::Reset { data, .. } if data == format!("{mode_history}{erase}"))
    );
    assert_eq!(buffer.output.text(), "");
    buffer.output.append("next", 100);
    assert_eq!(buffer.output.text(), "next");
}

#[test]
fn config_cache_excludes_each_connections_clock_sample() {
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    config.server_time = Some(serde_json::from_value(json!("2026-11-14T00:00:00.000Z")).unwrap());
    let first = cacheable_config(&config);
    assert!(first.server_time.is_none());
    config.server_time = Some(serde_json::from_value(json!("2026-11-14T00:01:00.000Z")).unwrap());
    assert_eq!(first, cacheable_config(&config));
}

#[test]
fn terminal_attach_recovery_reuses_the_session_and_reconciles_the_resumed_snapshot() {
    use futures::{FutureExt, StreamExt, channel::mpsc};
    use std::rc::Rc;
    use t3_client::{
        ClientError,
        environment::SessionIdentity,
        services::{runtime::Services, server::drive_terminal_attach, sessions::ServiceSession},
        testing::{FakePlatform, RpcScript, ScriptedRpc},
        types::{EnvironmentId, Generation, ThreadId, ThreadKey},
    };
    let platform = FakePlatform::default();
    let services = Services::default();
    let rpc = Rc::new(ScriptedRpc::default());
    let input: t3_contracts::terminal::TerminalAttachInput =
        serde_json::from_value(json!({"threadId":"thread","terminalId":"terminal","cwd":"/repo"}))
            .unwrap();
    let snapshot = |history| json!({"type":"snapshot","snapshot":{"threadId":"thread","terminalId":"terminal","cwd":"/repo","worktreePath":null,"status":"running","pid":123,"history":history,"exitCode":null,"exitSignal":null,"label":"Terminal","updatedAt":"2026-11-14T00:00:00Z"}});
    for (resumed, replies) in [
        (
            false,
            vec![
                Ok(snapshot("old")),
                Err(ClientError::InvalidPayload("RPC response schema".into())),
            ],
        ),
        (
            true,
            vec![
                Ok(snapshot("restored")),
                Ok(
                    json!({"type":"output","threadId":"thread","terminalId":"terminal","data":"next"}),
                ),
            ],
        ),
    ] {
        let mut request = json!(input);
        request["maxHistoryBytes"] = json!(DEFAULT_MAX_TERMINAL_BUFFER_BYTES);
        if resumed {
            request["resume"] = json!(true);
        }
        rpc.scripts.borrow_mut().push_back(RpcScript {
            method: "terminal.attach".into(),
            input: request,
            replies,
        });
    }
    let environment = EnvironmentId("drift".into());
    let (sessions, changes) = mpsc::unbounded();
    sessions
        .unbounded_send(Some(ServiceSession {
            identity: SessionIdentity {
                environment: environment.clone(),
                generation: Generation(1),
                account: None,
                dpop_thumbprint: None,
            },
            endpoint: rpc.clone(),
        }))
        .unwrap();
    let key = TerminalKey {
        thread: ThreadKey {
            environment: environment.clone(),
            thread: ThreadId("thread".into()),
        },
        terminal: "terminal".into(),
    };
    let mut driver = Box::pin(drive_terminal_attach(
        &services,
        &platform,
        environment,
        changes.boxed_local(),
        input,
    ));
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(services.terminal_buffer(&key).unwrap().output.text(), "old");
    platform.advance(1000);
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(
        services.terminal_buffer(&key).unwrap().output.text(),
        "restorednext"
    );
    assert!(rpc.scripts.borrow().is_empty());
    drop(driver);
    assert_eq!(platform.pending_timers(), 0);
}
