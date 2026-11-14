#![cfg(feature = "claude-adapter")]

use serde_json::{Value, json};
use std::sync::Arc;
use t3_claude_cli::control::{PermissionDecision, PermissionMode, ToolPermission};
use t3_contracts::provider_policy::{
    ProviderApprovalDecision, ProviderInteractionMode, RuntimeMode,
};
use t3_orchestration::{
    adapters::claude::*,
    ports::claude::adapters::claude_adapter_v2_testkit::*,
    provider::*,
    testing::{ManualClock, SequenceIds},
};

#[test]
fn attachment_pdf_count_rejects_hidden_and_incremental_page_trees() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n2 0 obj << /Type /Pages /Count 1 >> endobj\n3 0 obj << /Type /Page >> endobj\n%%EOF";
    assert_eq!(estimate_claude_pdf_pages(pdf), Some(1));
    for extra in [
        "/Encrypt",
        "/Prev 1",
        "/Type /ObjStm",
        "/Type /XRef",
        "/Count 1 0 R",
        "/Type /Page",
    ] {
        let mut modified = pdf.to_vec();
        modified.push(b'\n');
        modified.extend_from_slice(extra.as_bytes());
        assert_eq!(estimate_claude_pdf_pages(&modified), None, "{extra}");
    }
    let quoted =
        b"%PDF-1.7\n(/Type /Page) <2f54797065202f50616765>\nstream\n/Type /Page\nendstream\n%%EOF";
    assert_eq!(estimate_claude_pdf_pages(quoted), None);
}

#[test]
fn attachment_message_reserves_images_before_pdfs_and_keeps_paths_when_full() {
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("report.pdf");
    let image = directory.path().join("picture.png");
    std::fs::write(
        &pdf,
        b"%PDF-1.7\n<< /Type /Catalog >>\n<< /Type /Pages /Count 1 >>\n<< /Type /Page >>\n%%EOF",
    )
    .unwrap();
    std::fs::write(&image, [1u8; 60]).unwrap();
    let mut message = fixture_turn_input().unwrap().message;
    message.attachments = serde_json::from_value(json!([
        {"type":"file","id":"report","name":"report.pdf","mimeType":"application/pdf","sizeBytes":1},
        {"type":"image","id":"picture","name":"picture.png","mimeType":"image/png","sizeBytes":1}
    ])).unwrap();
    let source = DirectoryClaudeAttachments::new(directory.path().to_owned());
    let prepared =
        prepare_claude_message(&message, None, &source, ClaudeAttachmentUsage::default()).unwrap();
    let content = serde_json::to_value(prepared.message).unwrap();
    assert_eq!(content["message"]["content"][0]["type"], "document");
    assert_eq!(content["message"]["content"][1]["type"], "image");
    assert_eq!(prepared.usage.pages_used, 1);
    let paths = content["message"]["content"][2]["text"].as_str().unwrap();
    assert!(paths.contains(pdf.to_str().unwrap()));
    assert!(paths.contains(image.to_str().unwrap()));
    let nearly_full = ClaudeAttachmentUsage {
        pages_used: 99,
        bytes_used: CLAUDE_ATTACHMENT_MAX_BYTES - paths.len() as u64 - 400,
    };
    let prepared = prepare_claude_message(&message, None, &source, nearly_full).unwrap();
    let content = serde_json::to_value(prepared.message).unwrap();
    assert_eq!(content["message"]["content"][0]["type"], "image");
    assert_eq!(content["message"]["content"].as_array().unwrap().len(), 2);
    assert_eq!(prepared.usage.pages_used, 99);
    std::fs::remove_file(image).unwrap();
    assert!(matches!(
        prepare_claude_message(&message, None, &source, ClaudeAttachmentUsage::default()),
        Err(ProviderError::Protocol {
            reason: Some(ProtocolReason::AttachmentUnreadable),
            ..
        })
    ));
}

#[test]
fn launch_settings_consume_owned_flags_without_exposing_inline_settings_errors() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("append.txt"), "File instructions\r\n").unwrap();
    let mut launch = t3_claude_cli::launch::LaunchOptions {
        extra_args: vec![
        ("append-system-prompt-file".into(), Some("append.txt".into())),
        ("append-system-prompt".into(), Some("Inline instructions".into())),
        ("settings".into(), Some("{\"permissions\":{\"disableBypassPermissionsMode\":\"disable\",},/* hi */\"env\":{\"X\":\"//value\"}}".into())),
        ("add-dir".into(), Some("extra".into())),
        ("debug-to-stderr".into(), None),
        ],
        ..Default::default()
    };
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.cwd = Some(directory.path().to_owned());
    let host = ClaudeHostOptions {
        runtime_instructions: "Runtime instructions".into(),
        ..Default::default()
    };
    let compiled =
        prepare_claude_launch(&launch, &input, &RuntimePolicyOptions::default(), &host).unwrap();
    assert_eq!(compiled.policy.permission_mode.as_str(), "auto");
    assert!(
        compiled
            .options
            .additional_directories
            .contains(&"extra".into())
    );
    assert!(
        !compiled
            .options
            .extra_args
            .iter()
            .any(|(k, _)| k == "settings" || k == "append-system-prompt")
    );
    let t3_claude_cli::launch::SystemPrompt::ClaudeCode {
        append: Some(append),
    } = compiled.options.system_prompt
    else {
        panic!("preset prompt")
    };
    assert_eq!(
        append,
        "Runtime instructions\n\nFile instructions\n\nInline instructions"
    );
    let t3_claude_cli::launch::Settings::Json(settings) = compiled.options.settings.unwrap() else {
        panic!("inline settings")
    };
    assert_eq!(settings["env"]["X"], "//value");
    assert_eq!(settings["showThinkingSummaries"], true);
    launch.extra_args = vec![(
        "settings".into(),
        Some("{secret_token:super-secret}".into()),
    )];
    let error = prepare_claude_launch(&launch, &input, &RuntimePolicyOptions::default(), &host)
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("super-secret"));
}

#[test]
fn launch_settings_pin_auto_tool_search_without_overriding_explicit_modes() {
    use t3_claude_cli::launch::{LaunchOptions, Settings};
    for (mode, expected) in [
        (Some("auto"), Some("true")),
        (Some("auto:10"), Some("true")),
        (Some("auto:99.5"), Some("true")),
        (Some("auto:x"), Some("true")),
        (Some("auto:Infinity"), Some("true")),
        (Some("auto:1e-1"), Some("true")),
        (Some("auto:1e1"), Some("true")),
        (Some("auto:1,00"), Some("true")),
        (None, None),
        (Some("true"), None),
        (Some("false"), None),
        (Some("auto:0"), None),
        (Some("auto:-5"), None),
        (Some("auto:100"), None),
        (Some("auto:1e2"), None),
        (Some("auto:1_000"), None),
        (Some("auto:1,000"), None),
        (Some("auto:1\u{202f}000"), None),
        (Some("AUTO"), None),
    ] {
        let base = LaunchOptions {
            env: mode
                .into_iter()
                .map(|value| ("ENABLE_TOOL_SEARCH".into(), value.into()))
                .collect(),
            settings: Some(Settings::Json(json!({"env":{"OTHER":"kept"}}))),
            ..Default::default()
        };
        let prepared = prepare_claude_launch(
            &base,
            &fixture_turn_input().unwrap(),
            &RuntimePolicyOptions::default(),
            &ClaudeHostOptions::default(),
        )
        .unwrap();
        let Some(Settings::Json(settings)) = prepared.options.settings else {
            panic!("expected compiled settings")
        };
        assert_eq!(settings["env"]["OTHER"], "kept");
        assert_eq!(
            settings["env"]["ENABLE_TOOL_SEARCH"].as_str(),
            expected,
            "{mode:?}"
        );
    }
}

