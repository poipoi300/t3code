#![cfg(feature = "acp-adapter")]

use serde_json::{Value, json};
use t3_contracts::provider_policy::RuntimeMode;
use t3_orchestration::ports::claude::adapters::antigravity_adapter_v2::*;

#[test]
fn successful_subagent_launches_stay_running_and_preserve_permission_prompt_details() {
    let update = antigravity_subagent_update(
        &json!({"toolCallId":"trajectory:4", "title":"Running start_subagent",
        "kind":"other", "status":"completed", "data":{"rawOutput":"Started two agents."}}),
    )
    .unwrap();
    assert_eq!(update["status"], "running");
    assert_eq!(update["prompt"], "Started two agents.");
    assert!(update["result"].is_null());
    let update = antigravity_subagent_update(
        &json!({"toolCallId":"launch", "title":"Run invoke_subagent?",
        "kind":"other", "status":"pending", "data":{"rawInput":{"Subagents":[
            {"TypeName":"research", "Role":"Readme Reader", "Prompt":"Read README.md"}]}}}),
    )
    .unwrap();
    assert_eq!(update["title"], "Readme Reader");
    assert_eq!(update["prompt"], "Read README.md");
    assert!(
        antigravity_subagent_update(
            &json!({"title":"Run invoke_subagent?", "data":{"meta":{"is_mcp_tool_call":true}}})
        )
        .is_none()
    );
}

