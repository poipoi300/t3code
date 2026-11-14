use futures::{FutureExt, SinkExt, StreamExt, channel::mpsc, executor::block_on};
use serde_json::{Value, json};
use std::{collections::BTreeSet, rc::Rc};
use t3_client::{
    ClientStore,
    permissions::SessionGrants,
    platform::{Platform, Storage, StorageArea},
    rpc::{self, SessionEndpoint},
    testing::FakePlatform,
    thread_views::composer::send::{SendOptions, SendService, SendState},
    thread_views::outbox::{DeliveryResult, DraftAttachment},
    types::{EnvironmentId, Generation, ThreadId, ThreadKey},
};
use t3_contracts::{orchestration_v2::*, server::ServerConfig};
use t3_rpc::{ClientMessage, Exit, Frame, Request, ServerMessage, codec};
const NOW: &str = "2026-11-13T00:00:00.000Z";
fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread".into()),
    }
}
fn grants() -> SessionGrants {
    SessionGrants {
        valid: true,
        scopes: BTreeSet::from([
            "orchestration:operate".into(),
            "orchestration:read".into(),
            "workspace:write".into(),
            "workspace:read".into(),
        ]),
    }
}
fn config() -> Rc<ServerConfig> {
    let mut config: ServerConfig = serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .expect("sample"),
    )
    .expect("config");
    config.environment.environment_id = key().environment;
    config.managed_policy = None;
    config.settings = Default::default();
    config.providers.0=vec![serde_json::from_value(json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":NOW,"models":[{"slug":"gpt-5.4","name":"GPT","isCustom":false,"capabilities":null}],"slashCommands":[],"skills":[]})).expect("provider")];
    Rc::new(config)
}
fn thread() -> Value {
    json!({"id":"thread","projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}
fn store() -> (ClientStore, Generation) {
    let store = ClientStore::default();
    let generation = store.begin_session(key().environment);
    let mut shell_thread = thread();
    shell_thread.as_object_mut().expect("thread").extend(json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}).as_object().expect("fields").clone());
    store.apply_shell(&key().environment,generation,vec![serde_json::from_value(json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":1,"threads":[shell_thread],"archivedThreads":[],"projects":[{"id":"project","title":"Project","workspaceRoot":"/workspace","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]}})).expect("snapshot"),serde_json::from_value(json!({"kind":"synchronized"})).expect("synchronized")]);
    (store, generation)
}
fn seen(
    store: &ClientStore,
    generation: Generation,
    id: &t3_contracts::base_schemas::MessageId,
    sequence: u64,
) {
    let mut message: Value = serde_json::from_str(
        include_str!("../../../fixtures/samples/OrchestrationV2ConversationMessage.jsonl")
            .lines()
            .next()
            .expect("sample"),
    )
    .expect("message");
    message["id"] = json!(id);
    message["threadId"] = json!("thread");
    let snapshot = json!({"kind":"snapshot","snapshotSequence":sequence,"historyCursor":null,"projection":{"thread":thread(),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[message],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW}});
    store
        .apply_thread(
            key(),
            generation,
            vec![
                serde_json::from_value(snapshot).expect("detail"),
                serde_json::from_value(json!({"kind":"synchronized"})).expect("synchronized"),
            ],
        )
        .expect("apply");
}
fn request(frame: Frame) -> Request {
    let messages: Vec<ClientMessage> = codec::decode(&frame).expect("wire");
    messages
        .into_iter()
        .find_map(|message| match message {
            ClientMessage::Request(request) => Some(request),
            _ => None,
        })
        .expect("request")
}
type Service = SendService<FakePlatform, SessionEndpoint>;

