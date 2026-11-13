use super::*;
use serde_json::json;

fn provider(driver: &str, id: &str) -> ServerProvider {
    serde_json::from_value(json!({
        "driver": driver, "instanceId": id, "enabled": true, "installed": true,
        "status": "ready", "auth": {"status": "authenticated"}, "version": null,
        "checkedAt": "2026-03-29T00:00:00.000Z", "models": [], "slashCommands": [], "skills": []
    }))
    .unwrap()
}

fn image(id: &str, preview: &str) -> AttachmentView {
    AttachmentView { attachment: serde_json::from_value(json!({"type": "image", "id": id, "name": "preview.png", "mimeType": "image/png", "sizeBytes": 1})).unwrap(), preview_url: Some(preview.into()), downloadable: false, resource: None }
}

fn completed_run() -> RunDispatchView {
    RunDispatchView {
        run_id: Some(RunId("turn-1".into())),
        requested_at: Some("2026-03-29T00:00:00.000Z".into()),
        started_at: Some("2026-03-29T00:00:01.000Z".into()),
        completed_at: Some("2026-03-29T00:00:10.000Z".into()),
    }
}

fn ready_session() -> RuntimeDispatchView {
    RuntimeDispatchView {
        status: Some("completed".into()),
        updated_at: Some("2026-03-29T00:00:10.000Z".into()),
        active_run_id: None,
    }
}

fn dispatch(
    run: Option<&RunDispatchView>,
    runtime: Option<&RuntimeDispatchView>,
    message: Option<&str>,
) -> LocalDispatchSnapshot {
    create_local_dispatch_snapshot(
        "2026-03-29T00:00:00.000Z".into(),
        run,
        runtime,
        false,
        message.map(|m| MessageId(m.into())),
        SubmissionIntent::Foreground,
    )
}

fn acknowledged(
    local: &LocalDispatchSnapshot,
    phase: SessionPhase,
    run: &RunDispatchView,
    runtime: &RuntimeDispatchView,
    message: Option<&MessageId>,
) -> bool {
    has_server_acknowledged_local_dispatch(DispatchAcknowledgement {
        local_dispatch: Some(local),
        phase,
        latest_run: Some(run),
        latest_user_message_id: message,
        runtime: Some(runtime),
        has_pending_approval: false,
        has_pending_user_input: false,
        thread_error: None,
    })
}

#[test]
fn send_state_excludes_expired_terminal_pills() {
    let contexts = [TerminalContextDraft {
        id: "ctx-expired".into(),
        terminal_label: "Terminal 1".into(),
        line_start: 4,
        line_end: 4,
        text: "".into(),
    }];
    let state = derive_composer_send_state(
        "yoo [Terminal 1](t3-context://v1/terminal/ctx-expired) waddup",
        0,
        0,
        &contexts,
        0,
    );
    assert_eq!(state.trimmed_prompt, "yoo  waddup");
    assert_eq!(state.expired_terminal_context_count, 1);
    assert!(state.has_sendable_content);
    assert!(!derive_composer_send_state("", 0, 0, &contexts, 0).has_sendable_content);
    assert!(derive_composer_send_state("", 0, 1, &contexts, 0).has_sendable_content);
    let mut context = contexts[0].clone();
    context.text = "\r\n\r\n".into();
    assert!(!context.has_text());
    context.text = " ".into();
    assert!(context.has_text());
}

#[test]
fn imported_session_locks_environment_before_first_run() {
    for (id, expected) in [
        (None, false),
        (
            Some(t3_contracts::base_schemas::ProviderThreadId(
                "imported".into(),
            )),
            true,
        ),
    ] {
        assert_eq!(
            is_thread_environment_locked(0, false, id.as_ref()),
            expected
        );
    }
}

#[test]
fn expired_context_copy_preserves_plural_meaning() {
    assert_eq!(
        build_expired_terminal_context_toast_copy(2.0, true),
        ToastCopy {
            title: "Expired terminal contexts won't be sent".into(),
            description: "Remove them or re-add them to include terminal output.".into()
        }
    );
    assert_eq!(
        build_expired_terminal_context_toast_copy(2.0, false),
        ToastCopy {
            title: "Expired terminal contexts omitted from message".into(),
            description: "Re-add them if you want that terminal output included.".into()
        }
    );
    assert_eq!(
        build_expired_terminal_context_toast_copy(1.0, false).description,
        "Re-add it if you want that terminal output included."
    );
}

#[test]
fn acknowledgement_accepts_settled_newer_background_turn() {
    let run = completed_run();
    let runtime = ready_session();
    let mut local = dispatch(Some(&run), Some(&runtime), None);
    local.submission_intent = SubmissionIntent::Background;
    let newer = RunDispatchView {
        run_id: Some(RunId("turn-2".into())),
        ..run
    };
    assert!(acknowledged(
        &local,
        SessionPhase::Ready,
        &newer,
        &runtime,
        None
    ));
    assert_eq!(local.started_at, "2026-03-29T00:00:00.000Z");
}

#[test]
fn acknowledgement_holds_preparing_first_send() {
    let local = dispatch(None, None, None);
    let run = RunDispatchView {
        started_at: None,
        completed_at: None,
        ..completed_run()
    };
    let runtime = RuntimeDispatchView {
        status: Some("preparing".into()),
        active_run_id: run.run_id.clone(),
        ..ready_session()
    };
    assert!(!acknowledged(
        &local,
        SessionPhase::Disconnected,
        &run,
        &runtime,
        None
    ));
}

