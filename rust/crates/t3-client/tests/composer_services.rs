#![allow(clippy::unwrap_used)]
#[path = "support/fake_server.rs"]
mod fake_server;

use fake_server::Harness;
use futures::{FutureExt, StreamExt};
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    platform::{Storage, StorageArea, StorageWrite},
    thread_views::{
        composer::draft_service::ComposerDraftService,
        drafts::{DraftFormat, SavedDraftDocument, storage_key},
    },
    types::{EnvironmentId, ThreadId, ThreadKey},
};

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread".into()),
    }
}

#[test]
fn canonical_draft_chips_and_stash_import_preserve_payloads_and_rewrite_colliding_ids() {
    use t3_client::thread_views::composer::context_records::{
        draft_chip_records, import_records, rewrite_references,
    };
    let h = Harness::new("env");
    let original = json!({"terminalContexts":[{"id":"term","threadId":"thread","terminalId":"terminal","terminalLabel":"Terminal","lineStart":1,"lineEnd":2,"text":"first\r\nsecond\n"}]});
    let records = draft_chip_records(&original).unwrap();
    let terminal = records.get("terminal_term").unwrap();
    assert_eq!(
        serde_json::to_value(terminal).unwrap()["text"],
        "first\nsecond"
    );
    let mut document = json!({"draftsByThreadKey":{"env:thread":original},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}});
    assert!(
        import_records(
            &h.platform,
            &mut document,
            "env:thread",
            Some(&key()),
            std::slice::from_ref(terminal),
            &[]
        )
        .unwrap()
        .is_empty()
    );
    let mut changed = serde_json::to_value(terminal).unwrap();
    changed["text"] = json!("different excerpt");
    changed["lineStart"] = json!(3);
    changed["lineEnd"] = json!(4);
    let changed = serde_json::from_value(changed).unwrap();
    let rewritten = import_records(
        &h.platform,
        &mut document,
        "env:thread",
        Some(&key()),
        &[changed],
        &[],
    )
    .unwrap();
    assert_ne!(rewritten["terminal_term"], "terminal_term");
    let prompt = rewrite_references(
        "🦀 [Terminal](t3-context://v1/terminal/terminal_term) ",
        &rewritten,
    );
    assert!(prompt.starts_with("🦀 [Terminal]"));
    assert!(prompt.contains(&rewritten["terminal_term"]));
    assert_eq!(
        document["draftsByThreadKey"]["env:thread"]["terminalContexts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn citation_comments_guard_source_preserve_drafts_before_rewrite_and_undo_inserted_spaces() {
    use t3_client::thread_views::composer::{
        citation_comments::{CitationInsertedSpaces, ComposerCitationCommentContext},
        citations,
    };
    let mut h = Harness::new("env");
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    let citation=serde_json::from_value(json!({"version":1,"environmentId":"env","threadId":"thread","messageId":"message","text":"quote","start":0,"end":5,"prefix":"","suffix":""})).unwrap();
    let source = citations::serialize(&citation);
    let prompt = format!("🦀 {source} ");
    drafts
        .set_prompt(&key(), prompt.clone(), prompt.encode_utf16().count())
        .unwrap();
    let context = ComposerCitationCommentContext::new(drafts.clone(), key());
    context
        .open(
            "node".into(),
            3,
            Some("anchor".into()),
            Some(CitationInsertedSpaces {
                before: true,
                after: true,
            }),
        )
        .unwrap();
    context.draft_changed("node", "comment".into()).unwrap();
    let saved = context.before_controlled_rewrite(&prompt).unwrap().unwrap();
    assert!(saved.prompt.contains("comment=comment"));
    assert_eq!(drafts.view(&key()).prompt.as_ref(), prompt);
    context
        .open(
            "node".into(),
            3,
            None,
            Some(CitationInsertedSpaces {
                before: true,
                after: true,
            }),
        )
        .unwrap();
    let cancelled = context.cancel("node").unwrap().unwrap();
    assert_eq!(cancelled.prompt, "🦀");
    assert_eq!(cancelled.cursor, 2);
    drafts
        .set_prompt(&key(), source.clone(), source.encode_utf16().count())
        .unwrap();
    context.open("node".into(), 0, None, None).unwrap();
    context
        .draft_changed(
            "node",
            "😀".repeat(t3_contracts::assistant_citations::ASSISTANT_CITATION_MAX_COMMENT_LENGTH),
        )
        .unwrap();
    assert!(!context.dismiss("node", "outside-press").unwrap());
    drafts.set_prompt(&key(), "newer".into(), 5).unwrap();
    assert!(matches!(
        context.save("node", "saved"),
        Err(t3_client::ClientError::Cancelled)
    ));
    assert_eq!(&*drafts.view(&key()).prompt, "newer");
}

#[test]
fn new_thread_attachment_edits_fence_clearing_synchronously_and_survive_retarget() {
    use t3_client::thread_views::composer::{
        attachment_intake::IncomingFile, target::ComposerTarget,
    };
    let mut h = Harness::new("env");
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt_key("draft-id", "seed".into(), 4).unwrap();
    let captured = drafts.view_key("draft-id");
    let project = t3_contracts::environment::ScopedProjectRef {
        environment_id: EnvironmentId("env".into()),
        project_id: t3_client::types::ProjectId("p".into()),
    };
    drafts
        .update(|document| {
            t3_client::thread_views::composer::web_sessions::set_logical_project(
                document,
                "logical",
                &project,
                "draft-id",
                &json!({"cwd":"/workspace"}),
                "2026-01-01T00:00:00.000Z",
                &Default::default(),
            );
            Ok(())
        })
        .unwrap();
    assert!(drafts.view_key("draft-id").revision > captured.revision);
    assert!(
        !drafts
            .clear_key_revision("draft-id", captured.revision)
            .unwrap()
    );
    let captured = drafts.view_key("draft-id");
    let attachments = runtime
        .composer_attachments(&ComposerTarget::Draft {
            draft_id: "draft-id".into(),
            environment_id: Some(EnvironmentId("env".into())),
        })
        .unwrap();
    let intake = attachments.intake_bytes(vec![IncomingFile {
        name: "note.txt".into(),
        mime: "text/plain".into(),
        bytes: b"note".to_vec(),
        source: None,
        insertion: None,
    }]);
    assert!(matches!(intake.now_or_never(), Some(Ok(()))));
    assert!(drafts.view_key("draft-id").revision > captured.revision);
    assert!(
        !drafts
            .clear_key_revision("draft-id", captured.revision)
            .unwrap()
    );
    assert!(drafts.view_key("draft-id").prompt.starts_with("seed"));
    let id = attachments.items()[0].id.clone();
    drafts
        .update(|document| {
            let file = &mut document["draftsByThreadKey"]["draft-id"]["files"][0];
            file["uploadedAttachmentId"] = json!("uploaded");
            file["uploadEnvironmentId"] = json!("env");
            Ok(())
        })
        .unwrap();
    let replacement = runtime
        .composer_attachments(&ComposerTarget::Draft {
            draft_id: "draft-id".into(),
            environment_id: Some(EnvironmentId("other".into())),
        })
        .unwrap();
    assert_eq!(replacement.items().len(), 1);
    assert!(
        drafts.document().borrow()["draftsByThreadKey"]["draft-id"]["files"][0]
            .get("uploadedAttachmentId")
            .is_none()
    );
    attachments.remove(&id);
    assert_eq!(replacement.items().len(), 1);
    let revision = drafts.view_key("draft-id").revision;
    replacement.remove(&id);
    assert!(drafts.view_key("draft-id").revision > revision);
    assert!(!drafts.clear_key_revision("draft-id", revision).unwrap());
    let flush = drafts.clone();
    h.run(async move {
        flush.flush().await.unwrap();
    });
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    assert!(fresh.view_key("draft-id").prompt.starts_with("seed"));
}

#[test]
fn questions_share_literal_drafts_keep_displaced_text_and_refuse_stale_or_denied_actions() {
    use t3_contracts::base_schemas::RuntimeRequestId;
    let mut h = Harness::new("env");
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(t3_client::connection::PairingInput {
        pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)),
        ..Default::default()
    }))
    .unwrap();
    let key = ThreadKey {
        thread: ThreadId("t1".into()),
        ..key()
    };
    runtime.open_thread(&key);
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&key, "normal draft".into(), 12).unwrap();
    let generation = runtime.store().snapshot().environments[&key.environment].session;
    runtime.store().update_thread(&key,generation,|thread|{
        let projection=Rc::make_mut(thread.projection.as_mut().unwrap());
        projection.runtime_requests.push(serde_json::from_value(json!({"id":"request","nodeId":"node","providerTurnId":null,"nativeRequestRef":null,"kind":"user_input","status":"pending","responseCapability":{"type":"live","providerSessionId":"session"},"createdAt":"2026-01-01T00:00:00.000Z","resolvedAt":null})).unwrap());
        projection.turn_items.push(serde_json::from_value(json!({"type":"user_input_request","id":"input-item","threadId":"t1","runId":null,"nodeId":"node","providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"running","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-01-01T00:00:00.000Z","requestId":"request","questions":[{"id":"scope","header":"Scope","question":"Which scope?","options":[{"label":"First","description":"First scope","value":"a"},{"label":"Second","description":"Second scope","value":"b"}],"initialAnswer":"seed"}]})).unwrap());true
    });
    let session = runtime.pending_user_input(&key);
    let view = session.view().unwrap();
    assert!(!view.disabled);
    assert_eq!(&*drafts.view(&key).prompt, "seed");
    assert_eq!(
        &*drafts.view_key(&drafts.thread_draft_key(&key)).prompt,
        "normal draft"
    );
    drafts.set_prompt(&key, "custom answer".into(), 13).unwrap();
    let request = RuntimeRequestId("request".into());
    session.toggle_option(&request, "scope", "a").unwrap();
    assert_eq!(session.view().unwrap().selected_option_values, vec!["a"]);
    assert_eq!(
        &*drafts.view_key(&drafts.thread_draft_key(&key)).prompt,
        "normal draft\n\ncustom answer"
    );
    assert!(matches!(
        session.advance(&request, "stale-question"),
        Err(t3_client::ClientError::Cancelled)
    ));
    session.advance(&request, "scope").unwrap();
    assert!(session.view().unwrap().is_responding);
    h.settle();
    let failed = session.view().unwrap();
    assert!(!failed.is_responding);
    assert!(failed.error.is_some());
    assert_eq!(failed.selected_option_values, vec!["a"]);
    h.server().state.borrow_mut().permissions = Some(vec!["orchestration:read".into()]);
    h.run(runtime.reconnect(&key.environment)).unwrap();
    h.settle();
    assert!(session.view().is_none_or(|view| view.disabled));
    assert!(session.toggle_option(&request, "scope", "b").is_err());
}

#[test]
fn stash_reloads_latest_entries_before_mutating_and_restores_the_shared_draft() {
    use t3_client::thread_views::composer::target::ComposerTarget;
    let mut h = Harness::new("env");
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    let stash = runtime.prompt_stash(ComposerTarget::Thread(key()));
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts
        .set_prompt(&key(), " saved prompt ".into(), 14)
        .unwrap();
    assert!(stash.snapshot().can_stash);
    h.run(stash.stash_current()).unwrap();
    assert_eq!(&*drafts.view(&key()).prompt, "");
    let entries = stash.snapshot().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].prompt, "saved prompt");
    drafts.set_prompt(&key(), "newer".into(), 5).unwrap();
    h.run(stash.restore(entries[0].id.clone()).unwrap())
        .unwrap();
    assert_eq!(&*drafts.view(&key()).prompt, "newer\n\nsaved prompt");
    assert!(stash.snapshot().entries.is_empty());
    assert!(stash.restore(entries[0].id.clone()).is_err());
    drafts
        .set_prompt_key("new-task-1", "draft id".into(), 8)
        .unwrap();
    let raw = drafts.view_key("new-task-1");
    assert_eq!(&*raw.prompt, "draft id");
    assert!(Rc::ptr_eq(&raw, &drafts.view_key("new-task-1")));
    assert!(
        drafts
            .clear_key_revision("new-task-1", raw.revision)
            .unwrap()
    );
}

#[test]
fn attachment_session_updates_the_same_persisted_document_and_send_blocker() {
    use t3_client::thread_views::composer::{
        attachment_intake::IncomingFile, target::ComposerTarget,
    };
    let mut h = Harness::new("env");
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.settle();
    let attachments = runtime
        .composer_attachments(&ComposerTarget::Thread(key()))
        .unwrap();
    h.settle();
    let intake = attachments.clone();
    h.run(async move {
        intake
            .intake_bytes(vec![IncomingFile {
                name: "note.txt".into(),
                mime: "text/plain".into(),
                bytes: b"text".to_vec(),
                source: None,
                insertion: None,
            }])
            .await
    })
    .unwrap();
    h.settle();
    assert_eq!(attachments.items().len(), 1);
    assert!(
        attachments
            .block_reason()
            .unwrap()
            .contains("Waiting for the server")
    );
    let drafts = runtime.composer_drafts();
    let revision = drafts.view(&key()).revision;
    assert!(revision > 0);
    let drafts_to_flush = drafts.clone();
    h.run(async move {
        drafts_to_flush.flush().await.unwrap();
    });
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    assert_eq!(
        fresh.document().borrow()["draftsByThreadKey"]["env:thread"]["files"][0]["name"],
        "note.txt"
    );
    attachments.remove(&attachments.items()[0].id);
    h.settle();
    assert!(drafts.view(&key()).revision > revision);
}

#[test]
fn composer_banners_bypass_grace_for_approval_preserve_code_and_reject_stale_actions() {
    use t3_client::{
        connection::{ConnectionFailure, FailureKind, Phase},
        thread_views::composer::target::ComposerTarget,
    };
    let mut h = Harness::new("env");
    let runtime = t3_client::composition::ClientRuntime::new(
        h.platform.clone(),
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(t3_client::connection::PairingInput {
        pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)),
        ..Default::default()
    }))
    .unwrap();
    h.settle();
    let id = EnvironmentId("env".into());
    let banners = runtime.composer_banners(ComposerTarget::Thread(key()));
    assert!(banners.snapshot().is_empty());
    let mut state = runtime.client(&id).unwrap().state();
    state.phase = Phase::Backoff;
    runtime.store().set_connection(id.clone(), state.clone());
    assert!(banners.snapshot().is_empty());
    state.last_failure = Some(ConnectionFailure::new(
        FailureKind::ApprovalPending,
        "Approve this device with code ABC-123",
    ));
    runtime.store().set_connection(id.clone(), state.clone());
    let view = banners.snapshot();
    assert_eq!(view.len(), 1);
    assert!(view[0].title.ends_with("is waiting to approve this device"));
    assert_eq!(
        view[0].description.as_deref(),
        Some("Approve this device with code ABC-123")
    );
    assert_eq!(view[0].actions[0].label, "Retry now");
    assert!(Rc::ptr_eq(&view[0], &banners.snapshot()[0]));
    state.phase = Phase::Connected;
    state.last_failure = None;
    runtime.store().set_connection(id, state);
    assert!(matches!(
        h.run(banners.invoke(&view[0].id, "reconnect")),
        Err(t3_client::ClientError::Cancelled)
    ));
}