#[test]
fn unreadable_recovery_receipts_keep_the_outbox_and_retry_without_another_dispatch() {
    block_on(async {
        use t3_client::{
            platform::StorageWrite,
            thread_views::{
                composer::mobile_store::MobileDraftStore,
                drafts::{DraftFormat, SavedDraftDocument, storage_key},
            },
        };
        for attached in [false, true] {
            let platform = Rc::new(FakePlatform::default());
            let draft_key = storage_key(DraftFormat::Mobile);
            platform.write(StorageArea::Ordinary,vec![StorageWrite {key:draft_key.clone(),value:Some(serde_json::to_vec(&json!({"schemaVersion":1,"drafts":{},"recoveredMessages":"unreadable receipt metadata"})).expect("json"))}]).await.expect("seed");
            let (store, generation) = store();
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
            let endpoint = Rc::new(endpoint);
            let service = Service::new(
                key().environment,
                platform.clone(),
                store,
                OrchestrationV2CreationSource::Web,
            );
            service
                .connect(generation, endpoint.clone(), config())
                .expect("connect");
            let drafts = if attached {
                let drafts = Rc::new(MobileDraftStore::new(platform.clone()));
                drafts.load().await.expect("load");
                service.attach_drafts(drafts.clone());
                Some(drafts)
            } else {
                None
            };
            let id = service
                .send_message(&key(), "recover once".into())
                .await
                .expect("submit");
            let scenario = async {
                let rejected = async {
                    let command = request(requests.next().await.expect("dispatch"));
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:command.id,exit:Exit::fail(json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":command.payload["commandId"],"commandType":"message.dispatch","message":"Thread archived"}))}).expect("encode"))).await.expect("reply");
                };
                let (result, ()) = futures::join!(service.step(), rejected);
                assert_eq!(result.expect("retain"), Some(DeliveryResult::Retry));
                assert_eq!(service.pending_messages(&key()).len(), 1);
                service.retry_message(&id).expect("retry recovery");
                assert_eq!(
                    service.step().await.expect("retain again"),
                    Some(DeliveryResult::Retry)
                );
                assert!(requests.try_recv().is_err());
                let saved = SavedDraftDocument::decode_mobile(
                    &platform
                        .get(StorageArea::Ordinary, draft_key.clone())
                        .await
                        .expect("read")
                        .expect("saved"),
                )
                .expect("document");
                assert_eq!(
                    saved.raw()["recoveredMessages"],
                    "unreadable receipt metadata"
                );
                assert!(
                    saved.raw()["drafts"]
                        .as_object()
                        .expect("drafts")
                        .is_empty()
                );
                if let Some(drafts) = &drafts {
                    drafts
                        .update(|current| {
                            let mut next = current.clone();
                            next["recoveredMessages"] = json!({});
                            Ok(next)
                        })
                        .expect("repair");
                } else {
                    let mut next = saved.raw().clone();
                    next["recoveredMessages"] = json!({});
                    platform
                        .write(
                            StorageArea::Ordinary,
                            vec![StorageWrite {
                                key: draft_key.clone(),
                                value: Some(serde_json::to_vec(&next).expect("json")),
                            }],
                        )
                        .await
                        .expect("repair");
                }
                service.retry_message(&id).expect("retry after repair");
                assert_eq!(
                    service.step().await.expect("restore"),
                    Some(DeliveryResult::Restored)
                );
                assert!(requests.try_recv().is_err());
                // Restored, and listed as refused until the user dismisses it.
                assert!(matches!(
                    service.pending_messages(&key()).as_slice(),
                    [pending] if matches!(pending.state, SendState::Restored { .. })
                ));
                let saved = SavedDraftDocument::decode_mobile(
                    &platform
                        .get(StorageArea::Ordinary, draft_key.clone())
                        .await
                        .expect("read")
                        .expect("saved"),
                )
                .expect("document");
                assert_eq!(saved.raw()["drafts"]["env:thread"]["text"], "recover once");
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
        }
    });
}

#[test]
fn disposing_during_dispatch_cancels_the_driver_future_and_retains_uncertain_delivery() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (_peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = service
            .send_message(&key(), "in flight".into())
            .await
            .expect("submit");
        let (done, finished) = futures::channel::oneshot::channel();
        let run = async {
            let result = service.run().await;
            done.send(result).expect("driver result");
        };
        let scenario = async {
            let command = request(requests.next().await.expect("dispatch"));
            assert_eq!(command.payload["messageId"], json!(id));
            service.dispose();
            finished.await.expect("driver finished").expect("disposed");
            assert_eq!(platform.pending_timers(), 0);
            let restored = Service::new(
                key().environment,
                platform.clone(),
                store,
                OrchestrationV2CreationSource::Web,
            );
            restored
                .connect(generation, endpoint.clone(), config())
                .expect("connect");
            assert_eq!(
                restored.step().await.expect("restore uncertain"),
                Some(DeliveryResult::Wait)
            );
            assert_eq!(restored.state(&id), Some(SendState::AwaitingConfirmation));
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), run, scenario);
    });
}

#[test]
fn local_outbox_edit_and_discard_are_durable_and_do_not_dispatch() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, _generation) = store();
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        let id = service
            .send_message(&key(), "before".into())
            .await
            .expect("submit");
        assert!(
            service
                .update_pending(&id, "edited".into(), SendOptions::default())
                .await
                .expect("edit")
        );
        assert!(service.flush_outbox().await.expect("flush"));
        let saved = t3_client::thread_views::outbox::load(platform.as_ref())
            .await
            .expect("disk");
        assert_eq!(saved.messages[0].text, "edited");
        assert_eq!(saved.messages[0].message_id, id);
        assert_eq!(
            service
                .discard_pending(&id)
                .await
                .expect("discard")
                .expect("message")
                .text,
            "edited"
        );
        assert!(service.pending_messages(&key()).is_empty());
        assert!(
            t3_client::thread_views::outbox::load(platform.as_ref())
                .await
                .expect("disk")
                .messages
                .is_empty()
        );
    });
}

