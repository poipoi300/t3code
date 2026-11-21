#![cfg(feature = "claude-adapter")]

use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_claude_cli::{Session, SessionOptions, launch::LaunchOptions};
use t3_contracts::{
    base_schemas::*, provider_instance::ProviderInstanceId,
    provider_policy::ProviderApprovalDecision,
};
use t3_orchestration::{
    adapters::claude::*,
    ports::claude::adapters::claude_adapter_v2_testkit::*,
    provider::*,
    testing::{ManualClock, SequenceIds},
};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio_util::sync::CancellationToken;

struct Peer {
    read: Lines<BufReader<ReadHalf<DuplexStream>>>,
    write: WriteHalf<DuplexStream>,
}
impl Peer {
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).expect("encode peer frame");
        bytes.push(b'\n');
        self.write.write_all(&bytes).await.expect("send peer frame");
    }
    async fn read(&mut self) -> Value {
        serde_json::from_str(
            &self
                .read
                .next_line()
                .await
                .expect("read peer frame")
                .expect("peer frame"),
        )
        .expect("decode peer frame")
    }
}
async fn running() -> (Arc<dyn SessionRuntime>, EventSubscription, Peer) {
    running_with_host(ClaudeHostOptions::default()).await
}
async fn running_with_host(
    host: ClaudeHostOptions,
) -> (Arc<dyn SessionRuntime>, EventSubscription, Peer) {
    running_with_input(
        host,
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        fixture_turn_input().expect("fixture turn input"),
    )
    .await
}
async fn running_with_input(
    host: ClaudeHostOptions,
    launch: LaunchOptions,
    input: TurnInput,
) -> (Arc<dyn SessionRuntime>, EventSubscription, Peer) {
    let (client, server) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(client);
    let (peer_read, peer_write) = tokio::io::split(server);
    let connector = Arc::new(PipeConnector::new(Session::connect(
        read,
        write,
        SessionOptions::default(),
    )));
    let runtime = fixture_runtime(host, launch, connector, &input, CancellationToken::new()).await;
    let subscription = runtime
        .subscribe()
        .await
        .expect("subscribe to Claude fixture");
    let mut peer = Peer {
        read: BufReader::new(peer_read).lines(),
        write: peer_write,
    };
    let handle = runtime.clone();
    let start = tokio::spawn(async move { handle.start_turn(input).await });
    let init = peer.read().await;
    assert_eq!(init["request"]["subtype"], "initialize");
    assert!(
        init["request"].get("systemPrompt").is_none(),
        "default chat sessions retain Claude Code's system prompt"
    );
    peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":init["request_id"],"response":{}}})).await;
    let user = peer.read().await;
    assert_eq!(user["type"], "user");
    start
        .await
        .expect("join fixture start")
        .expect("start fixture turn");
    (runtime, subscription, peer)
}

async fn fixture_runtime(
    host: ClaudeHostOptions,
    launch: LaunchOptions,
    connector: Arc<dyn ClaudeConnector>,
    input: &TurnInput,
    cancellation: CancellationToken,
) -> Arc<dyn SessionRuntime> {
    let adapter = ClaudeAdapterV2::new(ClaudeAdapterOptions {
        instance_id: ProviderInstanceId("claudeAgent".into()),
        launch,
        connector,
        clock: Arc::new(ManualClock::new(FIXTURE_DATE)),
        ids: Arc::new(SequenceIds::new("runtime")),
        new_native_session_id: Arc::new(|| "00000000-0000-4000-8000-000000000001".into()),
        mcp_servers: BTreeMap::new(),
        transcript_root: None,
        attachments: Arc::new(DirectoryClaudeAttachments::default()),
        host,
    });
    adapter
        .open_session(
            OpenSession {
                thread_id: input.thread_id.clone(),
                provider_session_id: ProviderSessionId("session".into()),
                model_selection: input.model_selection.clone(),
                runtime_policy: input.runtime_policy.clone(),
                prompt_suggestions: None,
                resume_from_session: None,
                initial_native_thread_id: None,
                initial_provider_item_identity_version: None,
            },
            cancellation,
        )
        .await
        .expect("open Claude fixture session")
}

#[tokio::test]
async fn claude_lifecycle_close_mid_turn_projects_interrupted() {
    let (runtime, _events, _peer) = running().await;
    let thread = fixture_turn_input()
        .expect("fixture turn input")
        .provider_thread;
    runtime.close().await.expect("close Claude mid-turn");
    let snapshot = runtime
        .snapshot(thread)
        .await
        .expect("snapshot closed session");
    assert_eq!(snapshot.provider_turns.len(), 1);
    assert_eq!(
        snapshot.provider_turns[0].status,
        t3_contracts::orchestration_v2::ProviderTurnStatus::Interrupted
    );
}