#[test]
fn web_draft_hydration_cursor_and_captured_revision_preserve_new_edits() {
    let mut h = Harness::new("env");
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    drafts.set_prompt(&key(), "first 🦀".into(), 8).unwrap();
    let captured = drafts.view(&key());
    assert_eq!(captured.cursor, 8);
    assert!(Rc::ptr_eq(&captured, &drafts.view(&key())));
    drafts
        .set_prompt(&key(), "typed meanwhile".into(), 99)
        .unwrap();
    assert!(!drafts.clear_revision(&key(), captured.revision).unwrap());
    assert_eq!(&*drafts.view(&key()).prompt, "typed meanwhile");
    let save = drafts.clone();
    h.run(async move {
        save.flush().await.unwrap();
    });
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    assert_eq!(&*fresh.view(&key()).prompt, "typed meanwhile");
    assert_eq!(fresh.view(&key()).cursor, 15);
    assert!(
        fresh
            .clear_revision(&key(), fresh.view(&key()).revision)
            .unwrap()
    );
}

#[test]
fn unreadable_web_document_is_retained_and_other_writers_are_merged() {
    let mut h = Harness::new("env");
    let platform = h.platform.clone();
    h.run(async move {
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: storage_key(DraftFormat::Web),
                    value: Some(b"broken".to_vec()),
                }],
            )
            .await
            .unwrap();
    });
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    assert!(h.run(async move { load.load().await }).is_err());
    assert!(!drafts.view(&key()).writable);
    assert!(
        drafts
            .set_prompt_key("new-thread-draft", "overwrite".into(), 9)
            .is_err()
    );
    assert!(
        drafts
            .update(|document| {
                document["draftsByThreadKey"]["new-thread-draft"] = json!({"prompt":"overwrite"});
                Ok(())
            })
            .is_err()
    );
    assert!(drafts.view_key("new-thread-draft").prompt.is_empty());
    let platform = h.platform.clone();
    assert_eq!(
        h.run(async move {
            platform
                .get(StorageArea::Ordinary, storage_key(DraftFormat::Web))
                .await
                .unwrap()
        }),
        Some(b"broken".to_vec())
    );
    let platform = h.platform.clone();
    h.run(async move { platform.write(StorageArea::Ordinary, vec![StorageWrite { key: storage_key(DraftFormat::Web), value: Some(serde_json::to_vec(&json!({"version":9,"state":{"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{},"future":{"keep":true}}})).unwrap()) }]).await.unwrap(); });
    let load = drafts.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    let other = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = other.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    let mut changes = drafts.changes();
    assert!(changes.next().now_or_never().unwrap().is_some());
    drafts.set_prompt(&key(), "local".into(), 5).unwrap();
    assert!(changes.next().now_or_never().unwrap().is_some());
    let second = ThreadKey {
        thread: ThreadId("other".into()),
        ..key()
    };
    other.set_prompt(&second, "remote".into(), 6).unwrap();
    h.run(async move {
        other.flush().await.unwrap();
    });
    let save = drafts.clone();
    h.run(async move {
        save.flush().await.unwrap();
    });
    let platform = h.platform.clone();
    let bytes = h.run(async move {
        platform
            .get(StorageArea::Ordinary, storage_key(DraftFormat::Web))
            .await
            .unwrap()
            .unwrap()
    });
    let saved = SavedDraftDocument::decode_web(&bytes).unwrap();
    assert_eq!(
        saved.state()["draftsByThreadKey"]["env:thread"]["prompt"],
        "local"
    );
    assert_eq!(
        saved.state()["draftsByThreadKey"]["env:other"]["prompt"],
        "remote"
    );
    assert_eq!(saved.state()["future"]["keep"], true);
}

#[test]
fn accepted_send_clears_its_snapshot_and_recovers_beside_a_newer_edit() {
    use t3_client::{ClientStore, composition::ClientRuntime, connection::PairingInput};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    let mut h = Harness::new("env");
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)),
        ..Default::default()
    }))
    .unwrap();
    h.settle();
    let target = ThreadKey {
        thread: ThreadId("t1".into()),
        ..key()
    };
    runtime.open_thread(&target);
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts
        .set_prompt(&target, "refused prompt".into(), 14)
        .unwrap();
    h.server().state.borrow_mut().hold_dispatches = true;
    h.run(runtime.send_message(&target, "refused prompt".into()))
        .unwrap();
    assert!(drafts.view(&target).prompt.is_empty());
    drafts
        .set_prompt(&target, "typed meanwhile".into(), 15)
        .unwrap();
    h.server().hang_up();
    h.settle();
    assert_eq!(&*drafts.view(&target).prompt, "typed meanwhile");
    let message = runtime.pending_messages(&target)[0].message.clone();
    let recovery = drafts.clone();
    h.run(async move {
        recovery.recover_message(&message).await.unwrap();
    });
    assert_eq!(
        &*drafts.view(&target).prompt,
        "typed meanwhile\n\nrefused prompt"
    );
    let message = runtime.pending_messages(&target)[0].message.clone();
    let recovery = drafts.clone();
    h.run(async move {
        recovery.recover_message(&message).await.unwrap();
    });
    assert_eq!(
        &*drafts.view(&target).prompt,
        "typed meanwhile\n\nrefused prompt"
    );
    drop(runtime);
    h.settle();
}

#[test]
fn draft_subscription_delivers_an_edit_then_ends_with_its_authority() {
    let mut h = Harness::new("env");
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    let mut changes = drafts.changes();
    assert_eq!(changes.next().now_or_never(), Some(Some(())));
    drafts.set_prompt(&key(), "edit".into(), 4).unwrap();
    assert_eq!(changes.next().now_or_never(), Some(Some(())));
    drop(drafts);
    assert_eq!(changes.next().now_or_never(), Some(None));
}

#[test]
fn web_draft_round_trip_retains_image_bytes_contexts_model_intent_and_unknown_fields() {
    let mut h = Harness::new("env");
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = drafts.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    drafts.update(|document| {
        document["draftsByThreadKey"]["env:thread"] = json!({"prompt":"explain", "images":[{"id":"image","type":"image","name":"pixel.png","mimeType":"image/png","sizeBytes":3,"dataUrl":"data:image/png;base64,AQID","previewUrl":"data:image/png;base64,AQID"}], "files":[], "terminalContexts":[],"reviewComments":[],"previewAnnotations":[], "threadContexts":[],"environmentContexts":[], "modelSelectionByProvider":{"codex":{"instanceId":"codex","model":"saved","options":[{"id":"effort","value":"high"}]}},"activeProvider":"codex","future":{"keep":true}});
        Ok(())
    }).unwrap();
    let save = drafts.clone();
    h.run(async move {
        save.flush().await.unwrap();
    });
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone();
    h.run(async move {
        load.load().await.unwrap();
    });
    let document = fresh.document();
    let document = document.borrow();
    let draft = &document["draftsByThreadKey"]["env:thread"];
    assert_eq!(draft["images"][0]["dataUrl"], "data:image/png;base64,AQID");
    assert_eq!(
        draft["modelSelectionByProvider"]["codex"]["options"][0]["value"],
        "high"
    );
    assert_eq!(draft["future"]["keep"], true);
}

#[test]
fn voice_without_pcm_capture_is_unavailable_without_requesting_microphone_permission() {
    use futures::StreamExt;
    use t3_client::thread_views::composer::voice_service::{
        ComposerVoiceSession, VOICE_UNAVAILABLE_REASON,
    };
    let mut h = Harness::new("env");
    let voice = ComposerVoiceSession::default();
    let first = voice.snapshot();
    assert!(!first.available);
    assert_eq!(first.unavailable_reason, Some(VOICE_UNAVAILABLE_REASON));
    assert!(Rc::ptr_eq(&first, &voice.snapshot()));
    h.run(async move {
        let mut changes = voice.changes();
        assert!(changes.next().await.is_some());
        assert!(changes.next().await.is_none());
        assert!(matches!(
            voice.start().await,
            Err(t3_client::ClientError::Unsupported(
                VOICE_UNAVAILABLE_REASON
            ))
        ));
    });
}

#[test]
fn primary_actions_and_interrupt_recheck_destination_permission_and_preserve_rejection_detail() {
    use t3_client::{
        ClientError, ClientStore, composition::ClientRuntime, connection::PairingInput,
    };
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    let mut h = Harness::new("env");
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    let target = ThreadKey {
        thread: ThreadId("t1".into()),
        ..key()
    };
    let actions = runtime.composer_primary_actions(&target);
    assert!(actions.snapshot().is_environment_unavailable);
    assert!(!actions.snapshot().can_operate_thread);
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{}/pair#token=pairing-token", fake_server::ORIGIN)),
        ..Default::default()
    }))
    .unwrap();
    runtime.open_thread(&target);
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&target, "hello".into(), 5).unwrap();
    assert!(actions.snapshot().has_sendable_content);
    assert!(actions.snapshot().can_operate_thread);
    let endpoint = runtime
        .client(&target.environment)
        .unwrap()
        .session()
        .unwrap();
    let platform = h.platform.clone();
    let store = runtime.store().clone();
    let key = target.clone();
    let session = endpoint.clone();
    let error = h
        .run(async move {
            t3_client::thread_views::composer::interrupt::interrupt_thread(
                &platform, &store, session, &key, true,
            )
            .await
        })
        .unwrap_err();
    assert!(matches!(error, ClientError::Rejected { .. }));
    assert!(
        error
            .to_string()
            .contains("No orchestration provider adapter is registered for codex.")
    );
    h.server().state.borrow_mut().permissions = Some(vec!["orchestration:read".into()]);
    h.run(runtime.reconnect(&target.environment)).unwrap();
    h.settle();
    assert!(!actions.snapshot().can_operate_thread);
    assert_eq!(
        actions.snapshot().send_disabled_reason.as_deref(),
        Some("This connection cannot change threads.")
    );
    assert!(matches!(
        h.run(runtime.interrupt_thread(&target)),
        Err(ClientError::PermissionDenied { .. })
    ));
}