#[test]
fn signed_upload_persists_reference_before_dispatch_and_verifies_it_after_restart() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        platform.push_http(Ok(t3_client::platform::HttpResponse {
            status: 200,
            headers: Default::default(),
            body: vec![],
        }));
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect_with_upload_base(
                generation,
                endpoint.clone(),
                config(),
                "https://remote.example/",
            )
            .expect("connect");
        let attachment=DraftAttachment::decode(json!({"type":"image","id":"image","name":"x.png","mimeType":"image/png","sizeBytes":1,"previewUri":"data:image/png;base64,AA==","dataUrl":"data:image/png;base64,AA=="})).expect("attachment");
        let id = service
            .submit(
                &key(),
                "image".into(),
                SendOptions {
                    attachments: vec![attachment],
                    ..Default::default()
                },
            )
            .await
            .expect("submit");
        let scenario = async {
            let response = async {
                let mint = request(requests.next().await.expect("mint"));
                assert_eq!(mint.tag, "attachments.createUploadUrl");
                assert_eq!(mint.payload["sizeBytes"], 1);
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:mint.id,exit:Exit::success(json!({"attachmentId":"uploaded","relativeUrl":"/api/upload/signed","expiresAt":10000}))}).expect("encode"))).await.expect("reply");
            };
            let (result, ()) = futures::join!(service.step(), response);
            assert_eq!(
                result.expect("persist references"),
                Some(DeliveryResult::StaleSession)
            );
            assert!(requests.try_recv().is_err());
            let http = platform.http_requests();
            assert_eq!(http.len(), 1);
            assert_eq!(http[0].method, "PUT");
            assert_eq!(http[0].url, "https://remote.example/api/upload/signed");
            assert_eq!(http[0].body, vec![0]);
            assert_eq!(http[0].credentials, t3_client::platform::Credentials::Omit);
            let rows = t3_client::thread_views::outbox::load(platform.as_ref())
                .await
                .expect("load");
            assert_eq!(
                rows.messages[0].attachments[0].value()["uploadedAttachmentId"],
                "uploaded"
            );
            drop(service);
            let restored = Service::new(
                key().environment,
                platform.clone(),
                store,
                OrchestrationV2CreationSource::Web,
            );
            restored
                .connect_with_upload_base(
                    generation,
                    endpoint.clone(),
                    config(),
                    "https://remote.example/",
                )
                .expect("reconnect");
            let response = async {
                let verify = request(requests.next().await.expect("verify"));
                assert_eq!(verify.tag, "assets.createUrl");
                assert_eq!(verify.payload["resource"]["attachmentId"], "uploaded");
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: verify.id,
                    exit: Exit::success(
                        json!({"relativeUrl":"/api/asset/signed","expiresAt":10000}),
                    ),
                })
                .expect("encode")))
                    .await
                    .expect("reply");
                let command = request(requests.next().await.expect("dispatch"));
                assert_eq!(command.payload["messageId"], json!(id));
                assert_eq!(command.payload["attachments"][0]["id"], "uploaded");
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: command.id,
                    exit: Exit::success(json!({"sequence":9})),
                })
                .expect("encode")))
                    .await
                    .expect("reply");
            };
            let (result, ()) = futures::join!(restored.step(), response);
            assert_eq!(
                result.expect("send"),
                Some(DeliveryResult::Accepted(Some(9)))
            );
            assert_eq!(platform.http_requests().len(), 1);
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn multiple_models_launch_distinct_worktrees_and_recover_only_the_rejected_target() {
    block_on(async {
        use t3_client::thread_views::composer::{
            delivery_policy::ThreadSettings, send::FanoutTarget,
        };
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let model = |name: &str| {
            serde_json::from_value(json!({"instanceId":"codex","model":name})).expect("model")
        };
        let attachment=DraftAttachment::decode(json!({"type":"image","id":"image","name":"x.png","mimeType":"image/png","sizeBytes":1,"previewUri":"data:image/png;base64,AA==","dataUrl":"data:image/png;base64,AA=="})).expect("attachment");
        let options=SendOptions {attachments:vec![attachment],settings:Some(ThreadSettings {model_selection:model("one"),runtime_mode:t3_contracts::provider_policy::RuntimeMode::FullAccess,interaction_mode:t3_contracts::provider_policy::ProviderInteractionMode::Default}),creation:Some(serde_json::from_value(json!({"projectId":"project","projectCwd":"/workspace","workspaceMode":"worktree","branch":"main","worktreePath":null})).expect("creation")),..Default::default()};
        let submitted = service
            .submit_multiple(
                vec![
                    FanoutTarget {
                        model: model("one"),
                        text: "first model".into(),
                    },
                    FanoutTarget {
                        model: model("two"),
                        text: "second model".into(),
                    },
                ],
                options,
            )
            .await
            .expect("fanout");
        assert_eq!(submitted.len(), 2);
        assert_ne!(submitted[0].thread, submitted[1].thread);
        assert!(
            submitted
                .iter()
                .all(|submission| submission.message.is_ok())
        );
        let scenario = async {
            for rejected in [false, true] {
                let response = async {
                    if !rejected {
                        let upload = request(requests.next().await.expect("one shared upload"));
                        assert_eq!(upload.tag, "assets.persistChatAttachments");
                        peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:upload.id,exit:Exit::success(json!({"attachments":[{"type":"image","id":"uploaded","name":"x.png","mimeType":"image/png","sizeBytes":1}]}))}).expect("encode"))).await.expect("reply");
                    }
                    let command = request(requests.next().await.expect("launch"));
                    assert_eq!(command.tag, "orchestration.launchThread");
                    assert_eq!(command.payload["workspaceStrategy"]["type"], "worktree");
                    assert_eq!(
                        command.payload["initialMessage"]["attachments"][0]["id"],
                        "uploaded"
                    );
                    let exit = if rejected {
                        Exit::fail(
                            json!({"_tag":"OrchestrationV2ThreadLaunchError","commandId":command.payload["commandId"],"projectId":"project","message":"provider unavailable"}),
                        )
                    } else {
                        let mut projection: Value = serde_json::from_str(
                            include_str!(
                                "../../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl"
                            )
                            .lines()
                            .next()
                            .expect("sample"),
                        )
                        .expect("projection");
                        projection["thread"]["id"] = command.payload["threadId"].clone();
                        Exit::success(
                            json!({"threadId":command.payload["threadId"],"projection":projection,"resumed":false,"sequence":20}),
                        )
                    };
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                        request_id: command.id,
                        exit,
                    })
                    .expect("encode")))
                        .await
                        .expect("reply");
                };
                let (result, ()) = futures::join!(service.step(), response);
                assert_eq!(
                    result.expect("send"),
                    Some(if rejected {
                        DeliveryResult::Restored
                    } else {
                        DeliveryResult::Accepted(Some(20))
                    })
                );
            }
            let bytes = platform
                .get(
                    StorageArea::Ordinary,
                    t3_client::thread_views::drafts::storage_key(
                        t3_client::thread_views::drafts::DraftFormat::Mobile,
                    ),
                )
                .await
                .expect("read")
                .expect("recovery");
            let saved = t3_client::thread_views::drafts::SavedDraftDocument::decode_mobile(&bytes)
                .expect("document");
            let drafts = saved.raw()["drafts"].as_object().expect("drafts");
            assert_eq!(drafts.len(), 1);
            let restored = drafts.values().next().expect("failed model");
            assert_eq!(restored["modelSelection"]["model"], "two");
            assert_eq!(restored["text"], "second model");
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn queued_run_edit_uses_the_same_durable_dispatch_path_without_appending_a_message() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = t3_contracts::base_schemas::MessageId("existing-message".into());
        seen(&store, generation, &id, 2);
        service
            .edit_queued_run(
                &key(),
                t3_contracts::base_schemas::RunId("queued-run".into()),
                id.clone(),
                " edited text ".into(),
                SendOptions::default(),
            )
            .await
            .expect("save edit");
        assert!(service.pending_messages(&key()).is_empty());
        assert_eq!(service.pending_edits(&key()).len(), 1);
        let scenario = async {
            let response = async {
                let command = request(requests.next().await.expect("edit"));
                assert_eq!(command.tag, "orchestration.dispatchCommand");
                assert_eq!(command.payload["type"], "queued-run.edit");
                assert_eq!(command.payload["text"], "edited text");
                assert_eq!(command.payload["runId"], "queued-run");
                assert_eq!(command.payload["attachments"], json!([]));
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: command.id,
                    exit: Exit::success(json!({"sequence":9})),
                })
                .expect("encode")))
                    .await
                    .expect("reply");
            };
            let (result, ()) = futures::join!(service.step(), response);
            assert_eq!(
                result.expect("edit"),
                Some(DeliveryResult::Accepted(Some(9)))
            );
            assert_eq!(service.pending_edits(&key()).len(), 1);
            seen(&store, generation, &id, 9);
            assert!(service.step().await.expect("confirmed").is_none());
            assert!(service.pending_edits(&key()).is_empty());
            assert!(
                platform
                    .list(StorageArea::Ordinary, "outbox".into())
                    .await
                    .expect("rows")
                    .is_empty()
            );
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn creation_uses_the_real_launch_rpc_for_root_existing_and_new_worktrees() {
    block_on(async {
        for (mode, path, expected) in [
            ("local", None, "root"),
            ("local", Some("/existing"), "existing_worktree"),
            ("worktree", None, "worktree"),
        ] {
            let platform = Rc::new(FakePlatform::default());
            let (store, generation) = store();
            let target = ThreadKey {
                thread: ThreadId(format!("new-{expected}")),
                ..key()
            };
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
            let endpoint = Rc::new(endpoint);
            let service = Service::new(
                key().environment,
                platform,
                store,
                OrchestrationV2CreationSource::Web,
            );
            service
                .connect(generation, endpoint.clone(), config())
                .expect("connect");
            let options=SendOptions {settings:Some(t3_client::thread_views::composer::delivery_policy::ThreadSettings {model_selection:serde_json::from_value(json!({"instanceId":"codex","model":"gpt-5.4"})).expect("model"),runtime_mode:t3_contracts::provider_policy::RuntimeMode::FullAccess,interaction_mode:t3_contracts::provider_policy::ProviderInteractionMode::Default}),creation:Some(serde_json::from_value(json!({"projectId":"project","projectCwd":"/workspace","workspaceMode":mode,"branch":"main","worktreePath":path,"startFromOrigin":true})).expect("creation")),..Default::default()};
            let id = service
                .submit(&target, "  first message  ".into(), options)
                .await
                .expect("submit");
            let scenario = async {
                let response = async {
                    let command = request(requests.next().await.expect("launch"));
                    assert_eq!(command.tag, "orchestration.launchThread");
                    assert_eq!(command.payload["threadId"], json!(target.thread));
                    assert_eq!(command.payload["workspaceStrategy"]["type"], expected);
                    assert_eq!(command.payload["initialMessage"]["messageId"], json!(id));
                    assert_eq!(command.payload["initialMessage"]["text"], "first message");
                    if expected == "worktree" {
                        assert_eq!(command.payload["workspaceStrategy"]["baseRef"], "main");
                        assert_eq!(
                            command.payload["workspaceStrategy"]["startFromOrigin"],
                            true
                        );
                    }
                    let mut projection: Value = serde_json::from_str(
                        include_str!(
                            "../../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl"
                        )
                        .lines()
                        .next()
                        .expect("sample"),
                    )
                    .expect("projection");
                    projection["thread"]["id"] = json!(target.thread);
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:command.id,exit:Exit::success(json!({"threadId":target.thread,"projection":projection,"resumed":false,"sequence":20}))}).expect("encode"))).await.expect("reply");
                };
                let (result, ()) = futures::join!(service.step(), response);
                assert_eq!(
                    result.expect("launch"),
                    Some(DeliveryResult::Accepted(Some(20)))
                );
                assert_eq!(service.pending_messages(&target).len(), 1);
                assert!(requests.try_recv().is_err());
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
        }
    });
}