#[test]
fn managed_and_file_restrictions_survive_selection_settings_and_cwd_changes() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".claude")).unwrap();
    std::fs::write(
        directory.path().join(".claude/settings.local.json"),
        r#"{"permissions":{"disableBypassPermissionsMode":"disable"},"disableAutoMode":"disable"}"#,
    )
    .unwrap();
    let mut input = fixture_turn_input().unwrap();
    input.runtime_policy.cwd = Some(directory.path().to_owned());
    input.model_selection.options = Some(serde_json::from_value(json!([{"id":"thinking","value":false},{"id":"fastMode","value":true},{"id":"effort","value":"high"}])).unwrap());
    let compiled = prepare_claude_launch(
        &Default::default(),
        &input,
        &RuntimePolicyOptions::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(compiled.policy.permission_mode.as_str(), "default");
    assert_eq!(compiled.options.effort.as_deref(), Some("high"));
    assert!(
        !compiled
            .options
            .extra_args
            .iter()
            .any(|(k, _)| k == "thinking-display")
    );
    let t3_claude_cli::launch::Settings::Json(settings) = compiled.options.settings.unwrap() else {
        panic!("settings")
    };
    assert_eq!(settings["alwaysThinkingEnabled"], false);
    assert_eq!(settings["fastMode"], true);
    input.runtime_policy.cwd = None;
    let host = ClaudeHostOptions {
        managed_settings: vec![json!({"permissions":{"disableBypassPermissionsMode":"disable"}})],
        ..Default::default()
    };
    let compiled = prepare_claude_launch(
        &Default::default(),
        &input,
        &RuntimePolicyOptions::default(),
        &host,
    )
    .unwrap();
    assert_eq!(compiled.policy.permission_mode.as_str(), "auto");
}

#[test]
fn native_tool_non_execution_metadata_controls_status_without_inferring_from_text() {
    for kind in [
        Some("cancelled"),
        Some("interrupted"),
        Some("denied"),
        Some("permission_denied"),
        None,
    ] {
        let mut projection = fixture_projection(
            Arc::new(ManualClock::new(FIXTURE_DATE)),
            Arc::new(SequenceIds::new("non-execution")),
        );
        projection.begin(fixture_turn_input().unwrap()).unwrap();
        projection.handle_frame(&json!({"type":"assistant","uuid":"calls","message":{"content":[
            {"type":"tool_use","id":"error","name":"Read","input":{"file_path":"error.txt"}},
            {"type":"tool_use","id":"ok","name":"Read","input":{"file_path":"ok.txt"}}
        ]}})).unwrap();
        let mut frame = json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"error","is_error":true,"content":"STOP and wait for the user."},
            {"type":"tool_result","tool_use_id":"ok","content":"OK"}
        ]}});
        if let Some(kind) = kind {
            frame["tool_result_meta"] = json!([
                {"id":"error","non_execution_kind":kind},
                {"id":"ok","non_execution_kind":null}
            ]);
        }
        projection.handle_frame(&frame).unwrap();
        let error = serde_json::to_value(
            &projection.items[&provider_id("turn-item", "native-item", "error")],
        )
        .unwrap();
        assert_eq!(
            error["status"],
            kind.filter(|kind| matches!(*kind, "cancelled" | "interrupted"))
                .unwrap_or("failed")
        );
        assert_eq!(error["toolNonExecutionKind"].as_str(), kind);
        let ok =
            serde_json::to_value(&projection.items[&provider_id("turn-item", "native-item", "ok")])
                .unwrap();
        assert_eq!(ok["status"], "completed");
        assert!(!ok.as_object().unwrap().contains_key("toolNonExecutionKind"));
    }
}

#[test]
fn query_exit_settles_unanswered_root_tools_without_changing_finished_tools() {
    for interrupted in [true, false] {
        let mut projection = fixture_projection(
            Arc::new(ManualClock::new(FIXTURE_DATE)),
            Arc::new(SequenceIds::new("query-exit")),
        );
        projection.begin(fixture_turn_input().unwrap()).unwrap();
        projection.handle_frame(&json!({"type":"assistant","uuid":"calls","message":{
            "content":[
                {"type":"tool_use","id":"pending","name":"Bash","input":{"command":"node -e wait"}},
                {"type":"tool_use","id":"finished","name":"Read","input":{"file_path":"done.txt"}}
            ]
        }})).unwrap();
        projection
            .handle_frame(&json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":"finished","content":"done"}
            ]}}))
            .unwrap();
        if interrupted {
            projection.interrupt();
        }
        let events = projection.query_ended().unwrap();
        let pending = serde_json::to_value(
            &projection.items[&provider_id("turn-item", "native-item", "pending")],
        )
        .unwrap();
        assert_eq!(
            pending["status"],
            if interrupted { "interrupted" } else { "failed" }
        );
        assert_eq!(pending["completedAt"], FIXTURE_DATE);
        let finished = serde_json::to_value(
            &projection.items[&provider_id("turn-item", "native-item", "finished")],
        )
        .unwrap();
        assert_eq!(finished["status"], "completed");
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.kind, ProviderEventKind::TurnItem { .. }))
        );
    }
}

