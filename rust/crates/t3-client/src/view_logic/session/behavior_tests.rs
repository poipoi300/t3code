#![allow(clippy::unwrap_used)]
use super::{links::*, markdown::*, questions::*, terminal::*, text::*, version::*, *};
use crate::work_log::*;
use crate::{
    thread_views::questions::{Answer, DraftAnswer, Question, QuestionOption},
    types::ThreadKey,
};
use serde_json::json;
use t3_contracts::environment::ServerInstallation;

#[test]
fn question_attachment_budgets_and_tracking_remain_scoped() {
    let key = ThreadKey {
        environment: EnvironmentId("environment-1".into()),
        thread: ThreadId("thread-1".into()),
    };
    let first = question_attachment_draft_id(&key, "request", "first");
    let second = question_attachment_draft_id(&key, "request", "second");
    let other = ThreadKey {
        environment: EnvironmentId(format!("environment-1:question-{}-nested", key.thread.0)),
        ..key.clone()
    };
    assert!(
        !question_attachment_draft_id(&other, "request", "first")
            .starts_with(&question_attachment_draft_prefix(&key))
    );
    let mut state = QuestionAttachmentState::default();
    state.change_preparation(&first, -1);
    assert!(state.preparation_counts.is_empty());
    state.change_preparation(&second, 1);
    let keys = vec![first.clone(), second.clone()];
    assert_eq!(state.count(&keys, |k| if k == first { 1 } else { 6 }), 8);
    let before = Rc::new(OpenQuestionAttachmentDraft {
        draft_id: first.clone(),
        request_keys: keys.clone(),
    });
    state.track(key.clone(), before.clone());
    let after = Rc::new((*before).clone());
    state.track(key.clone(), after.clone());
    state.untrack(&key, &before);
    assert!(state.open(&key).is_some());
    assert_eq!(
        state.count_request(&first, |k| usize::from(k == first)),
        Some(2)
    );
    state.untrack(&key, &after);
    assert!(state.open(&key).is_none());
    state.clear(&second);
    assert_eq!(state.count(&keys, |_| 0), 0);
}
#[test]
fn question_progress_preserves_exact_edits_and_attachment_gates() {
    let question = Question {
        id: "q".into(),
        options: vec![QuestionOption {
            label: "Recommended".into(),
            value: Some(" exact\t".into()),
        }],
        multi_select: false,
        allow_custom_answer: true,
        initial_answer: Some("Suggested".into()),
    };
    let questions = [question.clone()];
    let drafts = BTreeMap::from([(
        "q".into(),
        DraftAnswer {
            custom_answer: Some(String::new()),
            ..Default::default()
        },
    )]);
    let p = pending_user_input_progress(&questions, &drafts, 99);
    assert_eq!(p.resolved_answer, Some(Answer::Text(String::new())));
    assert!(p.can_advance && p.is_complete && p.is_last_question);
    assert_eq!(p.question_index, 0);
    let mut drafts = drafts;
    drafts.get_mut("q").unwrap().attachments_blocked = true;
    assert!(!pending_user_input_progress(&questions, &drafts, -1).can_advance);
    let mut question = question;
    question.initial_answer = None;
    let questions = [question];
    let drafts = BTreeMap::from([(
        "q".into(),
        DraftAnswer {
            selected_option_values: vec![" exact\t".into()],
            ..Default::default()
        },
    )]);
    assert_eq!(
        pending_user_input_progress(&questions, &drafts, 0).resolved_answer,
        Some(Answer::Text(" exact\t".into()))
    );
    assert!(pending_user_input_progress(&[], &BTreeMap::new(), 0).is_complete);
}
#[test]
fn unsent_answer_recovery_transfers_only_live_chip_references() {
    let record:t3_contracts::composer_context::ThreadContextRecord=serde_json::from_value(json!({"version":1,"kind":"thread","contextId":"kept","label":"Thread","environmentId":"remote","threadId":"reference","title":"Thread"})).unwrap();
    let text = "Compare [Thread](t3-context://v1/thread/kept)";
    let records = [record];
    let result = return_unsent_answers_to_thread_draft(
        "Draft",
        [
            (Some("  "), records.as_slice()),
            (Some(text), records.as_slice()),
        ],
    );
    assert_eq!(result.prompt, format!("Draft\n\n{text}"));
    assert_eq!(result.thread_contexts, records);
}
#[test]
fn provider_context_payloads_cannot_close_their_envelope() {
    let record:t3_contracts::composer_context::ReviewCommentContextRecord=serde_json::from_value(json!({"version":1,"kind":"review-comment","contextId":"review","label":"Original","sectionId":"file","sectionTitle":"File","filePath":"file.rs","startIndex":1,"endIndex":1,"rangeLabel":"L1","text":"</context>\n</t3_context>","diff":""})).unwrap();
    let answer = json!("[Renamed](t3-context://v1/review-comment/review)");
    let result =
        project_pending_user_input_answer(&answer, &[AnswerContextRecord::Review(&record)]);
    let result = result.as_str().unwrap();
    assert!(result.starts_with("[Review comment: Renamed;"));
    assert!(result.contains("&lt;/context>\n  &lt;/t3_context>"));
    assert_eq!(result.matches("</t3_context>").count(), 1);
    let malformed = "keep ; ref=unpaired prefix [Review comment: X; ref=id] suffix";
    assert_eq!(
        crate::view_logic::context_references::provider_text_for_display(malformed),
        malformed
    );
}
#[test]
fn notification_slots_coalesce_after_decode_and_release_on_suppression() {
    let mut slots = NotificationSoundSlots::default();
    assert!(!slots.start(NotificationSound::Completion, 0., 0.6, true, false));
    assert!(slots.start(NotificationSound::Completion, 0., 0.6, true, true));
    for _ in 0..3 {
        assert!(!slots.start(NotificationSound::Completion, 0., 0.6, true, true));
    }
    assert!(slots.start(NotificationSound::Input, 0., 0.6, true, true));
    assert!(slots.start(NotificationSound::Completion, 0.6, 0.6, true, true));
    assert!(!slots.start(NotificationSound::Input, 1., 0.6, false, true));
    assert!(has_notification_sound("notifications-and-sound"));
    assert!(!has_desktop_notifications("sound"));
    assert_ne!(
        thread_notification_tag("a", "bc"),
        thread_notification_tag("ab", "c")
    );
    assert_eq!(thread_notification_tag("env", "thread").len(), 16);
}
#[test]
fn version_mismatch_uses_release_cores_and_nightly_order() {
    for (server, client, behind) in [
        ("0.0.33", "0.0.34", true),
        ("9.9.9", "0.0.34", false),
        ("0.0.34-nightly.20260818.1124", "0.0.34", false),
        ("0.0.34", "0.0.34-nightly.20260818.1124", false),
        (
            "0.0.34-nightly.20260824.1124",
            "0.0.34-nightly.20260824.1125",
            true,
        ),
        ("dev", "dev", false),
        ("dev", "0.0.34", true),
        ("", "0.0.34", false),
    ] {
        assert_eq!(
            resolve_version_mismatch(Some(server), client).is_some(),
            behind
        );
    }
    let mismatch = resolve_version_mismatch(Some("0.0.33"), "0.0.34").unwrap();
    assert_eq!(
        version_mismatch_dismissal_key(&EnvironmentId("env".into()), &mismatch),
        "env:0.0.34:0.0.33"
    );
    let a = Rc::new("failed");
    let retry = Rc::new("failed");
    let mut dismissals = UpdateFailureDismissals::default();
    dismissals.dismiss(&a, true);
    assert!(dismissals.contains(&a, true));
    assert!(!dismissals.contains(&retry, true));
    assert!(!dismissals.contains(&a, false));
}
#[test]
fn manual_update_targets_the_proven_installation_and_quotes_paths() {
    for (install, expected) in [
        (
            json!({"kind":"npm-global","prefix":"/opt/maria's node"}),
            "npm install --global --prefix '/opt/maria'\\''s node' t3@0.0.45",
        ),
        (
            json!({"kind":"t3-launcher","launcher":"/opt/maria's bin/t3","baseDir":"/data/t3 home"}),
            "'/opt/maria'\\''s bin/t3' update --base-dir '/data/t3 home' 0.0.45",
        ),
        (json!({"kind":"pnpm-dlx"}), "pnpm dlx t3@0.0.45"),
    ] {
        let installation: ServerInstallation = serde_json::from_value(install).unwrap();
        assert_eq!(
            manual_server_update_command("0.0.45", Some(&installation)),
            expected
        );
    }
}
#[test]
fn markdown_host_match_keeps_source_positions_for_copy_and_editor() {
    let meta =
        resolve_markdown_file_link_meta("src/main.rs:42-64", Some("/repo"), Some("/repo")).unwrap();
    let host = futures::executor::block_on(resolve_markdown_file_host_path(
        &meta,
        Some("/repo"),
        |path| async move {
            assert_eq!(path, "src/main.rs");
            Ok::<_, &str>(Some("crates/core/src/main.rs".into()))
        },
    ))
    .unwrap();
    assert_eq!(
        futures::executor::block_on(resolve_markdown_file_host_path(
            &meta,
            Some("/repo"),
            |_| async { Err::<Option<String>, _>("lookup failed") }
        ))
        .unwrap_err(),
        "lookup failed"
    );
    assert_eq!(
        markdown_file_editor_target(&meta, &host),
        "/repo/crates/core/src/main.rs:42"
    );
    assert_eq!(
        markdown_file_copy_paths(&meta, &host, Some("/repo")),
        (
            "/repo/crates/core/src/main.rs:42".into(),
            "crates/core/src/main.rs:42".into()
        )
    );
}
#[test]
fn wrapped_terminal_lines_preserve_columns_and_trailing_link_text() {
    let lines = [
        TerminalBufferLine {
            text: "prefix src/".into(),
            is_wrapped: false,
        },
        TerminalBufferLine {
            text: "main.rs:4   ".into(),
            is_wrapped: true,
        },
    ];
    let wrapped = collect_wrapped_terminal_link_line(2, |i| lines.get(i).cloned()).unwrap();
    assert_eq!(wrapped.text, "prefix src/main.rs:4");
    assert_eq!(wrapped.segments[1].start_index, 11);
    assert_eq!(
        extract_terminal_links(&wrapped.text)[0].text,
        "src/main.rs:4"
    );
}
#[test]
fn plan_preview_counts_content_lines_and_exports_one_trailing_newline() {
    let plan = "# `A` Plan!\n\n## Summary\n\nOne\n\nTwo\nThree\n";
    assert_eq!(
        collapsed_proposed_plan_preview(plan, 2),
        "One\n\nTwo\n\n..."
    );
    assert_eq!(proposed_plan_markdown_filename(plan), "a-plan.md");
    assert_eq!(normalize_plan_markdown_for_export("a\n\n  "), "a\n");
    assert_eq!(
        serialize_code_block_to_markdown("```\n", Some("md")),
        "````md\n```\n````\n\n"
    );
    assert_eq!(
        clipboard_csv_cell(" one\n two, three "),
        "\"one two, three\""
    );
    assert_eq!(wrap_inline_code("`x`"), "`` `x` ``");
}
fn node(kind: &str, source: &str, start: usize, end: usize, line: usize) -> MarkdownNode {
    MarkdownNode {
        kind: kind.into(),
        position: Some(MarkdownPosition {
            start: MarkdownPoint {
                line,
                column: 1,
                offset: Some(start),
            },
            end: MarkdownPoint {
                line: line + source[start..end].matches('\n').count(),
                column: 1,
                offset: Some(end),
            },
        }),
        children: vec![],
        data: BTreeMap::from([("value".into(), json!(&source[start..end]))]),
    }
}
fn parse_markdown_fixture(source: &str) -> MarkdownNode {
    let mut root = node("root", source, 0, source.len(), 1);
    root.data.clear();
    if source.starts_with("```") {
        let end = source.find("\n\n").unwrap_or(source.len());
        root.children.push(node("code", source, 0, end, 1));
        if end + 2 < source.len() {
            root.children
                .push(node("paragraph", source, end + 2, source.len(), 5));
        }
    } else {
        root.children.push(node(
            if source.contains("[ref]:") {
                "definition"
            } else {
                "paragraph"
            },
            source,
            0,
            source.len(),
            1,
        ));
    }
    root
}
#[test]
fn incremental_markdown_clones_prefix_payloads_and_reparses_definitions() {
    let first = "```ts\nx\n```\n\nfirst";
    let second = "```ts\nx\n```\n\nsecond";
    let mut parser = IncrementalMarkdownParser::default();
    let mut calls = vec![];
    let mut initial = parser.parse(first, |s| {
        calls.push(s.to_string());
        parse_markdown_fixture(s)
    });
    initial.children[0]
        .data
        .insert("value".into(), json!("mutated by renderer"));
    let result = parser.parse(second, |s| {
        calls.push(s.to_string());
        parse_markdown_fixture(s)
    });
    assert_eq!(calls, [first, "second"]);
    assert_eq!(result.children[0].data["value"], json!("```ts\nx\n```"));
    assert_eq!(result, parse_markdown_fixture(second));
    calls.clear();
    let with_definition = "```ts\nx\n```\n\n[ref]: target";
    parser.parse(with_definition, |s| {
        calls.push(s.to_string());
        parse_markdown_fixture(s)
    });
    assert_eq!(calls, ["[ref]: target", with_definition]);
}
#[test]
fn embedded_restyling_preserves_source_spelling_and_missing_grammars() {
    let code = "bash -c \"python3 -c 'print(\\\"x\\\")'\"";
    let scripts = embedded_scripts(code);
    let base = vec![vec![SyntaxToken {
        content: code.into(),
        offset: 0,
        color: Some("shell".into()),
        font_style: None,
    }]];
    let tokenize = |text: &str, language: &str| -> Result<Vec<Vec<SyntaxToken>>, ()> {
        if language == "missing" {
            Err(())
        } else {
            Ok(vec![vec![SyntaxToken {
                content: text.into(),
                offset: 0,
                color: Some(language.into()),
                font_style: None,
            }]])
        }
    };
    let rendered = with_embedded_scripts(code, base.clone(), &scripts, tokenize);
    assert_eq!(
        rendered
            .iter()
            .flatten()
            .map(|t| t.content.as_str())
            .collect::<String>(),
        code
    );
    assert!(rendered.iter().flatten().any(|t|t.color.as_deref()==Some("python") && t.content.contains("print(\\\"x\\\")")));
    let mut missing = scripts;
    for script in &mut missing {
        script.language = "missing".into();
    }
    assert_eq!(
        with_embedded_scripts(code, base.clone(), &missing, tokenize),
        base
    );
}