#[test]
fn lost_launch_reply_replays_the_same_workspace_and_holds_for_operation_access() {
    block_on(async {
        for (pull_request, edited, restarted) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let platform = Rc::new(FakePlatform::default());
            let (store, generation) = store();
            let target = ThreadKey {
                thread: ThreadId("new-thread".into()),
                ..key()
            };
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let mut access = grants();
            access.scopes.insert("source-control:write".into());
            let (endpoint, driver) =
                rpc::session(wire, incoming, key().environment, access.clone(), 0);
            let endpoint = Rc::new(endpoint);
            let service = Service::new(
                key().environment,
                platform.clone(),
                store.clone(),
                OrchestrationV2CreationSource::Mobile,
            );
            service
                .connect(generation, endpoint.clone(), config())
                .expect("connect");
            let mut creation = json!({"projectId":"project","projectCwd":"/workspace","workspaceMode":"local","branch":null,"worktreePath":null});
            if pull_request {
                creation["pullRequestReference"] = json!("https://github.com/acme/repo/pull/123");
            }
            let options = SendOptions {
                settings: Some(
                    t3_client::thread_views::composer::delivery_policy::ThreadSettings {
                        model_selection: serde_json::from_value(
                            json!({"instanceId":"codex","model":"gpt-5.4"}),
                        )
                        .expect("model"),
                        runtime_mode: t3_contracts::provider_policy::RuntimeMode::FullAccess,
                        interaction_mode:
                            t3_contracts::provider_policy::ProviderInteractionMode::Default,
                    },
                ),
                creation: Some(serde_json::from_value(creation).expect("creation")),
                ..Default::default()
            };
            let id = service
                .submit(&target, "My dictated idea".into(), options)
                .await
                .expect("submit");
            let scenario = async {
                let first = async {
                    if pull_request {
                        let checkout = request(requests.next().await.expect("checkout"));
                        assert_eq!(checkout.tag, "git.preparePullRequestThread");
                        peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit { request_id: checkout.id, exit: Exit::success(json!({"pullRequest":{"number":123,"title":"PR","url":"https://github.com/acme/repo/pull/123","baseBranch":"main","headBranch":"pr-123","state":"open"},"branch":"pr-123","worktreePath":"/workspace/.t3/worktrees/pr-123","isOnPullRequestHead":true})) }).expect("encode"))).await.expect("checkout reply");
                    }
                    let launch = request(requests.next().await.expect("launch"));
                    assert_eq!(launch.tag, "orchestration.launchThread");
                    if edited {
                        service
                            .update_pending(&id, "Edited idea".into(), SendOptions::default())
                            .now_or_never()
                            .expect("creation edits persist while the launch is in flight")
                            .expect("edit");
                    }
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                        request_id: launch.id,
                        exit: Exit::fail(
                            json!({"_tag":"RpcClientError","message":"Socket is not connected"}),
                        ),
                    })
                    .expect("encode")))
                        .await
                        .expect("lost reply");
                    launch.payload
                };
                let (original, pending) = match futures::future::select(
                    service.step().boxed_local(),
                    first.boxed_local(),
                )
                .await
                {
                    futures::future::Either::Left((outcome, _)) => {
                        panic!("launch ended before the expected server requests: {outcome:?}")
                    }
                    futures::future::Either::Right((original, pending)) => (original, pending),
                };
                let outcome = pending.await;
                assert!(matches!(
                    outcome.expect("lost reply"),
                    Some(DeliveryResult::Retry | DeliveryResult::Wait)
                ));
                let mut shell = serde_json::to_value(
                    store.snapshot().environments[&key().environment]
                        .shell
                        .snapshot
                        .as_ref()
                        .expect("shell")
                        .as_ref(),
                )
                .expect("shell json");
                let mut appeared = shell["threads"][0].clone();
                appeared["id"] = json!(target.thread);
                shell["threads"]
                    .as_array_mut()
                    .expect("threads")
                    .push(appeared);
                store.apply_shell(
                    &key().environment,
                    generation,
                    vec![
                        serde_json::from_value(json!({"kind":"snapshot","snapshot":shell}))
                            .expect("shell"),
                        serde_json::from_value(json!({"kind":"synchronized"}))
                            .expect("synchronized"),
                    ],
                );
                if edited || restarted {
                    let active_service = if restarted {
                        let fresh = Service::new(
                            key().environment,
                            platform.clone(),
                            store.clone(),
                            OrchestrationV2CreationSource::Mobile,
                        );
                        fresh
                            .connect(generation, endpoint.clone(), config())
                            .expect("reconnect");
                        fresh
                    } else {
                        service.clone()
                    };
                    active_service.wake();
                    assert_eq!(active_service.step().await.expect("recover"), None);
                    assert!(
                        active_service
                            .draft_text(&target)
                            .expect("recovered draft")
                            .contains(if edited {
                                "Edited idea"
                            } else {
                                "My dictated idea"
                            })
                    );
                    assert!(requests.try_recv().is_err());
                    endpoint.invalidate();
                    return;
                }
                access.scopes.remove("orchestration:operate");
                endpoint.update_grants(access.clone());
                service.wake();
                let held = service
                    .step()
                    .now_or_never()
                    .expect("a missing operation grant holds delivery without sending an RPC")
                    .expect("hold");
                assert!(
                    matches!(held, Some(DeliveryResult::Wait | DeliveryResult::Retry)),
                    "{held:?}"
                );
                assert!(requests.try_recv().is_err());
                assert_eq!(service.pending_messages(&target).len(), 1);
                assert!(service.draft_text(&target).is_none());
                access.scopes.insert("orchestration:operate".into());
                endpoint.update_grants(access);
                service.wake();
                let replay = async {
                    let resent = request(requests.next().await.expect("resend"));
                    assert_eq!(resent.tag, "orchestration.launchThread");
                    assert_eq!(resent.payload, original);
                    let mut projection: Value = serde_json::from_str(
                        include_str!(
                            "../../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl"
                        )
                        .lines()
                        .next()
                        .expect("sample"),
                    )
                    .expect("projection");
                    projection["thread"]["id"] = json!(target.thread);
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit { request_id: resent.id, exit: Exit::success(json!({"threadId":target.thread,"projection":projection,"resumed":false,"sequence":20})) }).expect("encode"))).await.expect("replayed reply");
                };
                let (outcome, ()) = futures::join!(service.step(), replay);
                assert_eq!(
                    outcome.expect("resend"),
                    Some(DeliveryResult::Accepted(Some(20)))
                );
                assert_eq!(
                    service.state(&id),
                    Some(SendState::Accepted { sequence: Some(20) })
                );
                assert!(requests.try_recv().is_err());
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
        }
    });
}