#[tokio::test]
async fn nested_native_agents_project_children_without_extra_provider_turns() {
    let recording = std::fs::read_to_string(
        fixture_recording_root().join("claude_nested_subagent_model/claude_transcript.ndjson"),
    )
    .unwrap();
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("nested")),
    );
    let events = replay_messages(&recording, &mut projection, fixture_turn_input().unwrap())
        .await
        .unwrap();
    let agents: std::collections::BTreeMap<_, _> = events
        .iter()
        .filter_map(|e| match &e.kind {
            ProviderEventKind::Subagent(agent) => Some((
                serde_json::to_string(&agent.id).unwrap(),
                serde_json::to_value(agent).unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(agents.len(), 2);
    let outer = agents
        .values()
        .find(|a| a["nativeTaskRef"]["nativeId"] == "a457ca677bc1ecd82")
        .unwrap();
    let nested = agents
        .values()
        .find(|a| a["nativeTaskRef"]["nativeId"] == "aac1d7a2af5371c7d")
        .unwrap();
    assert_eq!(outer["status"], "completed");
    assert_eq!(nested["parentNodeId"], outer["id"]);
    assert_eq!(nested["model"], "claude-haiku-4-5");
    assert_eq!(nested["providerThreadId"], Value::Null);
    let children: std::collections::BTreeMap<_, _> = events
        .iter()
        .filter_map(|e| match &e.kind {
            ProviderEventKind::AppThread { app_thread, .. } => {
                Some((serde_json::to_string(&app_thread.id).unwrap(), app_thread))
            }
            _ => None,
        })
        .collect();
    assert_eq!(children.len(), 2);
    assert_eq!(projection.turns.len(), 1);
    let usage =
        serde_json::to_value(projection.turns.values().next().unwrap()).unwrap()["turnTokenUsage"]
            .clone();
    assert_eq!(usage["inputTokens"], 74167);
    assert_eq!(usage["reasoningTokens"], 30);
    assert_eq!(usage["models"], json!(["claude-sonnet-4-6"]));
    assert_eq!(
        usage["otherUsage"],
        json!({"inputTokens":87409,"outputTokens":335})
    );
    let child = nested["childThreadId"].as_str().unwrap();
    let result_ids = events
        .iter()
        .filter_map(|event| match &event.kind {
            ProviderEventKind::Message(message)
                if serde_json::to_value(&message.thread_id).unwrap() == child
                    && message.text.trim() == "NESTED_LEAF_OK"
                    && !message.streaming =>
            {
                Some(serde_json::to_string(&message.id).unwrap())
            }
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        result_ids.len(),
        1,
        "a native child reply and its task summary must share one visible result"
    );
    assert!(events.iter().any(|e| match &e.kind {
        ProviderEventKind::Message(message) =>
            serde_json::to_value(&message.thread_id).unwrap() == child
                && message.text.contains("NESTED_LEAF_OK")
                && message.run_id.is_none(),
        _ => false,
    }));
    assert!(!projection.items.values().any(|item| {
        let item = serde_json::to_value(item).unwrap();
        item["threadId"] == "thread" && item["type"] == "dynamic_tool"
    }));
    let head = projection
        .thread
        .as_ref()
        .unwrap()
        .native_conversation_head_ref
        .clone();
    projection.handle_frame(&json!({"type":"assistant","uuid":"late-child-cursor","parent_tool_use_id":"toolu_015wdGtqfaZX3yxLAXJKHzxY","message":{"id":"late-child-message","model":"claude-haiku-4-5","content":[{"type":"text","text":"Late child details"}]}})).unwrap();
    assert_eq!(
        projection
            .thread
            .as_ref()
            .unwrap()
            .native_conversation_head_ref,
        head,
        "child messages must not move the root transcript cursor"
    );
}

#[test]
fn native_fork_remaps_progress_and_compaction_links_without_mutating_source() {
    let rows = vec![
        json!({"type":"user","uuid":"u","parentUuid":null,"sessionId":"source","timestamp":"old","message":{"content":"Hello"}}),
        json!({"type":"progress","uuid":"p","parentUuid":"u","sessionId":"source","timestamp":"old"}),
        json!({"type":"assistant","uuid":"a","parentUuid":"p","logicalParentUuid":"u","sessionId":"source","timestamp":"old","message":{"id":"api-a","content":[]},"teamName":"secret-team"}),
        json!({"type":"system","subtype":"compact_boundary","uuid":"compact","parentUuid":"a","sessionId":"source","timestamp":"old","compactMetadata":{"preservedMessages":{"anchorUuid":"u","uuids":["a"],"allUuids":["u","a"]},"runningTurnSends":["old"]}}),
        json!({"type":"attachment","uuid":"queued","parentUuid":"compact","sessionId":"source","timestamp":"old","attachment":{"type":"queued_command","source_uuid":"a"}}),
        json!({"type":"custom-title","sessionId":"source","customTitle":"Original"}),
        json!({"type":"tools-used","sessionId":"source","tools":["Read"]}),
        json!({"type":"assistant","uuid":"side","isSidechain":true,"sessionId":"source"}),
    ];
    let original = rows.clone();
    let mut sequence = 0;
    let fork = fork_claude_records(&rows, "source", "fork", None, FIXTURE_DATE, || {
        sequence += 1;
        format!("uuid-{sequence}")
    })
    .unwrap();
    assert_eq!(rows, original);
    assert!(
        !fork
            .entries
            .iter()
            .any(|r| r["type"] == "progress" || r["uuid"] == "side")
    );
    let user = fork.entries.iter().find(|r| r["type"] == "user").unwrap();
    let assistant = fork
        .entries
        .iter()
        .find(|r| r["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["parentUuid"], user["uuid"]);
    assert!(assistant.get("teamName").is_none());
    assert_eq!(assistant["forkedFrom"]["messageUuid"], "a");
    let compact = fork
        .entries
        .iter()
        .find(|r| r["subtype"] == "compact_boundary")
        .unwrap();
    assert_eq!(
        compact["compactMetadata"]["preservedMessages"]["uuids"][0],
        assistant["uuid"]
    );
    assert!(compact["compactMetadata"].get("runningTurnSends").is_none());
    let queued = fork
        .entries
        .iter()
        .find(|r| r["attachment"]["type"] == "queued_command")
        .unwrap();
    assert_eq!(queued["attachment"]["source_uuid"], assistant["uuid"]);
    assert_eq!(queued["timestamp"], FIXTURE_DATE);
    assert_eq!(fork.head_message_id.as_deref(), assistant["uuid"].as_str());
    assert!(
        fork.entries
            .iter()
            .any(|r| r["customTitle"] == "Original (fork)")
    );
    assert!(
        fork_claude_records(
            &rows,
            "source",
            "fork",
            Some("absent"),
            FIXTURE_DATE,
            || "new".into()
        )
        .is_err()
    );
    let fork = fork_claude_records(&rows, "source", "fork", Some("a"), FIXTURE_DATE, || {
        sequence += 1;
        format!("uuid-{sequence}")
    })
    .unwrap();
    assert!(
        !fork
            .entries
            .iter()
            .any(|r| r["subtype"] == "compact_boundary")
    );
}

#[test]
fn native_fork_publishes_in_original_project_and_rejects_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("moved-project");
    std::fs::create_dir(&project).unwrap();
    let source = "00000000-0000-4000-8000-000000000001";
    let target = "00000000-0000-4000-8000-000000000002";
    let transcript = format!(
        "{}\nmalformed interrupted append\n{}\n",
        json!({"type":"user","uuid":"prompt","parentUuid":null,"sessionId":source,"timestamp":FIXTURE_DATE,"message":{"content":"prompt"}}),
        json!({"type":"assistant","uuid":"answer","parentUuid":"prompt","sessionId":source,"timestamp":FIXTURE_DATE,"message":{"id":"api","content":[{"type":"text","text":"answer"}]}})
    );
    let path = project.join(format!("{source}.jsonl"));
    std::fs::write(&path, &transcript).unwrap();
    let fork = fork_claude_session_file(
        directory.path(),
        source,
        target,
        Some("answer"),
        FIXTURE_DATE,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), transcript);
    let fork_path = project.join(format!("{target}.jsonl"));
    let copied = std::fs::read_to_string(&fork_path).unwrap();
    assert!(copied.contains(fork.head_message_id.as_deref().unwrap()));
    assert!(!copied.contains("malformed interrupted append"));
    assert!(
        fork_claude_session_file(directory.path(), source, target, None, FIXTURE_DATE).is_err()
    );
    assert_eq!(std::fs::read_to_string(fork_path).unwrap(), copied);
    assert!(
        fork_claude_session_file(directory.path(), "../outside", target, None, FIXTURE_DATE)
            .is_err()
    );
}

#[test]
fn permission_policy_matrix_preserves_runtime_sandbox_and_explicit_approval() {
    let upstream =
        include_str!("../../../../../apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts");
    let expected = upstream
        .split("export const CLAUDE_READ_ONLY_T3_MCP_ALLOWED_TOOLS: ReadonlyArray<string> = [")
        .nth(1)
        .unwrap()
        .split("];")
        .next()
        .unwrap();
    let mut expected = expected
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('"')
                .and_then(|line| line.split('"').next())
        })
        .collect::<Vec<_>>();
    let mut actual = CLAUDE_READ_ONLY_T3_MCP_ALLOWED_TOOLS.to_vec();
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(actual, expected);
    let base = fixture_turn_input().unwrap().runtime_policy;
    for (mode, expected) in [
        (RuntimeMode::ApprovalRequired, PermissionMode::Default),
        (RuntimeMode::AutoAcceptEdits, PermissionMode::AcceptEdits),
        (RuntimeMode::Auto, PermissionMode::Auto),
        (RuntimeMode::FullAccess, PermissionMode::BypassPermissions),
    ] {
        let mut policy = base.clone();
        policy.runtime_mode = mode;
        assert_eq!(
            permission_mode_for_runtime_policy(&policy).as_str(),
            expected.as_str()
        );
        policy.interaction_mode = ProviderInteractionMode::Plan;
        let plan = runtime_query_policy(&policy, BypassAvailability::default());
        assert_eq!(plan.permission_mode.as_str(), "plan");
        assert!(plan.install_permission_callback);
    }
    let mut policy = base.clone();
    policy.approval_policy = Some(json!("never"));
    policy.sandbox_policy = Some(json!({"type":"readOnly","access":{"type":"fullAccess"}}));
    let query = runtime_query_policy(&policy, BypassAvailability::default());
    assert_eq!(query.permission_mode.as_str(), "dontAsk");
    assert_eq!(query.tools.unwrap(), vec!["Read", "Glob", "Grep"]);
    assert_eq!(query.allowed_tools, vec!["Read", "Glob", "Grep"]);
    policy.sandbox_policy = Some(json!({"type":"readOnly","access":{"type":"restricted"}}));
    assert!(
        runtime_query_policy(&policy, BypassAvailability::default())
            .allowed_tools
            .is_empty()
    );
    policy.sandbox_policy = Some(json!({"type":"dangerFullAccess"}));
    policy.approval_policy = None;
    policy.runtime_mode = RuntimeMode::ApprovalRequired;
    assert_eq!(
        permission_mode_for_runtime_policy(&policy).as_str(),
        "default"
    );
    policy.runtime_mode = RuntimeMode::FullAccess;
    for (auto_disabled, expected) in [(false, "auto"), (true, "default")] {
        let query = runtime_query_policy(
            &policy,
            BypassAvailability {
                bypass_disabled: true,
                auto_disabled,
            },
        );
        assert_eq!(query.permission_mode.as_str(), expected);
        assert!(query.install_permission_callback);
        assert!(!query.allow_dangerously_skip_permissions);
    }
    policy.approval_policy = Some(json!("on-request"));
    assert_eq!(
        permission_mode_for_runtime_policy(&policy).as_str(),
        "default"
    );
}
#[test]
fn permission_callback_session_grants_preserve_unknown_native_suggestions() {
    let request: ToolPermission = serde_json::from_value(json!({
        "tool_name":"Bash", "input":{"command":"pwd"}, "tool_use_id":"tool",
        "permission_suggestions":[
            {"type":"futureScopedRule", "scope":{"command":"pwd"}, "opaque":17, "destination":"userSettings"},
            {"type":"setMode", "mode":"futureRestrictedMode", "destination":"projectSettings"}
        ]
    })).unwrap();
    let answer = serde_json::to_value(permission_result_from_decision(
        &request,
        ProviderApprovalDecision::AcceptForSession,
        None,
        false,
    ))
    .unwrap();
    assert_eq!(
        answer["updatedPermissions"],
        json!([
            {"type":"futureScopedRule", "scope":{"command":"pwd"}, "opaque":17, "destination":"session"},
            {"type":"setMode", "mode":"futureRestrictedMode", "destination":"session"}
        ])
    );
    assert_eq!(answer["updatedInput"], json!({"command":"pwd"}));
}

#[test]
fn permission_callback_empty_session_suggestions_grant_only_the_current_call() {
    for (suggestions, managed) in [
        (None, false),
        (Some(vec![]), false),
        (
            Some(vec![
                json!({"type":"setMode","mode":"bypassPermissions","destination":"userSettings"}),
            ]),
            true,
        ),
    ] {
        let mut request: ToolPermission = serde_json::from_value(json!({
            "tool_name":"Bash", "input":{"command":"pwd"}, "tool_use_id":"tool",
        }))
        .unwrap();
        request.permission_suggestions = suggestions;
        let answer = serde_json::to_value(permission_result_from_decision(
            &request,
            ProviderApprovalDecision::AcceptForSession,
            None,
            managed,
        ))
        .unwrap();
        assert_eq!(answer["behavior"], "allow");
        assert!(
            answer.get("updatedPermissions").is_none(),
            "must not invent a standing tool rule: {answer}"
        );
        assert_eq!(answer["decisionClassification"], "user_temporary");
    }
}

#[test]
fn decisions_never_upgrade_refusals_and_session_grants_are_session_scoped() {
    let request:ToolPermission=serde_json::from_value(json!({"tool_name":"Bash","input":{"command":"pwd"},"tool_use_id":"tool",
        "permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"pwd"}],"behavior":"allow","destination":"userSettings"}]})).unwrap();
    for decision in [
        ProviderApprovalDecision::Decline,
        ProviderApprovalDecision::Cancel,
        ProviderApprovalDecision::AcceptAlways,
    ] {
        let answer = serde_json::to_value(permission_result_from_decision(
            &request,
            decision,
            Some("  No thanks  "),
            false,
        ))
        .unwrap();
        assert_eq!(answer["behavior"], "deny");
        assert_eq!(answer["decisionClassification"], "user_reject");
        assert_eq!(answer["message"], "No thanks");
        assert_eq!(
            answer.get("interrupt"),
            if decision == ProviderApprovalDecision::Cancel {
                Some(&Value::Bool(true))
            } else {
                None
            }
        );
    }
    let answer = serde_json::to_value(permission_result_from_decision(
        &request,
        ProviderApprovalDecision::AcceptForSession,
        None,
        false,
    ))
    .unwrap();
    assert_eq!(answer["updatedPermissions"][0]["destination"], "session");
    assert_eq!(answer["decisionClassification"], "user_permanent");
    assert_eq!(answer["updatedInput"], json!({"command":"pwd"}));
    let mut request = request;
    request.permission_suggestions = Some(vec![
        json!({"type":"setMode","mode":"bypassPermissions","destination":"userSettings"}),
    ]);
    let answer = serde_json::to_value(permission_result_from_decision(
        &request,
        ProviderApprovalDecision::AcceptForSession,
        None,
        true,
    ))
    .unwrap();
    assert_eq!(
        answer["updatedPermissions"],
        json!([{"type":"addRules","rules":[{"toolName":"Bash"}],"behavior":"allow","destination":"session"}])
    );
    assert!(matches!(
        permission_result_from_decision(&request, ProviderApprovalDecision::Accept, None, false),
        PermissionDecision::Allow {
            updated_permissions: None,
            ..
        }
    ));
}
#[test]
fn structured_questions_use_question_text_as_answer_keys() {
    let questions = claude_user_input_questions(
        &json!({"questions":[{"question":"  Which option? ","header":"  Pick ","multiSelect":true,"options":[{"label":" One ","description":" "},{"label":"Two","description":"Second","preview":"html"},{"label":" "}]},{"question":" "},{"question":"Another"}]}),
    );
    let questions = serde_json::to_value(questions).unwrap();
    assert_eq!(questions[0]["id"], "Which option?");
    assert_eq!(questions[0]["header"], "Pick");
    assert_eq!(
        questions[0]["options"],
        json!([{"label":"One","description":"One"},{"label":"Two","description":"Second","preview":"html"}])
    );
    assert_eq!(questions[1]["header"], "Question 3");
    assert_eq!(
        claude_sdk_user_input_answers(
            &[
                ("Which option?".into(), json!(["One", 3, "Two"])),
                ("empty".into(), Value::Null)
            ]
            .into_iter()
            .collect()
        ),
        json!({"Which option?":"One, Two","empty":""})
    );
}
#[test]
fn usage_merges_cumulative_counts_and_dates_cache_from_request_start() {
    let initial = json!({"input_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":20,"output_tokens":1,"cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":0}});
    let next = merge_claude_stream_usage(
        &initial,
        &json!({"input_tokens":null,"cache_read_input_tokens":0,"output_tokens":7}),
    );
    assert_eq!(next["input_tokens"], 3);
    assert_eq!(next["cache_read_input_tokens"], 0);
    assert_eq!(next["output_tokens"], 7);
    let usage = serde_json::to_value(
        claude_provider_turn_token_usage(
            &next,
            Some(200000),
            "2026-01-02T04:00:00.000Z",
            Some(150000),
            Some(FIXTURE_DATE),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(usage["usedTokens"], 20);
    assert_eq!(usage["promptCacheTtlMs"], 3600000);
    assert_eq!(usage["promptCacheExpiresAt"], "2026-01-02T04:04:05.000Z");
    assert_eq!(usage["autoCompactThreshold"], 117000);
    assert_eq!(
        add_iso_millis("2024-02-28T23:59:59.999Z", 1).unwrap(),
        "2024-02-29T00:00:00.000Z"
    );
    let capped = serde_json::to_value(
        claude_provider_turn_token_usage(&initial, Some(100000), FIXTURE_DATE, Some(100000), None)
            .unwrap(),
    )
    .unwrap();
    assert!(capped.get("autoCompactThreshold").is_none());
}
#[test]
fn explicit_abort_and_failure_reasons_override_success_envelopes() {
    for reason in ["aborted_tools", "aborted_streaming"] {
        assert_eq!(
            terminal_status_from_result(
                &json!({"subtype":"success","is_error":false,"terminal_reason":reason}),
                None
            ),
            "interrupted"
        );
    }
    for reason in [
        "api_error",
        "budget_exhausted",
        "blocking_limit",
        "prompt_too_long",
        "rapid_refill_breaker",
        "model_error",
        "image_error",
    ] {
        assert_eq!(
            terminal_status_from_result(
                &json!({"subtype":"success","is_error":false,"terminal_reason":reason}),
                None
            ),
            "failed"
        );
    }
    assert_eq!(
        terminal_status_from_result(&json!({"subtype":"success","api_error_status":429}), None),
        "failed"
    );
    assert_eq!(
        terminal_status_from_result(
            &json!({"subtype":"error_during_execution","errors":["interrupted"]}),
            None
        ),
        "interrupted"
    );
    assert_eq!(
        terminal_status_from_result(
            &json!({"subtype":"error_during_execution","errors":["cancelled"]}),
            None
        ),
        "cancelled"
    );
}
#[tokio::test]
async fn recorded_simple_and_read_only_fixtures_use_real_protocol_pipes() {
    for (recording, text, read_count) in [
        (
            include_str!(
                "../../../../../apps/server/src/orchestration-v2/testkit/fixtures/simple/claude_transcript.ndjson"
            ),
            "fixture simple ok",
            0,
        ),
        (
            include_str!(
                "../../../../../apps/server/src/orchestration-v2/testkit/fixtures/tool_call_read_only/claude_transcript.ndjson"
            ),
            "read only tool fixture complete",
            2,
        ),
    ] {
        let mut projection = fixture_projection(
            Arc::new(ManualClock::new(FIXTURE_DATE)),
            Arc::new(SequenceIds::new("replay")),
        );
        let events = replay_messages(recording, &mut projection, fixture_turn_input().unwrap())
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.kind,ProviderEventKind::Message(m)if m.text.contains(text)))
        );
        assert!(events.iter().any(|e| matches!(
            &e.kind,
            ProviderEventKind::Terminal {
                outcome: TerminalOutcome::Completed { .. },
                ..
            }
        )));
        assert_eq!(projection.items.values().filter(|item|matches!(item,t3_contracts::orchestration_v2::OrchestrationV2TurnItem::DynamicTool(tool)if tool.tool_name.as_ref().is_some_and(|n|n.0=="Read"))).count(),read_count);
        let mut tool_nodes = std::collections::BTreeMap::new();
        for event in &events {
            if let ProviderEventKind::Node(node) = &event.kind {
                let value = serde_json::to_value(node).unwrap();
                if value["kind"] == "tool_call" {
                    tool_nodes.insert(value["id"].as_str().unwrap().to_owned(), value);
                }
            }
        }
        assert_eq!(tool_nodes.len(), read_count);
        for node in tool_nodes.values() {
            assert_eq!(node["status"], "completed");
            assert_eq!(node["countsForRun"], false);
            assert!(!node["providerTurnId"].is_null());
            assert!(
                projection
                    .items
                    .values()
                    .any(|item| { serde_json::to_value(item).unwrap()["nodeId"] == node["id"] })
            );
        }
        assert!(projection.requests.is_empty());
        if read_count > 0 {
            let outputs = projection
                .items
                .values()
                .map(|v| serde_json::to_string(v).unwrap())
                .collect::<String>();
            assert!(outputs.contains("claude-read-only-fixture"));
            assert!(outputs.contains("ESNext"));
        }
    }
}

#[test]
fn rollback_cursors_preserve_prompts_and_allow_cursorless_compaction_only() {
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("rollback")),
    );
    let input = fixture_turn_input().unwrap();
    projection.begin(input.clone()).unwrap();
    let mut first = projection.turns.values().next().unwrap().clone();
    first.native_turn_ref =
        Some(serde_json::from_value(native_ref("assistant-1", "weak")).unwrap());
    first.status = t3_contracts::orchestration_v2::ProviderTurnStatus::Completed;
    let mut second = first.clone();
    second.ordinal = 2;
    second.native_turn_ref =
        Some(serde_json::from_value(native_ref("turn:synthetic", "weak")).unwrap());
    let mut later = first.clone();
    later.ordinal = 3;
    let make = |turn, message, turns| RollbackInput {
        provider_thread: input.provider_thread.clone(),
        target: RollbackTarget::ProviderTurn {
            checkpoint_id: t3_contracts::base_schemas::CheckpointId("checkpoint".into()),
            app_run_ordinal: 2,
            provider_turn: Box::new(turn),
            message,
        },
        provider_thread_turns: turns,
    };
    assert_eq!(
        rollback_resume_cursor(&make(
            first.clone(),
            None,
            vec![first.clone(), later.clone()]
        ))
        .unwrap(),
        Some("assistant-1".into())
    );
    assert_eq!(
        rollback_resume_cursor(&make(
            second.clone(),
            None,
            vec![first.clone(), second.clone()]
        ))
        .unwrap(),
        None
    );
    assert!(
        rollback_resume_cursor(&make(
            second.clone(),
            None,
            vec![first.clone(), second.clone(), later.clone()]
        ))
        .is_err()
    );
    let mut compact = input.message.clone();
    compact.text = "/compact".into();
    assert_eq!(
        rollback_resume_cursor(&make(
            second.clone(),
            Some(compact),
            vec![first.clone(), second, later]
        ))
        .unwrap(),
        Some("assistant-1".into())
    );
    first.provider_thread_id = t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
        t3_contracts::base_schemas::ProviderThreadId("another-thread".into()),
    );
    assert!(rollback_resume_cursor(&make(first, None, vec![])).is_err());
    let mut reset = make(
        projection.turns.values().next().unwrap().clone(),
        None,
        vec![],
    );
    reset.target = RollbackTarget::ThreadStart {
        checkpoint_id: t3_contracts::base_schemas::CheckpointId("start".into()),
    };
    assert_eq!(rollback_resume_cursor(&reset).unwrap(), None);
    let mut thread = serde_json::to_value(&reset.provider_thread).unwrap();
    thread["nativeMetadata"] = json!({"forkHeadMessageId":"fork-head"});
    reset.provider_thread = serde_json::from_value(thread).unwrap();
    assert_eq!(
        rollback_resume_cursor(&reset).unwrap(),
        Some("fork-head".into())
    );
}

