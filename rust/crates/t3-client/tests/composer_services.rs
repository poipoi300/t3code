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

#[derive(Clone)]
struct RefusingWrites {
    inner: fake_server::TestPlatform,
    namespace: Rc<std::cell::RefCell<Option<String>>>,
}
impl Storage for RefusingWrites {
    fn get(
        &self,
        area: StorageArea,
        key: t3_client::platform::StorageKey,
    ) -> t3_client::platform::HostFuture<'_, Option<Vec<u8>>> {
        self.inner.get(area, key)
    }
    fn write(
        &self,
        area: StorageArea,
        writes: Vec<StorageWrite>,
    ) -> t3_client::platform::HostFuture<'_, ()> {
        self.inner.write(area, writes)
    }
    fn list(
        &self,
        area: StorageArea,
        namespace: String,
    ) -> t3_client::platform::HostFuture<'_, Vec<t3_client::platform::StorageKey>> {
        self.inner.list(area, namespace)
    }
    fn transaction(
        &self,
        transaction: t3_client::platform::StorageTransaction,
    ) -> t3_client::platform::HostFuture<'_, bool> {
        if transaction
            .ordinary
            .iter()
            .any(|write| self.namespace.borrow().as_ref() == Some(&write.key.namespace))
        {
            return async {
                Err(t3_client::platform::PlatformError::Storage(
                    "disk full".into(),
                ))
            }
            .boxed_local();
        }
        self.inner.transaction(transaction)
    }
}
impl t3_client::platform::Platform for RefusingWrites {
    fn now_epoch_ms(&self) -> u64 {
        self.inner.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.inner.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, t3_client::platform::PlatformError> {
        self.inner.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> futures::future::LocalBoxFuture<'_, ()> {
        self.inner.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> futures::future::LocalBoxFuture<'_, ()> {
        self.inner.yield_to_host()
    }
    fn events(
        &self,
    ) -> futures::stream::LocalBoxStream<'static, t3_client::platform::PlatformEvent> {
        self.inner.events()
    }
    fn http(
        &self,
        request: t3_client::platform::HttpRequest,
    ) -> t3_client::platform::HostFuture<'_, t3_client::platform::HttpResponse> {
        self.inner.http(request)
    }
    fn websocket(
        &self,
        request: t3_client::platform::SocketRequest,
    ) -> t3_client::platform::HostFuture<'_, Box<dyn t3_client::platform::WebSocket>> {
        self.inner.websocket(request)
    }
    fn host(
        &self,
        action: t3_client::platform::HostAction,
    ) -> t3_client::platform::HostFuture<'_, t3_client::platform::HostReply> {
        self.inner.host(action)
    }
}