#[test]
fn acknowledgement_queued_message_table() {
    let run = completed_run();
    let runtime = ready_session();
    let local = dispatch(Some(&run), Some(&runtime), Some("message-before-send"));
    for (status, message, expected) in [
        ("queued", "message-queued", true),
        ("queued", "message-before-send", false),
        ("preparing", "message-queued", false),
        ("starting", "message-queued", false),
    ] {
        let runtime = RuntimeDispatchView {
            status: Some(status.into()),
            ..runtime.clone()
        };
        let run = RunDispatchView {
            run_id: Some(RunId("turn-queued".into())),
            started_at: None,
            completed_at: None,
            ..run.clone()
        };
        assert_eq!(
            acknowledged(
                &local,
                SessionPhase::Running,
                &run,
                &runtime,
                Some(&MessageId(message.into()))
            ),
            expected,
            "{status}/{message}"
        );
    }
}

#[test]
fn acknowledgement_requires_matching_active_run() {
    let run = completed_run();
    let runtime = ready_session();
    let local = dispatch(Some(&run), Some(&runtime), None);
    let running = RunDispatchView {
        run_id: Some(RunId("turn-2".into())),
        completed_at: None,
        ..run
    };
    let mut runtime = RuntimeDispatchView {
        status: Some("running".into()),
        active_run_id: Some(RunId("turn-other".into())),
        ..runtime
    };
    assert!(!acknowledged(
        &local,
        SessionPhase::Running,
        &running,
        &runtime,
        None
    ));
    runtime.active_run_id = running.run_id.clone();
    assert!(acknowledged(
        &local,
        SessionPhase::Running,
        &running,
        &runtime,
        None
    ));
}

#[test]
fn acknowledgement_accepts_steering_onto_same_run() {
    let run = RunDispatchView {
        completed_at: None,
        ..completed_run()
    };
    let runtime = RuntimeDispatchView {
        status: Some("running".into()),
        active_run_id: run.run_id.clone(),
        ..ready_session()
    };
    let local = dispatch(Some(&run), Some(&runtime), Some("message-before-steer"));
    assert!(acknowledged(
        &local,
        SessionPhase::Running,
        &run,
        &runtime,
        Some(&MessageId("message-steer".into()))
    ));
}

fn key(env: &str, thread: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(env.into()),
        thread: ThreadId(thread.into()),
    }
}

#[test]
fn thread_switch_uses_destination_memory() {
    let mut memory = ThreadTimelineMemory::default();
    memory.remember_ready(Some(key("env-1", "thread-a")), Rc::from(["a1", "a2"]));
    memory.remember_ready(Some(key("env-1", "thread-b")), Rc::from(["b1", "b2"]));
    assert_eq!(
        &*memory.peek(Some(&key("env-1", "thread-a"))).unwrap(),
        &["a1", "a2"]
    );
    assert_eq!(
        &*memory.resolve(true, Some(&key("env-1", "thread-a")), Rc::from([]), None),
        &["a1", "a2"]
    );
    assert!(
        memory
            .resolve(true, Some(&key("env-2", "thread-b")), Rc::from([]), None)
            .is_empty()
    );
    assert!(
        memory
            .resolve(false, Some(&key("env-1", "thread-a")), Rc::from([]), None)
            .is_empty()
    );
}

#[test]
fn thread_memory_is_bounded_and_remembering_refreshes_recency() {
    let mut memory = ThreadTimelineMemory::default();
    for i in 0..16 {
        memory.remember_ready(Some(key("env", &i.to_string())), Rc::from([i]));
    }
    memory.remember_ready(Some(key("env", "0")), Rc::from([99]));
    memory.remember_ready(Some(key("env", "16")), Rc::from([16]));
    assert!(memory.peek(Some(&key("env", "1"))).is_none());
    assert_eq!(&*memory.peek(Some(&key("env", "0"))).unwrap(), &[99]);
    memory.reset();
    assert!(memory.peek(Some(&key("env", "0"))).is_none());
}

#[test]
fn handoff_blob_previews_are_ephemeral_only_on_user_images() {
    for (preview, expected) in [("blob:handoff", true), ("https://cdn.example/a.png", false)] {
        let images = [image("preview", preview)];
        assert_eq!(
            timeline_has_ephemeral_preview_urls([(true, true, images.as_slice())]),
            expected
        );
        assert!(!timeline_has_ephemeral_preview_urls([
            (true, false, images.as_slice()),
            (false, true, images.as_slice())
        ]));
    }
}

#[test]
fn imported_history_keeps_custom_driver_instance() {
    for (driver, instance) in [
        ("claudeAgent", "claude_work"),
        ("codex", "codex_work"),
        ("ollama", "local_models"),
    ] {
        let imported = provider(driver, instance);
        let other = provider(
            if driver == "codex" {
                "claudeAgent"
            } else {
                "codex"
            },
            "other",
        );
        let providers = [other, imported];
        let locked = derive_locked_provider(
            false,
            1,
            false,
            None,
            Some("other"),
            Some(instance),
            &providers,
        )
        .unwrap();
        assert_eq!(locked.0, driver);
        let entries: Vec<_> = providers
            .iter()
            .map(|p| ProviderInstanceEntry {
                snapshot: p,
                is_available: true,
                continuation_group_key: None,
            })
            .collect();
        assert_eq!(
            resolve_composer_provider_selection(
                &entries,
                &[Some(&providers[1].instance_id)],
                Some(&locked),
                Some(&providers[1].instance_id)
            )
            .selected_provider_entry
            .unwrap()
            .snapshot
            .instance_id
            .0,
            instance
        );
    }
}