#[tokio::test]
async fn recorded_claude_messages_project_over_real_protocol_pipes() {
    let root = fixture_recording_root();
    let mut recordings = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("claude_transcript.ndjson"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    recordings.sort();
    assert_eq!(recordings.len(), 39);
    let mut failures = Vec::new();
    for path in recordings {
        let recording = std::fs::read_to_string(&path).unwrap();
        let mut projection = fixture_projection(
            Arc::new(ManualClock::new(FIXTURE_DATE)),
            Arc::new(SequenceIds::new("replay")),
        );
        let events =
            match replay_messages(&recording, &mut projection, fixture_turn_input().unwrap()).await
            {
                Ok(events) => events,
                Err(error) => {
                    failures.push(format!("{}: {error:?}", path.display()));
                    continue;
                }
            };
        if !events.iter().any(|event| {
            matches!(
                event.kind,
                ProviderEventKind::Terminal {
                    outcome: TerminalOutcome::Completed { .. }
                        | TerminalOutcome::Interrupted { .. }
                        | TerminalOutcome::Failed { .. }
                        | TerminalOutcome::Cancelled { .. },
                    ..
                }
            )
        }) {
            failures.push(format!("{}: missing terminal event", path.display()));
        }
        let scenario = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        if scenario == "web_search" {
            let web = projection
                .items
                .values()
                .map(|item| serde_json::to_value(item).unwrap())
                .find(|item| item["type"] == "web_search")
                .unwrap();
            assert_eq!(web["results"].as_array().unwrap().len(), 10);
            assert!(
                web["results"][0]["url"]
                    .as_str()
                    .unwrap()
                    .contains("fifa.com")
            );
        }
        if scenario == "claude_result_is_error" {
            let mut turns = projection.turns.values().collect::<Vec<_>>();
            turns.sort_by_key(|turn| turn.ordinal);
            assert_eq!(
                turns
                    .iter()
                    .map(|turn| serde_json::to_value(&turn.status).unwrap())
                    .collect::<Vec<_>>(),
                vec![json!("failed"), json!("completed")]
            );
            assert!(
                events.iter().any(|event| matches!(&event.kind,
                    ProviderEventKind::Terminal { outcome:TerminalOutcome::Failed{failure,..},.. }
                    if failure.code.as_ref().is_some_and(|code|code.0 == "api_error_401")
                    && failure.message.0 == "Claude could not authenticate. For subscription login, run `claude auth login` on this environment's machine, then start a new thread. For API-key authentication, check this instance's configured credentials."
                )),
                "authentication failure must terminalize the first turn"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn result_usage_recovers_process_baselines_and_clamps_reasoning() {
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("billing")),
    );
    let mut input = fixture_turn_input().unwrap();
    for (index, ledger, expected_other) in [
        (
            0,
            json!({"model":{"inputTokens":100,"outputTokens":20}}),
            Some((76, 15)),
        ),
        (1, json!({}), None),
        (
            2,
            json!({"model":{"inputTokens":300,"outputTokens":60}}),
            None,
        ),
        (
            3,
            json!({"model":{"inputTokens":400,"outputTokens":80}}),
            Some((76, 15)),
        ),
        (
            4,
            json!({"model":{"inputTokens":10,"outputTokens":1}}),
            None,
        ),
        (
            5,
            json!({"model":{"inputTokens":500,"outputTokens":100}}),
            None,
        ),
        (
            6,
            json!({"model":{"inputTokens":600,"outputTokens":120}}),
            Some((76, 15)),
        ),
    ] {
        input.provider_turn_ordinal = index + 1;
        input.run_ordinal = index + 1;
        input.run_id = t3_contracts::base_schemas::RunId(format!("usage-run-{index}"));
        projection.begin(input.clone()).unwrap();
        let frame = json!({"type":"result","subtype":"success","local_command":"compact","usage":{"input_tokens":4,"cache_read_input_tokens":10,"cache_creation_input_tokens":10,"output_tokens":5,"output_tokens_details":{"thinking_tokens":12}},"modelUsage":ledger});
        let observed = projection.observe_result(&frame);
        projection.handle_frame(&observed).unwrap();
        let turn = projection
            .turns
            .values()
            .find(|turn| turn.ordinal == index + 1)
            .unwrap();
        let usage = serde_json::to_value(turn).unwrap()["turnTokenUsage"].clone();
        assert_eq!(usage["inputTokens"], 24);
        assert_eq!(usage["reasoningTokens"], 5);
        assert_eq!(usage["localCommand"], true);
        assert_eq!(
            usage["otherUsage"],
            expected_other
                .map(|(input, output)| json!({"inputTokens":input,"outputTokens":output}))
                .unwrap_or(Value::Null)
        );
    }
    projection.reset_process_usage();
    input.provider_turn_ordinal = 8;
    projection.begin(input).unwrap();
    projection.handle_frame(&json!({"type":"result","subtype":"error_during_execution","usage":{"input_tokens":0,"output_tokens":0}})).unwrap();
    let usage = serde_json::to_value(
        projection
            .turns
            .values()
            .find(|turn| turn.ordinal == 8)
            .unwrap(),
    )
    .unwrap()["turnTokenUsage"]
        .clone();
    assert_eq!(usage["usageStatus"], "unavailable");
    assert!(usage.get("inputTokens").is_none());
}

#[test]
fn opaque_roster_disappearance_retains_completion_until_native_wake_consumes_it() {
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("roster")),
    );
    projection.begin(fixture_turn_input().unwrap()).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_started","task_id":"shell","task_type":"local_bash","tool_use_id":"launch-shell","description":"Build","is_backgrounded":true})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"shell","task_type":"local_bash","description":"Build"}]})).unwrap();
    projection
        .handle_frame(&json!({"type":"result","subtype":"success","result":"Started"}))
        .unwrap();
    projection
        .handle_frame(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[]}))
        .unwrap();
    assert!(!projection.pending_background_work(true));
    assert_eq!(
        serde_json::to_value(projection.thread.as_ref().unwrap()).unwrap()["pendingBackgroundTasks"],
        json!([{"taskId":"shell","kind":"command","description":"Build"}])
    );
    projection.handle_frame(&json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed","summary":"Build completed"})).unwrap();
    projection.consume_native_wake().unwrap();
    assert!(!projection.pending_background_work(true));
    assert!(serde_json::to_value(projection.thread.as_ref().unwrap()).unwrap()["pendingBackgroundTasks"].as_array().unwrap().is_empty());
    let count = projection.items.len();
    projection.handle_frame(&json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed","summary":"Duplicate"})).unwrap();
    assert!(!projection.pending_background_work(true));
    assert_eq!(projection.items.len(), count);
}

#[test]
fn native_agent_rosters_do_not_create_background_command_entries() {
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("agent-roster")),
    );
    projection.begin(fixture_turn_input().unwrap()).unwrap();
    projection.handle_frame(&json!({"type":"assistant","uuid":"launch","message":{"id":"launch","content":[{"type":"tool_use","id":"agent-tool","name":"Agent","input":{"prompt":"Child","run_in_background":true}}]}})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"agent","task_type":"local_agent","description":"Child"}]})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_started","task_id":"agent","task_type":"local_agent","tool_use_id":"agent-tool","is_backgrounded":true,"prompt":"Child","description":"Child"})).unwrap();
    projection
        .handle_frame(&json!({"type":"result","subtype":"success","result":"Started"}))
        .unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_notification","task_id":"agent","status":"completed","summary":"Child complete"})).unwrap();
    assert!(!projection.pending_background_work(true));
    assert!(serde_json::to_value(projection.thread.as_ref().unwrap()).unwrap()["pendingBackgroundTasks"].as_array().unwrap().is_empty());
    projection.consume_native_wake().unwrap();
    let duplicate = json!({"type":"system","subtype":"task_notification","task_id":"agent","status":"completed","summary":"Child complete"});
    projection.handle_frame(&duplicate).unwrap();
    assert!(
        projection
            .native_wake_notification(&duplicate)
            .unwrap()
            .is_none()
    );
}