#[test]
fn stash_write_failure_never_clears_and_restore_retry_does_not_duplicate_text() {
    use t3_client::thread_views::composer::{stash_service, target::ComposerTarget};
    let mut h = Harness::new("env");
    let namespace = Rc::new(std::cell::RefCell::new(None));
    let platform = RefusingWrites {
        inner: h.platform.clone(),
        namespace: namespace.clone(),
    };
    let runtime = t3_client::composition::ClientRuntime::new(
        platform,
        t3_client::ClientStore::default(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    let stash = runtime.prompt_stash(ComposerTarget::Thread(key()));
    h.settle();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&key(), "saved".into(), 5).unwrap();
    *namespace.borrow_mut() = Some("prompt-stash".into());
    assert!(h.run(stash.stash_current()).is_err());
    assert_eq!(&*drafts.view(&key()).prompt, "saved");
    assert!(stash.snapshot().entries.is_empty());
    *namespace.borrow_mut() = None;
    h.run(stash.stash_current()).unwrap();
    let entry = stash.snapshot().entries[0].clone();
    drafts.set_prompt(&key(), "newer".into(), 5).unwrap();
    *namespace.borrow_mut() = Some(storage_key(DraftFormat::Web).namespace);
    assert!(h.run(stash.restore(entry.id.clone()).unwrap()).is_err());
    let storage = h.platform.clone();
    let stored = h
        .run(async move {
            storage
                .get(StorageArea::Ordinary, stash_service::storage_key())
                .await
        })
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(stored).unwrap().contains(&entry.id));
    assert_eq!(&*drafts.view(&key()).prompt, "newer\n\nsaved");
    *namespace.borrow_mut() = None;
    h.run(stash.restore(entry.id.clone()).unwrap()).unwrap();
    assert_eq!(&*drafts.view(&key()).prompt, "newer\n\nsaved");
    assert!(stash.snapshot().entries.is_empty());
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
fn outgoing_context_preserves_source_order_binds_attachments_and_uses_contract_limits() {
    use t3_client::thread_views::{
        composer::context_records::outgoing_message_context, outbox::DraftAttachment,
    };
    let citation = json!({"version":1,"environmentId":"env","threadId":"thread","messageId":"message","text":"quote","start":0,"end":5,"prefix":"","suffix":""});
    let mut captured = json!({
        "terminalContexts":[
            {"id":"z","terminalId":"terminal","terminalLabel":"Terminal","lineStart":1,"lineEnd":2,"text":"first\r\nsecond\n"},
            {"id":"a","terminalId":"terminal","terminalLabel":"Terminal","lineStart":3,"lineEnd":3,"text":"last"},
            {"id":"empty","terminalId":"terminal","terminalLabel":"Terminal","lineStart":4,"lineEnd":4,"text":"\n"}
        ],
        "reviewComments":[{"id":"review","sectionId":"working","sectionTitle":"Working tree","filePath":"file.rs","startIndex":0,"endIndex":0,"rangeLabel":"Line 1","text":"Check this","diff":"-old\n+new"}],
        "threadContexts":[{"version":1,"kind":"thread","contextId":"thread_other","label":"Linked","title":"Linked","threadId":"other","environmentId":"env"}],
        "environmentContexts":[{"version":1,"kind":"environment","contextId":"environment_peer","label":"Peer","environmentId":"peer"}],
        "replyTo":{"version":1,"kind":"response","contextId":"response_message","label":"Response","citation":citation,"itemId":"item"},
        "previewAnnotations":[{"id":"shot","pageUrl":"https://example.com","pageTitle":null,"comment":"Fix this","elements":[],"regions":[],"strokes":[],"styleChanges":[],"screenshot":null,"createdAt":"2026-01-01T00:00:00.000Z"}]
    });
    let attachments = vec![
        DraftAttachment::decode(json!({"type":"image","id":"shot","name":"shot.png","mimeType":"image/png","sizeBytes":3,"previewUri":"data:image/png;base64,AQID","dataUrl":"data:image/png;base64,AQID"})).unwrap(),
        DraftAttachment::decode(json!({"type":"file","id":"file","name":"answer.txt","mimeType":"text/plain","sizeBytes":4,"fileUri":"opaque","uploadedAttachmentId":"uploaded-file","uploadEnvironmentId":"env"})).unwrap(),
    ];
    let context = serde_json::to_value(
        outgoing_message_context(&captured, &attachments)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let records = context["records"].as_array().unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record["contextId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "terminal_z",
            "terminal_a",
            "review-comment_review",
            "thread_other",
            "environment_peer",
            "response_message",
            "preview-annotation_shot",
            "image_shot",
            "file_file"
        ]
    );
    assert_eq!(records[0]["text"], "first\nsecond");
    assert_eq!(records[6]["screenshotContextId"], "image_shot");
    assert_eq!(records[7]["attachmentId"], "shot");
    assert_eq!(records[8]["attachmentId"], "file");
    let unbound =
        serde_json::to_value(outgoing_message_context(&captured, &[]).unwrap().unwrap()).unwrap();
    assert!(unbound["records"][6].get("screenshotContextId").is_none());
    assert!(outgoing_message_context(&json!({}), &[]).unwrap().is_none());
    let duplicate = captured["threadContexts"][0].clone();
    captured["threadContexts"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(outgoing_message_context(&captured, &[]).is_err());
    let records = (0..=t3_contracts::composer_context::COMPOSER_CONTEXT_MAX_RECORDS).map(|index| json!({"version":1,"kind":"environment","contextId":format!("environment_e{index}"),"label":"Environment","environmentId":format!("e{index}")})).collect::<Vec<_>>();
    assert!(outgoing_message_context(&json!({"environmentContexts":records}), &[]).is_err());
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
    h.server()
        .state
        .borrow_mut()
        .extra_methods
        .push(("assets.createUrl", t3_rpc::CallKind::Unary));
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
    drafts.set_prompt(&key, "custom answer".into(), 13).unwrap();
    assert!(session.view().unwrap().selected_option_values.is_empty());
    assert_eq!(
        &*drafts.view_key(&drafts.thread_draft_key(&key)).prompt,
        "normal draft"
    );
    session.toggle_option(&request, "scope", "a").unwrap();
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
    runtime.store().update_thread(&key,generation,|thread|{
        let projection=Rc::make_mut(thread.projection.as_mut().unwrap());
        if let t3_contracts::orchestration_v2::OrchestrationV2TurnItem::UserInputRequest(item)=projection.turn_items.last_mut().unwrap(){item.questions.push(serde_json::from_value(json!({"id":"next","header":"Next","question":"Next?","options":[],"initialAnswer":"/mo"})).unwrap());}true
    });
    drafts.set_prompt(&key, "/mo".into(), 3).unwrap();
    let suggestions = runtime.composer_suggestions(&key, None);
    let query = t3_client::thread_views::composer::suggestions::SuggestionRequest {
        trigger: t3_client::thread_views::composer::policy::detect_trigger("/mo", 3).unwrap(),
        generation: 1,
    };
    h.run(suggestions.query(Some(query.clone()))).unwrap();
    session.advance(&request, "scope").unwrap();
    assert_eq!(session.view().unwrap().question_index, 1);
    assert_eq!(&*drafts.view(&key).prompt, "/mo");
    assert!(matches!(
        suggestions.select(&query, "slash:model", "/mo", 3),
        Err(t3_client::ClientError::Cancelled)
    ));
    session.previous(&request, "next").unwrap();
    session.view();
    drafts
        .set_prompt(
            &key,
            "unsent [Linked](t3-context://v1/thread/thread_other)".into(),
            50,
        )
        .unwrap();
    let question_key = failed.question_draft_key.clone();
    let retired = runtime
        .composer_attachments(
            &t3_client::thread_views::composer::target::ComposerTarget::Draft {
                draft_id: question_key.clone(),
                environment_id: Some(key.environment.clone()),
            },
        )
        .unwrap();
    drafts.update(|document|{document["draftsByThreadKey"][&question_key]["threadContexts"]=json!([{"version":1,"kind":"thread","contextId":"thread_other","label":"Linked","title":"Linked","threadId":"other","environmentId":"env"}]);Ok(())}).unwrap();
    runtime.store().update_thread(&key, generation, |thread| {
        let projection = Rc::make_mut(thread.projection.as_mut().unwrap());
        projection.runtime_requests[0].status =
            t3_contracts::orchestration_v2::RuntimeRequestStatus::Expired;
        true
    });
    h.settle();
    assert!(
        drafts
            .view_key(&drafts.thread_draft_key(&key))
            .prompt
            .contains("unsent [Linked]")
    );
    assert!(matches!(
        retired
            .intake_bytes(vec![
                t3_client::thread_views::composer::attachment_intake::IncomingFile {
                    name: "late.txt".into(),
                    mime: "text/plain".into(),
                    bytes: b"late".to_vec(),
                    source: None,
                    insertion: None
                }
            ])
            .now_or_never(),
        Some(Err(t3_client::ClientError::Cancelled))
    ));
    assert!(session.view().is_none());
    let document = drafts.document();
    let document = document.borrow();
    assert_eq!(
        document["draftsByThreadKey"][drafts.thread_draft_key(&key)]["threadContexts"][0]["threadId"],
        "other"
    );
    assert!(drafts.view(&key).prompt.contains("unsent [Linked]"));
    drop(document);
    let normal = drafts
        .view_key(&drafts.thread_draft_key(&key))
        .prompt
        .clone();
    let answered_request = RuntimeRequestId("accepted-request".into());
    let mut config = runtime.store().snapshot().environments[&key.environment]
        .config
        .as_deref()
        .unwrap()
        .clone();
    config.environment.capabilities.question_attachments = Some(true);
    config.environment.capabilities.file_attachments = Some(
        t3_contracts::environment::ExecutionEnvironmentCapabilitiesFileAttachments {
            max_upload_bytes: 10_000,
        },
    );
    assert!(
        runtime
            .store()
            .set_config(&key.environment, generation, config)
    );
    runtime.store().update_thread(&key, generation, |thread| {
        let projection = Rc::make_mut(thread.projection.as_mut().unwrap());
        projection.runtime_requests[0].id =
            t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(answered_request.clone());
        projection.runtime_requests[0].status =
            t3_contracts::orchestration_v2::RuntimeRequestStatus::Pending;
        if let t3_contracts::orchestration_v2::OrchestrationV2TurnItem::UserInputRequest(item) =
            projection.turn_items.last_mut().unwrap()
        {
            item.request_id =
                t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(answered_request.clone());
            item.questions.truncate(1);
        }
        true
    });
    let accepted_key = session.view().unwrap().question_draft_key.clone();
    drafts.set_prompt(&key, "answered".into(), 8).unwrap();
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("assets.createUrl".into())
        .or_default()
        .push_back(Ok(json!({"relativeUrl":"/asset/answer","expiresAt":10000})));
    drafts
        .update(|document| {
            document["draftsByThreadKey"][&accepted_key]["files"] = json!([{
                "type":"file", "id":"local-file", "name":"answer.txt",
                "mimeType":"text/plain", "sizeBytes":4, "file":null,
                "uploadedAttachmentId":"uploaded-answer", "uploadEnvironmentId":"env"
            }]);
            Ok(())
        })
        .unwrap();
    h.settle();
    assert!(!session.view().unwrap().response_disabled);
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("orchestration.dispatchCommand".into())
        .or_default()
        .push_back(Ok(json!({"sequence":2})));
    session.advance(&answered_request, "scope").unwrap();
    assert!(session.advance(&answered_request, "scope").is_err());
    h.settle();
    assert!(session.view().unwrap().error.is_none());
    let call = h
        .server()
        .state
        .borrow()
        .rpc_calls
        .iter()
        .rev()
        .find(|(method, _)| method == "orchestration.dispatchCommand")
        .unwrap()
        .1
        .clone();
    assert_eq!(call["answers"]["scope"], "answered");
    assert_eq!(
        call["attachmentsByQuestionId"]["scope"][0]["id"],
        "uploaded-answer"
    );
    runtime.store().update_thread(&key, generation, |thread| {
        Rc::make_mut(thread.projection.as_mut().unwrap()).runtime_requests[0].status =
            t3_contracts::orchestration_v2::RuntimeRequestStatus::Resolved;
        true
    });
    h.settle();
    assert!(session.view().is_none());
    assert_eq!(
        drafts.view_key(&drafts.thread_draft_key(&key)).prompt,
        normal
    );
    assert!(drafts.view_key(&accepted_key).prompt.is_empty());
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
    runtime.store().set_connection(id.clone(), state);
    assert!(matches!(
        h.run(banners.invoke(&view[0].id, "reconnect")),
        Err(t3_client::ClientError::Cancelled)
    ));
    use t3_client::services::updates::{ServerUpdateStage, ServerUpdateState};
    let updates = runtime.composer_server_updates().state(id);
    let failure = ServerUpdateState::Failed {
        stage: ServerUpdateStage::Downloading,
        from_version: runtime.store().snapshot().environments[&key().environment]
            .config
            .as_ref()
            .unwrap()
            .environment
            .server_version
            .0
            .clone(),
        target_version: "0.0.2".into(),
        message: "download refused".into(),
    };
    assert!(updates.finish(updates.begin(), failure.clone(), 0, true));
    let failed = banners.snapshot();
    assert_eq!(failed.len(), 1);
    h.run(banners.invoke(&failed[0].id, "dismiss")).unwrap();
    assert!(banners.snapshot().is_empty());
    let remounted = runtime.composer_banners(ComposerTarget::Thread(key()));
    assert!(remounted.snapshot().is_empty());
    assert!(updates.finish(updates.begin(), ServerUpdateState::Idle, 0, true));
    assert!(updates.finish(updates.begin(), failure, 0, true));
    assert_eq!(banners.snapshot().len(), 1);
    assert_eq!(remounted.snapshot().len(), 1);
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
fn prepared_checkout_links_once_after_acceptance_and_link_refusal_keeps_the_launch() {
    use t3_client::thread_views::composer::{
        delivery_policy::ThreadSettings,
        send::{SendOptions, SendState},
    };
    for link_succeeds in [true, false] {
        let mut h = Harness::new("env");
        h.server()
            .state
            .borrow_mut()
            .extra_methods
            .push(("orchestration.launchThread", t3_rpc::CallKind::Unary));
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
        let target = ThreadKey {
            thread: ThreadId("prepared".into()),
            ..key()
        };
        let generation = runtime.store().snapshot().environments[&target.environment].session;
        let send = runtime.messages(&target.environment).unwrap();
        let mut config: t3_contracts::server::ServerConfig =
            serde_json::from_value(fake_server::config("env")).unwrap();
        config.managed_policy = None;
        config.settings = Default::default();
        config.providers.0 =
            vec![serde_json::from_value(json!({
            "instanceId":"codex", "driver":"codex", "enabled":true, "installed":true,
            "version":null, "status":"ready", "auth":{"status":"authenticated"},
            "checkedAt":"2026-01-01T00:00:00.000Z",
            "models":[{"slug":"gpt-5.4", "name":"GPT", "isCustom":false, "capabilities":null}],
            "slashCommands":[], "skills":[]
        })).unwrap()];
        send.connect(
            generation,
            Rc::new(
                runtime
                    .client(&target.environment)
                    .unwrap()
                    .session()
                    .unwrap(),
            ),
            Rc::new(config),
        )
        .unwrap();
        let url = "https://github.com/Acme/Repo/pull/123";
        let mut projection = fake_server::thread_snapshot("t1")["projection"].clone();
        projection["thread"]["id"] = json!(target.thread);
        let (accept_launch, acceptance) = futures::channel::oneshot::channel::<()>();
        let acceptance = Rc::new(std::cell::RefCell::new(Some(acceptance)));
        let launches = Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed_launches = launches.clone();
        let reply = json!({"threadId":target.thread, "projection":projection, "resumed":false, "sequence":20});
        h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
            if request.tag != "orchestration.launchThread" {
                return None;
            }
            observed_launches.borrow_mut().push(request.payload.clone());
            let accepted = acceptance.borrow_mut().take().unwrap();
            let reply = reply.clone();
            Some(t3_rpc::HandlerOutput::Unary(
                async move {
                    accepted.await.unwrap();
                    t3_rpc::Exit::success(reply)
                }
                .boxed_local(),
            ))
        }));
        {
            let mut server = h.server().state.borrow_mut();
            server.rpc_script.entry("orchestration.dispatchCommand".into()).or_default().push_back(
                if link_succeeds { Ok(json!({"sequence":21})) } else {
                    Err(json!({"_tag":"OrchestrationV2DispatchCommandError", "commandId":"link", "commandType":"thread.pull-request.link", "message":"Link refused", "detail":"forge is offline"}))
                },
            );
        }
        let creation: t3_client::thread_views::outbox::QueuedCreation =
            serde_json::from_value(json!({
                "projectId":"project", "projectCwd":"/workspace", "workspaceMode":"local",
                "branch":"pr-123", "worktreePath":"/prepared", "checkoutPullRequestUrl":url
            }))
            .unwrap();
        let saved = serde_json::to_value(&creation).unwrap();
        assert_eq!(saved["checkoutPullRequestUrl"], url);
        assert!(saved.get("pullRequestReference").is_none());
        assert_eq!(
            serde_json::from_value::<t3_client::thread_views::outbox::QueuedCreation>(saved)
                .unwrap(),
            creation
        );
        let drafts = runtime.composer_drafts();
        drafts
            .set_prompt(&target, "first message".into(), 13)
            .unwrap();
        let queued = send.clone();
        let destination = target.clone();
        let id = h
            .run(async move {
                queued
                    .submit(
                        &destination,
                        "first message".into(),
                        SendOptions {
                            creation: Some(creation),
                            settings: Some(ThreadSettings {
                                model_selection: serde_json::from_value(
                                    json!({"instanceId":"codex", "model":"gpt-5.4"}),
                                )
                                .unwrap(),
                                runtime_mode:
                                    t3_contracts::provider_policy::RuntimeMode::FullAccess,
                                interaction_mode:
                                    t3_contracts::provider_policy::ProviderInteractionMode::Default,
                            }),
                            ..Default::default()
                        },
                    )
                    .await
            })
            .unwrap();
        h.settle();
        assert_eq!(send.state(&id), Some(SendState::Dispatching));
        assert!(
            !h.server()
                .state
                .borrow()
                .rpc_calls
                .iter()
                .any(|(method, input)| {
                    method == "orchestration.dispatchCommand"
                        && input["type"] == "thread.pull-request.link"
                })
        );
        accept_launch.send(()).unwrap();
        h.settle();
        assert_eq!(
            send.state(&id),
            Some(SendState::Accepted { sequence: Some(20) })
        );
        assert!(drafts.view(&target).prompt.is_empty());
        assert!(send.delivery_problem().is_none());
        let calls = h.server().state.borrow().rpc_calls.clone();
        let links = calls
            .iter()
            .enumerate()
            .filter(|(_, (method, input))| {
                method == "orchestration.dispatchCommand"
                    && input["type"] == "thread.pull-request.link"
            })
            .collect::<Vec<_>>();
        assert_eq!(launches.borrow().len(), 1);
        assert_eq!(links.len(), 1);
        assert_eq!(
            launches.borrow()[0]["workspaceStrategy"]["type"],
            "existing_worktree"
        );
        assert_eq!(links[0].1.1["host"], "github.com");
        assert_eq!(links[0].1.1["repository"], "acme/repo");
        assert_eq!(links[0].1.1["number"], 123);
        assert_eq!(links[0].1.1["url"], url);
        assert_eq!(links[0].1.1["source"], "manual");
        assert!(
            !calls
                .iter()
                .any(|(method, _)| method == "git.preparePullRequestThread")
        );
        assert_eq!(
            send.checkout_pull_request_link_error(&id).is_none(),
            link_succeeds
        );
        if !link_succeeds {
            assert!(
                send.checkout_pull_request_link_error(&id)
                    .unwrap()
                    .to_string()
                    .contains("forge is offline")
            );
        }
        let stepped = send.clone();
        assert!(
            h.run(async move { stepped.step().await })
                .unwrap()
                .is_none()
        );
        h.settle();
        assert_eq!(
            h.server()
                .state
                .borrow()
                .rpc_calls
                .iter()
                .filter(|(method, input)| method == "orchestration.dispatchCommand"
                    && input["type"] == "thread.pull-request.link")
                .count(),
            1
        );
    }
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
fn prepared_checkout_url_survives_outbox_encoding_and_durable_refusal_recovery() {
    use t3_client::thread_views::outbox::{QueuedMessage, decode_message, encode_message};
    let mut h = Harness::new("env");
    let url = "https://github.com/acme/repo/pull/123";
    let message: QueuedMessage = serde_json::from_value(json!({
        "environmentId":"env", "threadId":"prepared", "messageId":"first-message",
        "commandId":"launch", "text":"first prompt", "attachments":[],
        "createdAt":"2026-01-01T00:00:00.000Z",
        "creation":{"projectId":"project", "workspaceMode":"local", "branch":"pr-123",
            "worktreePath":"/prepared", "checkoutPullRequestUrl":url}
    }))
    .unwrap();
    let encoded = encode_message(&message).unwrap();
    let decoded = decode_message(encoded.clone()).unwrap();
    assert_eq!(
        decoded
            .creation
            .as_ref()
            .unwrap()
            .checkout_pull_request_url
            .as_deref(),
        Some(url)
    );
    let mut legacy = encoded;
    legacy["creation"]
        .as_object_mut()
        .unwrap()
        .remove("checkoutPullRequestUrl");
    assert!(
        decode_message(legacy)
            .unwrap()
            .creation
            .unwrap()
            .checkout_pull_request_url
            .is_none()
    );
    let drafts = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let recovery = drafts.clone();
    let key = h
        .run(async move { recovery.recover_message(&decoded).await })
        .unwrap();
    let fresh = Rc::new(ComposerDraftService::new(Rc::new(h.platform.clone())));
    let load = fresh.clone();
    h.run(async move { load.load().await }).unwrap();
    let creation = fresh.document().borrow()["draftsByThreadKey"][&key]["queuedCreation"].clone();
    assert_eq!(creation["checkoutPullRequestUrl"], url);
    assert_eq!(creation["worktreePath"], "/prepared");
    assert_eq!(&*fresh.view_key(&key).prompt, "first prompt");
}

#[test]
fn persisted_file_verification_marks_expiry_and_retains_ids_after_transient_failure() {
    use t3_client::thread_views::composer::{
        attachment_intake::UploadState, target::ComposerTarget,
    };
    for missing in [true, false] {
        let mut h = Harness::new("env");
        h.server()
            .state
            .borrow_mut()
            .extra_methods
            .push(("assets.createUrl", t3_rpc::CallKind::Unary));
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
        let snapshot = runtime.store().snapshot();
        let generation = snapshot.environments[&id].session;
        let mut config = snapshot.environments[&id]
            .config
            .as_deref()
            .unwrap()
            .clone();
        config.environment.capabilities.file_attachments = Some(
            t3_contracts::environment::ExecutionEnvironmentCapabilitiesFileAttachments {
                max_upload_bytes: 10_000,
            },
        );
        runtime.store().set_config(&id, generation, config);
        let resource = t3_contracts::assets::AssetResource::Attachment {
            attachment_id: t3_contracts::base_schemas::TrimmedNonEmptyString("uploaded".into()),
            file_name: None,
            mime_type: None,
            disposition: None,
            render_intent: None,
        };
        h.server().state.borrow_mut().rpc_script.entry("assets.createUrl".into()).or_default()
            .push_back(Err(if missing {
                json!({"_tag":"AssetAttachmentNotFoundError", "resource":resource})
            } else {
                json!({"_tag":"AssetWorkspaceResolutionError", "resource":resource, "cause":{"message":"offline"}})
            }));
        let target = ComposerTarget::Draft {
            draft_id: "persisted-file".into(),
            environment_id: Some(id),
        };
        let session = runtime.composer_attachments(&target).unwrap();
        let drafts = runtime.composer_drafts();
        drafts.update(|document| {
            document["draftsByThreadKey"]["persisted-file"] = json!({"prompt":"", "files":[{
                "type":"file", "id":"file", "name":"answer.txt", "mimeType":"text/plain", "sizeBytes":4,
                "file":null, "uploadedAttachmentId":"uploaded", "uploadEnvironmentId":"env"
            }]});
            Ok(())
        }).unwrap();
        h.settle();
        let item = session.items().remove(0);
        let file =
            drafts.document().borrow()["draftsByThreadKey"]["persisted-file"]["files"][0].clone();
        if missing {
            assert!(item.needs_reattach);
            assert_eq!(item.upload, UploadState::Idle);
            assert!(file.get("uploadedAttachmentId").is_none());
        } else {
            assert!(!item.needs_reattach);
            assert!(
                matches!(item.upload, UploadState::Failed(ref reason) if reason == "Uploaded file could not be verified. Retry when the server reconnects.")
            );
            assert_eq!(file["uploadedAttachmentId"], "uploaded");
        }
        let calls = &h.server().state.borrow().rpc_calls;
        assert_eq!(
            calls
                .iter()
                .filter(|(method, _)| method == "assets.createUrl")
                .count(),
            1
        );
        assert!(!calls.iter().any(|(method, _)| matches!(
            method.as_str(),
            "attachments.createUploadUrl" | "attachments.delete"
        )));
    }
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