#[test]
fn session_driver_is_authoritative() {
    let providers = [
        provider("claudeAgent", "claude_work"),
        provider("ollama", "local_models"),
    ];
    assert_eq!(
        derive_locked_provider(
            false,
            1,
            true,
            Some("ollama"),
            Some("claude_work"),
            Some("claude_work"),
            &providers
        )
        .unwrap()
        .0,
        "ollama"
    );
}

#[test]
fn imported_missing_or_disabled_provider_does_not_move_to_another_driver() {
    let mut imported = provider("claudeAgent", "claude_work");
    imported.enabled = false;
    for missing in [true, false] {
        let providers = if missing {
            vec![provider("codex", "codex")]
        } else {
            vec![provider("codex", "codex"), imported.clone()]
        };
        let locked = derive_locked_provider(
            false,
            1,
            false,
            None,
            Some("codex"),
            Some("claude_work"),
            &providers,
        )
        .unwrap();
        let entries: Vec<_> = providers
            .iter()
            .map(|p| ProviderInstanceEntry {
                snapshot: p,
                is_available: true,
                continuation_group_key: None,
            })
            .collect();
        let id = ProviderInstanceId("claude_work".into());
        assert!(
            resolve_composer_provider_selection(
                &entries,
                &[Some(&providers[0].instance_id), Some(&id)],
                Some(&locked),
                Some(&id)
            )
            .selected_provider_entry
            .is_none()
        );
    }
}

#[test]
fn signed_out_selection_is_kept_and_send_explains_sign_in() {
    let mut signed_out = provider("antigravity", "google_work");
    signed_out.status = ServerProviderState::Error;
    signed_out.auth.status = ServerProviderAuthStatus::Unauthenticated;
    let other = provider("codex", "codex");
    let entries = [
        ProviderInstanceEntry {
            snapshot: &other,
            is_available: true,
            continuation_group_key: None,
        },
        ProviderInstanceEntry {
            snapshot: &signed_out,
            is_available: true,
            continuation_group_key: None,
        },
    ];
    let selection =
        resolve_composer_provider_selection(&entries, &[Some(&signed_out.instance_id)], None, None);
    let selected = selection.selected_provider_entry.unwrap();
    assert_eq!(selected.snapshot.instance_id, signed_out.instance_id);
    assert_eq!(
        get_antigravity_send_block_reason(Some(selected.snapshot), "gemini-pro"),
        Some("Sign in to Antigravity in provider settings before sending.")
    );
}

#[test]
fn saved_antigravity_model_is_catalog_checked_only_when_ready() {
    let mut p = provider("antigravity", "google_work");
    p.models = serde_json::from_value(json!([{"slug": "gemini-pro", "name": "Gemini Pro", "isCustom": false, "capabilities": null}])).unwrap();
    assert_eq!(
        get_antigravity_send_block_reason(Some(&p), "saved-model"),
        Some("That Antigravity model is no longer available. Choose another model.")
    );
    assert_eq!(
        get_antigravity_send_block_reason(Some(&p), "gemini-pro"),
        None
    );
    p.status = ServerProviderState::Error;
    assert_eq!(
        get_antigravity_send_block_reason(Some(&p), "saved-model"),
        None
    );
}

#[test]
fn antigravity_missing_profile_cannot_continue_under_another_profile() {
    let p = provider("antigravity", "antigravity");
    let entries = [ProviderInstanceEntry {
        snapshot: &p,
        is_available: true,
        continuation_group_key: None,
    }];
    let missing = ProviderInstanceId("google_work".into());
    let selection = resolve_composer_provider_selection(
        &entries,
        &[Some(&missing)],
        Some(&p.driver),
        Some(&missing),
    );
    assert!(selection.selected_provider_entry.is_none());
    assert_eq!(selection.unavailable_provider_instance_id, Some(&missing));
}

#[test]
fn continuation_group_cannot_change_when_another_instance_is_selected() {
    let mut work = provider("antigravity", "google_work");
    work.enabled = false;
    let personal = provider("antigravity", "google_personal");
    let entries = [
        ProviderInstanceEntry {
            snapshot: &work,
            is_available: true,
            continuation_group_key: Some("work-profile"),
        },
        ProviderInstanceEntry {
            snapshot: &personal,
            is_available: true,
            continuation_group_key: Some("personal-profile"),
        },
    ];
    assert!(
        resolve_composer_provider_selection(
            &entries,
            &[Some(&personal.instance_id), Some(&work.instance_id)],
            Some(&work.driver),
            Some(&work.instance_id)
        )
        .selected_provider_entry
        .is_none()
    );
}

#[test]
fn checkout_memory_does_not_confuse_separator_in_environment_and_path() {
    let mut memory = ChatSessionMemory::default();
    memory.remember_checkout_is_repo(EnvironmentId("env".into()), "a:b".into(), false);
    assert_eq!(
        memory.recall_checkout_is_repo(&EnvironmentId("env:a".into()), Some("b")),
        None
    );
    assert_eq!(
        memory.recall_checkout_is_repo(&EnvironmentId("env".into()), Some("a:b")),
        Some(false)
    );
}