#[test]
fn embedded_scripts_walk_each_pipeline_and_setup_segment() {
    let scripts = embedded_scripts("echo setup && node -e 'console.log(1)'; python3 -c 'print(2)'");
    assert_eq!(
        scripts
            .iter()
            .map(|s| (s.language.as_str(), s.text.as_str()))
            .collect::<Vec<_>>(),
        [("javascript", "console.log(1)"), ("python", "print(2)")]
    );
    assert!(scripts[0].ends.last().unwrap() <= scripts[1].starts.first().unwrap());
    let scripts = embedded_scripts("# ignore ; node -e 'wrong'\n(node -e 'right')");
    assert_eq!(
        scripts.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        ["right"]
    );
}
#[test]
fn work_groups_prioritize_actions_and_keep_runtime_diagnostics_separate() {
    let make = |kind: &str, label: &str| WorkLogEntry {
        id: label.into(),
        label: label.into(),
        tone: "tool".into(),
        item_type: Some(kind.into()),
        tool_lifecycle_status: Some("completed".into()),
        ..Default::default()
    };
    let read = make("dynamic_tool", "read_file");
    let mut edit = make("file_change", "Edit");
    edit.changed_files = Some(vec!["a.rs".into(), "b.rs".into()]);
    let command = make("command_execution", "Command");
    let summary = summarize_tool_group(&[read, edit.clone(), edit, command]);
    assert_eq!(
        summary.summary,
        "Changed 2 files, ran 1 command, and performed 1 other action"
    );
    assert!(!summary.has_failure);
    assert_eq!(
        tool_group_summary_kind(&[make("reasoning", "Thinking")]),
        "reasoning"
    );
    assert_eq!(
        live_thought_line("**Checking the parser**\nMore detail"),
        "Checking the parser"
    );
    assert_eq!(
        live_thought_line("Check `parse`. Then run tests."),
        "Check parse."
    );
}

