#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::io::{BufReader, Cursor};
use t3_contracts::{
    agent_sessions::*, project_clone::ProjectCloneStage, provider_instance::ProviderInstanceId,
};
use t3_project::{
    clones::parse_git_clone_progress_line, session_json::*, sessions::same_transcript_identity,
    transcript::*,
};

#[test]
fn projected_json_matches_duplicate_keys_at_every_chunk_boundary() {
    let inputs = [
        r#"{"a":1,"a":2,"b":"before","b":"after"}"#,
        r#"{"a":{"x":1},"a":{"y":2},"b":[],"c":{}}"#,
        r#"{"a":[null,true,false,1,-2.3e4,"😀\u0061\\\"",{},[],[1,2]]}"#,
        r#"{"a":"s","a":null,"b":null,"b":"s","c":0,"c":false}"#,
        r#"{"__proto__":{"polluted":true},"constructor":1,"__proto__":2}"#,
    ];
    for size in [1, 2, 7, 64, 1024] {
        for input in inputs {
            let actual = read_projected_json(
                BufReader::with_capacity(size, Cursor::new(input.as_bytes())),
                &mut (32 * 1024 * 1024),
                |_| true,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                actual,
                serde_json::from_str::<Value>(input).unwrap(),
                "chunk size {size}"
            );
        }
    }
}
#[test]
fn projection_skips_payloads_and_does_not_merge_duplicate_parents() {
    let input = r#"{"message":{"usage":{"input":100},"content":"large"},"message":{"usage":{"output":5},"content":[1,2]},"rows":[{"keep":1,"drop":2},{"keep":3}],"drop":{"keep":4}}"#;
    let value = read_projected_json(Cursor::new(input), &mut 10000, |p| {
        !p.iter()
            .any(|p| matches!(p,JsonPathPart::Key(k) if k=="content"||k=="drop"))
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        value,
        json!({"message":{"usage":{"output":5}},"rows":[{"keep":1},{"keep":3}]})
    );
    let big = format!(
        "{{\"image\":\"{}\",\"message\":{{\"content\":\"keep\"}}}}",
        "x".repeat(4 * 1024 * 1024)
    );
    assert_eq!(
        read_projected_json(
            BufReader::with_capacity(17, Cursor::new(big)),
            &mut 4096,
            select_transcript_path
        )
        .unwrap()
        .unwrap(),
        json!({"message":{"content":"keep"}})
    );
}
#[test]
fn malformed_json_and_resource_limits_fail_closed() {
    for input in [
        r#"{"a":"#,
        r#"{"a":1} trailing"#,
        r#"{"a":1}{"a":2}"#,
        r#"{"a":"bad\x"}"#,
        r#"{"a":[1,]}"#,
    ] {
        assert_eq!(
            read_projected_json(Cursor::new(input), &mut 10000, |_| true).unwrap(),
            None,
            "{input}"
        );
    }
    assert!(matches!(
        read_projected_json(Cursor::new("{\"a\":1}"), &mut 0, |_| true),
        Err(TranscriptJsonError::Budget)
    ));
    let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    assert!(matches!(
        read_projected_json(Cursor::new(&deep), &mut 100000, |_| false),
        Err(TranscriptJsonError::Depth)
    ));
}
#[test]
fn clone_progress_handles_cr_redraws_bounds_and_hostile_tails() {
    let cases = [
        (
            "remote: Enumerating objects: 10, done.",
            ProjectCloneStage::Counting,
            None,
            None,
        ),
        (
            "Receiving objects:  42% (42/100), 3 MiB | 1 MiB/s",
            ProjectCloneStage::Receiving,
            Some(42),
            Some("3 MiB | 1 MiB/s"),
        ),
        (
            "Resolving deltas: 999% (2/2), done.",
            ProjectCloneStage::Resolving,
            Some(100),
            None,
        ),
        (
            "Updating files: 12% (1/10)garbage",
            ProjectCloneStage::Checkout,
            None,
            None,
        ),
    ];
    for (line, stage, percent, detail) in cases {
        let p = parse_git_clone_progress_line(line).unwrap();
        assert_eq!(p.stage, stage);
        assert_eq!(p.percent, percent);
        assert_eq!(p.detail.as_deref(), detail);
    }
    assert!(parse_git_clone_progress_line("fatal: no access").is_none());
    let hostile = format!(
        "Receiving objects: {}% (1/1){}",
        "9".repeat(100000),
        " ".repeat(100000)
    );
    assert_eq!(
        parse_git_clone_progress_line(&hostile).unwrap().percent,
        None
    );
}
fn parse(source: AgentSessionSource, value: &str) -> SessionThread {
    parse_agent_session_transcript(
        source,
        ProviderInstanceId("fixture".into()),
        "fallback",
        "2026-01-01T00:00:00.000Z",
        value,
    )
    .unwrap()
}
#[test]
fn claude_uses_visible_latest_fields_and_custom_rename() {
    let input=[json!({"type":"user","sessionId":"actual","aiTitle":"Generated","message":{"content":" first request "}}),json!({"type":"assistant","customTitle":"My title","message":{"content":[{"type":"text","text":"answer"},{"type":"tool_result","text":"ignored"}],"model":"model-a"}}),json!({"type":"assistant","isMeta":true,"sessionId":"wrong","message":{"content":"hidden","model":"wrong"}}),json!({"type":"assistant","aiTitle":"Later generated","message":{"content":"local error","model":"<synthetic>"}})].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    let thread = parse(AgentSessionSource::ClaudeAgent, &input);
    assert_eq!(thread.provider_session_id, "actual");
    assert_eq!(thread.title, "My title");
    assert_eq!(thread.model.as_deref(), Some("model-a"));
    assert_eq!(
        thread
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        vec!["first request", "answer", "local error"]
    );
}
#[test]
fn transcript_window_keeps_first_user_and_latest_199_messages() {
    let input=(0..205).map(|i|json!({"type":if i==0{"user"}else{"assistant"},"message":{"content":format!("message {i}")}}).to_string()).collect::<Vec<_>>().join("\n");
    let thread = parse(AgentSessionSource::ClaudeAgent, &input);
    assert_eq!(thread.messages.len(), 200);
    assert_eq!(thread.messages[0].text, "message 0");
    assert_eq!(thread.messages[1].text, "message 6");
    assert_eq!(thread.messages[199].text, "message 204");
    assert_eq!(thread.omitted_message_count, 5);
}
#[test]
fn codex_deduplicates_canonical_user_copies_within_each_turn_only() {
    let records = [
        json!({"type":"session_meta","payload":{"id":"native"}}),
        json!({"type":"turn_context","payload":{"model":"chosen"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"hello"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"hello"}}),
    ];
    let thread = parse(
        AgentSessionSource::Codex,
        &records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert_eq!(thread.provider_session_id, "native");
    assert_eq!(thread.model.as_deref(), Some("chosen"));
    assert_eq!(
        thread
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "answer", "hello"]
    );
    assert!(
        parse_agent_session_transcript(
            AgentSessionSource::Codex,
            ProviderInstanceId("fixture".into()),
            "filename",
            "2026-01-01T00:00:00.000Z",
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"request"}}"#
        )
        .is_none()
    );
}
#[test]
fn pi_follows_active_tree_and_honors_latest_session_name() {
    let records = [
        json!({"type":"session","id":"native"}),
        json!({"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"request"}}),
        json!({"type":"message","id":"abandoned","parentId":"a","message":{"role":"assistant","content":"wrong"}}),
        json!({"type":"session_info","id":"name","parentId":"a","name":"Renamed"}),
        json!({"type":"message","id":"b","parentId":"name","message":{"role":"assistant","content":"right"}}),
    ];
    let thread = parse(
        AgentSessionSource::Pi,
        &records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert_eq!(thread.title, "Renamed");
    assert_eq!(
        thread
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        vec!["request", "right"]
    );
}
#[test]
fn transcript_identity_distinguishes_replacements_and_unsafe_legacy_file_ids() {
    let original = json!({"provider":"codex","providerInstanceId":"fixture","providerSessionId":"native","filePath":"fixture.jsonl","size":100,"mtimeMs":1000,"device":1,"inode":"42","birthtimeMs":500});
    let a: AgentSessionImportSource = serde_json::from_value(original.clone()).unwrap();
    let mut missing = original.clone();
    missing["inode"] = Value::Null;
    let mut literal_null = original.clone();
    literal_null["inode"] = json!("null");
    assert!(
        !same_transcript_identity(
            &serde_json::from_value(missing).unwrap(),
            &serde_json::from_value(literal_null).unwrap()
        ),
        "a missing inode is distinct from a literal string"
    );
    let mut numeric = original.clone();
    numeric["inode"] = json!(42);
    let b = serde_json::from_value(numeric).unwrap();
    assert!(same_transcript_identity(&a, &b));
    for (key, value) in [
        ("size", json!(101)),
        ("mtimeMs", json!(1001)),
        ("device", json!(2)),
        ("inode", json!("43")),
        ("birthtimeMs", json!(501)),
        ("inode", json!(9_007_199_254_740_992u64)),
    ] {
        let mut changed = original.clone();
        changed[key] = value;
        assert!(
            !same_transcript_identity(&a, &serde_json::from_value(changed).unwrap()),
            "{key}"
        );
    }
}