#[test]
fn captured_settings_are_synchronized_before_the_message_dispatch() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform,
            store,
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let settings = t3_client::thread_views::composer::delivery_policy::ThreadSettings {
            model_selection: serde_json::from_value(
                json!({"instanceId":"codex","model":"gpt-5.4"}),
            )
            .expect("model"),
            runtime_mode: t3_contracts::provider_policy::RuntimeMode::ApprovalRequired,
            interaction_mode: t3_contracts::provider_policy::ProviderInteractionMode::Plan,
        };
        service
            .submit(
                &key(),
                "settings".into(),
                SendOptions {
                    settings: Some(settings),
                    ..Default::default()
                },
            )
            .await
            .expect("submit");
        let scenario = async {
            let response = async {
                for (index, kind) in [
                    "thread.runtime-mode.set",
                    "thread.interaction-mode.set",
                    "message.dispatch",
                ]
                .iter()
                .enumerate()
                {
                    let command = request(requests.next().await.expect("command"));
                    assert_eq!(command.payload["type"], *kind);
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                        request_id: command.id,
                        exit: Exit::success(json!({"sequence":index+2})),
                    })
                    .expect("encode")))
                        .await
                        .expect("reply");
                }
            };
            let (result, ()) = futures::join!(service.step(), response);
            assert_eq!(
                result.expect("send"),
                Some(DeliveryResult::Accepted(Some(4)))
            );
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn automatic_driver_wakes_on_submit_and_disposes_without_an_idle_timer() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, rpc_driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let changes = service.changes();
        let scenario = async {
            let id = service
                .send_message(&key(), "automatic".into())
                .await
                .expect("submit");
            let command = request(requests.next().await.expect("automatic dispatch"));
            peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                request_id: command.id,
                exit: Exit::success(json!({"sequence":8})),
            })
            .expect("encode")))
                .await
                .expect("reply");
            let mut accepted = changes.filter_map(|()| {
                futures::future::ready(
                    matches!(service.state(&id), Some(SendState::Accepted { .. })).then_some(()),
                )
            });
            accepted.next().await.expect("accepted notification");
            assert_eq!(platform.pending_timers(), 0);
            service.dispose();
            endpoint.invalidate();
        };
        let (_, result, ()) = futures::join!(
            rpc_driver.run(futures::stream::pending(), || 0),
            service.run(),
            scenario
        );
        result.expect("driver");
        assert_eq!(platform.pending_timers(), 0);
    });
}