#[test]
fn native_interactions_only_accept_the_agents_choices() {
    let mut request = json!({"toolCall":{"toolCallId":"interaction_1", "title":"Which branch?"}, "options":[
        {"optionId":"opt-main", "name":"main", "kind":"allow_once"},
        {"optionId":"opt-dev", "name":"dev", "kind":"allow_once"}]});
    let question =
        serde_json::to_value(antigravity_user_input_question(&request).unwrap()).unwrap();
    assert_eq!(question["allowCustomAnswer"], false);
    assert_eq!(
        question["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| &o["value"])
            .collect::<Vec<_>>(),
        vec!["opt-main", "opt-dev"]
    );
    for answer in [json!("opt-dev"), json!("dev"), json!(["opt-dev"])] {
        assert_eq!(
            antigravity_user_input_response(&request, &json!({"interaction_1":answer})),
            Some(json!({"outcome":{"outcome":"selected", "optionId":"opt-dev"}}))
        );
    }
    for answer in [json!("nope"), json!(["opt-main", "opt-dev"]), Value::Null] {
        assert!(
            antigravity_user_input_response(&request, &json!({"interaction_1":answer})).is_none()
        );
    }
    assert!(antigravity_approval_options(&request).unwrap().is_empty());
    request["options"][1]["optionId"] = json!("opt-main");
    assert!(antigravity_user_input_question(&request).is_none());
    request["toolCall"]["toolCallId"] = json!("tool_1");
    assert!(antigravity_user_input_question(&request).is_none());
}

#[test]
fn served_reads_repair_step_errors_in_order_and_reset_between_turns() {
    let mut repair = AntigravityReadRepair::default();
    let read = |id: &str| {
        json!({"sessionId":"session", "update":{"sessionUpdate":"tool_call", "toolCallId":id,
        "title":"Running client_view_file", "status":"in_progress", "rawInput":{"absolute_path":"/a"}}})
    };
    let failed = |id: &str| {
        json!({"sessionId":"session", "update":{"sessionUpdate":"tool_call_update", "toolCallId":id,
        "status":"failed", "rawOutput":"Unrelated step error"}})
    };
    repair.repair(read("first"));
    repair.repair(read("second"));
    assert_eq!(repair.attempted("/a", false), None);
    assert_eq!(
        repair.attempted("/a", true),
        Some(("session".into(), "second".into()))
    );
    assert_eq!(repair.repair(failed("first"))["update"]["status"], "failed");
    let second = repair.repair(failed("second"));
    assert_eq!(second["update"]["status"], "completed");
    assert!(second["update"].get("rawOutput").is_none());
    repair.attempted("/a", true);
    assert_eq!(
        repair.repair(read("early"))["update"]["status"],
        "completed"
    );
    repair.reset();
    assert_eq!(repair.repair(failed("early"))["update"]["status"], "failed");
}

#[test]
fn managed_modes_keep_the_cli_asking_and_approval_warnings_reach_clients() {
    assert_eq!(
        ANTIGRAVITY_SUPPORTED_RUNTIME_MODES,
        [
            RuntimeMode::ApprovalRequired,
            RuntimeMode::AutoAcceptEdits,
            RuntimeMode::FullAccess
        ],
    );
    for (mode, native) in [
        (RuntimeMode::FullAccess, "yolo"),
        (RuntimeMode::AutoAcceptEdits, "auto_edit"),
        (RuntimeMode::Auto, "default"),
        (RuntimeMode::ApprovalRequired, "default"),
    ] {
        assert_eq!(antigravity_permission_mode(mode), native);
        assert_eq!(antigravity_cli_session_mode(native), "default");
    }
    assert_eq!(antigravity_runtime_mode_for_session_mode("unknown"), None);
    let options = antigravity_approval_options(&json!({"toolCall":{"toolCallId":"tool"}, "options":[
        {"kind":"allow_always", "optionId":"always", "_meta":{"agy.security.warning":{"message":"  Grants broad access  "}}},
        {"kind":"allow_once", "optionId":"once"}, {"kind":"reject_once", "optionId":"deny"}]})).unwrap();
    let options = serde_json::to_value(options).unwrap();
    assert_eq!(options[1]["warning"], "Grants broad access");
    assert_eq!(
        options
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["decision"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["accept", "acceptForSession", "decline", "cancel"]
    );
}

#[test]
fn client_reads_are_confined_utf8_sliced_and_never_write() {
    let workspace = tempfile::tempdir().unwrap();
    let attachments = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let roots = vec![workspace.path().to_owned(), attachments.path().to_owned()];
    let inside = workspace.path().join("inside.txt");
    std::fs::write(&inside, "\u{feff}first\né中🙂\nlast").unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &inside, None, None).unwrap(),
        "first\né中🙂\nlast"
    );
    assert_eq!(
        read_antigravity_client_text_file(&roots, &inside, Some(2), Some(1)).unwrap(),
        "é中🙂"
    );
    let pasted = attachments.path().join("pasted.txt");
    std::fs::write(&pasted, "pasted").unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &pasted, None, None).unwrap(),
        "pasted"
    );
    for bytes in [vec![0x89, 0x50, 0x4e, 0x47], vec![b'a', 0, b'b']] {
        std::fs::write(&inside, bytes).unwrap();
        assert_eq!(
            read_antigravity_client_text_file(&roots, &inside, None, None)
                .unwrap_err()
                .code,
            -32602
        );
    }
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &secret, None, None)
            .unwrap_err()
            .code,
        -32602
    );
    assert_eq!(
        read_antigravity_client_text_file(
            &roots,
            &outside.path().join("missing/secret.txt"),
            None,
            None
        )
        .unwrap_err()
        .code,
        -32602
    );
    assert_eq!(
        read_antigravity_client_text_file(
            &roots,
            &workspace.path().join("missing.txt"),
            None,
            None
        )
        .unwrap_err()
        .code,
        -32002
    );
    let mut sparse = std::fs::File::create(&inside).unwrap();
    use std::io::Write;
    sparse.write_all(b"a").unwrap();
    sparse
        .set_len(ANTIGRAVITY_CLIENT_FILE_MAX_BYTES + 1)
        .unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &inside, None, None)
            .unwrap_err()
            .code,
        -32602
    );
}

#[cfg(windows)]
#[test]
fn reads_follow_inside_links_but_refuse_escaping_and_dangling_links() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let roots = vec![workspace.path().to_owned()];
    let inside = workspace.path().join("inside.txt");
    let secret = outside.path().join("secret.txt");
    std::fs::write(&inside, "inside").unwrap();
    std::fs::write(&secret, "secret").unwrap();
    let link = workspace.path().join("link.txt");
    std::os::windows::fs::symlink_file(&inside, &link).unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &link, None, None).unwrap(),
        "inside"
    );
    std::fs::remove_file(&link).unwrap();
    std::os::windows::fs::symlink_file(&secret, &link).unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &link, None, None)
            .unwrap_err()
            .code,
        -32602
    );
    std::fs::remove_file(&secret).unwrap();
    assert_eq!(
        read_antigravity_client_text_file(&roots, &link, None, None)
            .unwrap_err()
            .code,
        -32602
    );
}