#[test]
fn send_message_resume_reuses_child_thread_and_appends_ordered_conversation() {
    let mut projection = fixture_projection(
        Arc::new(ManualClock::new(FIXTURE_DATE)),
        Arc::new(SequenceIds::new("resume-child")),
    );
    let first = fixture_turn_input().unwrap();
    projection.begin(first.clone()).unwrap();
    projection.handle_frame(&json!({"type":"assistant","uuid":"launch","message":{"id":"launch","content":[{"type":"tool_use","id":"original-tool","name":"Agent","input":{"prompt":"Reply A_FIRST","model":"haiku"}}]}})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_started","task_id":"agent","task_type":"local_agent","tool_use_id":"original-tool","prompt":"Reply A_FIRST","description":"Agent A"})).unwrap();
    projection.handle_frame(&json!({"type":"assistant","uuid":"first-child","parent_tool_use_id":"original-tool","message":{"id":"first-child","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"A_FIRST"}]}})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_notification","task_id":"agent","status":"completed","summary":"A_FIRST"})).unwrap();
    projection
        .handle_frame(&json!({"type":"result","subtype":"success","result":"LAUNCHED"}))
        .unwrap();
    let mut second = first.clone();
    second.run_id = t3_contracts::base_schemas::RunId("resume-run".into());
    second.root_node_id = t3_contracts::base_schemas::NodeId("resume-root".into());
    second.attempt_id = t3_contracts::base_schemas::RunAttemptId("resume-attempt".into());
    second.run_ordinal = 2;
    second.provider_turn_ordinal = 2;
    projection.begin(second.clone()).unwrap();
    projection.handle_frame(&json!({"type":"assistant","uuid":"send","message":{"id":"send","content":[{"type":"tool_use","id":"resume-tool","name":"SendMessage","input":{"to":"agent","message":"Reply A_SECOND"}}]}})).unwrap();
    let events=projection.handle_frame(&json!({"type":"system","subtype":"task_started","task_id":"agent","task_type":"local_agent","tool_use_id":"resume-tool","prompt":"Reply A_SECOND","description":"Agent A"})).unwrap();
    let task = events
        .iter()
        .find_map(|event| match &event.kind {
            ProviderEventKind::Subagent(task) => Some(serde_json::to_value(task).unwrap()),
            _ => None,
        })
        .unwrap();
    assert_eq!(task["runId"], "resume-run");
    assert_eq!(task["result"], Value::Null);
    assert_eq!(task["model"], "claude-haiku-4-5");
    assert_eq!(task["id"], provider_id("node", "native-item", "task:agent"));
    let node = events
        .iter()
        .find_map(|event| match &event.kind {
            ProviderEventKind::Node(node)
                if serde_json::to_value(&node.id).unwrap() == task["id"] =>
            {
                Some(serde_json::to_value(node).unwrap())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        node["rootNodeId"],
        serde_json::to_value(&first.root_node_id).unwrap()
    );
    projection.handle_frame(&json!({"type":"assistant","uuid":"second-child","parent_tool_use_id":"original-tool","message":{"id":"second-child","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"A_SECOND"}]}})).unwrap();
    projection.handle_frame(&json!({"type":"system","subtype":"task_notification","task_id":"agent","status":"completed","summary":"A_SECOND"})).unwrap();
    projection
        .handle_frame(&json!({"type":"result","subtype":"success","result":"RESUMED"}))
        .unwrap();
    let mut conversation = projection
        .items
        .values()
        .map(|item| serde_json::to_value(item).unwrap())
        .filter(|item| {
            item["threadId"] == task["childThreadId"]
                && matches!(
                    item["type"].as_str(),
                    Some("user_message" | "assistant_message")
                )
        })
        .collect::<Vec<_>>();
    conversation.sort_by_key(|item| item["ordinal"].as_u64().unwrap());
    assert_eq!(
        conversation
            .iter()
            .map(|item| item["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Reply A_FIRST", "A_FIRST", "Reply A_SECOND", "A_SECOND"]
    );
    assert!(
        conversation
            .iter()
            .all(|item| item["runId"].is_null() && item["providerTurnId"].is_null())
    );
    assert_eq!(projection.turns.len(), 2);
}

mod composed_claude {
    use async_trait::async_trait;
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};
    use t3_claude_cli::launch::LaunchOptions;
    use t3_contracts::{orchestration_v2::*, provider_instance::ProviderInstanceId};
    use t3_orchestration::{
        adapters::claude::*,
        api::*,
        composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
        error::{Error, Result},
        ports::{
            acp_replay::testkit::{
                deterministic_runtime::{DEFAULT_RANDOM_SEED, SeededIds},
                fixtures::shared,
                orchestrator_scenario::{self, ScenarioServices},
            },
            checkpoints_forks::checkpoint_service::CheckpointMetadata,
            claude::adapters::claude_adapter_v2_testkit::*,
        },
        runtime::sessions::AdapterRegistry,
        testing::{
            ManualClock, TestDatabase,
            replay::{ReplayGate, decode_ndjson},
        },
    };
    use tokio_util::sync::CancellationToken;
    struct AdmittedRuntime(Arc<OrchestrationRuntime>);
    #[async_trait]
    impl CommandDispatcher for AdmittedRuntime {
        async fn dispatch(
            &self,
            command: OrchestrationV2ServerCommand,
        ) -> Result<t3_orchestration::stores::receipts::CommandReceipt> {
            self.0.dispatch_command(command).await
        }
    }
    struct NoRepository;

    #[async_trait]
    impl CheckpointBackend for NoRepository {
        async fn capture(
            &self,
            _input: CaptureCheckpoint,
            _cancellation: CancellationToken,
        ) -> Result<CheckpointRef> {
            Err(Error::Unsupported("capture outside a repository"))
        }
        async fn lookup(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _ordinal: u64,
        ) -> Result<Option<CheckpointRef>> {
            Ok(None)
        }
        async fn diff(
            &self,
            _cwd: PathBuf,
            _from: CheckpointRef,
            _to: CheckpointRef,
        ) -> Result<CheckpointDiff> {
            Err(Error::Unsupported("diff outside a repository"))
        }
        async fn restore(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _ordinal: u64,
            _cancellation: CancellationToken,
        ) -> Result<()> {
            Err(Error::Unsupported("restore outside a repository"))
        }
        async fn prune(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _preserve: Vec<u64>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl CheckpointMetadata for NoRepository {
        fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
            CheckpointRef(format!("refs/t3/test/{}/{ordinal}", scope.0))
        }
        async fn is_repository(&self, _cwd: PathBuf) -> Result<bool> {
            Ok(false)
        }
        async fn delete_refs(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _checkpoints: Vec<OrchestrationV2Checkpoint>,
        ) -> Result<()> {
            Ok(())
        }
    }

    struct FixturePolicy {
        cwd: PathBuf,
        overrides: serde_json::Value,
    }
    #[async_trait]
    impl RuntimePolicyResolver for FixturePolicy {
        async fn resolve(
            &self,
            thread: OrchestrationV2AppThread,
            selection: t3_contracts::model_selection::ModelSelection,
        ) -> Result<t3_orchestration::provider::ConfiguredRuntimePolicy> {
            use t3_orchestration::composition::BaseRuntimePolicy;
            let mut resolved = BaseRuntimePolicy.resolve(thread, selection).await?;
            resolved.policy.cwd = Some(self.cwd.clone());
            resolved.policy.approval_policy = self.overrides.get("approvalPolicy").cloned();
            resolved.policy.sandbox_policy = self.overrides.get("sandboxPolicy").cloned();
            Ok(resolved)
        }
    }

    async fn run_recording(name: &str) -> Result<()> {
        let definitions = t3_orchestration::ports::acp_replay::testkit::fixtures::index::replay_fixture_definitions()?;
        let definition = definitions
            .iter()
            .find(|fixture| fixture.name == name)
            .expect("registered fixture");
        let variant = definition
            .providers
            .iter()
            .find(|variant| variant.driver.0 == "claudeAgent")
            .expect("Claude variant");
        let input = build_claude_fixture_input(definition)?;
        let selection = variant.model_selection.clone();
        let transcript = decode_ndjson(&std::fs::read_to_string(
            fixture_recording_root().join(format!("{name}/claude_transcript.ndjson")),
        )?)
        .map_err(|e| Error::Service {
            operation: "Claude recording",
            detail: e.to_string(),
        })?;
        let labels = input
            .steps
            .iter()
            .filter_map(|step| serde_json::to_value(step).ok())
            .filter_map(|step| step["label"].as_str().map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>();
        let gate = ReplayGate::new(labels).map_err(|e| Error::Service {
            operation: "Claude gate",
            detail: e.to_string(),
        })?;
        let connector = Arc::new(ClaudeReplayConnector::new(&transcript, gate.clone())?);
        let clock = Arc::new(ManualClock::new(FIXTURE_DATE));
        let ids = Arc::new(SeededIds::new(DEFAULT_RANDOM_SEED));
        let native = transcript
            .metadata
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|m| m.get("nativeSessionId"))
            .and_then(serde_json::Value::as_str)
            .expect("recorded native session")
            .to_owned();
        let (continuation_sender, continuation_receiver) = tokio::sync::mpsc::channel(16);
        let registration = claude_registration(
            ClaudeAdapterOptions {
                instance_id: ProviderInstanceId("claudeAgent".into()),
                launch: LaunchOptions {
                    cli_path: Some("fixture-claude".into()),
                    ..Default::default()
                },
                connector: connector.clone(),
                clock: clock.clone(),
                ids: ids.clone(),
                new_native_session_id: Arc::new(move || native.clone()),
                mcp_servers: Default::default(),
                transcript_root: None,
                attachments: Arc::new(DirectoryClaudeAttachments::default()),
                host: ClaudeHostOptions {
                    continuation_requests: Some(continuation_sender),
                    ..Default::default()
                },
            },
            "claudeAgent:instance:claudeAgent".into(),
            true,
            Vec::new(),
        );
        let materialized =
            shared::materialize_fixture_input(name, &input, "claudeAgent", &selection, &ids)?;
        let project = serde_json::to_value(&materialized.commands[0])?["projectId"].clone();
        let scenario = materialized.into_scenario(format!("Claude {name}"))?;
        let database = TestDatabase::temporary()?;
        let workspace = tempfile::tempdir()?;
        let mut ports = RuntimePorts::new(
            database.stores.clone(),
            Arc::new(AdapterRegistry::new(vec![registration])),
            Arc::new(NoRepository),
            clock.clone(),
        );
        ports.ids = ids.clone();
        ports.policy = Arc::new(FixturePolicy {
            cwd: workspace.path().to_owned(),
            overrides: variant
                .runtime_policy_override
                .clone()
                .unwrap_or_else(|| json!({})),
        });
        let mut settings = serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
        settings["providerInstances"]["claudeAgent"] =
            json!({"driver":"claudeAgent","enabled":true});
        ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
        let runtime = Arc::new(OrchestrationRuntime::start(ports).await?);
        let services = ScenarioServices {
            database: database.stores.clone(),
            dispatcher: Arc::new(AdmittedRuntime(runtime.clone())),
            committer: Arc::new(runtime.events.clone()),
            projections: runtime.projections.clone(),
            worker: runtime.effect_worker(),
            clock,
            gate,
        };
        let continuations = Arc::new(
            t3_orchestration::runtime::sessions::ProviderContinuationService::new(
                runtime.projections.clone(),
                services.dispatcher.clone(),
                Arc::new(runtime.events.clone()),
                ids,
            ),
        );
        let cancellation = CancellationToken::new();
        let (continuation_error_sender, mut continuation_error) = tokio::sync::mpsc::channel(1);
        let continuation_task = {
            let service = continuations.clone();
            let cancel = cancellation.clone();
            tokio::spawn(async move {
                let mut requests = continuation_receiver;
                loop {
                    let request = tokio::select! { _ = cancel.cancelled() => break, request = requests.recv() => match request { Some(request) => request, None => break } };
                    if let Err(error) = service.process(&request).await {
                        let _ = continuation_error_sender.send(error).await;
                        break;
                    }
                }
                Ok::<_, Error>(())
            })
        };
        let result=async {
            runtime.project_command(serde_json::from_value(json!({"type":"project.create","commandId":"claude-fixture-setup","projectId":project,"title":"Claude recording","workspaceRoot":workspace.path()}))?).await?;
            let result=tokio::select!{result=orchestrator_scenario::run_scenario(scenario,&services)=>result?,error=connector.failed()=>return Err(Error::Provider(error)),error=continuation_error.recv()=>return Err(error.expect("continuation channel closed"))};
            if let Err(error) = connector.assert_complete() {
                for projection in result.projections.values() {
                    for item in &projection.turn_items {
                        if let t3_contracts::orchestration_v2::OrchestrationV2TurnItem::Error(error) = item { eprintln!("Claude scenario failure: {:?}", error.failure); }
                    }
                }
                return Err(error.into());
            }
            assert_claude_fixture_output(&variant.assert_output, &result, &transcript)
        }.await;
        cancellation.cancel();
        continuations.shutdown().await;
        continuation_task
            .await
            .expect("continuation worker panicked")?;
        let shutdown = runtime.shutdown().await;
        connector.join().await;
        result?;
        shutdown
    }
    #[tokio::test(start_paused = true)]
    async fn composed_recording_simple() -> Result<()> {
        run_recording("simple").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_tool_call_read_only() -> Result<()> {
        run_recording("tool_call_read_only").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_multi_turn() -> Result<()> {
        run_recording("multi_turn").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_web_search() -> Result<()> {
        run_recording("web_search").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_claude_result_is_error() -> Result<()> {
        run_recording("claude_result_is_error").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_claude_nested_subagent_model() -> Result<()> {
        run_recording("claude_nested_subagent_model").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_subagent() -> Result<()> {
        run_recording("subagent").await
    }
    #[tokio::test(start_paused = true)]
    async fn composed_recording_tool_call_denied_write() -> Result<()> {
        run_recording("tool_call_denied_write").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_tool_call_restricted_granular() -> Result<()> {
        run_recording("tool_call_restricted_granular").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_tool_call_workspace_never() -> Result<()> {
        run_recording("tool_call_workspace_never").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_claude_local_bash_task() -> Result<()> {
        run_recording("claude_local_bash_task").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_turn_interrupt() -> Result<()> {
        run_recording("turn_interrupt").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_turn_interrupt_mid_tool() -> Result<()> {
        run_recording("turn_interrupt_mid_tool").await
    }

    #[tokio::test(start_paused = true)]
    async fn composed_recording_turn_interrupt_restart() -> Result<()> {
        run_recording("turn_interrupt_restart").await
    }
}