#[test]
fn actual_rpc_dispatch_keeps_optimistic_message_until_authoritative_confirmation() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = service
            .send_message(&key(), "hello from the real composer".into())
            .await
            .expect("persist");
        assert_eq!(service.pending_messages(&key()).len(), 1);
        assert_eq!(
            platform
                .list(StorageArea::Ordinary, "outbox".into())
                .await
                .expect("rows")
                .len(),
            1
        );
        let scenario = async {
            let response = async {
                let command = request(requests.next().await.expect("outgoing"));
                assert_eq!(command.tag, "orchestration.dispatchCommand");
                assert_eq!(command.payload["type"], "message.dispatch");
                assert_eq!(command.payload["messageId"], json!(id));
                assert_eq!(command.payload["text"], "hello from the real composer");
                assert_eq!(service.state(&id), Some(SendState::Dispatching));
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: command.id,
                    exit: Exit::success(json!({"sequence":9})),
                })
                .expect("encode")))
                    .await
                    .expect("reply");
            };
            let (delivered, ()) = futures::join!(service.step(), response);
            assert_eq!(
                delivered.expect("delivery"),
                Some(DeliveryResult::Accepted(Some(9)))
            );
            assert_eq!(service.pending_messages(&key()).len(), 1);
            assert!(
                platform
                    .list(StorageArea::Ordinary, "outbox".into())
                    .await
                    .expect("rows")
                    .is_empty()
            );
            seen(&store, generation, &id, 9);
            assert!(service.step().await.expect("reconcile").is_none());
            assert!(service.pending_messages(&key()).is_empty());
            assert!(requests.try_recv().is_err());
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn actual_server_rejection_restores_content_durably_before_removing_the_outbox_row() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = service
            .send_message(&key(), "keep this rejected message".into())
            .await
            .expect("submit");
        let drafts = Rc::new(
            t3_client::thread_views::composer::mobile_store::MobileDraftStore::new(
                platform.clone(),
            ),
        );
        drafts.load().await.expect("load drafts");
        service.attach_drafts(drafts.clone());
        let scenario = async {
            let response = async {
                let command = request(requests.next().await.expect("command"));
                drafts
                    .update(|current| {
                        let mut next = current.clone();
                        next["drafts"]["env:thread"] =
                            json!({"text":"newer unsaved typing","attachments":[]});
                        Ok(next)
                    })
                    .expect("concurrent typing");
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:command.id,exit:Exit::fail(json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":command.payload["commandId"],"commandType":"message.dispatch","message":"Failed to dispatch orchestration V2 command","detail":"service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex."}))}).expect("encode"))).await.expect("reply");
            };
            let (result, ()) = futures::join!(service.step(), response);
            assert_eq!(result.expect("recover"), Some(DeliveryResult::Restored));
            let reason = "Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex.";
            assert!(
                matches!(service.state(&id), Some(SendState::Restored { reason: r, .. }) if r == reason),
                "{:?}",
                service.state(&id)
            );
            let bytes = platform
                .get(
                    StorageArea::Ordinary,
                    t3_client::thread_views::drafts::storage_key(
                        t3_client::thread_views::drafts::DraftFormat::Mobile,
                    ),
                )
                .await
                .expect("read")
                .expect("saved");
            let document =
                t3_client::thread_views::drafts::SavedDraftDocument::decode_mobile(&bytes)
                    .expect("draft");
            assert_eq!(
                document.raw()["drafts"]["env:thread"]["text"],
                "newer unsaved typing\n\nkeep this rejected message"
            );
            assert_eq!(
                drafts.snapshot()["drafts"]["env:thread"]["text"],
                document.raw()["drafts"]["env:thread"]["text"]
            );
            // The refused message stays visible, with the server's reason, until dismissed; its
            // text is in the draft the composer reads. The outbox row is gone either way.
            let pending = service.pending_messages(&key());
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].message.message_id, id);
            assert!(
                matches!(&pending[0].state, SendState::Restored { reason: r, .. } if r == reason)
            );
            assert_eq!(
                service.draft_text(&key()).as_deref(),
                Some("newer unsaved typing\n\nkeep this rejected message")
            );
            assert!(
                platform
                    .list(StorageArea::Ordinary, "outbox".into())
                    .await
                    .expect("rows")
                    .is_empty()
            );
            assert!(
                service.retry_message(&id).is_err(),
                "resent from the composer, not here"
            );
            service.dismiss_message(&id);
            assert!(service.pending_messages(&key()).is_empty());
            assert_eq!(service.state(&id), None);
            endpoint.invalidate();
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
    });
}

