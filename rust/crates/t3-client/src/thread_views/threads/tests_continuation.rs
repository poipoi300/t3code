use super::*;
use futures::{FutureExt, executor::block_on};
use t3_contracts::base_schemas::{MessageId, TurnItemId};

fn message(run: &str, id: &str, role: &str) -> OrchestrationV2ConversationMessage {
    serde_json::from_value(json!({"id":id,"threadId":"thread","runId":run,"nodeId":null,"role":role,"text":"Message","attachments":[],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":"1","updatedAt":"1"})).unwrap()
}
fn projected(item: OrchestrationV2TurnItem) -> OrchestrationV2ProjectedTurnItem {
    serde_json::from_value(json!({"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":"item","item":item})).unwrap()
}
fn agent(id: &str, run: Option<&str>, status: &str) -> OrchestrationV2Subagent {
    serde_json::from_value(json!({"id":id,"threadId":"thread","runId":run,"parentNodeId":"root","origin":"provider_native","createdBy":"agent","driver":"codex","providerInstanceId":"codex","providerThreadId":null,"childThreadId":null,"nativeTaskRef":null,"prompt":"Build","title":null,"model":null,"status":status,"result":null,"startedAt":"1","completedAt":null,"updatedAt":"2"})).unwrap()
}
fn shell(id: &str) -> OrchestrationV2ThreadShell {
    serde_json::from_value(thread_json(id)).unwrap()
}
fn checkpoint(run: Option<&str>, ordinal: Option<u64>) -> OrchestrationV2Checkpoint {
    serde_json::from_value(json!({"id":format!("checkpoint-{}",run.unwrap_or("none")),"threadId":"thread","scopeId":"scope","runId":run,"nodeId":"root","parentCheckpointId":null,"ordinalWithinScope":1,"appRunOrdinal":ordinal,"ref":"refs/checkpoint","status":"ready","files":[],"capturedAt":"2026-10-26T09:30:00.123+11:00"})).unwrap()
}
fn shared(
    p: OrchestrationV2ThreadProjection,
    old: Option<&crate::state::sharing::SharedThreadProjection>,
) -> Rc<crate::state::sharing::SharedThreadProjection> {
    Rc::new(crate::state::sharing::SharedThreadProjection::update(
        Rc::new(p),
        old,
    ))
}

#[test]
fn checkpoints_keep_order_identity_and_reverse_array_associations() {
    use checkpoints::*;
    let mut p = projection();
    p.checkpoints = vec![
        checkpoint(None, Some(1)),
        checkpoint(Some("b"), Some(1)),
        checkpoint(Some("a"), Some(2)),
        checkpoint(Some("b"), Some(3)),
        checkpoint(Some("a"), None),
    ];
    p.messages = vec![
        message("a", "a-first", "assistant"),
        message("b", "b-first", "assistant"),
        message("a", "a-last", "assistant"),
        message("b", "b-last", "assistant"),
        message("a", "user-tail", "user"),
        message("a", "system-tail", "system"),
    ];
    let rows = derive_thread_checkpoint_summaries(&p);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|r| r.assistant_message.unwrap().id.clone())
            .collect::<Vec<_>>(),
        ["b-last", "a-last", "b-last"].map(|id| IdOrIndex::Id(MessageId(id.into())))
    );
    assert!(std::ptr::eq(rows[0].checkpoint, &p.checkpoints[1]));
    assert_eq!(
        rows[0].checkpoint.captured_at.0,
        "2026-10-26T09:30:00.123+11:00"
    );
    p.messages.clear();
    assert!(
        derive_thread_checkpoint_summaries(&p)
            .iter()
            .all(|r| r.assistant_message.is_none())
    );
    p.messages = vec![message("a", "replacement", "assistant")];
    assert_eq!(
        derive_thread_checkpoint_summaries(&p)[1]
            .assistant_message
            .unwrap()
            .id,
        IdOrIndex::Id(MessageId("replacement".into()))
    );
    p.checkpoints[1].files = serde_json::from_value(
        json!([{"path":"src/one.rs","kind":"modified","additions":10,"deletions":2}]),
    )
    .unwrap();
    assert_eq!(checkpoint_file_totals(&p.checkpoints[1]).additions, 10);
    p.checkpoints[1].files_total =
        Some(serde_json::from_value(json!({"count":20,"additions":100,"deletions":50})).unwrap());
    assert_eq!(checkpoint_file_totals(&p.checkpoints[1]).count, 20);
}

#[test]
fn item_support_borrows_execution_entities_and_falls_back_to_synthetic_rows() {
    use item_support::*;
    let mut p = projection();
    p.runs = vec![run("run", 1, "running")];
    let command = item(
        "command_execution",
        "running",
        json!({"input":"vp check","nodeId":"node","providerThreadId":"provider","providerTurnId":"turn"}),
    );
    p.turn_items = vec![command.clone()];
    p.nodes=vec![serde_json::from_value(json!({"id":"node","threadId":"thread","runId":"run","parentNodeId":null,"rootNodeId":"node","kind":"tool_call","status":"running","countsForRun":true,"providerThreadId":"provider","providerTurnId":"turn","nativeItemRef":null,"runtimeRequestId":"request","checkpointScopeId":null,"startedAt":"1","completedAt":null})).unwrap()];
    p.attempts=vec![serde_json::from_value(json!({"id":"attempt","runId":"run","attemptOrdinal":1,"rootNodeId":"node","providerInstanceId":"codex","providerThreadId":"provider","providerTurnId":"turn","reason":"initial","status":"running","startedAt":"1","completedAt":null})).unwrap()];
    p.provider_threads=vec![serde_json::from_value(json!({"id":"provider","driver":"codex","providerInstanceId":"codex","providerSessionId":null,"appThreadId":"thread","ownerNodeId":"node","nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"active","firstRunOrdinal":1,"lastRunOrdinal":1,"handoffIds":[],"forkedFrom":null,"createdAt":"1","updatedAt":"1"})).unwrap()];
    p.provider_turns=vec![serde_json::from_value(json!({"id":"turn","providerThreadId":"provider","nodeId":"node","runAttemptId":"attempt","nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":"1","completedAt":null})).unwrap()];
    p.runtime_requests=vec![serde_json::from_value(json!({"id":"request","nodeId":"node","providerTurnId":"turn","nativeRequestRef":null,"kind":"dynamic_tool_call","status":"pending","responseCapability":{"type":"live","providerSessionId":"session"},"createdAt":"1","resolvedAt":null})).unwrap()];
    let id = IdOrIndex::Id(TurnItemId("item".into()));
    let s = resolve_item_support(&p, &id);
    assert!(std::ptr::eq(s.item.unwrap(), &p.turn_items[0]));
    assert!(std::ptr::eq(s.run.unwrap(), &p.runs[0]));
    assert!(std::ptr::eq(s.attempts[0], &p.attempts[0]));
    assert!(std::ptr::eq(s.node.unwrap(), &p.nodes[0]));
    assert!(s.provider_thread.is_some());
    assert!(s.provider_turn.is_some());
    assert!(s.runtime_request.is_some());
    p.turn_items.clear();
    p.visible_turn_items = vec![projected(command)];
    assert!(std::ptr::eq(
        resolve_item_support(&p, &id).item.unwrap(),
        &p.visible_turn_items[0].item
    ));
    assert!(
        resolve_item_support(&p, &IdOrIndex::Index(0))
            .item
            .is_none()
    );
}

#[test]
fn quiet_detail_holds_text_and_visits_but_sees_every_structural_change() {
    use detail::*;
    let mut p = projection();
    p.turn_items = vec![
        item(
            "assistant_message",
            "running",
            json!({"text":"one","messageId":"message","streaming":true}),
        ),
        item(
            "reasoning",
            "running",
            json!({"text":"think","streaming":true}),
        ),
    ];
    p.visible_turn_items = p.turn_items.iter().cloned().map(projected).collect();
    let first = shared(p.clone(), None);
    let mut views = ThreadDetailViews::default();
    views.update(Some(first.clone()), None, &parse);
    let quiet = views.quiet().unwrap();
    let timeline = views.timeline();
    let queue = views.queue.clone().unwrap();
    if let OrchestrationV2TurnItem::AssistantMessage(i) = &mut p.turn_items[0] {
        i.text = "two".into();
        i.updated_at = WireDateTime("9".into());
    }
    p.visible_turn_items[0].item = p.turn_items[0].clone();
    p.thread.last_visited_at = Some(WireDateTime("9".into()));
    p.thread.updated_at = WireDateTime("9".into());
    p.updated_at = WireDateTime("9".into());
    let streamed = shared(p.clone(), Some(&first));
    assert!(same_thread_structure(&first, &streamed, None));
    assert!(same_thread_structure(
        &first,
        &streamed,
        Some(DetailReplacement {
            item: 0,
            visible: 0
        })
    ));
    views.update(
        Some(streamed.clone()),
        Some(DetailReplacement {
            item: 0,
            visible: 0,
        }),
        &parse,
    );
    assert!(Rc::ptr_eq(&quiet, &views.quiet().unwrap()));
    assert!(Rc::ptr_eq(&timeline, &views.timeline()));
    assert!(Rc::ptr_eq(&queue, views.queue.as_ref().unwrap()));
    assert_eq!(
        views.assistant_message_text(&IdOrIndex::Id(TurnItemId("item".into()))),
        Some("two")
    );
    for change in 0..6 {
        let mut changed = p.clone();
        match change {
            0 => {
                if let OrchestrationV2TurnItem::AssistantMessage(i) = &mut changed.turn_items[0] {
                    i.streaming = false;
                }
            }
            1 => {
                if let OrchestrationV2TurnItem::AssistantMessage(i) = &mut changed.turn_items[0] {
                    i.title = Some("title".into());
                }
            }
            2 => {
                changed.thread.title =
                    t3_contracts::base_schemas::TrimmedNonEmptyString("New title".into())
            }
            3 => changed.runs.push(run("run", 1, "running")),
            4 => changed.turn_items.push(item(
                "command_execution",
                "completed",
                json!({"input":"ls"}),
            )),
            _ => changed.visible_turn_items[0].position += 1,
        }
        assert!(!same_thread_structure(
            &streamed,
            &shared(changed, Some(&streamed)),
            None
        ));
    }
    views.update(None, None, &parse);
    assert!(views.quiet().is_none());
    assert!(views.timeline().is_empty());
    assert!(!is_complete_turn_item_row(&p.visible_turn_items[0]));
    assert_eq!(derive_workspace_mutation_id(&[], None).unwrap(), None);
    let command = Rc::new(projected(item(
        "command_execution",
        "completed",
        json!({"input":"ls"}),
    )));
    assert_eq!(
        derive_workspace_mutation_id(&[command], Some("10")).unwrap(),
        Some("[\"item\",\"10\"]".into())
    );
}

#[test]
fn workflow_queue_filters_automation_sorts_and_tracks_compaction_and_handoff() {
    use workflows::*;
    let mut p = projection();
    let mut a = run("a", 3, "queued");
    a.queue_position = Some(Some(1));
    let mut b = run("b", 2, "queued");
    b.queue_position = Some(Some(2));
    b.queue_held = Some(Some(true));
    p.runs = vec![b.clone(), a.clone()];
    p.messages = vec![message("a", "message-a", "user")];
    p.messages[0].text = "Queued edit".into();
    let state = derive_thread_queue_workflow_state(&p);
    assert!(state.is_held);
    assert!(state.can_send_now);
    assert_eq!(
        state
            .queued_runs
            .iter()
            .map(|q| &q.run.id)
            .collect::<Vec<_>>(),
        vec![&a.id, &b.id]
    );
    assert_eq!(state.queued_runs[0].text, "Queued edit");
    assert_eq!(state.queued_runs[1].text, "Queued message");
    p.messages[0].delegated_completion = Some(Some(
        serde_json::from_value(json!({"parentRunId":"parent","generation":1,"taskIds":[]}))
            .unwrap(),
    ));
    assert_eq!(get_user_queued_thread_runs(&p).len(), 1);
    let active = run("active", 1, "running");
    p.runs.insert(0, active);
    p.messages.push(message("active", "message-active", "user"));
    p.messages[1].text = "/compact".into();
    let state = derive_thread_queue_workflow_state(&p);
    assert!(state.promote_skips_compaction);
    assert!(state.can_promote_to_steer);
    assert!(!state.can_send_now);
    p.runs[0].status = OrchestrationV2RunStatus::Waiting;
    assert_eq!(resolve_latest_merge_back_run(&p).unwrap().id, p.runs[0].id);
    p.runs.push(run("newer", 4, "running"));
    assert!(resolve_latest_merge_back_run(&p).is_none());
    p.runs.clear();
    assert!(thread_supports_provider_handoff(Some(&p)));
    assert!(!can_detach_thread_provider_session(&p));
    let assistant = projected(item(
        "assistant_message",
        "completed",
        json!({"text":"Done","messageId":"m","streaming":false}),
    ));
    assert!(can_fork_projected_assistant_item(&assistant, None));
}

#[test]
fn subagent_roster_keeps_background_and_workflows_and_obeys_run_ordinals() {
    use subagents::*;
    let mut p = projection();
    p.runs = vec![
        run("latest", 3, "completed"),
        run("old", 1, "completed"),
        run("queued", 4, "queued"),
    ];
    p.subagents = vec![
        agent("old", Some("old"), "completed"),
        agent("new", Some("latest"), "completed"),
    ];
    p.subagents[0].updated_at = WireDateTime("9".into());
    let state = derive_thread_turn_subagents(&p, parse).unwrap();
    assert_eq!(state.run_id, Some(&p.runs[0].id));
    assert_eq!(state.subagents.len(), 1);
    assert!(state.latest_turn);
    assert_eq!(
        resolve_subagent_pill_segment(Some(&state)).unwrap().0,
        "1 done"
    );
    p.subagents[0].status = OrchestrationV2TurnItemStatus::Running;
    let state = derive_thread_turn_subagents(&p, parse).unwrap();
    assert_eq!(state.subagents.len(), 2);
    assert_eq!(state.live_count, 1);
    assert_eq!(
        resolve_subagent_pill_segment(Some(&state)).unwrap().1,
        "1 of 2 agents working"
    );
    p.runs[1].status = OrchestrationV2RunStatus::RolledBack;
    assert_eq!(
        derive_thread_turn_subagents(&p, parse)
            .unwrap()
            .subagents
            .len(),
        1
    );
    p.runs.push(run("later", 5, "completed"));
    let state = derive_thread_turn_subagents(&p, parse).unwrap();
    assert!(!state.latest_turn);
    assert!(resolve_subagent_pill_segment(Some(&state)).is_none());
    p.subagents[1].workflow = Some(Some(
        serde_json::from_value(json!({"agents":[],"phases":[]})).unwrap(),
    ));
    assert!(
        resolve_subagent_pill_segment(derive_thread_turn_subagents(&p, parse).as_ref()).is_some()
    );
    p.subagents[1].status = OrchestrationV2TurnItemStatus::Idle;
    let state = derive_thread_turn_subagents(&p, parse).unwrap();
    assert_eq!((state.live_count, state.settled_count), (0, 0));
}

#[test]
fn workflow_expansion_keeps_wall_clock_times_and_settles_unfinished_members() {
    use subagent_runtime::*;
    let mut coordinator = agent("coordinator", None, "completed");
    coordinator.completed_at = Some(WireDateTime("9000".into()));
    coordinator.workflow=Some(Some(serde_json::from_value(json!({"name":"Build","phases":[{"index":1,"title":"Declared"},{"index":3,"title":"Later"}],"agents":[{"index":1,"label":"a","state":"completed","phaseIndex":1,"phaseTitle":"Guessed","model":"model","effort":"xhigh","speed":"standard","startedAt":1000,"completedAt":4432,"durationMs":1500,"totalTokens":71141,"toolCalls":3,"prompt":"Reply A","result":"A"},{"index":2,"label":"b","state":"running","phaseIndex":2},{"index":3,"label":"c","state":"queued"}]})).unwrap()));
    let agents =
        projected_subagents_to_runtime(&[coordinator.clone()], |n| WireDateTime(n.to_string()));
    assert_eq!(agents.len(), 4);
    assert_eq!(agents[1].completed_at.as_ref().unwrap().0, "4432");
    assert_eq!(agents[1].usage.as_ref().unwrap().duration_ms, Some(1500));
    assert_eq!(agents[1].progress.as_deref(), Some("Reply A"));
    assert_eq!(agents[1].result.as_deref(), Some("A"));
    assert_eq!(agents[2].status, OrchestrationV2TurnItemStatus::Completed);
    assert_eq!(agents[2].completed_at, coordinator.completed_at);
    let groups = derive_workflow_groups(&agents, parse);
    assert_eq!(
        groups[0]
            .phases
            .iter()
            .map(|p| (p.index, p.title.as_str(), p.settled_count))
            .collect::<Vec<_>>(),
        vec![(1, "Declared", 1), (2, "Phase 3", 1), (3, "Later", 0)]
    );
    assert_eq!(groups[0].unphased_members.len(), 1);
    assert_eq!(groups[0].phases[2].state, WorkflowPhaseState::Pending);
    let selection = workflow_member_model_selection(
        &agents[1],
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(selection).unwrap(),
        json!({"instanceId":"codex","model":"model","options":[{"id":"effort","value":"xhigh"},{"id":"fastMode","value":false}]})
    );
    assert!(
        workflow_member_model_selection(
            &agents[0],
            t3_contracts::provider_instance::ProviderInstanceId("codex".into())
        )
        .is_none()
    );
    for status in ["failed", "cancelled", "interrupted"] {
        let mut raw = serde_json::to_value(&coordinator).unwrap();
        raw["status"] = json!(status);
        let rows = projected_subagents_to_runtime(&[serde_json::from_value(raw).unwrap()], |n| {
            WireDateTime(n.to_string())
        });
        assert_eq!(rows[2].status, rows[0].status);
        assert_eq!(rows[1].status, OrchestrationV2TurnItemStatus::Completed);
    }
    coordinator.workflow = None;
    coordinator.prompt = format!("{}😀suffix", "x".repeat(76));
    let rows = projected_subagents_to_runtime(&[coordinator], |n| WireDateTime(n.to_string()));
    assert_eq!(rows[0].title, format!("{}...", "x".repeat(76)));
}

#[test]
fn workflow_openings_gate_parent_reads_and_honor_explicit_marks() {
    use subagent_runtime::*;
    let mut p = projection();
    p.thread.creation_source = OrchestrationV2CreationSource::Provider;
    p.thread.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    p.thread.lineage.parent_thread_id = Some(ThreadId("parent".into()));
    p.messages = vec![message("run", "opening", "user")];
    p.messages[0].text = "// lead\n/* comment */ export const meta = {}".into();
    assert!(workflow_opening_parent_thread_id(&p).is_some());
    let opening = resolve_workflow_opening(&p, None, |n| WireDateTime(n.to_string())).unwrap();
    assert_eq!(
        workflow_opening_headline(&opening),
        ("Workflow".into(), None, None)
    );
    p.messages[0].workflow_script = Some(Some(false));
    assert!(resolve_workflow_opening(&p, None, |n| WireDateTime(n.to_string())).is_none());
    assert!(workflow_opening_parent_thread_id(&p).is_none());
    p.messages[0].workflow_script = Some(Some(true));
    p.messages[0].text = "marked script".into();
    assert!(resolve_workflow_opening(&p, None, |n| WireDateTime(n.to_string())).is_some());
    for script in [
        "export const metadata = {}",
        "/* unterminated",
        "// no script",
        "ordinary task",
    ] {
        assert!(!is_workflow_script_message(
            &ConversationMessageRole::User,
            script,
            None
        ));
    }
    assert!(!is_workflow_script_message(
        &ConversationMessageRole::Assistant,
        "export const meta = {}",
        None
    ));
}

#[test]
fn announcements_baseline_each_thread_and_announce_turns_and_requests_once() {
    use activity_announcement::*;
    let snap =
        |status, id: Option<&str>, run_status, request: Option<&str>| ThreadActivitySnapshot {
            status,
            run_id: id.map(|id| IdOrIndex::Id(RunId(id.into()))),
            run_status,
            request_id: request.map(str::to_owned),
        };
    let (mut memory, said) = next_thread_announcement(
        None,
        snap(
            ThreadActivityStatus::Idle,
            Some("old"),
            Some(OrchestrationV2RunStatus::Completed),
            None,
        ),
    );
    assert!(said.is_none());
    for (status, id, run_status, request, expected) in [
        (
            ThreadActivityStatus::Working,
            Some("old"),
            Some(OrchestrationV2RunStatus::Completed),
            None,
            Some("Working"),
        ),
        (
            ThreadActivityStatus::Working,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            None,
            None,
        ),
        (
            ThreadActivityStatus::Approval,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            Some("a"),
            Some("Needs approval"),
        ),
        (
            ThreadActivityStatus::Approval,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            Some("a"),
            None,
        ),
        (
            ThreadActivityStatus::Working,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            None,
            None,
        ),
        (
            ThreadActivityStatus::Input,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            Some("q"),
            Some("Needs your input"),
        ),
        (
            ThreadActivityStatus::Input,
            Some("new"),
            Some(OrchestrationV2RunStatus::Running),
            Some("q2"),
            Some("Needs your input"),
        ),
        (
            ThreadActivityStatus::Idle,
            Some("new"),
            Some(OrchestrationV2RunStatus::Waiting),
            None,
            None,
        ),
        (
            ThreadActivityStatus::Idle,
            Some("new"),
            Some(OrchestrationV2RunStatus::Completed),
            None,
            Some("Done"),
        ),
        (
            ThreadActivityStatus::Idle,
            Some("new"),
            Some(OrchestrationV2RunStatus::Completed),
            None,
            None,
        ),
        (
            ThreadActivityStatus::Working,
            Some("queued"),
            Some(OrchestrationV2RunStatus::Running),
            None,
            Some("Working"),
        ),
        (
            ThreadActivityStatus::Idle,
            Some("queued"),
            Some(OrchestrationV2RunStatus::RolledBack),
            None,
            None,
        ),
        (
            ThreadActivityStatus::Working,
            Some("next"),
            Some(OrchestrationV2RunStatus::Running),
            None,
            Some("Working"),
        ),
        (
            ThreadActivityStatus::Idle,
            Some("next"),
            Some(OrchestrationV2RunStatus::Failed),
            None,
            Some("Failed"),
        ),
    ] {
        let (next, said) =
            next_thread_announcement(Some(&memory), snap(status, id, run_status, request));
        assert_eq!(said, expected);
        memory = next;
    }
    for outcome in [
        OrchestrationV2RunStatus::Completed,
        OrchestrationV2RunStatus::Failed,
        OrchestrationV2RunStatus::Interrupted,
        OrchestrationV2RunStatus::Cancelled,
    ] {
        let (baseline, said) = next_thread_announcement(
            None,
            snap(
                ThreadActivityStatus::Working,
                Some("run"),
                Some(OrchestrationV2RunStatus::Running),
                None,
            ),
        );
        assert!(said.is_none());
        let (_, said) = next_thread_announcement(
            Some(&baseline),
            snap(
                ThreadActivityStatus::Idle,
                Some("run"),
                Some(outcome.clone()),
                None,
            ),
        );
        assert_eq!(said, run_outcome_text(Some(&outcome)));
    }
}

#[test]
fn announcements_ignore_stale_endings_and_cancelled_unstarted_queue_rows() {
    use activity_announcement::*;
    let mut p = projection();
    p.runs = vec![run("old", 1, "completed"), run("queued", 2, "cancelled")];
    p.runs[1].started_at = None;
    assert_eq!(
        announced_run(Some(&p.runs[1]), &p.runs, &parse).unwrap().id,
        p.runs[0].id
    );
    let snap = |status, run_status| ThreadActivitySnapshot {
        status,
        run_id: Some(p.runs[0].id.clone()),
        run_status: Some(run_status),
        request_id: None,
    };
    let (idle, _) = next_thread_announcement(
        None,
        snap(
            ThreadActivityStatus::Idle,
            OrchestrationV2RunStatus::Waiting,
        ),
    );
    let (working, said) = next_thread_announcement(
        Some(&idle),
        snap(
            ThreadActivityStatus::Working,
            OrchestrationV2RunStatus::Waiting,
        ),
    );
    assert_eq!(said, Some("Working"));
    let (waiting, said) = next_thread_announcement(
        Some(&working),
        snap(
            ThreadActivityStatus::Idle,
            OrchestrationV2RunStatus::Completed,
        ),
    );
    assert!(said.is_none());
    assert_eq!(waiting.turn_run, Some(TurnRun::Unknown));
    assert!(is_detail_behind_shell(
        None,
        Some((&p.runs[0].id, &p.runs[0].status))
    ));
    assert!(!is_detail_behind_shell(
        Some((&p.runs[0].id, &p.runs[0].status)),
        Some((&p.runs[0].id, &p.runs[0].status))
    ));
}

#[test]
fn feedback_reports_before_upload_preserves_errors_and_allows_concurrent_submissions() {
    use feedback::*;
    for (text, expected) in [
        ("/feedback", Some(None)),
        (" /FEEDBACK \n reason \t", Some(Some("reason"))),
        ("/feedbackx", None),
        ("word /feedback", None),
        ("/feedback line\nsecond", Some(Some("line\nsecond"))),
    ] {
        assert_eq!(parse_codex_feedback_command(text), expected);
    }
    let updates = Rc::new(std::cell::RefCell::new(Vec::new()));
    let clear = Rc::new(std::cell::Cell::new(false));
    let details = FeedbackSubmission {
        id: MessageId("a".into()),
        command: "/feedback reason".into(),
        created_at: "1".into(),
        status: FeedbackStatus::Uploading,
    };
    let (tx, rx) = futures::channel::oneshot::channel::<Result<String, crate::ClientError>>();
    let mut upload = Box::pin(submit_codex_feedback(
        details.clone(),
        {
            let clear = clear.clone();
            move || clear.set(true)
        },
        {
            let updates = updates.clone();
            move |s| updates.borrow_mut().push(s)
        },
        || async { rx.await.unwrap() },
        String::clone,
    ));
    assert!(upload.as_mut().now_or_never().is_none());
    assert!(clear.get());
    assert_eq!(updates.borrow().len(), 1);
    let mut other = details.clone();
    other.id = MessageId("b".into());
    let result = block_on(submit_codex_feedback(
        other,
        || {},
        {
            let updates = updates.clone();
            move |s| updates.borrow_mut().push(s)
        },
        || async { Ok::<_, crate::ClientError>("b-result".to_owned()) },
        String::clone,
    ));
    assert!(result.is_ok());
    assert_eq!(updates.borrow().last().unwrap().id.0, "b");
    tx.send(Err(crate::ClientError::InvalidPayload(
        "upload failed".into(),
    )))
    .unwrap();
    assert!(block_on(upload).is_err());
    assert!(
        matches!(&updates.borrow().last().unwrap().status,FeedbackStatus::Failed {error_message} if error_message.contains("upload failed"))
    );
    let interrupted = block_on(submit_codex_feedback(
        details.clone(),
        || {},
        |s| {
            if s.status == FeedbackStatus::Interrupted {
                assert!(codex_feedback_notice(&s).is_none());
            }
        },
        || async { Err::<String, _>(crate::ClientError::Cancelled) },
        String::clone,
    ));
    assert!(interrupted.is_err());
    let mut sent = details;
    sent.status = FeedbackStatus::Sent {
        feedback_id: "feedback".into(),
    };
    assert_eq!(
        codex_feedback_message(&sent, false).text,
        "/feedback reason"
    );
    assert_eq!(codex_feedback_message(&sent, true).id.0, "a:feedback");
    assert_eq!(
        codex_feedback_notice(&sent).unwrap().1.as_deref(),
        Some("Thread ID: feedback")
    );
}

#[test]
fn undo_tokens_are_scoped_and_restore_pin_before_future_snooze() {
    use undo::*;
    let registry = ThreadUndoRegistry::default();
    let mut subscription = registry.subscribe();
    assert_eq!(
        block_on(futures::StreamExt::next(&mut subscription)),
        Some(0)
    );
    let first = registry.begin("settle".into(), key("env", "thread"));
    let second = registry.begin("settle".into(), key("env", "thread"));
    let remote = registry.begin("settle".into(), key("other", "thread"));
    assert!(!first.is_current());
    assert!(second.is_current());
    first.finish();
    assert!(second.is_current());
    second.finish();
    assert!(!second.is_current());
    assert!(remote.is_current());
    assert_eq!(
        block_on(futures::StreamExt::next(&mut subscription)),
        Some(registry.revision())
    );
    assert!(
        futures::StreamExt::next(&mut subscription)
            .now_or_never()
            .is_none()
    );
    let plan = plan_thread_undo(
        UndoAction::Settle,
        true,
        Some("order".into()),
        Some("10".into()),
    );
    assert_eq!(
        plan,
        vec![
            ThreadUndoStep::Unsettle,
            ThreadUndoStep::Pin {
                order_key: Some("order".into())
            },
            ThreadUndoStep::Snooze {
                snoozed_until: "10".into()
            }
        ]
    );
    let mut seen = vec![];
    assert_eq!(
        block_on(execute_thread_undo(
            &plan,
            |step| {
                seen.push(step);
                async { true }
            },
            |ok| *ok,
            || 20,
            |s| s.parse().ok()
        )),
        Some(true)
    );
    assert_eq!(seen.len(), 2);
    seen.clear();
    assert_eq!(
        block_on(execute_thread_undo(
            &plan,
            |step| {
                seen.push(step);
                async { false }
            },
            |ok| *ok,
            || 0,
            |s| s.parse().ok()
        )),
        Some(false)
    );
    assert_eq!(seen.len(), 1);
    assert_eq!(
        plan_thread_undo(UndoAction::Snooze, false, None, None),
        vec![ThreadUndoStep::Unsnooze]
    );
}

#[test]
fn family_queries_wait_for_authority_fail_config_and_reject_old_generations() {
    use families::*;
    let platform = crate::testing::FakePlatform::default();
    let mut service = ThreadFamilies::default();
    let key = key("env", "parent");
    assert!(
        service
            .query(key.clone(), 0, crate::types::Generation(1))
            .is_none()
    );
    let (mount, resource) = service
        .mount(key.clone(), 1, crate::types::Generation(1), 0)
        .unwrap();
    let observer = resource.observe(0);
    let read = || async {
        Ok(OrchestrationV2ThreadFamily {
            snapshot_sequence: 3,
            threads: vec![shell("child")],
        })
    };
    assert!(
        block_on(read_family(
            &resource,
            &platform,
            key.thread.clone(),
            async { Err(crate::ClientError::Disconnected) },
            |_| read()
        ))
        .is_err()
    );
    assert_eq!(thread_family_read(&resource.snapshot()), FamilyRead::Failed);
    let mut state=crate::state::shell::ShellState {generation:1,snapshot:Some(Rc::new(serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":3,"projects":[],"threads":[thread_json("parent")],"archivedThreads":[]})).unwrap())),..Default::default()};
    assert_eq!(
        service
            .mounts
            .merge(&key.environment, &state)
            .unwrap()
            .threads
            .len(),
        1
    );
    assert!(
        block_on(read_family(
            &resource,
            &platform,
            key.thread.clone(),
            async { Ok(true) },
            |_| read()
        ))
        .is_ok()
    );
    assert_eq!(
        thread_family_read(&resource.snapshot()),
        FamilyRead::Settled
    );
    assert_eq!(
        service
            .mounts
            .merge(&key.environment, &state)
            .unwrap()
            .threads
            .len(),
        2
    );
    state.generation = 2;
    let next = service
        .query(key.clone(), 2, crate::types::Generation(1))
        .unwrap();
    assert_eq!(thread_family_read(&next.snapshot()), FamilyRead::Pending);
    assert_eq!(
        service
            .mounts
            .merge(&key.environment, &state)
            .unwrap()
            .threads
            .len(),
        1
    );
    let called = std::cell::Cell::new(false);
    assert!(
        block_on(read_family(
            &next,
            &platform,
            key.thread.clone(),
            async { Ok(false) },
            |_| {
                called.set(true);
                read()
            }
        ))
        .is_ok()
    );
    assert!(!called.get());
    assert!(next.snapshot().value.unwrap().threads.is_empty());
    drop((observer, mount));
}

#[test]
fn mounted_family_rows_share_lifetime_and_filter_snapshot_cuts_and_removals() {
    use families::*;
    let env = EnvironmentId("env".into());
    let mounts = FamilyMounts::default();
    let rows = Rc::new(std::cell::RefCell::new(vec![
        Rc::new(OrchestrationV2ThreadFamily {
            snapshot_sequence: 5,
            threads: vec![shell("kept"), shell("old"), shell("removed")],
        }),
        Rc::new(OrchestrationV2ThreadFamily {
            snapshot_sequence: 3,
            threads: vec![shell("stale")],
        }),
    ]));
    let mut state=crate::state::shell::ShellState {generation:1,snapshot:Some(Rc::new(serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":9,"projects":[],"threads":[thread_json("kept")],"archivedThreads":[],"olderSettled":{"before":"10","asOfSequence":4,"counts":[]}})).unwrap())),..Default::default()};
    state.removed_at.insert(ThreadId("removed".into()), 9);
    let mut first = mounts.mount_rows(env.clone(), "page".into(), rows);
    let second = mounts.mount_rows(
        env.clone(),
        "page".into(),
        Rc::new(std::cell::RefCell::new(vec![])),
    );
    assert_eq!(
        mounts
            .merge(&env, &state)
            .unwrap()
            .threads
            .iter()
            .map(|t| t.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["kept", "old"]
    );
    first.release();
    first.release();
    assert_eq!(mounts.merge(&env, &state).unwrap().threads.len(), 2);
    drop(second);
    assert!(Rc::ptr_eq(
        &mounts.merge(&env, &state).unwrap(),
        state.snapshot.as_ref().unwrap()
    ));
    let old = mounts.mount_rows(
        env.clone(),
        "page".into(),
        Rc::new(std::cell::RefCell::new(vec![])),
    );
    mounts.remove_environment(&env);
    let replacement = mounts.mount_rows(
        env.clone(),
        "page".into(),
        Rc::new(std::cell::RefCell::new(vec![Rc::new(
            OrchestrationV2ThreadFamily {
                snapshot_sequence: 9,
                threads: vec![shell("replacement")],
            },
        )])),
    );
    drop(old);
    assert_eq!(mounts.merge(&env, &state).unwrap().threads.len(), 2);
    drop(replacement);
}

#[test]
fn search_keys_scopes_results_visibility_and_shown_order_match_source() {
    use crate::types::QueryState;
    use search::*;
    let ids = vec![EnvironmentId("b".into()), EnvironmentId("a".into())];
    let encoded = make_thread_search_key(&ids, "needle", &ThreadSearchScope::default()).unwrap();
    assert_eq!(encoded, "[[\"a\",\"b\"],\"needle\"]");
    assert!(parse_thread_search_key(&encoded).is_some());
    for bad in [
        "null",
        "[[],3]",
        "[[2],\"query\"]",
        "[[],\"query\",null]",
        "[[],\"x\"]",
    ] {
        assert!(parse_thread_search_key(bad).is_none());
    }
    assert_ne!(
        thread_search_match_key(&key("a:b", "c")).unwrap(),
        thread_search_match_key(&key("a", "b:c")).unwrap()
    );
    let scope = ThreadSearchScope {
        project_ids: Some(vec![t3_contracts::base_schemas::ProjectId("p".into())]),
        environment_labels: Some(BTreeMap::from([(ids[0].clone(), "Laptop".into())])),
        titles_only: None,
    };
    assert!(thread_search_input("ui", &scope, &ids[0], None).is_none());
    let input =
        thread_search_input("ui", &scope, &ids[0], Some(WireDateTime("10".into()))).unwrap();
    assert_eq!(input.titles_only, Some(true));
    assert_eq!(input.project_ids, scope.project_ids);
    assert_eq!(input.environment_label.as_deref(), Some("Laptop"));
    assert!(
        thread_search_input("needle", &scope, &ids[1], None)
            .unwrap()
            .project_ids
            .is_none()
    );
    let fast: Vec<_> = (0..12).map(|i| format!("a{i}")).collect();
    let first = next_shown_order(None, "rust", &fast, 10);
    let mut ranked: Vec<_> = (0..4).map(|i| format!("b{i}")).collect();
    ranked.extend(fast.clone());
    let second = next_shown_order(Some(first), "rust", &ranked, 10);
    assert_eq!(second.shown, 13);
    assert_eq!(&second.keys[..10], &fast[..10]);
    assert_eq!(&second.keys[10..13], &["b0", "b1", "b2"]);
    assert!(Rc::ptr_eq(
        &second,
        &next_shown_order(Some(second.clone()), "rust", &ranked, 10)
    ));
    let previous = Rc::new(SettledThreadSearch {
        query: "websock".into(),
        matches: vec![],
    });
    for query in ["websocket", "webso"] {
        assert!(Rc::ptr_eq(
            &previous,
            &visible_thread_search(query, None, Some(previous.clone()))
        ));
    }
    assert!(
        visible_thread_search("billing", None, Some(previous))
            .query
            .is_empty()
    );
    let ready = QueryState {
        value: Some(Rc::new(OrchestrationV2SearchThreadsResult {
            matches: vec![],
            snapshot_sequence: Some(9),
            threads: Some(vec![shell("old")]),
        })),
        loading: false,
        error: None,
        revision: 0,
    };
    let failed = QueryState {
        value: None,
        loading: false,
        error: Some("secret".into()),
        revision: 0,
    };
    let merged = merge_thread_search_results([(&ids[0], &ready), (&ids[1], &failed)]);
    assert_eq!(merged.rows.len(), 1);
    assert!(!merged.is_loading);
    assert_eq!(merged.rows[0].1.snapshot_sequence, 9);
}

#[test]
fn archive_errors_are_generic_and_children_follow_hidden_parents_across_environments() {
    use crate::types::QueryState;
    use archived::*;
    let parent = EnvironmentId("parent".into());
    let task_env = EnvironmentId("tasks".into());
    let mut task = shell("task");
    task.delegated_from = Some(Some(
        serde_json::from_value(
            json!({"environmentId":"parent","threadId":"owner","title":"Owner"}),
        )
        .unwrap(),
    ));
    let snapshot = |threads| {
        Rc::new(OrchestrationV2ArchivedShellSnapshot {
            schema_version: 1,
            snapshot_sequence: 1,
            projects: vec![],
            threads,
        })
    };
    let tasks = QueryState {
        value: Some(snapshot(vec![task])),
        loading: false,
        error: None,
        revision: 0,
    };
    let mut owner = shell("owner");
    owner.tags = Some(Some(t3_contracts::thread_tags::StoredThreadTags(vec![
        "private".into(),
    ])));
    let tags = vec!["private".into()];
    for (parent_page, live, expected) in [
        (Some(snapshot(vec![owner])), false, 0),
        (Some(snapshot(vec![])), true, 1),
        (None, false, 0),
    ] {
        let query = QueryState {
            value: parent_page,
            loading: false,
            error: None,
            revision: 0,
        };
        let live = if live {
            BTreeSet::from([key("parent", "owner")])
        } else {
            BTreeSet::new()
        };
        let result = archived_thread_snapshots(
            &[
                ArchivedEnvironment {
                    id: &parent,
                    query: &query,
                    hidden_tags: Some(&tags),
                },
                ArchivedEnvironment {
                    id: &task_env,
                    query: &tasks,
                    hidden_tags: None,
                },
            ],
            &BTreeSet::new(),
            &live,
            true,
        );
        assert_eq!(
            result
                .snapshots
                .iter()
                .find(|(id, _)| id == &task_env)
                .unwrap()
                .1
                .threads
                .len(),
            expected
        );
    }
    let failed = QueryState {
        value: None,
        loading: false,
        error: Some("credential=secret".into()),
        revision: 0,
    };
    let result = archived_thread_snapshots(
        &[ArchivedEnvironment {
            id: &parent,
            query: &failed,
            hidden_tags: None,
        }],
        &BTreeSet::new(),
        &BTreeSet::new(),
        false,
    );
    assert_eq!(result.error, Some("Failed to load archived threads."));
    assert!(!result.is_loading);
    assert!(result.snapshots.is_empty());
}

#[test]
fn older_settled_frontiers_hold_a_correct_global_prefix_and_count_loaded_rows_once() {
    use older_settled::*;
    let parse = |at: &WireDateTime| at.0.parse().ok();
    let old = |before: i64| OlderSettled {
        before: WireDateTime(before.to_string()),
        before_ms: before,
        counts: BTreeMap::from([(t3_contracts::base_schemas::ProjectId("project".into()), 3)]),
    };
    let cursor = |at: &str, id: Option<&str>| OrchestrationV2OlderSettledCursor {
        settled_at: WireDateTime(at.into()),
        thread_id: id.map(|id| ThreadId(id.into())),
    };
    let mut environments = BTreeMap::from([
        (
            EnvironmentId("a".into()),
            OlderSettledShelfEnvironment {
                older: Some(old(30)),
                pages: OlderSettledPages {
                    frontier: Some(cursor("30", None)),
                    ..Default::default()
                },
            },
        ),
        (
            EnvironmentId("b".into()),
            OlderSettledShelfEnvironment {
                older: Some(old(20)),
                pages: OlderSettledPages {
                    frontier: Some(cursor("20", None)),
                    ..Default::default()
                },
            },
        ),
    ]);
    assert_eq!(
        settled_shelf_frontier(environments.values(), &parse)
            .unwrap()
            .settled_at
            .0,
        "30"
    );
    let mut row = shell("thread");
    row.settled_override = Some(AppThreadSettledOverride::Settled);
    row.settled_at = Some(WireDateTime("25".into()));
    assert!(!is_listed_settled_thread(
        &row,
        settled_shelf_frontier(environments.values(), &parse),
        &parse
    ));
    let wanted = wanted_older_settled_pages(&BTreeMap::new(), &environments, 0, 10, &parse);
    assert_eq!(wanted, BTreeMap::from([(EnvironmentId("a".into()), 1)]));
    environments
        .get_mut(&EnvironmentId("a".into()))
        .unwrap()
        .pages
        .status = PageStatus::Pending;
    assert!(wanted_older_settled_pages(&BTreeMap::new(), &environments, 0, 10, &parse).is_empty());
    environments
        .get_mut(&EnvironmentId("a".into()))
        .unwrap()
        .pages
        .status = PageStatus::Failed;
    assert_eq!(
        settled_shelf_frontier(environments.values(), &parse)
            .unwrap()
            .settled_at
            .0,
        "20"
    );
    environments
        .get_mut(&EnvironmentId("a".into()))
        .unwrap()
        .pages = OlderSettledPages {
        frontier: Some(cursor("20", Some("A"))),
        ..Default::default()
    };
    assert_eq!(
        settled_shelf_frontier(environments.values(), &parse)
            .unwrap()
            .thread_id
            .as_ref()
            .unwrap()
            .0,
        "A"
    );
    row.settled_at = Some(WireDateTime("20".into()));
    row.id = ThreadId("B".into());
    assert!(!is_listed_settled_thread(
        &row,
        settled_shelf_frontier(environments.values(), &parse),
        &parse
    ));
    row.id = ThreadId("A".into());
    assert!(is_listed_settled_thread(
        &row,
        settled_shelf_frontier(environments.values(), &parse),
        &parse
    ));
    let scopes = [OlderSettledScope {
        environment: EnvironmentId("a".into()),
        project_ids: None,
    }];
    let key = key("a", "A");
    row.settled_at = Some(WireDateTime("10".into()));
    assert_eq!(
        unloaded_older_settled_count(
            &scopes,
            &BTreeMap::from([(EnvironmentId("a".into()), old(30))]),
            &[(&key, &row), (&key, &row)],
            &parse
        ),
        2
    );
    let initial = older_settled_count_key(&[row.clone()], 30, &BTreeMap::new(), &parse);
    row.settled_override = None;
    assert_ne!(
        initial,
        older_settled_count_key(&[row.clone()], 30, &BTreeMap::new(), &parse)
    );
    let removed = BTreeMap::from([(ThreadId("removed".into()), 9)]);
    assert_ne!(
        older_settled_count_key(&[], 30, &removed, &parse),
        older_settled_count_key(&[], 30, &BTreeMap::new(), &parse)
    );
    row.created_at = WireDateTime("40".into());
    assert_eq!(
        older_settled_count_key(&[row.clone()], 30, &BTreeMap::new(), &parse),
        "0|"
    );
    row.created_at = WireDateTime("10".into());
    row.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    assert_eq!(
        older_settled_count_key(&[row], 30, &BTreeMap::new(), &parse),
        "0|"
    );
    environments
        .get_mut(&EnvironmentId("a".into()))
        .unwrap()
        .pages
        .frontier = Some(cursor("2026-11-13T08:00:00-05:00", None));
    environments
        .get_mut(&EnvironmentId("b".into()))
        .unwrap()
        .pages
        .frontier = Some(cursor("2026-11-13T12:30:00Z", None));
    let offsets = |at: &WireDateTime| Some(if at.0.contains("-05:00") { 1300 } else { 1230 });
    assert_eq!(
        settled_shelf_frontier(environments.values(), &offsets)
            .unwrap()
            .settled_at
            .0,
        "2026-11-13T08:00:00-05:00"
    );
}

#[test]
fn older_settled_pages_wait_in_order_stop_on_short_pages_and_ignore_cached_generations() {
    use crate::types::QueryState;
    use older_settled::*;
    let mut state=crate::state::shell::ShellState {generation:1,snapshot:Some(Rc::new(serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":4,"projects":[],"threads":[],"archivedThreads":[],"olderSettled":{"before":"30","asOfSequence":4,"counts":[]}})).unwrap())),..Default::default()};
    let mut rows: Vec<_> = (0..25)
        .map(|i| {
            let mut t = shell(&format!("t{i:02}"));
            t.settled_at = Some(WireDateTime("20".into()));
            t
        })
        .collect();
    let full = Rc::new(OrchestrationV2OlderSettledThreadsPage {
        snapshot_sequence: 4,
        threads: rows.clone(),
        counts: vec![],
    });
    let mut calls = vec![];
    let pages = read_older_settled_pages(&state, 2, |cursor| {
        calls.push(cursor.clone());
        if calls.len() == 1 {
            QueryState {
                value: Some(full.clone()),
                loading: false,
                error: None,
                revision: 0,
            }
        } else {
            QueryState {
                value: None,
                loading: true,
                error: Some("retrying".into()),
                revision: 0,
            }
        }
    });
    assert_eq!(calls.len(), 2);
    assert_eq!(pages.status, PageStatus::Pending);
    assert_eq!(pages.frontier.unwrap().thread_id.unwrap().0, "t24");
    rows.truncate(2);
    let short = Rc::new(OrchestrationV2OlderSettledThreadsPage {
        snapshot_sequence: 4,
        threads: rows,
        counts: vec![],
    });
    let pages = read_older_settled_pages(&state, 5, |_| QueryState {
        value: Some(short.clone()),
        loading: false,
        error: None,
        revision: 0,
    });
    assert!(pages.frontier.is_none());
    assert_eq!(pages.reads.len(), 1);
    state.generation = 0;
    let called = std::cell::Cell::new(false);
    let pages = read_older_settled_pages(&state, 1, |_| {
        called.set(true);
        QueryState {
            value: None,
            loading: false,
            error: None,
            revision: 0,
        }
    });
    assert!(!called.get());
    assert!(pages.frontier.is_none());
}

#[test]
fn query_count_debounce_cancels_and_generations_never_reuse_old_search_rows() {
    use crate::types::{Generation, SyncStatus};
    use queries::*;
    let platform = crate::testing::FakePlatform::default();
    let mut queries = ThreadQueries::default();
    let env = EnvironmentId("env".into());
    let resource = queries
        .counts(
            env.clone(),
            1,
            Generation(1),
            &WireDateTime("30".into()),
            "key",
        )
        .unwrap()
        .unwrap();
    let observer = resource.observe(0);
    let called = std::cell::Cell::new(false);
    let mut work = Box::pin(read_older_settled_counts(
        &resource,
        &platform,
        WireDateTime("30".into()),
        |input| {
            called.set(true);
            assert_eq!(input.limit, 0);
            async {
                Ok(OrchestrationV2OlderSettledThreadsPage {
                    snapshot_sequence: 1,
                    threads: vec![],
                    counts: vec![],
                })
            }
        },
    ));
    assert!(work.as_mut().now_or_never().is_none());
    assert_eq!(platform.pending_timers(), 1);
    platform.advance(499);
    assert!(work.as_mut().now_or_never().is_none());
    assert!(!called.get());
    platform.advance(1);
    assert!(block_on(work).is_ok());
    assert!(called.get());
    drop(observer);
    let mut pending = Box::pin(read_older_settled_counts(
        &resource,
        &platform,
        WireDateTime("30".into()),
        |_| async {
            Ok(OrchestrationV2OlderSettledThreadsPage {
                snapshot_sequence: 1,
                threads: vec![],
                counts: vec![],
            })
        },
    ));
    assert!(pending.as_mut().now_or_never().is_none());
    resource.cancel();
    assert!(block_on(pending).is_err());
    assert_eq!(platform.pending_timers(), 0);
    let input =
        search::thread_search_input("needle", &search::ThreadSearchScope::default(), &env, None)
            .unwrap();
    assert!(
        queries
            .search(env.clone(), 0, Generation(1), &input)
            .unwrap()
            .is_none()
    );
    let first = queries
        .search(env.clone(), 1, Generation(1), &input)
        .unwrap()
        .unwrap();
    let token = first.begin();
    first.finish(
        token,
        OrchestrationV2SearchThreadsResult {
            matches: vec![],
            snapshot_sequence: Some(4),
            threads: Some(vec![shell("old")]),
        },
        0,
        false,
    );
    let second = queries
        .search(env, 2, Generation(1), &input)
        .unwrap()
        .unwrap();
    assert!(second.snapshot().value.is_none());
    assert!(!first.is_current(token));
    assert!(search_waiting_for_shell(0, SyncStatus::Synchronizing));
    assert!(!search_waiting_for_shell(0, SyncStatus::Cached));
}

#[test]
fn search_subject_uses_user_authored_recency_and_each_local_open() {
    let mut opens = search::ThreadOpens::default();
    let key = key("env", "thread");
    let mut thread = shell("thread");
    thread.created_at = WireDateTime("1".into());
    thread.updated_at = WireDateTime("20".into());
    thread.latest_user_message_at = Some(WireDateTime("20".into()));
    thread.latest_user_authored_message_at = Some(None);
    let parse = |s: &str| s.parse().ok();
    assert!(opens.claim(key.clone(), 5000));
    let subject =
        search::thread_search_subject(&key, &thread, Some("Project"), None, &opens, &parse);
    assert_eq!(subject.activity_at, Some(1.0));
    assert_eq!(subject.opens.unwrap().opened_at.as_f64(), Some(5000.0));
    assert_eq!(subject.project_title.as_deref(), Some("Project"));
    assert!(!opens.claim(key.clone(), 7000));
    thread.latest_user_authored_message_at = Some(Some(WireDateTime("5".into())));
    let subject = search::thread_search_subject(&key, &thread, None, None, &opens, &parse);
    assert_eq!(subject.activity_at, Some(5.0));
    assert_eq!(subject.opens.unwrap().opened_at.as_f64(), Some(7000.0));
    thread.latest_user_authored_message_at = None;
    assert_eq!(
        search::thread_search_subject(&key, &thread, None, None, &opens, &parse).activity_at,
        Some(20.0)
    );
    let thread = Rc::new(thread);
    let mut cache = search::ThreadSearchSubjects::default();
    let subject = cache.subject(&key, &thread, (None, None), &opens, &parse);
    assert!(Rc::ptr_eq(
        &subject,
        &cache.subject(&key, &thread, (None, None), &opens, &parse)
    ));
    opens.claim(key.clone(), 8000);
    let next = cache.subject(&key, &thread, (None, None), &opens, &parse);
    assert!(!Rc::ptr_eq(&subject, &next));
    assert_eq!(
        next.opens.as_ref().unwrap().opened_at.as_f64(),
        Some(8000.0)
    );
}

#[test]
fn compaction_before_send_obeys_provider_identity_policy_history_requests_and_ttl() {
    use context_window::*;
    let provider = |id: &str, driver: &str, home: &str| {
        let mut raw: Value = serde_json::from_str(
            include_str!("../../../../../fixtures/samples/ServerProvider.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        raw["instanceId"] = json!(id);
        raw["driver"] = json!(driver);
        raw["continuation"] = json!({"groupKey":home});
        raw["slashCommands"] = json!([{"name":"compact"}]);
        serde_json::from_value::<t3_contracts::server::ServerProvider>(raw).unwrap()
    };
    let providers = vec![
        provider("claudeAgent", "claudeAgent", "home"),
        provider("claude_work", "claudeAgent", "home"),
        provider("claude_other", "claudeAgent", "other"),
        provider("codex", "codex", "codex"),
    ];
    let mut base = projection();
    base.thread.model_selection.instance_id =
        t3_contracts::provider_instance::ProviderInstanceId("claudeAgent".into());
    base.thread.provider_instance_id =
        IdOrIndex::Id(base.thread.model_selection.instance_id.clone());
    base.runs = vec![run("run", 1, "completed")];
    base.runs[0].model_selection = base.thread.model_selection.clone();
    base.thread.active_provider_thread_id = Some(IdOrIndex::Id(
        t3_contracts::base_schemas::ProviderThreadId("current".into()),
    ));
    base.provider_turns=vec![serde_json::from_value(json!({"id":"turn","providerThreadId":"current","nodeId":"root","runAttemptId":null,"nativeTurnRef":null,"ordinal":1,"status":"completed","startedAt":"0","completedAt":"1","tokenUsage":{"usedTokens":293611,"updatedAt":"0"}})).unwrap()];
    base.visible_turn_items = vec![projected(item(
        "user_message",
        "completed",
        json!({"text":"Fix the build","messageId":"message","attachments":[]}),
    ))];
    for case in 0..20 {
        let mut p = base.clone();
        let mut selected = p.thread.model_selection.clone();
        let mut providers = providers.clone();
        let mut settings = t3_contracts::settings::ServerSettings::default();
        let mut text = "back again";
        let mut thread_idle = true;
        let mut now = "10000000";
        let expected = match case {
            0 => true,
            1 => {
                now = "3600000";
                false
            }
            2 => {
                p.provider_turns[0]
                    .token_usage
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .used_tokens = 150000;
                false
            }
            3 => {
                settings.idle_compaction = Some(t3_contracts::settings::IdleCompactionMode::Off);
                false
            }
            4 => {
                settings.provider_instances.insert(
                    selected.instance_id.clone(),
                    serde_json::from_value(
                        json!({"driver":"claudeAgent","config":{"idleCompaction":"off"}}),
                    )
                    .unwrap(),
                );
                false
            }
            5 => {
                selected.instance_id =
                    t3_contracts::provider_instance::ProviderInstanceId("claude_work".into());
                true
            }
            6 => {
                selected.instance_id =
                    t3_contracts::provider_instance::ProviderInstanceId("claude_other".into());
                false
            }
            7 => {
                providers[0].slash_commands.clear();
                false
            }
            8 => {
                thread_idle = false;
                false
            }
            9 => {
                text = "/compact keep the plan";
                false
            }
            10 => {
                let mut held = run("held", 2, "queued");
                held.queue_held = Some(Some(true));
                p.runs.push(held);
                false
            }
            11 => {
                p.runtime_requests.push(serde_json::from_value(json!({"id":"request","nodeId":"root","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":{"type":"not_resumable","reason":"old"},"createdAt":"0","resolvedAt":null})).unwrap());
                false
            }
            12 => {
                p.runtime_requests.push(serde_json::from_value(json!({"id":"request","nodeId":"root","providerTurnId":null,"nativeRequestRef":null,"kind":"user_input","status":"resolved","responseCapability":{"type":"not_resumable","reason":"old"},"createdAt":"0","resolvedAt":"1","answers":{"This session is 1h 30m old and uses 250,000 tokens. Compact it before continuing?":"Don't ask again"}})).unwrap());
                false
            }
            13 => {
                if let OrchestrationV2TurnItem::UserMessage(i) = &mut p.visible_turn_items[0].item {
                    i.text = "/compact".into();
                }
                false
            }
            14 => {
                p.visible_turn_items = vec![projected(item(
                    "assistant_message",
                    "completed",
                    json!({"text":"fixed","messageId":"message","streaming":false}),
                ))];
                true
            }
            15 => {
                p.provider_turns[0].provider_thread_id = IdOrIndex::Id(
                    t3_contracts::base_schemas::ProviderThreadId("earlier".into()),
                );
                false
            }
            16 => {
                now = "600000";
                p.provider_turns[0]
                    .token_usage
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .prompt_cache_ttl_ms = Some(Some(300000));
                true
            }
            17 => {
                p.thread.active_provider_thread_id = None;
                p.runs.clear();
                p.provider_turns.clear();
                p.thread.forked_from = Some(
                    serde_json::from_value(
                        json!({"type":"run","threadId":"source","runId":"source-run"}),
                    )
                    .unwrap(),
                );
                p.context_transfers.push(serde_json::from_value(json!({"id":"transfer","type":"fork","sourceThreadId":"source","targetThreadId":"thread","sourcePoint":{"threadId":"source","runId":"source-run"},"basePoint":null,"sourceProviderInstanceId":"claudeAgent","targetProviderInstanceId":"claudeAgent","targetRunId":null,"status":"pending","resolution":null,"createdBy":"user","error":null,"updatedAt":"0","createdAt":"0","resolvedAt":null,"consumedAt":null,"sourceTokenUsage":{"usedTokens":293611,"updatedAt":"0"}})).unwrap());
                true
            }
            18 => {
                selected.instance_id =
                    t3_contracts::provider_instance::ProviderInstanceId("codex".into());
                false
            }
            _ => {
                settings.idle_compaction =
                    Some(t3_contracts::settings::IdleCompactionMode::BeforeCacheExpires);
                true
            }
        };
        let result = compact_before_send_tokens(
            CompactBeforeSendInput {
                projection: &p,
                selection: &selected,
                providers: &providers,
                settings: &settings,
                text,
                thread_idle,
                now,
            },
            |s| s.parse().ok(),
        );
        assert_eq!(result, expected.then_some(293611), "case {case}");
    }
}

#[test]
fn context_usage_prefers_newest_live_report_then_thread_then_compaction() {
    use context_window::*;
    let live = |used, at: &str| {
        serde_json::from_value::<OrchestrationV2ProviderTurnTokenUsage>(
            json!({"usedTokens":used,"maxTokens":100,"updatedAt":at}),
        )
        .unwrap()
    };
    let snapshot = derive_latest_context_window_snapshot(&[], Some(&live(120, "1")), None).unwrap();
    assert_eq!(snapshot.usage.used_tokens, 120);
    assert_eq!(snapshot.remaining_tokens, Some(0));
    assert_eq!(snapshot.used_percentage, Some(100.0));
    assert_eq!(snapshot.remaining_percentage, Some(0.0));
    let rows = vec![projected(item(
        "compaction",
        "completed",
        json!({"driver":null,"beforeTokenCount":500,"afterTokenCount":100}),
    ))];
    let snapshot = derive_latest_context_window_snapshot(&rows, None, None).unwrap();
    assert_eq!(snapshot.usage.used_tokens, 100);
    assert_eq!(snapshot.usage.total_processed_tokens, Some(Some(500)));
    assert!(snapshot.used_percentage.is_none());
    let mut p = projection();
    p.provider_turns=vec![serde_json::from_value(json!({"id":"t1","providerThreadId":"pt","nodeId":"root","runAttemptId":null,"nativeTurnRef":null,"ordinal":1,"status":"completed","startedAt":"0","completedAt":"1","tokenUsage":live(10,"2")})).unwrap()];
    let mut second = p.provider_turns[0].clone();
    second.token_usage = Some(Some(live(20, "1")));
    p.provider_turns.push(second);
    assert_eq!(
        latest_provider_turn_token_usage(&p.provider_turns, None, None, |s| s.parse().ok())
            .unwrap()
            .used_tokens,
        10
    );
    p.provider_turns[1].token_usage = Some(Some(live(30, "2")));
    assert_eq!(
        latest_provider_turn_token_usage(&p.provider_turns, None, None, |s| s.parse().ok())
            .unwrap()
            .used_tokens,
        30
    );
    assert!(!should_offer_resume_compaction(
        "claudeAgent",
        Some(300000),
        Some("invalid"),
        None,
        "10000000",
        |s| s.parse().ok()
    ));
    assert!(!should_offer_resume_compaction(
        "codex",
        Some(300000),
        Some("0"),
        None,
        "10000000",
        |s| s.parse().ok()
    ));
}

#[test]
fn title_regeneration_waits_for_commit_and_archived_outcome_then_cleans_up_on_disconnect() {
    use crate::types::SyncStatus;
    use title_regeneration::*;
    let store = crate::ClientStore::default();
    let key = key("env", "thread");
    let session = store.begin_session(key.environment.clone());
    let request = CommandId("request".into());
    let mut snapshot:OrchestrationV2ShellSnapshot=serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":2,"projects":[],"threads":[thread_json("thread")],"archivedThreads":[]})).unwrap();
    snapshot.threads[0].title_regeneration = Some(Some(
        serde_json::from_value(json!({"requestId":"request","startedAt":"1"})).unwrap(),
    ));
    store.update_shell(&key.environment, session, |state| {
        state.install(snapshot.clone());
        state.status = SyncStatus::Live;
        true
    });
    let mut waiting = Box::pin(wait_for_title_regeneration_outcome(
        &store, &key, &request, 3,
    ));
    assert!(waiting.as_mut().now_or_never().is_none());
    snapshot.snapshot_sequence = 3;
    let mut thread = snapshot.threads.remove(0);
    thread.last_title_regeneration=Some(Some(serde_json::from_value(json!({"requestId":"request","status":"unchanged","message":"Same title","completedAt":"3"})).unwrap()));
    snapshot.archived_threads.push(thread);
    store.update_shell(&key.environment, session, |state| {
        state.install(snapshot.clone());
        true
    });
    let outcome = block_on(waiting).unwrap();
    assert_eq!(outcome.message, Some(Some("Same title".into())));
    let newer = CommandId("newer".into());
    let mut waiting = Box::pin(wait_for_title_regeneration_outcome(&store, &key, &newer, 4));
    assert!(waiting.as_mut().now_or_never().is_none());
    store.update_shell(&key.environment, session, |state| {
        state.disconnect();
        true
    });
    assert!(block_on(waiting).is_none());
    assert!(matches!(
        settled_outcome(Some(&snapshot), &ThreadId("missing".into()), &request, 3),
        TitleOutcome::Gone
    ));
}

#[test]
fn calendar_snooze_presets_use_calendar_days_and_duration_pick_restarts_the_timer() {
    use crate::thread_views::shelves::*;
    let settings:Vec<t3_contracts::settings::SnoozePresetSetting>=serde_json::from_value(json!([{"kind":"after","minutes":60},{"kind":"at","day":"tomorrow","time":"09:00"},{"kind":"at","day":"today","time":"08:00"}])).unwrap();
    let local_wake = |days, hour, minute| {
        if days == 0 {
            Some(-1)
        } else {
            Some(days * 23 * 3_600_000 + i64::from(hour) * 3_600_000 + i64::from(minute) * 60_000)
        }
    };
    let calendar = SnoozeCalendar {
        now_ms: 0,
        weekday: 0,
        local_wake: &local_wake,
        same_day: &|a, b| a / 86_400_000 == b / 86_400_000,
        format_time: &|n| format!("time:{n}"),
        short_weekday: &|_| "Mon".into(),
        to_iso: &|n| n.to_string(),
    };
    let presets = resolve_snooze_presets(&settings, &calendar);
    assert_eq!(presets.len(), 2);
    assert_eq!(presets[1].wake_ms, 32 * 3_600_000);
    assert_eq!(presets[1].label, "Tomorrow");
    assert_eq!(presets[0].label, "In 1 hour");
    let later = SnoozeCalendar {
        now_ms: 1_000,
        ..calendar
    };
    match pick_snooze_preset(&presets, &presets[0].id, &settings, &later) {
        PickDecision::Picked(p) => assert_eq!(p.wake_ms, 3_601_000),
        _ => panic!("duration remains pickable"),
    };
    assert_eq!(snooze_wake_label(Some(3_600_001), Some(0)), "2h");
    assert_eq!(
        resolve_custom_snooze_duration("1.5", 60_000, 10),
        Some(90010)
    );
    assert!(resolve_custom_snooze_duration("0", 60_000, 10).is_none());
    assert!(resolve_custom_snooze_duration("NaN", 60_000, 10).is_none());
    assert!(
        resolve_custom_snooze_date(
            "2026-03-08",
            "02:30",
            0,
            |_, _| Some(1),
            |_| ("2026-03-08".into(), "03:30".into())
        )
        .is_none()
    );
}

#[test]
fn resolved_question_history_overlays_answers_without_mutating_or_replacing_unchanged_rows() {
    use crate::thread_views::requests::*;
    let question = projected(item(
        "user_input_request",
        "completed",
        json!({"requestId":"question","questions":[{"id":"__proto__","header":"Pick","question":"Choose?","options":[]}]}),
    ));
    let rows: Rc<[_]> = vec![
        Rc::new(question),
        Rc::new(projected(item(
            "command_execution",
            "completed",
            json!({"input":"ls"}),
        ))),
    ]
    .into();
    let request:OrchestrationV2RuntimeRequest=serde_json::from_value(json!({"id":"question","nodeId":"root","providerTurnId":null,"nativeRequestRef":null,"kind":"user_input","status":"resolved","responseCapability":{"type":"not_resumable","reason":"old"},"createdAt":"1","resolvedAt":"2","answers":{"__proto__":"answer"}})).unwrap();
    let requests: Rc<[_]> = vec![Rc::new(request)].into();
    let mut projector = QuestionHistoryProjector::default();
    let result = projector.project(rows.clone(), requests.clone(), None);
    assert!(!Rc::ptr_eq(&rows, &result));
    assert!(Rc::ptr_eq(&rows[1], &result[1]));
    assert!(
        matches!(&rows[0].item,OrchestrationV2TurnItem::UserInputRequest(i) if i.question_answer.is_none())
    );
    let OrchestrationV2TurnItem::UserInputRequest(answered) = &result[0].item else {
        panic!("question")
    };
    let answer = answered.question_answer.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(answer.answers["__proto__"], json!("answer"));
    assert_eq!(
        answer
            .question_text_by_id
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()["__proto__"],
        "Choose?"
    );
    assert!(Rc::ptr_eq(
        &result,
        &projector.project(rows.clone(), requests.clone(), None)
    ));
    let mut updated = rows.to_vec();
    updated[1] = Rc::new(projected(item(
        "command_execution",
        "completed",
        json!({"input":"pwd"}),
    )));
    let next = projector.project(updated.into(), requests, Some(1));
    assert!(Rc::ptr_eq(&next[0], &result[0]));
    assert!(!Rc::ptr_eq(&next[1], &result[1]));
    let hub = RequestResolutionHub::default();
    let mut stream = hub.subscribe();
    hub.publish(ScopedResolvedRequests {
        thread: key("env", "thread"),
        requests: ResolvedRequests {
            pending: vec![],
            unanswered: vec![],
        },
    });
    assert_eq!(
        block_on(futures::StreamExt::next(&mut stream))
            .unwrap()
            .thread,
        key("env", "thread")
    );
    drop(stream);
}

#[test]
fn approval_responders_decline_all_live_requests_and_only_send_displayed_feedback() {
    use crate::thread_views::requests::*;
    let requests:Vec<OrchestrationV2RuntimeRequest>=["a","b","stale"].iter().map(|id| serde_json::from_value(json!({"id":id,"nodeId":"root","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":if *id=="stale" {json!({"type":"not_resumable","reason":"old"})} else {json!({"type":"live","providerSessionId":"s","supportsFeedback":true})},"createdAt":"1","resolvedAt":null})).unwrap()).collect();
    let approvals = derive_pending(&requests, &[]).approvals;
    let mut seen = vec![];
    assert!(block_on(decline_pending_approvals(
        &approvals,
        |id, _, _| {
            seen.push(id);
            async { true }
        }
    )));
    assert_eq!(seen.len(), 2);
    seen.clear();
    assert!(!block_on(decline_pending_approvals(
        &approvals,
        |id, _, _| {
            seen.push(id);
            async { false }
        }
    )));
    assert_eq!(seen.len(), 1);
    seen.clear();
    assert!(block_on(send_approval_feedback(
        &approvals[1],
        "  explain  ",
        |id, _, feedback| {
            assert_eq!(feedback.as_deref(), Some("explain"));
            seen.push(id);
            async { true }
        }
    )));
    assert_eq!(seen, vec![requests[1].id.clone()]);
    assert!(!block_on(send_approval_feedback(
        &approvals[1],
        " \t",
        |_, _, _| async { true }
    )));
    assert!(!block_on(send_approval_feedback(
        &approvals[2],
        "explain",
        |_, _, _| async { true }
    )));
}

#[test]
fn detail_filters_only_actual_limit_notices_and_restores_history_after_the_next_run() {
    use detail::ThreadDetailViews;
    let mut p = projection();
    let mut failed = run("run", 1, "failed");
    failed.root_node_id = Some(IdOrIndex::Id(NodeId("root".into())));
    failed.completed_at = Some(WireDateTime("10".into()));
    p.runs.push(failed);
    p.turn_items = vec![
        item(
            "error",
            "failed",
            json!({"nodeId":"root","providerTurnId":"turn","failure":{"class":"usage_limit","message":"Limit","code":"usageLimitExceeded","retryable":null}}),
        ),
        item(
            "assistant_message",
            "completed",
            json!({"id":"notice","providerTurnId":"turn","messageId":"m","text":"You've hit your 5-hour limit · resets 7:10am (America/Toronto).","streaming":false}),
        ),
    ];
    p.visible_turn_items = p.turn_items.iter().cloned().map(projected).collect();
    let mut views = ThreadDetailViews::default();
    let first = shared(p.clone(), None);
    views.update(Some(first.clone()), None, &parse);
    assert!(views.visible.is_empty());
    assert_eq!(first.source().turn_items.len(), 2);
    if let OrchestrationV2TurnItem::AssistantMessage(i) = &mut p.turn_items[1] {
        i.text = "The test hit a limit, and I fixed it.".into();
    }
    p.visible_turn_items[1].item = p.turn_items[1].clone();
    let changed = shared(p.clone(), Some(&first));
    views.update(Some(changed.clone()), None, &parse);
    assert_eq!(views.visible.len(), 1);
    p.runs.push(run("next", 2, "completed"));
    views.update(Some(shared(p, Some(&changed))), None, &parse);
    assert_eq!(views.visible.len(), 2);
    for text in [
        "You're out of extra usage",
        "Claude AI usage limit reached",
        "You’ve hit your weekly limit · resets 12pm",
        "You've hit your limit.",
    ] {
        assert!(
            execution_presentation::is_provider_limit_notice(text),
            "{text}"
        );
    }
    for text in [
        "The tests hit your limit",
        "You've hit your limit · resets 13pm",
        "You've hit your limit · resets 7:99am",
        "You've hit your limit · resets 07am",
    ] {
        assert!(
            !execution_presentation::is_provider_limit_notice(text),
            "{text}"
        );
    }
}

#[test]
fn detail_queue_and_checkpoint_views_only_change_when_their_inputs_change() {
    let mut p = projection();
    p.runs = vec![run("queued", 1, "queued")];
    p.messages = vec![message("queued", "message-queued", "user")];
    p.checkpoints = vec![checkpoint(Some("done"), Some(1))];
    let first = shared(p.clone(), None);
    let mut views = detail::ThreadDetailViews::default();
    views.update(Some(first.clone()), None, &parse);
    let queue = views.queue.clone().unwrap();
    let checkpoints = views.checkpoint_summaries.clone();
    p.messages[0].text = "Changed queued message".into();
    let changed = shared(p.clone(), Some(&first));
    views.update(Some(changed.clone()), None, &parse);
    assert!(!Rc::ptr_eq(&queue, views.queue.as_ref().unwrap()));
    assert!(Rc::ptr_eq(&checkpoints, &views.checkpoint_summaries));
    assert_eq!(
        views.queue.as_ref().unwrap().queued_runs[0].text,
        "Changed queued message"
    );
    p.messages.push(message("done", "answer", "assistant"));
    views.update(Some(shared(p, Some(&changed))), None, &parse);
    assert!(!Rc::ptr_eq(&checkpoints, &views.checkpoint_summaries));
    assert_eq!(
        views.checkpoint_summaries[0].assistant_message_id,
        Some(IdOrIndex::Id(MessageId("answer".into())))
    );
}

#[test]
fn child_approvals_only_load_approvals_and_keep_the_destination_environment() {
    use crate::thread_views::requests::*;
    let child = |environment: &str, kind: &str| {
        let mut source = shell("child");
        source.pending_runtime_request = Some(
            serde_json::from_value(json!({"id":"request","kind":kind,"createdAt":"1"})).unwrap(),
        );
        Rc::new(shell::ThreadShellView {
            key: key(environment, "child"),
            source: Rc::new(source),
            pending_child_request: None,
        })
    };
    let approvals = child("remote", "command");
    let question = child("other", "user_input");
    let mut p = projection();
    p.runtime_requests=vec![serde_json::from_value(json!({"id":"a","nodeId":"root","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":{"type":"live","providerSessionId":"s"},"createdAt":"1","resolvedAt":null})).unwrap()];
    let mut reads = vec![];
    let result = child_thread_approvals(&[&approvals, &question], |key| {
        reads.push(key.clone());
        Some(&p)
    });
    assert_eq!(reads, vec![key("remote", "child")]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].thread, key("remote", "child"));
}

#[test]
fn shell_navigation_keeps_forks_separates_environments_and_holds_quiet_streaming_rows() {
    use shell::{EnvironmentShells, ThreadShellViews};
    let environment = EnvironmentId("env".into());
    let remote = EnvironmentId("remote".into());
    let mut root = shell("root");
    root.status = OrchestrationV2ShellThreadStatus::Running;
    root.item_count = 1;
    root.visible_item_count = 1;
    let mut child = shell("child");
    child.lineage.parent_thread_id = Some(root.id.clone());
    child.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    let mut fork = shell("fork");
    fork.lineage.parent_thread_id = Some(root.id.clone());
    fork.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Fork);
    let rows = vec![Rc::new(root.clone()), Rc::new(child), Rc::new(fork)];
    let remote_rows = vec![Rc::new(shell("root"))];
    let mut views = ThreadShellViews::default();
    views.update(
        &[
            EnvironmentShells {
                environment: &environment,
                enabled: true,
                threads: &rows,
            },
            EnvironmentShells {
                environment: &remote,
                enabled: true,
                threads: &remote_rows,
            },
        ],
        BTreeSet::new(),
    );
    assert_eq!(
        views
            .navigation()
            .iter()
            .map(|r| r.key.clone())
            .collect::<Vec<_>>(),
        vec![
            key("env", "root"),
            key("env", "fork"),
            key("remote", "root")
        ]
    );
    let ordered = views.ordered.clone();
    let quiet = views.quiet_rows[&key("env", "root")].clone();
    let untouched = views.rows[&key("env", "fork")].clone();
    root.updated_at = WireDateTime("20".into());
    root.item_count = 3;
    root.visible_item_count = 3;
    root.latest_visible_message = Some(
        serde_json::from_value(
            json!({"id":"m","role":"assistant","text":"streamed","updatedAt":"20"}),
        )
        .unwrap(),
    );
    let mut next = rows.clone();
    next[0] = Rc::new(root.clone());
    views.update(
        &[
            EnvironmentShells {
                environment: &environment,
                enabled: true,
                threads: &next,
            },
            EnvironmentShells {
                environment: &remote,
                enabled: true,
                threads: &remote_rows,
            },
        ],
        BTreeSet::new(),
    );
    assert_eq!(views.ordered, ordered);
    assert!(Rc::ptr_eq(&quiet, &views.quiet_rows[&key("env", "root")]));
    assert!(Rc::ptr_eq(&untouched, &views.rows[&key("env", "fork")]));
    root.status = OrchestrationV2ShellThreadStatus::Completed;
    next[0] = Rc::new(root);
    views.update(
        &[EnvironmentShells {
            environment: &environment,
            enabled: true,
            threads: &next,
        }],
        BTreeSet::new(),
    );
    assert!(!Rc::ptr_eq(&quiet, &views.quiet_rows[&key("env", "root")]));
    assert!(!views.rows.contains_key(&key("remote", "root")));
}

#[test]
fn child_requests_roll_approval_and_user_only_input_through_nested_threads() {
    use shell::{EnvironmentShells, PendingChildRequest, ThreadShellViews};
    let environment = EnvironmentId("env".into());
    let root = Rc::new(shell("root"));
    let mut child = shell("child");
    child.creation_source = OrchestrationV2CreationSource::Mcp;
    child.status = OrchestrationV2ShellThreadStatus::Running;
    child.lineage.parent_thread_id = Some(root.id.clone());
    child.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    child.pending_runtime_request = Some(
        serde_json::from_value(json!({"id":"q","kind":"user_input","createdAt":"1"})).unwrap(),
    );
    let mut views = ThreadShellViews::default();
    let apply = |views: &mut ThreadShellViews, rows: &[Rc<OrchestrationV2ThreadShell>]| {
        views.update(
            &[EnvironmentShells {
                environment: &environment,
                enabled: true,
                threads: rows,
            }],
            BTreeSet::new(),
        )
    };
    apply(&mut views, &[root.clone(), Rc::new(child.clone())]);
    assert!(
        !views
            .pending_child_requests
            .contains_key(&key("env", "root"))
    );
    assert_eq!(views.child_inputs(&key("env", "root")).len(), 1);
    child.pending_runtime_request.as_mut().unwrap().user_only = Some(Some(true));
    apply(&mut views, &[root.clone(), Rc::new(child.clone())]);
    assert_eq!(
        views.pending_child_requests[&key("env", "root")],
        PendingChildRequest::UserInput
    );
    child.creation_source = OrchestrationV2CreationSource::Provider;
    apply(&mut views, &[root.clone(), Rc::new(child.clone())]);
    assert!(
        !views
            .pending_child_requests
            .contains_key(&key("env", "root"))
    );
    assert!(views.child_inputs(&key("env", "root")).is_empty());
    child.creation_source = OrchestrationV2CreationSource::Mcp;
    child.pending_runtime_request = None;
    let mut grandchild = child.clone();
    grandchild.id = ThreadId("grandchild".into());
    grandchild.lineage.parent_thread_id = Some(child.id.clone());
    grandchild.pending_runtime_request =
        Some(serde_json::from_value(json!({"id":"a","kind":"command","createdAt":"1"})).unwrap());
    apply(
        &mut views,
        &[root.clone(), Rc::new(child), Rc::new(grandchild)],
    );
    assert_eq!(
        views.pending_child_requests[&key("env", "root")],
        PendingChildRequest::Approval
    );
    let mut side = shell("side");
    side.lineage.parent_thread_id = Some(root.id.clone());
    side.lineage.side_chat = Some(Some(true));
    side.pending_runtime_request = Some(
        serde_json::from_value(json!({"id":"q","kind":"user_input","createdAt":"1"})).unwrap(),
    );
    apply(&mut views, &[root, Rc::new(side)]);
    assert_eq!(
        views.pending_child_requests[&key("env", "root")],
        PendingChildRequest::UserInput
    );
    assert_eq!(views.side_chats(&key("env", "root")).len(), 1);
}

#[test]
fn delegated_approvals_cross_three_environments_and_unlisted_tasks_stay_reachable() {
    use shell::{EnvironmentShells, PendingChildRequest, ThreadShellViews};
    let a = EnvironmentId("a".into());
    let b = EnvironmentId("b".into());
    let c = EnvironmentId("c".into());
    let roots = vec![Rc::new(shell("root"))];
    let task = |id: &str, parent_env: &str, parent_id: &str| {
        let mut task = shell(id);
        task.status = OrchestrationV2ShellThreadStatus::Running;
        task.creation_source = OrchestrationV2CreationSource::Mcp;
        task.delegated_from = Some(Some(
            serde_json::from_value(
                json!({"environmentId":parent_env,"threadId":parent_id,"title":"Parent"}),
            )
            .unwrap(),
        ));
        task
    };
    let middle = vec![Rc::new(task("middle", "a", "root"))];
    let mut leaf = task("leaf", "b", "middle");
    leaf.pending_runtime_request =
        Some(serde_json::from_value(json!({"id":"a","kind":"command","createdAt":"1"})).unwrap());
    let leaves = vec![Rc::new(leaf)];
    let mut views = ThreadShellViews::default();
    views.update(
        &[
            EnvironmentShells {
                environment: &a,
                enabled: true,
                threads: &roots,
            },
            EnvironmentShells {
                environment: &b,
                enabled: true,
                threads: &middle,
            },
            EnvironmentShells {
                environment: &c,
                enabled: true,
                threads: &leaves,
            },
        ],
        BTreeSet::new(),
    );
    assert_eq!(
        views.pending_child_requests[&key("a", "root")],
        PendingChildRequest::Approval
    );
    assert!(views.delegated_under_parent.contains(&key("b", "middle")));
    assert!(views.delegated_under_parent.contains(&key("c", "leaf")));
    assert_eq!(views.child_inputs(&key("a", "root")).len(), 1);
    assert_eq!(views.navigation().len(), 1);
    views.update(
        &[
            EnvironmentShells {
                environment: &a,
                enabled: true,
                threads: &roots,
            },
            EnvironmentShells {
                environment: &b,
                enabled: false,
                threads: &middle,
            },
            EnvironmentShells {
                environment: &c,
                enabled: true,
                threads: &leaves,
            },
        ],
        BTreeSet::new(),
    );
    assert!(!views.pending_child_requests.contains_key(&key("a", "root")));
    assert!(!views.delegated_under_parent.contains(&key("c", "leaf")));
    assert!(views.navigation().iter().any(|r| r.key == key("c", "leaf")));
}

#[test]
fn inbox_returns_hold_placement_expire_once_cancel_on_resume_and_skip_attention_or_stale_work() {
    use crate::thread_views::shelves::*;
    let thread = key("env", "thread");
    let working = ShelfInput {
        runtime: Some(RuntimeState::Running),
        ..Default::default()
    };
    let stopped = ShelfInput::default();
    fn row<'a>(
        key: &'a ThreadKey,
        shelf: &'a ShelfInput,
        placement: InboxPlacement,
    ) -> InboxTrackedThread<'a> {
        InboxTrackedThread {
            key,
            shelf,
            placement,
            has_runtime_error: false,
        }
    }
    let mut tracker = InboxReturnTracker::default();
    assert!(
        tracker
            .observe(
                Some(&[row(&thread, &working, InboxPlacement::default())]),
                10000
            )
            .returned_at(&thread)
            .is_none()
    );
    let held = tracker.observe(
        Some(&[row(&thread, &stopped, InboxPlacement::default())]),
        10000,
    );
    assert!(held.is_working(&thread, &stopped));
    assert_eq!(tracker.deadline(), Some(12000));
    assert!(!tracker.tick(11999));
    tracker.observe(
        Some(&[row(&thread, &working, InboxPlacement::default())]),
        11500,
    );
    assert!(tracker.deadline().is_none());
    assert!(!tracker.tick(12000));
    assert!(tracker.returned_at(&thread).is_none());
    tracker.observe(
        Some(&[row(&thread, &stopped, InboxPlacement::default())]),
        13000,
    );
    assert_eq!(tracker.deadline(), Some(15000));
    assert!(tracker.tick(15000));
    assert_eq!(tracker.revision, 1);
    assert_eq!(tracker.returned_at(&thread), Some(15000));
    assert!(!tracker.tick(15001));
    assert!(held.is_working(&thread, &stopped));
    assert!(held.returned_at(&thread).is_none());
    for scenario in 0..5 {
        let mut tracker = InboxReturnTracker::default();
        let mut stopped = ShelfInput::default();
        let mut placement = InboxPlacement::default();
        match scenario {
            0 => stopped.questions = true,
            1 => {
                stopped.latest_run = Some(Run {
                    state: Some(RunState::Completed),
                    completed_at: Some(1),
                    ..Default::default()
                })
            }
            2 => placement.pinned_at = Some("1".into()),
            3 => {
                stopped.snoozed_until = Some(300000);
            }
            _ => placement.active_order_key = Some("changed".into()),
        }
        tracker.observe(
            Some(&[row(&thread, &working, InboxPlacement::default())]),
            200000,
        );
        tracker.observe(Some(&[row(&thread, &stopped, placement)]), 200000);
        assert!(!tracker.is_working(&thread, &stopped));
        assert!(tracker.deadline().is_none());
        assert_eq!(tracker.returned_at(&thread), Some(200000));
    }
    tracker.observe(
        Some(&[row(&thread, &working, InboxPlacement::default())]),
        20000,
    );
    tracker.observe(
        Some(&[row(&thread, &stopped, InboxPlacement::default())]),
        20000,
    );
    tracker.observe(None, 21000);
    assert!(tracker.deadline().is_none());
    assert!(!tracker.tick(22000));
    assert!(tracker.returned_at(&thread).is_none());
}