#[tokio::test]
async fn claude_lifecycle_interrupt_followed_by_eof_is_interrupted() {
    let (runtime, mut events, mut peer) = running().await;
    let provider_turn_id = loop {
        let event = events
            .events
            .recv()
            .await
            .expect("provider event")
            .expect("valid event");
        if let ProviderEventKind::Turn { provider_turn, .. } = event.kind {
            let t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(id) = provider_turn.id
            else {
                panic!("fixture should emit a provider turn ID");
            };
            break id;
        }
    };
    let input = InterruptInput {
        provider_thread: fixture_turn_input()
            .expect("fixture turn input")
            .provider_thread,
        provider_turn_id,
        request_runtime_restart: false,
    };
    let captured = runtime.clone();
    let interrupt = tokio::spawn(async move { captured.interrupt_turn(input).await });
    let request = peer.read().await;
    assert_eq!(request["request"]["subtype"], "interrupt");
    peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}})).await;
    interrupt
        .await
        .expect("join interrupt")
        .expect("interrupt Claude");
    drop(peer);
    loop {
        let event = events
            .events
            .recv()
            .await
            .expect("provider event")
            .expect("valid event");
        if let ProviderEventKind::Terminal {
            outcome,
            disposition,
            ..
        } = event.kind
        {
            assert!(
                matches!(outcome, TerminalOutcome::Interrupted { .. }),
                "{outcome:?}"
            );
            assert_ne!(disposition, ThreadDisposition::Broken);
            break;
        }
    }
    runtime.close().await.expect("close Claude fixture session");
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_initialize_rejection_preserves_protocol_operation() {
    let rejection = INITIALIZE_CLI
        .replace(r#""subtype":"success""#, r#""subtype":"error""#)
        .replace(
            r#""response":{}"#,
            r#""error":"fixture rejected initialization""#,
        );
    let script = format!("{rejection}\nwhile IFS= read -r line; do :; done");
    let (_directory, runtime, input) = process_fixture(&script, false).await;
    let error = runtime
        .start_turn(input)
        .await
        .expect_err("initialization rejected");
    assert!(
        matches!(error, ProviderError::Operation { operation: ProviderOperation::Stream, ref detail } if detail == "Claude control request failed"),
        "{error:?}"
    );
    runtime.close().await.expect("close Claude fixture session");
}

#[cfg(unix)]
async fn process_fixture(
    script: &str,
    with_mcp: bool,
) -> (tempfile::TempDir, Arc<dyn SessionRuntime>, TurnInput) {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("create Claude fixture directory");
    let cli = directory.path().join("fake-claude");
    std::fs::write(&cli, format!("#!/bin/sh\nset -eu\n{script}\n"))
        .expect("write Claude fixture executable");
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
        .expect("make Claude fixture executable");
    let mut input = fixture_turn_input().expect("build Claude fixture turn");
    input.runtime_policy.cwd = Some(directory.path().to_owned());
    let mut launch = LaunchOptions {
        cli_path: Some(cli.to_string_lossy().into_owned()),
        ..Default::default()
    };
    if with_mcp {
        launch.mcp_servers.insert(
            "fixture".into(),
            json!({"type":"http","url":"http://127.0.0.1:1/mcp"}),
        );
    }
    let runtime = fixture_runtime(
        ClaudeHostOptions::default(),
        launch,
        Arc::new(ProcessConnector),
        &input,
        CancellationToken::new(),
    )
    .await;
    (directory, runtime, input)
}

#[cfg(unix)]
const INITIALIZE_CLI: &str = r#"
IFS= read -r initialize
id=${initialize#*'"request_id":"'}
id=${id%%'"'*}
printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
"#;

#[cfg(unix)]
async fn assert_startup_failure(script: &str, with_mcp: bool, expected: &str) {
    let (_directory, runtime, input) = process_fixture(script, with_mcp).await;
    let result = runtime.start_turn(input).await;
    assert!(
        matches!(&result, Err(ProviderError::Operation { operation: ProviderOperation::Open, detail }) if detail == expected),
        "expected startup failure {expected:?}, got {result:?}",
    );
    let _events = runtime
        .subscribe()
        .await
        .expect("the parent session remains live");
    runtime.close().await.expect("close Claude fixture session");
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_immediate_eof_before_any_protocol_message_fails_startup() {
    // The original composition fixture logged its launch and exited unsuccessfully.
    assert_startup_failure(
        "printf 'launched\n' > fixture.log; exit 1",
        false,
        "Claude Code process exited with code 1.",
    )
    .await;
    assert_startup_failure("exit 0", false, CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL).await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_eof_during_mcp_registration_fails_startup() {
    assert_startup_failure(
        &format!("{INITIALIZE_CLI}\nIFS= read -r registration\nexit 23"),
        true,
        "Claude Code process exited with code 23.",
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_stderr_only_root_refusal_is_curated() {
    let stderr = "token=sk-fixture\n--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons\nat /private/fixture/settings.json";
    let (_directory, runtime, input) =
        process_fixture(&format!("printf '%s\n' '{stderr}' >&2\nexit 1"), false).await;
    let error = runtime
        .start_turn(input)
        .await
        .expect_err("Claude startup must fail");
    assert!(
        matches!(&error, ProviderError::Operation { operation: ProviderOperation::Open, detail } if detail == "Claude Code exited before it started: --dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons"),
        "{error:?}",
    );
    runtime.close().await.expect("close Claude fixture session");

    assert_startup_failure(
        "printf 'token=sk-fixture\\n' >&2; exit 1",
        false,
        "Claude Code process exited with code 1.",
    )
    .await;
}

#[cfg(unix)]
async fn assert_mid_turn_failure(exit: &str, expected: &str, code: Option<&str>) {
    use t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass;

    let script = format!(
        r#"{INITIALIZE_CLI}
IFS= read -r prompt
printf '%s\n' '{{"type":"assistant","uuid":"partial","message":{{"id":"partial","role":"assistant","content":[{{"type":"text","text":"Partial reply"}}]}}}}'
printf 'token=sk-fixture\n' >&2
{exit}
"#,
    );
    let (_directory, runtime, input) = process_fixture(&script, false).await;
    let mut events = runtime
        .subscribe()
        .await
        .expect("subscribe to Claude fixture");
    runtime
        .start_turn(input)
        .await
        .expect("start Claude fixture turn");
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let event = events
                .events
                .recv()
                .await
                .expect("Claude event stream remains open")
                .expect("Claude provider event");
            if let ProviderEventKind::Terminal { outcome, .. } = event.kind {
                break outcome;
            }
        }
    })
    .await;
    runtime.close().await.expect("close Claude fixture session");
    let outcome = terminal.expect("process loss must settle the live turn");
    let TerminalOutcome::Failed { failure, .. } = outcome else {
        panic!("expected a failed turn after {exit}, got {outcome:?}");
    };
    assert_eq!(
        failure.class,
        OrchestrationV2ProviderFailureClass::TransportError
    );
    assert_eq!(failure.message.0, expected);
    assert_eq!(failure.code.as_ref().map(|code| code.0.as_str()), code);
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_clean_eof_mid_turn_is_a_transport_failure() {
    assert_mid_turn_failure("exit 0", CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL, None).await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_nonzero_exit_mid_turn_is_a_transport_failure() {
    assert_mid_turn_failure(
        "exit 37",
        "Claude Code process exited with code 37.",
        Some("process_exited_nonzero"),
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_killed_mid_turn_is_a_transport_failure() {
    assert_mid_turn_failure(
        "kill -KILL $$",
        "Claude Code process terminated by signal SIGKILL.",
        Some("process_killed_by_signal"),
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_leader_exit_settles_with_inherited_pipes_open() {
    assert_mid_turn_failure(
        "mkfifo held\n(IFS= read -r release < held) &\nexit 38",
        "Claude Code process exited with code 38.",
        Some("process_exited_nonzero"),
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_leader_exit_preserves_buffered_result() {
    let script = format!(
        r#"{INITIALIZE_CLI}
IFS= read -r prompt
mkfifo held
(IFS= read -r release < held) &
printf '%s\n' '{{"type":"result","subtype":"success","uuid":"finished","result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{{"input_tokens":1,"output_tokens":1}}}}'
exit 38
"#,
    );
    let (_directory, runtime, input) = process_fixture(&script, false).await;
    let mut events = runtime
        .subscribe()
        .await
        .expect("subscribe to Claude fixture");
    runtime.start_turn(input).await.expect("start Claude turn");
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let event = events
                .events
                .recv()
                .await
                .expect("provider event")
                .expect("valid event");
            if let ProviderEventKind::Terminal { outcome, .. } = event.kind {
                break outcome;
            }
        }
    })
    .await;
    runtime.close().await.expect("close Claude fixture session");
    let outcome = terminal.expect("buffered result must settle the turn");
    assert!(
        matches!(outcome, TerminalOutcome::Completed { .. }),
        "{outcome:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn claude_lifecycle_stdout_eof_while_cli_is_live_has_no_signal_code() {
    assert_mid_turn_failure(
        "mkfifo held\nexec 1>&-\nIFS= read -r release < held",
        CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL,
        None,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn claude_lifecycle_broken_stdin_settles_even_while_stdout_stays_open() {
    use t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass;

    let (reader, mut output) = tokio::io::duplex(65536);
    let (writer, input_pipe) = tokio::io::duplex(65536);
    let connector = Arc::new(PipeConnector::new(Session::connect(
        reader,
        writer,
        SessionOptions::default(),
    )));
    let input = fixture_turn_input().expect("build Claude fixture turn");
    let runtime = fixture_runtime(
        ClaudeHostOptions::default(),
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        connector,
        &input,
        CancellationToken::new(),
    )
    .await;
    let mut events = runtime
        .subscribe()
        .await
        .expect("subscribe to Claude fixture");
    let captured = runtime.clone();
    let start = tokio::spawn(async move { captured.start_turn(input).await });
    let mut input_pipe = BufReader::new(input_pipe).lines();
    let line = input_pipe
        .next_line()
        .await
        .expect("read Claude initialize request")
        .expect("Claude initialize request");
    let initialize: Value = serde_json::from_str(&line).expect("decode Claude initialize request");
    assert_eq!(initialize["request"]["subtype"], "initialize");

    // The control frame was read, but the prompt will hit a closed stdin.
    drop(input_pipe);
    let reply = json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":{}}});
    output
        .write_all(format!("{reply}\n").as_bytes())
        .await
        .expect("send Claude initialize reply");
    assert!(start.await.expect("join Claude startup task").is_err());

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events
                .events
                .recv()
                .await
                .expect("Claude event stream remains open")
                .expect("Claude provider event");
            if let ProviderEventKind::Terminal { outcome, .. } = event.kind {
                break outcome;
            }
        }
    })
    .await
    .expect("broken stdin must settle before the stdout pipe closes");
    let TerminalOutcome::Failed { failure, .. } = outcome else {
        panic!("expected a terminal failure, got {outcome:?}");
    };
    assert_eq!(
        failure.class,
        OrchestrationV2ProviderFailureClass::TransportError
    );
    assert_eq!(failure.code, None);
    drop(output);
    runtime.close().await.expect("close Claude fixture session");
}

#[tokio::test(start_paused = true)]
async fn claude_lifecycle_broken_startup_stdin_finishes_while_stdout_stays_open() {
    let (reader, output) = tokio::io::duplex(65536);
    let (writer, closed_peer) = tokio::io::duplex(65536);
    drop(closed_peer);
    let connector = Arc::new(PipeConnector::new(Session::connect(
        reader,
        writer,
        SessionOptions::default(),
    )));
    let input = fixture_turn_input().expect("build Claude fixture turn");
    let runtime = fixture_runtime(
        ClaudeHostOptions::default(),
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        connector,
        &input,
        CancellationToken::new(),
    )
    .await;
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(5), runtime.start_turn(input)).await;
    let error = result
        .expect("the query owns a bounded startup drain")
        .expect_err("broken startup stdin must fail");
    assert!(
        matches!(error, ProviderError::Operation { operation: ProviderOperation::Open, ref detail } if detail == CLAUDE_QUERY_ENDED_BEFORE_TURN_DETAIL),
        "{error:?}"
    );
    drop(output);
    runtime.close().await.expect("close Claude fixture session");
}

#[tokio::test(start_paused = true)]
async fn claude_lifecycle_initialize_timeout_and_parent_cancellation_are_distinct() {
    use std::time::Duration;

    for cancel in [false, true] {
        let (client, server) = tokio::io::duplex(65536);
        let (read, write) = tokio::io::split(client);
        let (peer_read, peer_write) = tokio::io::split(server);
        let connector = Arc::new(PipeConnector::new(Session::connect(
            read,
            write,
            SessionOptions::default(),
        )));
        let mut launch = LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        };
        launch.mcp_servers.insert(
            "fixture".into(),
            json!({"type":"http","url":"http://127.0.0.1:1/mcp"}),
        );
        let input = fixture_turn_input().expect("build Claude fixture turn");
        let parent = CancellationToken::new();
        let runtime = fixture_runtime(
            ClaudeHostOptions::default(),
            launch,
            connector,
            &input,
            parent.clone(),
        )
        .await;
        let captured = runtime.clone();
        let start = tokio::spawn(async move { captured.start_turn(input).await });
        let mut peer = Peer {
            read: BufReader::new(peer_read).lines(),
            write: peer_write,
        };
        assert_eq!(peer.read().await["request"]["subtype"], "initialize");
        if cancel {
            parent.cancel();
            assert!(matches!(
                start.await.expect("join Claude startup task"),
                Err(ProviderError::Cancelled)
            ));
        } else {
            tokio::time::advance(Duration::from_secs(300)).await;
            let error = start.await.expect("join Claude startup task").unwrap_err();
            assert!(
                matches!(&error, ProviderError::Operation { operation: ProviderOperation::Open, detail } if detail.starts_with("Claude timed out while starting up.")),
                "{error:?}",
            );
            assert!(runtime.subscribe().await.is_ok());
        }
        runtime.close().await.expect("close Claude fixture session");
    }
}

#[tokio::test]
async fn auto_compact_threshold_respects_disabled_settings_and_environment_after_compaction() {
    use t3_claude_cli::launch::Settings;
    for (user, environment, launch_settings, expected) in [
        (
            json!({"autoCompactWindow":400000,"autoCompactEnabled":false}),
            json!({}),
            json!({}),
            Value::Null,
        ),
        (
            json!({"autoCompactWindow":400000}),
            json!({"DISABLE_AUTO_COMPACT":"1"}),
            json!({}),
            Value::Null,
        ),
        (
            json!({"autoCompactWindow":400000}),
            json!({"DISABLE_AUTO_COMPACT":" True "}),
            json!({}),
            Value::Null,
        ),
        (
            json!({"autoCompactWindow":400000}),
            json!({"DISABLE_COMPACT":"yes"}),
            json!({}),
            Value::Null,
        ),
        (
            json!({"autoCompactWindow":400000}),
            json!({"DISABLE_AUTO_COMPACT":"0"}),
            json!({}),
            json!(367000),
        ),
        (
            json!({"autoCompactWindow":400000}),
            json!({"DISABLE_AUTO_COMPACT":"1"}),
            json!({"env":{"DISABLE_AUTO_COMPACT":"0"}}),
            json!(367000),
        ),
        (
            json!({"autoCompactWindow":400000,"autoCompactEnabled":false}),
            json!({}),
            json!({"autoCompactEnabled":true}),
            json!(367000),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            serde_json::to_vec(&user).unwrap(),
        )
        .unwrap();
        let host = ClaudeHostOptions {
            settings_directory: Some(directory.path().to_owned()),
            auto_compact_window: Some("400000".into()),
            ..Default::default()
        };
        let mut input = fixture_turn_input().unwrap();
        input.model_selection.model = "claude-opus-5-5".into();
        input.runtime_policy.cwd = Some(directory.path().join("workspace"));
        let launch = LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            env: environment
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                .collect(),
            settings: Some(Settings::Json(launch_settings)),
            ..Default::default()
        };
        let (runtime, mut events, mut peer) = running_with_input(host, launch, input).await;
        for (id, input_tokens) in [("before", 100), ("after", 20)] {
            if id == "after" {
                peer.send(json!({"type":"system","subtype":"compact_boundary","uuid":"compact","compact_metadata":{"trigger":"manual","pre_tokens":110}})).await;
            }
            peer.send(json!({"type":"assistant","uuid":id,"parent_tool_use_id":null,"message":{"id":id,"model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":id}],"usage":{"input_tokens":input_tokens,"output_tokens":10}}})).await;
        }
        peer.send(json!({"type":"result","subtype":"success","uuid":"complete","result":"after","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":20,"output_tokens":10}})).await;
        let mut thresholds = Vec::new();
        loop {
            match events.events.recv().await.unwrap().unwrap().kind {
                ProviderEventKind::Turn { provider_turn, .. } => {
                    let turn = serde_json::to_value(provider_turn).unwrap();
                    if turn["tokenUsage"].is_object() {
                        thresholds.push(turn["tokenUsage"]["autoCompactThreshold"].clone());
                    }
                }
                ProviderEventKind::Terminal { .. } => break,
                _ => {}
            }
        }
        runtime.close().await.unwrap();
        assert!(
            thresholds.len() >= 2,
            "usage snapshots must be emitted across compaction"
        );
        assert!(
            thresholds.iter().all(|value| value == &expected),
            "user {user}, environment {environment}: {thresholds:?}, expected {expected}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn process_connector_enters_cwd_in_the_child_and_missing_cwd_exits_125() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let cli = directory.path().join("fake-claude");
    std::fs::write(&cli, "#!/bin/sh\npwd -P > \"$0.cwd\"\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let workspace = directory.path().join("work space");
    std::fs::create_dir(&workspace).unwrap();
    let linked = directory.path().join("linked-workspace");
    std::os::unix::fs::symlink(&workspace, &linked).unwrap();
    for (cwd, expected) in [(linked, 0), (directory.path().join("missing"), 125)] {
        let options = LaunchOptions {
            cli_path: Some(cli.to_string_lossy().into_owned()),
            cwd: Some(cwd),
            ..Default::default()
        };
        let mut connection = ProcessConnector
            .connect(&options, CancellationToken::new())
            .await
            .expect("missing cwd must fail in the child, not at spawn");
        assert_eq!(
            connection
                .process
                .as_mut()
                .unwrap()
                .wait()
                .await
                .unwrap()
                .code(),
            Some(expected)
        );
        connection.session.shutdown().await.unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(directory.path().join("fake-claude.cwd")).unwrap(),
        format!("{}\n", std::fs::canonicalize(workspace).unwrap().display())
    );
}

#[tokio::test]
async fn failed_parallel_preparation_keeps_the_query_alive_and_compiles_selection_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let directory = tempfile::tempdir().unwrap();
    let append = directory.path().join("append.txt");
    std::fs::write(&append, "File instructions\n").unwrap();
    let compiled = Arc::new(AtomicUsize::new(0));
    let calls = compiled.clone();
    let host = ClaudeHostOptions {
        compile_model: Some(Arc::new(move |selection| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(ClaudeCompiledModel {
                model: selection.model.clone(),
                effort: None,
                settings: json!({}),
            })
        })),
        ..Default::default()
    };
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.cwd = Some(directory.path().to_owned());
    let launch = LaunchOptions {
        cli_path: Some("fixture-claude".into()),
        extra_args: vec![(
            "append-system-prompt-file".into(),
            Some("append.txt".into()),
        )],
        ..Default::default()
    };
    let (runtime, mut events, mut peer) = running_with_input(host, launch, input.clone()).await;
    let result = |id| json!({"type":"result","subtype":"success","uuid":id,"result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}});
    peer.send(result("first")).await;
    loop {
        if matches!(
            events.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    std::fs::remove_file(&append).unwrap();
    input.run_id = RunId("second-run".into());
    input.attempt_id = RunAttemptId("second-attempt".into());
    input.root_node_id = NodeId("second-root".into());
    input.run_ordinal = 2;
    input.provider_turn_ordinal = 2;
    input.message.message_id = MessageId("00000000-0000-4000-8000-000000000010".into());
    assert!(matches!(
        runtime.start_turn(input.clone()).await,
        Err(ProviderError::Operation {
            operation: ProviderOperation::Open,
            ..
        })
    ));
    peer.send(control(
        "query-still-live",
        "Bash",
        json!({"command":"pwd"}),
    ))
    .await;
    assert_eq!(
        peer.read().await["response"]["request_id"],
        "query-still-live"
    );
    std::fs::write(append, "File instructions\n").unwrap();
    let handle = runtime.clone();
    let mut start = tokio::spawn(async move { handle.start_turn(input).await });
    let mut started = false;
    let prompt = tokio::select! {
        result = &mut start => {result.unwrap().unwrap(); started = true; peer.read().await},
        frame = peer.read() => frame,
    };
    assert_eq!(
        prompt["type"], "user",
        "valid retry must reuse the old initialized native query"
    );
    if !started {
        start.await.unwrap().unwrap();
    }
    peer.send(result("second")).await;
    loop {
        if let ProviderEventKind::Terminal { run_ordinal, .. } =
            events.events.recv().await.unwrap().unwrap().kind
        {
            assert_eq!(run_ordinal, 2);
            break;
        }
    }
    assert_eq!(
        compiled.load(Ordering::SeqCst),
        3,
        "one catalog/policy preparation per attempted turn"
    );
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn native_background_wake_waits_for_an_app_run_and_never_offers_a_second_prompt() {
    use t3_contracts::orchestration_v2::{OrchestrationV2Actor, OrchestrationV2CreationSource};
    let (send, mut requests) = tokio::sync::mpsc::channel(4);
    let host = ClaudeHostOptions {
        continuation_requests: Some(send),
        ..Default::default()
    };
    let (runtime, mut subscription, mut peer) = running_with_host(host).await;
    peer.send(json!({"type":"system","subtype":"background_tasks_changed","uuid":"roster","tasks":[{"task_id":"background","task_type":"local_bash","description":"Build"}]})).await;
    let result = |id: &str, text: &str| json!({"type":"result","subtype":"success","uuid":id,"result":text,"is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}});
    peer.send(result("root-result", "STARTED")).await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    peer.send(json!({"type":"system","subtype":"background_tasks_changed","uuid":"empty-roster","tasks":[]})).await;
    peer.send(json!({"type":"system","subtype":"task_notification","task_id":"background","status":"completed","summary":"Build complete","uuid":"notice"})).await;
    peer.send(control("wake-barrier", "Bash", json!({"command":"pwd"})))
        .await;
    let barrier = peer.read().await;
    assert_eq!(barrier["response"]["response"]["behavior"], "deny");
    assert!(
        requests.try_recv().is_err(),
        "a completed opaque task does not prove that a native wake began"
    );
    assert!(
        !runtime
            .extensions()
            .unwrap()
            .pending_background_work(None, true)
            .await
            .unwrap()
    );
    peer.send(json!({"type":"assistant","uuid":"wake-assistant","parent_tool_use_id":null,"message":{"id":"wake-api","role":"assistant","content":[{"type":"text","text":"WAKE_DONE"}]}})).await;
    let request = requests.recv().await.unwrap();
    assert_eq!(
        serde_json::to_value(request.notification).unwrap()["summary"],
        "Command \"Build\" finished"
    );
    assert_eq!(request.detail.as_deref(), Some("Build complete"));
    assert!(
        runtime
            .extensions()
            .unwrap()
            .pending_background_work(None, true)
            .await
            .unwrap()
    );
    let mut wake = fixture_turn_input().unwrap();
    wake.run_id = RunId("wake-run".into());
    wake.run_ordinal = 2;
    wake.provider_turn_ordinal = 2;
    wake.message.created_by = OrchestrationV2Actor::Agent;
    wake.message.creation_source = Some(OrchestrationV2CreationSource::Provider);
    runtime.start_turn(wake).await.unwrap();
    peer.send(result("wake-result", "WAKE_DONE")).await;
    let mut saw_wake = false;
    loop {
        match subscription.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Message(message) if message.text == "WAKE_DONE" => {
                assert_eq!(serde_json::to_value(message.run_id).unwrap(), "wake-run");
                saw_wake = true;
            }
            ProviderEventKind::Terminal { run_ordinal, .. } => {
                assert_eq!(run_ordinal, 2);
                break;
            }
            _ => {}
        }
    }
    assert!(saw_wake);
    assert!(
        !runtime
            .extensions()
            .unwrap()
            .pending_background_work(None, true)
            .await
            .unwrap()
    );
    runtime.close().await.unwrap();
    assert!(
        peer.read.next_line().await.unwrap().is_none(),
        "native wake must not send another user prompt"
    );
}

#[tokio::test]
async fn monitor_progress_wakes_keep_later_wakes_separate_with_owned_or_absorbed_results() {
    use t3_contracts::orchestration_v2::{OrchestrationV2Actor, OrchestrationV2CreationSource};
    fn result(id: &str, text: &str) -> Value {
        json!({"type":"result","subtype":"success","uuid":id,"result":text,"is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}})
    }
    fn input(ordinal: u64, provider: bool) -> TurnInput {
        let mut input = fixture_turn_input().unwrap();
        input.run_id = RunId(format!("monitor-run-{ordinal}"));
        input.attempt_id = RunAttemptId(format!("monitor-attempt-{ordinal}"));
        input.root_node_id = NodeId(format!("monitor-root-{ordinal}"));
        input.run_ordinal = ordinal;
        input.provider_turn_ordinal = ordinal;
        input.message.message_id =
            MessageId(format!("00000000-0000-4000-8000-{:012}", 100 + ordinal));
        input.message.text = if provider {
            "Background task completed."
        } else {
            "Any survivors yet?"
        }
        .into();
        if provider {
            input.message.created_by = OrchestrationV2Actor::Agent;
            input.message.creation_source = Some(OrchestrationV2CreationSource::Provider);
        }
        input
    }
    async fn terminal(events: &mut EventSubscription, ordinal: u64) {
        loop {
            if let ProviderEventKind::Terminal { run_ordinal, .. } =
                events.events.recv().await.unwrap().unwrap().kind
            {
                assert_eq!(run_ordinal, ordinal);
                return;
            }
        }
    }
    fn buffered_terminal(events: &mut EventSubscription, ordinal: u64) -> Vec<String> {
        let mut texts = Vec::new();
        while let Ok(event) = events.events.try_recv() {
            match event.unwrap().kind {
                ProviderEventKind::Message(message)
                    if serde_json::to_value(&message.role).unwrap() == "assistant" =>
                {
                    texts.push(message.text)
                }
                ProviderEventKind::Terminal {
                    run_ordinal,
                    outcome,
                    ..
                } => {
                    assert_eq!(run_ordinal, ordinal);
                    assert!(matches!(outcome, TerminalOutcome::Completed { .. }));
                    return texts;
                }
                _ => {}
            }
        }
        panic!("a buffered or absorbed wake must settle before start_turn returns");
    }
    async fn barrier(peer: &mut Peer) {
        peer.send(control("monitor-barrier", "Bash", json!({"command":"pwd"})))
            .await;
        assert_eq!(
            peer.read().await["response"]["request_id"],
            "monitor-barrier"
        );
    }
    for (monitor_absorbed, later_absorbed, scheduled) in [
        (false, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
    ] {
        let (sender, mut requests) = tokio::sync::mpsc::channel(4);
        let (runtime, mut events, mut peer) = running_with_host(ClaudeHostOptions {
            continuation_requests: Some(sender),
            ..Default::default()
        })
        .await;
        peer.send(json!({"type":"assistant","uuid":"launch-monitor","message":{"id":"launch-monitor","role":"assistant","content":[{"type":"tool_use","id":"monitor-call","name":"Monitor","input":{"description":"line monitor","command":"./mutate.sh"}}]}})).await;
        peer.send(json!({"type":"system","subtype":"task_started","task_id":"monitor-task","tool_use_id":"monitor-call","description":"line monitor","task_type":"local_bash","is_backgrounded":true})).await;
        peer.send(json!({"type":"user","uuid":"monitor-started","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"monitor-call","content":"Monitor started. You will be notified on each event."}]},"tool_use_result":{"taskId":"monitor-task","persistent":false}})).await;
        if scheduled {
            peer.send(json!({"type":"assistant","uuid":"schedule","message":{"id":"schedule","role":"assistant","content":[{"type":"tool_use","id":"schedule-call","name":"ScheduleWakeup","input":{"seconds":0}}]}})).await;
            let due = t3_orchestration::ports::decisions::orchestrator::epoch_millis(FIXTURE_DATE)
                .unwrap();
            peer.send(json!({"type":"user","uuid":"scheduled","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"schedule-call","content":"Scheduled"}]},"tool_use_result":{"scheduledFor":due}})).await;
        }
        peer.send(result("root-end", "STARTED")).await;
        terminal(&mut events, 1).await;
        peer.send(json!({"type":"system","subtype":"init","uuid":"monitor-start"}))
            .await;
        peer.send(json!({"type":"assistant","uuid":"monitor-progress","message":{"id":"monitor-progress","role":"assistant","content":[{"type":"text","text":"Covered. Waiting for the rest."}]}})).await;
        barrier(&mut peer).await;
        let first = requests
            .try_recv()
            .expect("Monitor event must offer a continuation");
        let notification = serde_json::to_value(first.notification).unwrap();
        if scheduled {
            assert!(
                notification.is_null(),
                "a due scheduled wake is not Monitor progress"
            );
            runtime.close().await.unwrap();
            continue;
        }
        assert_eq!(notification["source"]["kind"], "monitor");
        assert_eq!(notification["outcome"], "updated");
        assert_eq!(notification["summary"], "Monitor reported new output");
        let mut settled = 1;
        if monitor_absorbed {
            settled += 1;
            runtime.start_turn(input(settled, false)).await.unwrap();
            assert_eq!(peer.read().await["type"], "user");
            peer.send(result("monitor-absorbed", "Covered. Waiting for the rest."))
                .await;
            terminal(&mut events, settled).await;
        } else {
            let mut end = result("monitor-own-end", "Covered. Waiting for the rest.");
            end["origin"] = json!({"kind":"task-notification"});
            peer.send(end).await;
            barrier(&mut peer).await;
        }
        peer.send(json!({"type":"system","subtype":"init","uuid":"later-start"}))
            .await;
        peer.send(json!({"type":"assistant","uuid":"scheduled-answer","message":{"id":"scheduled-answer","role":"assistant","content":[{"type":"text","text":"Scheduled answer."}]}})).await;
        barrier(&mut peer).await;
        if later_absorbed {
            settled += 1;
            runtime.start_turn(input(settled, false)).await.unwrap();
            assert_eq!(peer.read().await["type"], "user");
            peer.send(result("later-absorbed", "No survivors yet."))
                .await;
            terminal(&mut events, settled).await;
        } else {
            peer.send(result("later-own-end", "Scheduled answer."))
                .await;
            barrier(&mut peer).await;
        }
        runtime.start_turn(input(settled + 1, true)).await.unwrap();
        let texts = buffered_terminal(&mut events, settled + 1);
        assert!(!texts.iter().any(|text| text == "Scheduled answer."));
        let second = requests
            .try_recv()
            .expect("later wake needs its own continuation");
        let notification = serde_json::to_value(second.notification).unwrap();
        assert_eq!(
            notification["source"]["kind"],
            if later_absorbed {
                json!("monitor")
            } else {
                Value::Null
            }
        );
        runtime.start_turn(input(settled + 2, true)).await.unwrap();
        assert!(
            buffered_terminal(&mut events, settled + 2)
                .iter()
                .any(|text| text == "Scheduled answer.")
        );
        assert!(requests.try_recv().is_err());
        runtime.close().await.unwrap();
        assert!(
            peer.read.next_line().await.unwrap().is_none(),
            "provider continuations must not send a new prompt"
        );
    }
}
async fn request(subscription: &mut EventSubscription) -> RuntimeRequestId {
    loop {
        let event = subscription
            .events
            .recv()
            .await
            .expect("provider event")
            .expect("provider event result");
        if let ProviderEventKind::RuntimeRequest { request, .. } = event.kind
            && request.status == t3_contracts::orchestration_v2::RuntimeRequestStatus::Pending
        {
            return serde_json::from_value(
                serde_json::to_value(request.id).expect("encode request ID"),
            )
            .expect("decode request ID");
        }
    }
}
fn control(id: &str, tool: &str, input: Value) -> Value {
    json!({"type":"control_request","request_id":id,"request":{"subtype":"can_use_tool","tool_name":tool,"input":input,"tool_use_id":format!("tool-{id}")}})
}
async fn callback_reply_or_pending(
    runtime: &Arc<dyn SessionRuntime>,
    subscription: &mut EventSubscription,
    peer: &mut Peer,
) -> (bool, Value) {
    let observed = tokio::select! {
        reply = peer.read() => Ok(reply),
        id = request(subscription) => Err(id),
    };
    match observed {
        Ok(reply) => (false, reply["response"]["response"].clone()),
        Err(id) => {
            runtime
                .respond(RuntimeResponse {
                    request_id: id,
                    decision: Some(ProviderApprovalDecision::Decline),
                    feedback: None,
                    answers: None,
                })
                .await
                .expect("decline unexpected pending request");
            (true, peer.read().await["response"]["response"].clone())
        }
    }
}

#[tokio::test]
async fn permission_callback_denies_native_removal_safety_without_a_user_approval() {
    for reason in [
        "Dangerous rm operation not ruled out — the shell may read this command differently than it was parsed",
        "Dangerous rm operation on possibly-empty variable path: $TARGET\nUse a literal absolute path instead.",
        "This shell -c script runs rm and could not be checked",
        "A permission rule denies a command in this shell -c script",
        "dAnGeRoUs rmdir operation on a possibly-empty path",
    ] {
        let (runtime, mut subscription, mut peer) = running().await;
        let mut frame = control("removal", "Bash", json!({"command":"rm -rf \"$TARGET\""}));
        frame["request"]["decision_reason"] = json!(format!("\u{1b}[33m{reason}\u{1b}[39m"));
        peer.send(frame).await;
        let (asked, reply) =
            callback_reply_or_pending(&runtime, &mut subscription, &mut peer).await;
        runtime.close().await.unwrap();
        assert!(
            !asked,
            "native safety veto became a user-overridable approval: {reason}"
        );
        assert_eq!(reply["behavior"], "deny");
        assert_eq!(
            reply["message"],
            format!(
                "Claude Code's removal safety check denied this command; the user did not reject it. The command did not run. Do not bypass the check by splitting the removal, putting it in a script, or using another tool or shell. If the warning below supplies a safe rewrite, use it and let the same check evaluate it. Otherwise continue the rest of the task without the removal and tell the user what you needed to delete and why. Warning: {reason}"
            )
        );
        assert_eq!(reply["toolUseID"], "tool-removal");
        assert!(reply.get("decisionClassification").is_none());
    }
}

#[tokio::test]
async fn permission_callback_plan_without_approvals_denies_before_asking() {
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.interaction_mode =
        t3_contracts::provider_policy::ProviderInteractionMode::Plan;
    input.runtime_policy.approval_policy = Some(json!("never"));
    let (runtime, mut subscription, mut peer) = running_with_input(
        ClaudeHostOptions::default(),
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        input,
    )
    .await;
    peer.send(control(
        "plan-approval",
        "Write",
        json!({"file_path":"a","content":"x"}),
    ))
    .await;
    let (asked, reply) = callback_reply_or_pending(&runtime, &mut subscription, &mut peer).await;
    runtime.close().await.unwrap();
    assert!(!asked, "approvals-disabled Plan mode requested an approval");
    assert_eq!(reply["behavior"], "deny");
    assert_eq!(
        reply["message"],
        "This tool requires approval in Plan mode, but approvals are disabled for this thread."
    );
}

#[tokio::test]
async fn permission_callback_auto_without_callback_allows_without_asking() {
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.runtime_mode = t3_contracts::provider_policy::RuntimeMode::Auto;
    let (runtime, mut subscription, mut peer) = running_with_input(
        ClaudeHostOptions::default(),
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        input,
    )
    .await;
    peer.send(control(
        "auto",
        "Write",
        json!({"file_path":"a","content":"x"}),
    ))
    .await;
    let (asked, reply) = callback_reply_or_pending(&runtime, &mut subscription, &mut peer).await;
    runtime.close().await.unwrap();
    assert!(!asked, "Auto mode without a callback requested approval");
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(
        reply["updatedInput"],
        json!({"file_path":"a","content":"x"})
    );
    assert!(reply.get("decisionClassification").is_none());
}

#[tokio::test]
async fn permission_callback_keeps_forwarded_ask_rules_and_native_downgrades_pending() {
    for (tool, reason, matched, downgrade) in [
        ("Bash", "Dangerous rm operation", true, false),
        ("Bash", "Dangerous rm operation", false, true),
        ("Write", "Dangerous rm operation", false, false),
        (
            "Bash",
            "Dangerous rmdirSuffix is not a removal warning",
            false,
            false,
        ),
    ] {
        let (runtime, mut subscription, mut peer) = running().await;
        if downgrade {
            peer.send(
                json!({"type":"system","subtype":"status","status":null,"permissionMode":"auto"}),
            )
            .await;
        }
        let mut frame = control("still-ask", tool, json!({"command":"pwd"}));
        frame["request"]["decision_reason"] = json!(reason);
        if matched {
            frame["request"]["matched_ask_rule"] =
                json!({"toolName":"Bash","ruleContent":"*","source":"userSettings"});
        }
        peer.send(frame).await;
        let (asked, reply) =
            callback_reply_or_pending(&runtime, &mut subscription, &mut peer).await;
        runtime.close().await.unwrap();
        assert!(
            asked,
            "a forwarded request lost its user decision: {tool}, matched={matched}, downgrade={downgrade}"
        );
        assert_eq!(reply["decisionClassification"], "user_reject");
    }
}

#[tokio::test]
async fn permission_callback_forwards_unknown_session_suggestions_through_native_response() {
    let (runtime, mut subscription, mut peer) = running().await;
    let mut frame = control("future", "Bash", json!({"command":"pwd"}));
    frame["request"]["permission_suggestions"] = json!([
        {"type":"futureScopedRule","scope":{"command":"pwd"},"opaque":17,"destination":"userSettings"},
        {"type":"setMode","mode":"futureRestrictedMode","destination":"projectSettings"}
    ]);
    peer.send(frame).await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::AcceptForSession),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    runtime.close().await.unwrap();
    assert_eq!(
        reply["response"]["response"]["updatedPermissions"],
        json!([
            {"type":"futureScopedRule","scope":{"command":"pwd"},"opaque":17,"destination":"session"},
            {"type":"setMode","mode":"futureRestrictedMode","destination":"session"}
        ])
    );
    assert_eq!(reply["response"]["response"]["toolUseID"], "tool-future");
}

#[tokio::test]
async fn permission_callback_max_runtime_mode_filters_native_set_mode_suggestions() {
    use t3_contracts::provider_policy::RuntimeMode;
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.runtime_mode = RuntimeMode::ApprovalRequired;
    input.runtime_policy.max_runtime_mode = Some(RuntimeMode::ApprovalRequired);
    let (runtime, mut subscription, mut peer) = running_with_input(
        ClaudeHostOptions::default(),
        LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        input,
    )
    .await;
    let mut frame = control("managed", "Bash", json!({"command":"pwd"}));
    frame["request"]["permission_suggestions"] = json!([
        {"type":"setMode","mode":"bypassPermissions","destination":"userSettings"},
        {"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"pwd"}],"behavior":"allow","destination":"projectSettings"}
    ]);
    peer.send(frame).await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::AcceptForSession),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    let mut only_mode = control("managed-only-mode", "Bash", json!({"command":"pwd"}));
    only_mode["request"]["permission_suggestions"] = json!([
        {"type":"setMode","mode":"bypassPermissions","destination":"userSettings"}
    ]);
    peer.send(only_mode).await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::AcceptForSession),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let filtered = peer.read().await;
    runtime.close().await.unwrap();
    assert!(
        filtered["response"]["response"]
            .get("updatedPermissions")
            .is_none()
    );
    assert_eq!(
        filtered["response"]["response"]["decisionClassification"],
        "user_temporary"
    );
    assert_eq!(
        reply["response"]["response"]["updatedPermissions"],
        json!([
            {"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"pwd"}],"behavior":"allow","destination":"session"}
        ])
    );
}

#[tokio::test]
async fn full_access_callback_requires_explicit_decision_and_preserves_tool_id() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control("permission", "Bash", json!({"command":"pwd"})))
        .await;
    let id = request(&mut subscription).await;
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id.clone(),
                decision: None,
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Decline),
            feedback: Some("No".into()),
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "permission");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert_eq!(
        reply["response"]["response"]["toolUseID"],
        "tool-permission"
    );
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn independent_callbacks_are_correlated_and_only_explicit_allow_grants_access() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control(
        "first",
        "Write",
        json!({"file_path":"a","content":"one"}),
    ))
    .await;
    let first = request(&mut subscription).await;
    peer.send(control(
        "second",
        "Write",
        json!({"file_path":"b","content":"two"}),
    ))
    .await;
    let second = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: second,
            decision: Some(ProviderApprovalDecision::Accept),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "second");
    assert_eq!(reply["response"]["response"]["behavior"], "allow");
    assert_eq!(
        reply["response"]["response"]["updatedInput"]["file_path"],
        "b"
    );
    runtime
        .respond(RuntimeResponse {
            request_id: first,
            decision: Some(ProviderApprovalDecision::Cancel),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(reply["response"]["request_id"], "first");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert_eq!(reply["response"]["response"]["interrupt"], true);
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn user_questions_and_plan_exit_remain_pending_until_seam_response() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control(
        "questions",
        "AskUserQuestion",
        json!({"questions":[{"question":"Choose","options":[]}]}),
    ))
    .await;
    let id = request(&mut subscription).await;
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id.clone(),
                decision: Some(ProviderApprovalDecision::Accept),
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: None,
            feedback: None,
            answers: Some(
                [("Choose".into(), json!(["One", "Two"]))]
                    .into_iter()
                    .collect(),
            ),
        })
        .await
        .unwrap();
    let reply = peer.read().await;
    assert_eq!(
        reply["response"]["response"]["updatedInput"]["answers"],
        json!({"Choose":"One, Two"})
    );
    peer.send(control(
        "plan",
        "ExitPlanMode",
        json!({"plan":"# Proposed plan"}),
    ))
    .await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Decline),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    assert_eq!(
        peer.read().await["response"]["response"]["behavior"],
        "deny"
    );
    peer.send(control(
        "cancel-question",
        "AskUserQuestion",
        json!({"questions":[{"question":"Choose","options":[]}]}),
    ))
    .await;
    let id = request(&mut subscription).await;
    runtime
        .respond(RuntimeResponse {
            request_id: id,
            decision: Some(ProviderApprovalDecision::Cancel),
            feedback: None,
            answers: None,
        })
        .await
        .unwrap();
    let response = peer.read().await;
    assert_eq!(response["response"]["response"]["behavior"], "deny");
    assert_eq!(response["response"]["response"]["interrupt"], true);
    runtime.close().await.unwrap();
}
#[tokio::test]
async fn cancelled_and_closed_callbacks_never_receive_allow() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(control("cancelled", "Bash", json!({"command":"pwd"})))
        .await;
    let id = request(&mut subscription).await;
    peer.send(json!({"type":"control_cancel_request","request_id":"cancelled"}))
        .await;
    loop {
        let event = subscription.events.recv().await.unwrap().unwrap();
        if matches!(event.kind,ProviderEventKind::RuntimeRequest{request,..} if request.status==t3_contracts::orchestration_v2::RuntimeRequestStatus::Cancelled)
        {
            break;
        }
    }
    assert!(
        runtime
            .respond(RuntimeResponse {
                request_id: id,
                decision: Some(ProviderApprovalDecision::Accept),
                feedback: None,
                answers: None
            })
            .await
            .is_err()
    );
    peer.send(control("closed", "Bash", json!({"command":"pwd"})))
        .await;
    let _ = request(&mut subscription).await;
    runtime.close().await.unwrap();
    let mut bytes = String::new();
    use tokio::io::AsyncReadExt;
    peer.read
        .into_inner()
        .read_to_string(&mut bytes)
        .await
        .unwrap();
    assert!(!bytes.contains("\"allow\""));
}
#[tokio::test]
async fn unknown_frame_is_nonfatal_and_a_result_does_not_close_session_input() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(json!({"type":"future_frame","future":true}))
        .await;
    peer.send(json!({"type":"result","subtype":"success","uuid":"result","session_id":"session","result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}})).await;
    loop {
        let event = subscription.events.recv().await.unwrap().unwrap();
        if matches!(event.kind, ProviderEventKind::Terminal { .. }) {
            break;
        }
    }
    let handle = runtime.clone();
    let mut input = fixture_turn_input().unwrap();
    input.run_ordinal = 2;
    input.provider_turn_ordinal = 2;
    input.run_id = RunId("run:2".into());
    input.message.text = "Again".into();
    input.message.message_id = MessageId("00000000-0000-4000-8000-000000000003".into());
    let start = tokio::spawn(async move { handle.start_turn(input).await });
    let user = peer.read().await;
    assert_eq!(user["type"], "user");
    assert_eq!(user["message"]["content"], "Again");
    start.await.unwrap().unwrap();
    drop(peer);
    loop {
        if let ProviderEventKind::Terminal {
            disposition,
            outcome,
            ..
        } = subscription.events.recv().await.unwrap().unwrap().kind
        {
            assert_eq!(disposition, ThreadDisposition::Broken);
            assert!(matches!(outcome, TerminalOutcome::Failed { .. }));
            break;
        }
    }
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn reused_query_applies_model_and_effort_before_the_next_prompt() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(
        json!({"type":"result", "subtype":"success", "uuid":"result", "session_id":"session",
        "result":"Done", "is_error":false, "num_turns":1, "duration_ms":1, "duration_api_ms":1,
        "total_cost_usd":0, "usage":{"input_tokens":1,"output_tokens":1}}),
    )
    .await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    let mut input = fixture_turn_input().unwrap();
    input.run_ordinal = 2;
    input.provider_turn_ordinal = 2;
    input.model_selection.model = "claude-opus-4-6".into();
    let handle = runtime.clone();
    let mut start = tokio::spawn(async move {
        handle
            .start_turn_with_policy(
                input,
                RuntimePolicyOptions {
                    reasoning_effort: Some("high".into()),
                },
            )
            .await
    });
    let model = tokio::select! {
        result = &mut start => panic!("start returned before applying selection: {result:?}"),
        model = peer.read() => model,
    };
    assert_eq!(model["request"]["subtype"], "set_model");
    assert_eq!(model["request"]["model"], "claude-opus-4-6");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":model["request_id"], "response":{}}})).await;
    let effort = peer.read().await;
    assert_eq!(effort["request"]["subtype"], "apply_flag_settings");
    assert_eq!(effort["request"]["settings"]["effortLevel"], "high");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":effort["request_id"], "response":{}}})).await;
    assert_eq!(peer.read().await["type"], "user");
    start.await.unwrap().unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn background_work_outlives_root_completion_and_task_controls_target_its_session() {
    let (runtime, mut subscription, mut peer) = running().await;
    peer.send(json!({"type":"system", "subtype":"background_tasks_changed", "session_id":"session", "uuid":"roster",
        "tasks":[{"task_id":"background", "task_type":"local_bash", "description":"Build"}]})).await;
    peer.send(
        json!({"type":"result", "subtype":"success", "uuid":"result", "session_id":"session",
        "result":"Started", "is_error":false, "num_turns":1, "duration_ms":1, "duration_api_ms":1,
        "total_cost_usd":0, "usage":{"input_tokens":1,"output_tokens":1}}),
    )
    .await;
    loop {
        if matches!(
            subscription.events.recv().await.unwrap().unwrap().kind,
            ProviderEventKind::Terminal { .. }
        ) {
            break;
        }
    }
    let thread = fixture_turn_input().unwrap().provider_thread;
    assert!(
        runtime
            .extensions()
            .unwrap()
            .pending_background_work(Some(thread.clone()), true)
            .await
            .unwrap()
    );
    let mut wrong = thread.clone();
    wrong.native_thread_ref.as_mut().unwrap().native_id = Some(
        t3_contracts::base_schemas::TrimmedNonEmptyString("wrong-session".into()),
    );
    assert!(
        runtime
            .extensions()
            .unwrap()
            .stop_task(wrong, "background".into())
            .await
            .is_err()
    );
    let handle = runtime.clone();
    let stop = tokio::spawn(async move {
        handle
            .extensions()
            .unwrap()
            .stop_task(thread, "background".into())
            .await
    });
    let control = peer.read().await;
    assert_eq!(control["request"]["subtype"], "stop_task");
    assert_eq!(control["request"]["task_id"], "background");
    peer.send(json!({"type":"control_response", "response":{"subtype":"success", "request_id":control["request_id"], "response":{}}})).await;
    stop.await.unwrap().unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test]
async fn live_selection_updates_flags_and_defers_process_bound_changes() {
    let host = ClaudeHostOptions {
        compile_model: Some(Arc::new(|selection| {
            let options = serde_json::to_value(&selection.options).unwrap();
            let mut model = ClaudeCompiledModel {
                model: selection.model.clone(),
                effort: Some("high".into()),
                settings: json!({}),
            };
            for option in options.as_array().into_iter().flatten() {
                match option["id"].as_str() {
                    Some("effort") => {
                        model.effort = option["value"]
                            .as_str()
                            .filter(|value| *value != "default")
                            .map(str::to_owned)
                    }
                    Some("thinking") => {
                        model.settings["alwaysThinkingEnabled"] = option["value"].clone()
                    }
                    Some("fastMode" | "ultracode") => {
                        model.settings[option["id"].as_str().unwrap()] = option["value"].clone()
                    }
                    _ => {}
                }
            }
            Ok(model)
        })),
        ..Default::default()
    };
    let (runtime, mut events, mut peer) = running_with_host(host).await;
    let thread = fixture_turn_input().unwrap().provider_thread;
    let selection = |options| {
        serde_json::from_value(
            json!({"instanceId":"claudeAgent","model":"claude-sonnet-4-6","options":options}),
        )
        .unwrap()
    };
    let extensions = runtime.extensions().unwrap();
    for (options, expected) in [
        (
            json!([{"id":"effort","value":"medium"}]),
            json!({"effortLevel":"medium","fastMode":null,"ultracode":null}),
        ),
        (
            json!([{"id":"effort","value":"medium"},{"id":"fastMode","value":true},{"id":"ultracode","value":false}]),
            json!({"effortLevel":"medium","fastMode":true,"ultracode":false}),
        ),
    ] {
        let handle = runtime.clone();
        let thread = thread.clone();
        let next = selection(options);
        let mut apply = tokio::spawn(async move {
            handle
                .extensions()
                .unwrap()
                .apply_selection(thread, next)
                .await
        });
        let flags = tokio::select! {result = &mut apply => panic!("live flags returned before native acknowledgment: {result:?}"), frame = peer.read() => frame};
        assert_eq!(flags["request"]["subtype"], "apply_flag_settings");
        assert_eq!(flags["request"]["settings"], expected);
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":flags["request_id"],"response":{}}})).await;
        apply.await.unwrap().unwrap();
    }
    for next in [
        selection(
            json!([{"id":"effort","value":"medium"},{"id":"fastMode","value":true},{"id":"ultracode","value":false}]),
        ),
        selection(json!([{"id":"effort","value":"default"}])),
        selection(json!([{"id":"effort","value":"medium"},{"id":"thinking","value":false}])),
        serde_json::from_value(json!({"instanceId":"claudeAgent","model":"claude-opus-4-6"}))
            .unwrap(),
    ] {
        tokio::select! {result = extensions.apply_selection(thread.clone(), next) => result.unwrap(), frame = peer.read() => panic!("process-bound/no-op change sent a live frame: {frame}")};
    }
    peer.send(json!({"type":"result","subtype":"success","uuid":"live-flags-end","result":"Done","is_error":false,"num_turns":1,"duration_ms":1,"duration_api_ms":1,"total_cost_usd":0,"usage":{"input_tokens":1,"output_tokens":1}})).await;
    let mut turns = std::collections::BTreeSet::new();
    loop {
        match events.events.recv().await.unwrap().unwrap().kind {
            ProviderEventKind::Turn { provider_turn, .. } => {
                turns.insert(serde_json::to_string(&provider_turn.id).unwrap());
            }
            ProviderEventKind::Terminal { run_ordinal, .. } => {
                assert_eq!(run_ordinal, 1);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(turns.len(), 1);
    runtime.close().await.unwrap();
    assert!(
        peer.read.next_line().await.unwrap().is_none(),
        "live updates must not send a user prompt or interrupt"
    );
}

#[tokio::test]
async fn mcp_servers_keep_each_app_threads_context_and_session_lifetime() {
    use std::{collections::VecDeque, future::Future, pin::Pin, sync::Mutex};
    use t3_claude_cli::McpServer;
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;

    struct BoundServer(ThreadId);
    impl McpServer for BoundServer {
        fn notify(&self, _: Value, _: CancellationToken) -> Result<(), t3_claude_cli::Error> {
            Ok(())
        }
        fn handle<'a>(
            &'a self,
            message: Value,
            _: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<Value, t3_claude_cli::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                assert_eq!(message["method"], "tools/call");
                Ok(json!({"jsonrpc":"2.0","id":message["id"],"result":{"threadId":self.0}}))
            })
        }
    }
    #[derive(Default)]
    struct Factory(Mutex<Vec<CancellationToken>>);
    #[async_trait::async_trait]
    impl ClaudeMcpServerFactory for Factory {
        async fn create(
            &self,
            input: &OpenSession,
            cancellation: CancellationToken,
        ) -> ProviderResult<BTreeMap<String, Arc<dyn McpServer>>> {
            self.0.lock().unwrap().push(cancellation);
            Ok(BTreeMap::from([(
                "t3-code".into(),
                Arc::new(BoundServer(input.thread_id.clone())) as Arc<dyn McpServer>,
            )]))
        }
    }
    struct Pipes(Mutex<VecDeque<Session>>);
    impl ClaudeConnector for Pipes {
        fn connect<'a>(
            &'a self,
            options: &'a LaunchOptions,
            _: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = ProviderResult<ClaudeConnection>> + Send + 'a>> {
            for name in ["mcp__t3-code__send_to_user", "mcp__t3-code__t3_theme_read"] {
                assert!(
                    options.allowed_tools.iter().any(|tool| tool == name),
                    "read-only/plan native options omit {name}"
                );
            }
            assert!(
                !options
                    .allowed_tools
                    .iter()
                    .any(|tool| tool == "mcp__t3-code__t3_theme_set")
            );
            let session = self.0.lock().unwrap().pop_front();
            Box::pin(async move {
                Ok(ClaudeConnection {
                    session: session.ok_or(ProviderError::SessionStopped)?,
                    process: None,
                })
            })
        }
    }
    let mut sessions = VecDeque::new();
    let mut peers = Vec::new();
    for _ in 0..2 {
        let (client, server) = tokio::io::duplex(65536);
        let (read, write) = tokio::io::split(client);
        sessions.push_back(Session::connect(read, write, SessionOptions::default()));
        let (read, write) = tokio::io::split(server);
        peers.push(Peer {
            read: BufReader::new(read).lines(),
            write,
        });
    }
    let factory = Arc::new(Factory::default());
    let adapter = ClaudeAdapterV2::new(ClaudeAdapterOptions {
        instance_id: ProviderInstanceId("claudeAgent".into()),
        launch: LaunchOptions {
            cli_path: Some("fixture-claude".into()),
            ..Default::default()
        },
        connector: Arc::new(Pipes(Mutex::new(sessions))),
        clock: Arc::new(ManualClock::new(FIXTURE_DATE)),
        ids: Arc::new(SequenceIds::new("mcp")),
        new_native_session_id: Arc::new(|| "00000000-0000-4000-8000-000000000001".into()),
        mcp_servers: BTreeMap::new(),
        transcript_root: None,
        attachments: Arc::new(DirectoryClaudeAttachments::default()),
        host: ClaudeHostOptions {
            mcp_server_factory: Some(factory.clone()),
            ..Default::default()
        },
    });
    let mut runtimes = Vec::new();
    for (index, peer) in peers.iter_mut().enumerate() {
        let mut input = fixture_turn_input().unwrap();
        input.runtime_policy.sandbox_policy =
            Some(json!({"type":"readOnly","access":{"type":"restricted"}}));
        if index == 1 {
            input.runtime_policy.interaction_mode =
                t3_contracts::provider_policy::ProviderInteractionMode::Plan;
        }
        input.thread_id = ThreadId(format!("thread-{index}"));
        input.app_thread.id = IdOrIndex::Id(input.thread_id.clone());
        input.provider_thread.app_thread_id = Some(IdOrIndex::Id(input.thread_id.clone()));
        let session_id = ProviderSessionId(format!("session-{index}"));
        input.provider_thread.provider_session_id = Some(IdOrIndex::Id(session_id.clone()));
        let runtime = adapter
            .open_session(
                OpenSession {
                    thread_id: input.thread_id.clone(),
                    provider_session_id: session_id,
                    model_selection: input.model_selection.clone(),
                    runtime_policy: input.runtime_policy.clone(),
                    prompt_suggestions: None,
                    resume_from_session: None,
                    initial_native_thread_id: None,
                    initial_provider_item_identity_version: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let handle = runtime.clone();
        let mut start = tokio::spawn(async move { handle.start_turn(input).await });
        let initialize = tokio::select! {
            result = &mut start => panic!("native query ended before initialize: {result:?}"),
            frame = peer.read() => frame,
        };
        assert_eq!(initialize["request"]["sdkMcpServers"], json!(["t3-code"]));
        let upstream = include_str!(
            "../../../../../apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts"
        );
        let guidance = upstream
            .split("const CLAUDE_SEND_TO_USER_INSTRUCTIONS = `")
            .nth(1)
            .unwrap()
            .split("`;")
            .next()
            .unwrap()
            .replace("\\`", "`")
            .replace("\r\n", "\n");
        assert!(
            initialize["request"]["appendSystemPrompt"]
                .as_str()
                .unwrap_or_default()
                .contains(&guidance),
            "native query must carry the exact upstream send_to_user instructions"
        );
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":initialize["request_id"],"response":{}}})).await;
        assert_eq!(peer.read().await["type"], "user");
        start.await.unwrap().unwrap();
        runtimes.push(runtime);
    }
    for (index, peer) in peers.iter_mut().enumerate() {
        peer.send(json!({"type":"control_request","request_id":"mcp","request":{"subtype":"mcp_message","server_name":"t3-code","message":{"jsonrpc":"2.0","id":"context","method":"tools/call","params":{"name":"thread-context","arguments":{}}}}})).await;
        let response = peer.read().await;
        assert_eq!(response["response"]["request_id"], "mcp");
        assert_eq!(
            response["response"]["response"]["mcp_response"],
            json!({"jsonrpc":"2.0","id":"context","result":{"threadId":format!("thread-{index}")}})
        );
    }
    runtimes[0].close().await.unwrap();
    {
        let tokens = factory.0.lock().unwrap();
        assert_eq!(tokens.len(), 2);
        assert!(tokens[0].is_cancelled());
        assert!(!tokens[1].is_cancelled());
    }
    peers[1].send(json!({"type":"control_request","request_id":"after-close","request":{"subtype":"mcp_message","server_name":"t3-code","message":{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"thread-context","arguments":{}}}}})).await;
    assert_eq!(
        peers[1].read().await["response"]["response"]["mcp_response"]["result"]["threadId"],
        "thread-1"
    );
    runtimes[1].close().await.unwrap();
    assert!(factory.0.lock().unwrap()[1].is_cancelled());
}