#[test]
fn streamed_setup_never_regresses_to_older_held_snapshot() {
    let base: WorktreeSetupSnapshot = serde_json::from_value(json!({"threadId": "thread-1", "phase": "running", "startedAt": "2026-03-29T00:00:00.000Z", "endedAt": null, "branch": "feature", "baseRef": "main", "worktreePath": null, "setupScript": null, "stages": [{"id":"agent", "status":"pending", "startedAt":null, "endedAt":null, "percent":null, "detail":null, "tail":[]}], "error": null, "sequence": 1})).unwrap();
    let mut newest = base.clone();
    newest.phase = WorktreeSetupPhase::Done;
    newest.sequence = 9;
    newest.stages[0].status = WorktreeSetupStageStatus::Done;
    for (latest, held) in [(&newest, &base), (&base, &newest)] {
        let (snapshot, preparing) = resolve_worktree_setup_progress(
            &base.thread_id,
            false,
            Some(&OrchestrationV2RunStatus::Running),
            Some(latest),
            Some(held),
        );
        assert_eq!(snapshot, Some(&newest));
        assert!(!preparing);
    }
    assert_eq!(
        resolve_worktree_setup_progress(
            &ThreadId("other".into()),
            false,
            Some(&OrchestrationV2RunStatus::Running),
            Some(&base),
            Some(&base)
        ),
        (None, false)
    );
}

fn annotation() -> PreviewAnnotationPayloadSchema {
    serde_json::from_value(json!({"id":"annotation", "pageUrl":"https://example.com", "pageTitle":null,"comment":"fix", "elements":[],"regions":[],"strokes":[],"styleChanges":[],"screenshot":{"dataUrl":"data:image/png;base64,AAAA", "width":1,"height":1,"cropRect":{"x":0,"y":0,"width":1,"height":1}},"createdAt":"2026-03-29T00:00:00.000Z"})).unwrap()
}

#[test]
fn full_attachment_cap_drops_screenshot_claim() {
    let images: Vec<_> = (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS)
        .map(|i| image(&i.to_string(), "blob:preview"))
        .collect();
    let annotation = annotation();
    let direct = image("direct", "blob:direct");
    let (merged, annotations) =
        merge_direct_preview_annotation(&images, 0, &[], Some((&annotation, Some(&direct))));
    assert_eq!(merged, images);
    assert!(annotations[0].screenshot.is_none());
    let (merged, annotations) =
        merge_direct_preview_annotation(&[], 0, &[], Some((&annotation, Some(&direct))));
    assert_eq!(merged, [direct]);
    assert_eq!(annotations[0].screenshot.as_ref().unwrap().data_url, "");
}

fn turn_input() -> ComposerTurnInput<'static> {
    ComposerTurnInput {
        message_text: "hello",
        send_queues_behind_setup: false,
        is_server_thread: true,
        active_message_count: 0,
        send_env_mode: WorkspaceMode::Worktree,
        worktree_path: None,
        active_thread_branch: Some("feature/base"),
        resume_compaction_tokens: None,
        keep_full_history: false,
        phase: SessionPhase::Ready,
        dispatch_mode: DispatchMode::Auto,
        fresh_context: false,
    }
}

#[test]
fn first_turn_prepares_worktree_from_explicit_branch() {
    let plan = plan_composer_turn(turn_input());
    assert!(!plan.missing_worktree_base_branch);
    assert_eq!(plan.base_branch_for_worktree, Some("feature/base"));
    let plan = plan_composer_turn(ComposerTurnInput {
        active_thread_branch: None,
        ..turn_input()
    });
    assert!(plan.missing_worktree_base_branch);
}

#[test]
fn stale_session_compaction_queues_except_for_compact_command() {
    let plan = plan_composer_turn(ComposerTurnInput {
        resume_compaction_tokens: Some(1),
        ..turn_input()
    });
    assert!(plan.compact_before_send);
    assert_eq!(plan.turn_dispatch_mode, DispatchMode::Queue);
    assert!(plan.should_queue_behind_active_run);
    for text in ["/compact", "/compact custom", "/COMPACT"] {
        assert!(
            !plan_composer_turn(ComposerTurnInput {
                message_text: text,
                resume_compaction_tokens: Some(1),
                ..turn_input()
            })
            .compact_before_send
        );
    }
    assert!(
        plan_composer_turn(ComposerTurnInput {
            message_text: "/compactly",
            resume_compaction_tokens: Some(1),
            ..turn_input()
        })
        .compact_before_send
    );
}

#[test]
fn active_turn_queues_only_when_requested() {
    for (mode, expected) in [
        (DispatchMode::Auto, false),
        (DispatchMode::Steer, false),
        (DispatchMode::Restart, false),
        (DispatchMode::Queue, true),
    ] {
        assert_eq!(
            plan_composer_turn(ComposerTurnInput {
                phase: SessionPhase::Running,
                dispatch_mode: mode,
                ..turn_input()
            })
            .should_queue_behind_active_run,
            expected
        );
    }
    for mode in [DispatchMode::Steer, DispatchMode::Restart] {
        let plan = plan_composer_turn(ComposerTurnInput {
            fresh_context: true,
            dispatch_mode: mode,
            resume_compaction_tokens: Some(1),
            ..turn_input()
        });
        assert!(!plan.compact_before_send);
        assert_eq!(plan.turn_dispatch_mode, DispatchMode::Queue);
    }
}