#[test]
fn lost_dispatch_is_not_resent_after_restart_and_entity_evidence_confirms_it() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = service
            .send_message(&key(), "ambiguous".into())
            .await
            .expect("submit");
        let scenario = async {
            let lost = async {
                let _ = request(requests.next().await.expect("sent"));
                drop(peer);
            };
            let (result, ()) = futures::join!(service.step(), lost);
            assert_eq!(result.expect("unknown"), Some(DeliveryResult::Wait));
            assert_eq!(service.state(&id), Some(SendState::AwaitingConfirmation));
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
        drop(service);
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (_peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, _driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let restored = Service::new(
            key().environment,
            platform,
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        restored
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        assert_eq!(
            restored.step().await.expect("restore journal"),
            Some(DeliveryResult::Wait)
        );
        assert!(requests.try_recv().is_err());
        seen(&store, generation, &id, 12);
        assert_eq!(
            restored.step().await.expect("confirm"),
            Some(DeliveryResult::Accepted(None))
        );
        assert!(restored.pending_messages(&key()).is_empty());
        assert!(requests.try_recv().is_err());
    });
}

#[test]
fn upload_transport_failure_retries_with_the_injected_clock_and_stable_message_id() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint, config())
            .expect("connect");
        let attachment=DraftAttachment::decode(json!({"type":"image","id":"image","name":"x.png","mimeType":"image/png","sizeBytes":1,"previewUri":"data:image/png;base64,AA==","dataUrl":"data:image/png;base64,AA=="})).expect("image");
        let id = service
            .submit(
                &key(),
                "image".into(),
                SendOptions {
                    attachments: vec![attachment],
                    ..Default::default()
                },
            )
            .await
            .expect("submit");
        let scenario = async {
            let lost = async {
                let command = request(requests.next().await.expect("upload"));
                assert_eq!(command.tag, "assets.persistChatAttachments");
                assert_eq!(command.payload["messageId"], json!(id));
                drop(peer);
            };
            let (result, ()) = futures::join!(service.step(), lost);
            assert_eq!(result.expect("retry"), Some(DeliveryResult::Retry));
        };
        futures::join!(driver.run(futures::stream::pending(), || 0), scenario);
        assert_eq!(service.retry_deadline(), Some(1000));
        assert!(service.step().await.expect("backoff").is_none());
        platform.advance(999);
        assert!(service.step().await.expect("not yet").is_none());
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = rpc::session(wire, incoming, key().environment, grants(), 1000);
        let endpoint = Rc::new(endpoint);
        service
            .connect(generation, endpoint.clone(), config())
            .expect("reconnect");
        platform.advance(1);
        let scenario = async {
            let response = async {
                let upload = request(requests.next().await.expect("upload retry"));
                assert_eq!(upload.payload["messageId"], json!(id));
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {request_id:upload.id,exit:Exit::success(json!({"attachments":[{"type":"image","id":"uploaded","name":"x.png","mimeType":"image/png","sizeBytes":1}]}))}).expect("encode"))).await.expect("reply");
                let command = request(requests.next().await.expect("dispatch"));
                assert_eq!(command.payload["messageId"], json!(id));
                assert_eq!(command.payload["attachments"][0]["id"], "uploaded");
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: command.id,
                    exit: Exit::success(json!({"sequence":15})),
                })
                .expect("encode")))
                    .await
                    .expect("reply");
            };
            let (result, ()) = futures::join!(service.step(), response);
            assert_eq!(
                result.expect("send"),
                Some(DeliveryResult::Accepted(Some(15)))
            );
            endpoint.invalidate();
        };
        futures::join!(
            driver.run(futures::stream::pending(), || platform.monotonic_ms()),
            scenario
        );
    });
}