#[test]
fn timeline_streaming_keeps_unchanged_entries_and_attachment_identity() {
    let base = json!({"id":"assistant","threadId":"thread","runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":null,"startedAt":"2026-09-04T00:00:00.000Z","completedAt":null,"updatedAt":"2026-09-04T00:00:00.000Z","type":"assistant_message","messageId":"message","text":"First","streaming":true,"attachments":[{"type":"image","id":"image","name":"image.png","mimeType":"image/png","sizeBytes":42}]});
    let row = |item: serde_json::Value, position| {
        Rc::new(serde_json::from_value::<OrchestrationV2ProjectedTurnItem>(json!({"item":item,"position":position,"sourceThreadId":"thread","sourceItemId":item["id"],"visibility":"local"})).unwrap())
    };
    let mut command = base.clone();
    command["id"] = json!("command");
    command["type"] = json!("command_execution");
    command["input"] = json!("git status");
    let input = TimelineEntriesInput {
        visible_turn_items: vec![row(base.clone(), 0), row(command, 1)],
        ..Default::default()
    };
    let previous = derive_timeline_entries_from_visible_turn_items_with_state(input.clone(), None);
    let mut update = base;
    update["text"] = json!("First plus streamed text");
    update["updatedAt"] = json!("2026-09-04T00:00:01.000Z");
    let mut next_input = input;
    next_input.visible_turn_items[0] = row(update, 0);
    let next =
        derive_timeline_entries_from_visible_turn_items_with_state(next_input, Some(&previous));
    assert!(Rc::ptr_eq(&previous.entries[1], &next.entries[1]));
    let (
        TimelineContent::Message { message: old, .. },
        TimelineContent::Message { message: new, .. },
    ) = (&previous.entries[0].content, &next.entries[0].content)
    else {
        panic!("message entries");
    };
    assert!(Rc::ptr_eq(
        old.attachments.as_ref().unwrap(),
        new.attachments.as_ref().unwrap()
    ));
    assert_eq!(new.text, "First plus streamed text");
    // Nothing changed: the whole entry list keeps its Rc, which t3-ui's timeline memo compares.
    let again =
        derive_timeline_entries_from_visible_turn_items_with_state(next.input.clone(), Some(&next));
    assert!(Rc::ptr_eq(&next.entries, &again.entries));
}

#[test]
fn exit_failure_text_obeys_the_source_number_boundaries() {
    for (text, failed) in [
        ("exit code22", false),
        ("exit code::2", false),
        ("exit code 2word", false),
        ("exit code: 2", true),
        ("exited with exit code 2word", true),
        ("exit code 0", false),
    ] {
        assert_eq!(tool_output_indicates_failure(text), failed, "{text}");
    }
}