#[test]
fn attachment_bindings_prefer_upload_ids() {
    let attachments = [image("local-1", "blob:1"), image("local-2", "blob:2")];
    assert_eq!(
        bound_attachment_ids(&[Some("uploaded".into()), None], &attachments).unwrap(),
        ["uploaded", "local-2"]
    );
}

const NOW: &str = "2026-03-29T00:00:00.000Z";

fn projection_json() -> serde_json::Value {
    json!({"thread":{"id":"t","projectId":"p","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":"provider-thread","lineage":{"rootThreadId":"t","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null},"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW})
}

fn run_json(ordinal: u64, status: &str) -> serde_json::Value {
    json!({"id":format!("run-{ordinal}"),"threadId":"t","ordinal":ordinal,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"providerThreadId":"provider-thread","userMessageId":format!("message-{ordinal}"),"rootNodeId":null,"activeAttemptId":null,"status":status,"requestedAt":NOW,"startedAt":NOW,"completedAt":NOW,"checkpointId":null,"contextHandoffId":null})
}

fn provider_thread_json(id: &str) -> serde_json::Value {
    json!({"id":id,"driver":"codex","providerInstanceId":"codex","providerSessionId":"session","appThreadId":"t","ownerNodeId":null,"nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":1,"lastRunOrdinal":4,"handoffIds":[],"forkedFrom":null,"createdAt":NOW,"updatedAt":NOW})
}

fn rewind_projection() -> t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection {
    let mut p = projection_json();
    p["providerThreads"] = json!([provider_thread_json("provider-thread")]);
    p["runs"] = (1..=4).map(|i| run_json(i, "completed")).collect();
    serde_json::from_value(p).unwrap()
}

#[test]
fn rewind_rejects_with_projected_failure() {
    let initial = rewind_projection();
    let witness = RevertWitness::capture(
        &initial,
        1,
        t3_contracts::base_schemas::CommandId("rollback-1".into()),
    )
    .unwrap();
    let mut p = serde_json::to_value(initial).unwrap();
    p["thread"]["rollbackFailure"] =
        json!({"requestId":"rollback-1","message":"The provider could not roll back."});
    let p = serde_json::from_value(p).unwrap();
    assert!(
        matches!(witness.inspect(&p, false), Err(ClientError::InvalidPayload(reason)) if reason == "The provider could not roll back.")
    );
}

#[test]
fn rewind_waits_for_retained_suffix_when_source_message_and_run_are_paged_out() {
    use t3_contracts::{
        base_schemas::ProviderThreadId, orchestration_v2::OrchestrationV2RunStatus,
        orchestration_v2_id_table::IdOrIndex,
    };
    for (replacement, refetched) in [(false, false), (true, false), (true, true)] {
        let mut p = rewind_projection();
        p.runs = serde_json::from_value(json!([
            run_json(3, "rolled_back"),
            run_json(9, "cancelled")
        ]))
        .unwrap();
        p.provider_threads[0].last_run_ordinal = Some(9);
        let witness = RevertWitness::capture(
            &p,
            0,
            t3_contracts::base_schemas::CommandId("rollback-1".into()),
        )
        .unwrap();
        assert!(!witness.inspect(&p, true).unwrap());
        let mut completed = p.provider_threads[0].clone();
        completed.last_run_ordinal = None;
        if replacement {
            completed.id = IdOrIndex::Id(ProviderThreadId("replacement-provider-thread".into()));
            completed.first_run_ordinal = None;
        }
        if refetched {
            p.thread.active_provider_thread_id = Some(completed.id.clone());
        }
        if replacement {
            p.provider_threads.push(completed);
        } else {
            p.provider_threads = vec![completed];
        }
        p.runs[1].status = OrchestrationV2RunStatus::RolledBack;
        assert!(witness.inspect(&p, true).unwrap());
        assert!(!witness.inspect(&p, false).unwrap());
    }
}

#[test]
fn shallower_rewind_from_another_client_does_not_complete_requested_rewind() {
    use t3_contracts::orchestration_v2::OrchestrationV2RunStatus;
    let mut p = rewind_projection();
    let witness = RevertWitness::capture(
        &p,
        1,
        t3_contracts::base_schemas::CommandId("rollback-1".into()),
    )
    .unwrap();
    p.provider_threads[0].last_run_ordinal = Some(3);
    p.runs[3].status = OrchestrationV2RunStatus::RolledBack;
    assert!(!witness.inspect(&p, true).unwrap());
    p.provider_threads[0].last_run_ordinal = Some(1);
    for run in &mut p.runs {
        if run.ordinal > 1 {
            run.status = OrchestrationV2RunStatus::RolledBack;
        }
    }
    assert!(witness.inspect(&p, true).unwrap());
    assert!(
        RevertWitness::capture(
            &p,
            1,
            t3_contracts::base_schemas::CommandId("rollback-2".into())
        )
        .is_err()
    );
}

#[test]
fn available_reverts_follow_latest_provider_turn_for_each_attempt() {
    let mut p = projection_json();
    p["providerThreads"] = json!([provider_thread_json("provider-thread")]);
    let mut runs = Vec::new();
    let mut attempts = Vec::new();
    let mut turns = Vec::new();
    for ordinal in 1..=3 {
        let mut run = run_json(ordinal, "completed");
        run["activeAttemptId"] = json!(format!("attempt-{ordinal}"));
        runs.push(run);
        attempts.push(json!({"id":format!("attempt-{ordinal}"),"runId":format!("run-{ordinal}"),"attemptOrdinal":1,"rootNodeId":"node","providerInstanceId":"codex","providerThreadId":"provider-thread","providerTurnId":null,"reason":"initial","status":"completed","startedAt":NOW,"completedAt":NOW}));
        turns.push(json!({"id":format!("turn-{ordinal}-1"),"runAttemptId":format!("attempt-{ordinal}"),"ordinal":1,"providerThreadId":"provider-thread","nodeId":"node","nativeTurnRef":null,"status":"completed","startedAt":NOW,"completedAt":NOW}));
    }
    let mut latest = turns[1].clone();
    latest["id"] = "turn-2-2".into();
    latest["ordinal"] = 2.into();
    latest["providerThreadId"] = "other-provider-thread".into();
    turns.push(latest);
    p["runs"] = json!(runs);
    p["attempts"] = json!(attempts);
    p["providerTurns"] = json!(turns);
    let projection = serde_json::from_value(p.clone()).unwrap();
    assert_eq!(
        derive_available_revert_turn_counts(Some(&projection)),
        BTreeSet::from([0, 1, 3])
    );
    p["providerThreads"][0]["providerSessionId"] = serde_json::Value::Null;
    let projection = serde_json::from_value(p).unwrap();
    assert!(derive_available_revert_turn_counts(Some(&projection)).is_empty());
    assert!(derive_available_revert_turn_counts(None).is_empty());
}

fn command_json(ordinal: u64, native_id: &str, command: &str) -> serde_json::Value {
    json!({"id":format!("item-{native_id}"),"type":"command_execution","threadId":"t","runId":format!("run-{ordinal}"),"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":{"driver":"codex","nativeId":native_id,"strength":"strong"},"parentItemId":null,"ordinal":ordinal,"status":"running","title":null,"input":command,"output":null,"exitCode":null,"startedAt":NOW,"completedAt":null,"updatedAt":NOW})
}

fn assert_background(p: serde_json::Value, expected: serde_json::Value) {
    let p = serde_json::from_value(p).unwrap();
    let tasks = derive_thread_pending_background_tasks(Some(&p), |_| 0).unwrap();
    assert_eq!(serde_json::to_value(tasks).unwrap(), expected);
}

#[test]
fn earlier_background_command_survives_live_follow_up() {
    for status in ["queued", "preparing", "running"] {
        let mut p = projection_json();
        p["runs"] = json!([run_json(1, "completed"), run_json(2, status)]);
        p["turnItems"] = json!([
            command_json(1, "shell-1", "sleep 600"),
            command_json(2, "shell-2", "sleep 15")
        ]);
        assert_background(
            p,
            json!([{"taskId":"shell-1","description":"sleep 600","kind":"command"}]),
        );
    }
}

#[test]
fn foreground_child_is_removed_from_provider_roster() {
    let mut p = projection_json();
    p["runs"] = json!([run_json(1, "completed"), run_json(2, "running")]);
    let mut provider = provider_thread_json("provider-thread");
    provider["pendingBackgroundTasks"] = json!([{"taskId":"child-session-1","kind":"subagent"},{"taskId":"child-session-2","kind":"subagent"}]);
    p["providerThreads"] = json!([provider]);
    let item = |ordinal, id, prompt| {
        let mut item = command_json(ordinal, id, "");
        item["type"] = "subagent".into();
        item["subagentId"] = "subagent".into();
        item["origin"] = "app_owned".into();
        item["driver"] = "codex".into();
        item["providerInstanceId"] = "codex".into();
        item["childThreadId"] = serde_json::Value::Null;
        item["prompt"] = json!(prompt);
        item["result"] = serde_json::Value::Null;
        item
    };
    p["turnItems"] = json!([
        item(1, "child-session-1", "child 1"),
        item(2, "child-session-2", "child 2")
    ]);
    assert_background(
        p,
        json!([{"taskId":"child-session-1","kind":"subagent","providerInstanceId":"codex"}]),
    );
}

#[test]
fn roster_only_monitor_survives_follow_up_and_queued_work() {
    for (previous_status, next_status, kind, id) in [
        ("completed", "running", "monitor", "b1htjkjev"),
        ("waiting", "queued", "background_task", "task-1"),
    ] {
        let mut p = projection_json();
        p["runs"] = json!([run_json(1, previous_status), run_json(2, next_status)]);
        let mut provider = provider_thread_json("provider-thread");
        provider["pendingBackgroundTasks"] = json!([{"taskId":id,"kind":kind}]);
        p["providerThreads"] = json!([provider]);
        if next_status == "running" {
            let mut completed = command_json(1, "toolu-monitor", "");
            completed["type"] = "dynamic_tool".into();
            completed["toolName"] = "Monitor".into();
            completed["input"] = json!({});
            completed["status"] = "completed".into();
            p["turnItems"] = json!([completed, command_json(2, "shell-2", "sleep 15")]);
        }
        assert_background(p, json!([{"taskId":id,"kind":kind}]));
    }
}

#[test]
fn composer_snapshot_drops_expired_references_and_preserves_live_contexts() {
    let expired = TerminalContextDraft {
        id: "ctx-expired".into(),
        terminal_label: "Terminal 1".into(),
        line_start: 4,
        line_end: 4,
        text: "".into(),
    };
    let live = TerminalContextDraft {
        id: "ctx-live".into(),
        text: "pnpm test".into(),
        line_start: 9,
        line_end: 9,
        ..expired.clone()
    };
    let contexts = [expired, live];
    let sendable = [&contexts[1]];
    let prompt = "run [Terminal 1 line 4](t3-context://v1/terminal/terminal_ctx-expired) [Terminal 1 line 9](t3-context://v1/terminal/terminal_ctx-live) ";
    let snapshot = snapshot_composer_send(ComposerSendContents::<()> {
        prompt,
        images: &[],
        files: &[],
        terminal_contexts: &contexts,
        sendable_terminal_contexts: &sendable,
        preview_annotations: &[],
        review_comments: &[],
        thread_contexts: &[],
        environment_contexts: &[],
        reply_to: None,
    });
    assert_eq!(snapshot.terminal_contexts, [contexts[1].clone()]);
    assert_eq!(
        snapshot.message_text,
        "run [Terminal 1 line 9](t3-context://v1/terminal/terminal_ctx-live)"
    );
}

#[test]
fn composer_reply_remains_reference_on_send_and_returns_as_chip_on_failure() {
    let reply = serde_json::from_value(json!({"version":1,"kind":"response","contextId":"response_message-1","label":"Done.","citation":{"version":1,"environmentId":"environment-1","threadId":"thread-1","messageId":"message-1","text":"Done. I added clamp next to sum.","start":0,"end":32,"prefix":"","suffix":""},"itemId":"item-1","subagents":[]})).unwrap();
    let snapshot = snapshot_composer_send(ComposerSendContents::<()> {
        prompt: "[Done.](t3-context://v1/response/response_message-1)  why there?",
        images: &[],
        files: &[],
        terminal_contexts: &[],
        sendable_terminal_contexts: &[],
        preview_annotations: &[],
        review_comments: &[],
        thread_contexts: &[],
        environment_contexts: &[],
        reply_to: Some(&reply),
    });
    assert!(
        snapshot
            .message_text
            .contains("t3-context://v1/response/response_message-1")
    );
    assert_eq!(snapshot.draft_text, "why there?");
    assert_eq!(snapshot.reply_to, Some(reply));
}

#[test]
fn message_context_is_optional_inline() {
    let context = serde_json::from_value(json!({"version":1,"records":[]})).unwrap();
    assert_eq!(
        serde_json::to_value(with_message_context("hi".into(), Some(context))).unwrap(),
        json!({"text":"hi","context":{"version":1,"records":[]}})
    );
    assert_eq!(
        serde_json::to_value(with_message_context("hi".into(), None)).unwrap(),
        json!({"text":"hi"})
    );
}

#[test]
fn open_in_picker_accepts_both_local_environment_ids() {
    let primary = EnvironmentId("wsl".into());
    let host = EnvironmentId("host".into());
    for (active, expected) in [
        (&primary, true),
        (&host, true),
        (&EnvironmentId("other".into()), false),
    ] {
        assert_eq!(
            should_show_open_in_picker(Some("Project"), active, Some(&primary), Some(&host), true),
            expected
        );
    }
}

#[test]
fn committed_user_ids_exclude_assistant_and_projection_only_messages() {
    use t3_contracts::{
        orchestration_v2::OrchestrationV2ProjectedTurnItem, orchestration_v2_id_table::IdOrIndex,
    };
    let row = |position: u64, id, role, intent| {
        let mut item = command_json(position + 1, id, "");
        item["type"] = json!(role);
        item["messageId"] = json!(id);
        item["text"] = "message".into();
        if role == "user_message" {
            item["createdBy"] = "user".into();
            item["creationSource"] = "web".into();
            item["attachments"] = json!([]);
            item["inputIntent"] = json!(intent);
        } else {
            item["streaming"] = false.into();
        }
        serde_json::from_value::<OrchestrationV2ProjectedTurnItem>(json!({"position":position,"visibility":"local","sourceThreadId":"t","sourceItemId":format!("item-{id}"),"item":item})).unwrap()
    };
    let rows = [
        row(0, "message-turn-start", "user_message", "turn_start"),
        row(1, "message-assistant", "assistant_message", ""),
        row(2, "message-steer", "user_message", "steer"),
    ];
    assert_eq!(
        derive_committed_server_user_message_ids(&rows),
        [
            IdOrIndex::Id(MessageId("message-turn-start".into())),
            IdOrIndex::Id(MessageId("message-steer".into()))
        ]
    );
}

#[test]
fn rewind_waiter_observes_committed_state_and_releases_its_timer() {
    use crate::{ClientStore, testing::FakePlatform};
    use futures::{executor::block_on, poll};
    use t3_contracts::{base_schemas::CommandId, orchestration_v2::OrchestrationV2RunStatus};
    block_on(async {
        let store = ClientStore::default();
        let platform = FakePlatform::default();
        let key = key("env", "t");
        let session = store.begin_session(key.environment.clone());
        store.update_thread(&key, session, |thread| {
            thread.projection = Some(Rc::new(rewind_projection()));
            true
        });
        let called = std::cell::Cell::new(false);
        let mut waiting = Box::pin(wait_for_reverted_message(
            &store,
            &platform,
            &key,
            1,
            CommandId("rollback-1".into()),
            async {
                called.set(true);
                Ok(())
            },
            120_000,
        ));
        assert!(poll!(waiting.as_mut()).is_pending());
        assert!(called.get());
        assert_eq!(platform.pending_timers(), 1);
        store.update_thread(&key, session, |thread| {
            let projection = Rc::make_mut(thread.projection.as_mut().unwrap());
            projection.provider_threads[0].last_run_ordinal = Some(1);
            for run in &mut projection.runs {
                if run.ordinal > 1 {
                    run.status = OrchestrationV2RunStatus::RolledBack;
                }
            }
            true
        });
        waiting.await.unwrap();
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn rewind_waiter_rejects_stale_edits_before_dispatch_and_times_out_with_injected_clock() {
    use crate::{ClientStore, testing::FakePlatform};
    use futures::{executor::block_on, poll};
    use t3_contracts::{base_schemas::CommandId, orchestration_v2::OrchestrationV2RunStatus};
    block_on(async {
        let store = ClientStore::default();
        let platform = FakePlatform::default();
        let key = key("env", "t");
        let session = store.begin_session(key.environment.clone());
        let mut initial = rewind_projection();
        for run in &mut initial.runs {
            if run.ordinal > 1 {
                run.status = OrchestrationV2RunStatus::RolledBack;
            }
        }
        store.update_thread(&key, session, |thread| {
            thread.projection = Some(Rc::new(initial));
            true
        });
        let called = std::cell::Cell::new(false);
        assert!(
            wait_for_reverted_message(
                &store,
                &platform,
                &key,
                1,
                CommandId("rollback-1".into()),
                async {
                    called.set(true);
                    Ok(())
                },
                120_000
            )
            .await
            .is_err()
        );
        assert!(!called.get());
        assert_eq!(platform.pending_timers(), 0);
        store.update_thread(&key, session, |thread| {
            thread.projection = Some(Rc::new(rewind_projection()));
            true
        });
        let mut waiting = Box::pin(wait_for_reverted_message(
            &store,
            &platform,
            &key,
            1,
            CommandId("rollback-1".into()),
            async { Ok(()) },
            120_000,
        ));
        assert!(poll!(waiting.as_mut()).is_pending());
        platform.advance(120_000);
        assert!(
            matches!(waiting.await,Err(ClientError::InvalidPayload(reason)) if reason=="Timed out waiting for the thread to rewind.")
        );
        assert_eq!(platform.pending_timers(), 0);
        let mut cancelled = Box::pin(wait_for_reverted_message(
            &store,
            &platform,
            &key,
            1,
            CommandId("rollback-2".into()),
            futures::future::pending(),
            120_000,
        ));
        assert!(poll!(cancelled.as_mut()).is_pending());
        drop(cancelled);
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn reverted_attachments_use_host_io_and_keep_file_bytes_and_metadata() {
    use crate::{platform::HttpResponse, testing::FakePlatform};
    use futures::executor::block_on;
    block_on(async {
        let platform = FakePlatform::default();
        platform.push_http(Ok(HttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: vec![1, 2, 3],
        }));
        let attachment = image("image-1", "blob:old");
        let restored = prepare_reverted_message_attachments(
            &platform,
            &[attachment],
            "https://env.example/base/",
            |_| async {
                Ok(serde_json::from_value(
                    serde_json::json!({"relativeUrl":"/assets/a","expiresAt":1}),
                )
                .unwrap())
            },
        )
        .await
        .unwrap();
        assert_eq!(
            restored,
            [RestoredAttachment {
                name: "preview.png".into(),
                mime_type: "image/png".into(),
                bytes: vec![1, 2, 3]
            }]
        );
        assert_eq!(
            platform.http_requests()[0].url,
            "https://env.example/assets/a"
        );
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn draft_thread_construction_preserves_wire_dates_and_workspace() {
    let model = serde_json::from_value(json!({"instanceId":"codex","model":"gpt-5.4"})).unwrap();
    let result = build_local_draft_thread(
        ThreadId("draft".into()),
        DraftThreadInput {
            environment_id: EnvironmentId("env".into()),
            project_id: t3_contracts::base_schemas::ProjectId("p".into()),
            created_at: t3_contracts::wire::WireDateTime(NOW.into()),
            runtime_mode: t3_contracts::provider_policy::RuntimeMode::FullAccess,
            interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Default,
            branch: None,
            worktree_path: None,
        },
        model,
    )
    .unwrap();
    assert_eq!(result.shell.title, "New thread");
    assert_eq!(result.shell.created_at.0, NOW);
    assert_eq!(result.shell.item_count, 0);
    assert_eq!(
        result.shell.lineage.root_thread_id,
        ThreadId("draft".into())
    );
    let projection = rewind_projection();
    let presented = present_thread_shell_from_projection(
        EnvironmentId("env".into()),
        &ThreadId("t".into()),
        &projection,
        t3_contracts::wire::WireDateTime(NOW.into()),
        None,
        ExecutionPresentation {
            latest_run: Some((RunId("run-4".into()), "complete canonical run summary")),
            runtime: Some((
                RuntimeDispatchView {
                    status: Some("running".into()),
                    active_run_id: Some(RunId("run-4".into())),
                    ..RuntimeDispatchView::default()
                },
                "complete canonical runtime summary",
            )),
        },
    )
    .unwrap();
    assert_eq!(presented.latest_run, Some("complete canonical run summary"));
    assert_eq!(
        presented.runtime,
        Some("complete canonical runtime summary")
    );
    assert_eq!(presented.shell.active_run_id, Some(RunId("run-4".into())));
    assert_eq!(presented.shell.updated_at.0, NOW);
}