#[test]
fn an_endpoint_invalidated_mid_delivery_keeps_the_message_queued_for_retry() {
    // Review H1: the runtime invalidates the endpoint on a reconnect before the send service
    // hears of it. The message was never sent, so it must stay in the outbox, not be "refused".
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let (wire, _requests) = mpsc::unbounded::<Frame>();
        let (_peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, _driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        service
            .connect(generation, endpoint.clone(), config())
            .expect("connect");
        let id = service
            .send_message(&key(), "never reached the server".into())
            .await
            .expect("submit");
        endpoint.invalidate();
        let result = service.step().await.expect("step");
        assert_ne!(result, Some(DeliveryResult::Restored));
        assert!(
            !matches!(service.state(&id), Some(SendState::Restored { .. })),
            "{:?}",
            service.state(&id)
        );
        assert_eq!(
            platform
                .list(StorageArea::Ordinary, "outbox".into())
                .await
                .expect("rows")
                .len(),
            1,
            "the unsent message stays in the outbox"
        );
        assert!(matches!(
            service.pending_messages(&key()).as_slice(),
            [pending] if !matches!(pending.state, SendState::Restored { .. })
        ));
        // The next session delivers it.
        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (_peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (next, driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let next = Rc::new(next);
        let generation = store.begin_session(key().environment);
        store.apply_shell(
            &key().environment,
            generation,
            vec![serde_json::from_value(json!({"kind":"synchronized"})).expect("synchronized")],
        );
        service
            .connect(generation, next.clone(), config())
            .expect("reconnect");
        service.retry_message(&id).expect("retry");
        let delivered = async {
            let step = service.step();
            futures::pin_mut!(step);
            match futures::future::select(step, requests.next()).await {
                futures::future::Either::Right((Some(frame), _)) => request(frame),
                futures::future::Either::Left((result, _)) => panic!(
                    "the retried message was not dispatched: {result:?}, state {:?}",
                    service.state(&id)
                ),
                _ => panic!("the retried message was not dispatched"),
            }
        };
        let request = futures::future::select(
            driver.run(futures::stream::pending(), || 0).boxed_local(),
            delivered.boxed_local(),
        )
        .await;
        match request {
            futures::future::Either::Right((request, _)) => {
                assert_eq!(request.tag, "orchestration.dispatchCommand")
            }
            futures::future::Either::Left(_) => panic!("the session ended first"),
        }
    });
}

#[test]
fn a_delivery_step_that_fails_outright_says_why_and_tries_again() {
    // Review M7: the loop used to end on the first step error, leaving messages queued silently.
    block_on(async {
        use t3_client::platform::{StorageKey, StorageWrite};
        let platform = Rc::new(FakePlatform::default());
        let (store, generation) = store();
        let earlier = Service::new(
            key().environment,
            platform.clone(),
            store.clone(),
            OrchestrationV2CreationSource::Web,
        );
        let id = earlier
            .send_message(&key(), "after the stall".into())
            .await
            .expect("submit");
        earlier.dispose();
        let unreadable = StorageKey {
            namespace: "operation-outbox".into(),
            environment: Some(key().environment),
            record: "garbled".into(),
        };
        platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: unreadable.clone(),
                    value: Some(b"{".to_vec()),
                }],
            )
            .await
            .expect("seed");

        let (wire, mut requests) = mpsc::unbounded::<Frame>();
        let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, rpc_driver) = rpc::session(wire, incoming, key().environment, grants(), 0);
        let endpoint = Rc::new(endpoint);
        let service = Service::new(
            key().environment,
            platform.clone(),
            store,
            OrchestrationV2CreationSource::Web,
        );
        let changes = service.changes();
        let scenario = async {
            service
                .connect(generation, endpoint.clone(), config())
                .expect("connect");
            let mut stalled = changes
                .filter_map(|()| futures::future::ready(service.delivery_problem().map(|_| ())));
            stalled.next().await.expect("stall notification");
            assert!(
                service
                    .delivery_problem()
                    .is_some_and(|problem| problem.contains("Unreadable delivery records")),
                "{:?}",
                service.delivery_problem()
            );
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: unreadable,
                        value: None,
                    }],
                )
                .await
                .expect("repair");
            platform.advance(1_000);
            let command = request(requests.next().await.expect("dispatch after the backoff"));
            assert_eq!(command.tag, "orchestration.dispatchCommand");
            peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                request_id: command.id,
                exit: Exit::success(json!({"sequence":9})),
            })
            .expect("encode")))
                .await
                .expect("reply");
            let mut accepted = service.changes().filter_map(|()| {
                futures::future::ready(
                    matches!(service.state(&id), Some(SendState::Accepted { .. })).then_some(()),
                )
            });
            if !matches!(service.state(&id), Some(SendState::Accepted { .. })) {
                accepted.next().await.expect("accepted");
            }
            assert_eq!(service.delivery_problem(), None);
        };
        let supervised = async {
            match futures::future::select(service.run(), scenario.boxed_local()).await {
                futures::future::Either::Left((result, _)) => {
                    panic!("the delivery loop ended: {result:?}")
                }
                futures::future::Either::Right(((), run)) => {
                    service.dispose();
                    assert!(run.await.is_ok());
                }
            }
            endpoint.invalidate();
        };
        futures::join!(rpc_driver.run(futures::stream::pending(), || 0), supervised);
    });
}
