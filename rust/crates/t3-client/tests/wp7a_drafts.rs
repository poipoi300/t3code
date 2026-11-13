use serde_json::{Value, json};
use t3_client::thread_views::drafts::{
    DraftFormat, SavedDraftDocument, WebDraftPersistence, storage_key,
};
use t3_client::{
    platform::{Storage, StorageArea},
    testing::FakePlatform,
};
#[test]
fn every_historical_web_version_can_be_edited_and_saved() {
    futures::executor::block_on(async {
        for version in 0..=9 {
            let raw = json!({"version":version,"state":{"draftsByThreadId":{"t":{"prompt":"saved 😀","attachments":[],"modelSelection":{"provider":"codex","model":"custom-model","options":{"reasoningEffort":"high"}}}},"future":{"keep":true}}});
            let loaded = SavedDraftDocument::decode_web(&serde_json::to_vec(&raw).expect("json"))
                .expect("load");
            let mut state = loaded.state().clone();
            state["draftsByThreadKey"] = state["draftsByThreadId"].clone();
            state["draftThreadsByThreadKey"] = json!({});
            state["logicalProjectDraftThreadKeyByLogicalProjectKey"] = json!({});
            state["draftsByThreadKey"]["t"]["prompt"] = json!("edited 😀");
            let next = loaded
                .replace_current_web_state(state)
                .expect("legacy write");
            let host = FakePlatform::default();
            let key = storage_key(DraftFormat::Web);
            let mut writer = WebDraftPersistence::new(loaded).expect("writer");
            writer.schedule(next, 0).expect("schedule");
            writer.flush(&host, key.clone()).await.expect("flush");
            let bytes = host
                .get(StorageArea::Ordinary, key)
                .await
                .expect("read")
                .expect("record");
            let reloaded = SavedDraftDocument::decode_web(&bytes).expect("reload");
            assert!(!reloaded.requires_web_migration());
            assert_eq!(
                reloaded.drafts().expect("drafts")["t"]["prompt"],
                "edited 😀"
            );
            assert_eq!(reloaded.state()["future"], json!({"keep":true}));
            assert_eq!(
                serde_json::from_slice::<Value>(&raw_bytes(&raw)).expect("original"),
                raw
            );
        }
    });
}
fn raw_bytes(raw: &Value) -> Vec<u8> {
    serde_json::to_vec(raw).expect("json")
}

#[test]
fn mobile_migration_discards_live_edit_keys_but_keeps_archived_user_drafts() {
    use t3_client::thread_views::composer::mobile_drafts::migrate_document;
    let pick = json!({"instanceId":"codex","model":"custom"});
    let raw = json!({"schemaVersion":1,"drafts":{
        "e:t~queued-edit~r":{"text":"ephemeral edit","attachments":[]},
        "new-task:live":{"text":"","attachments":[],"modelSelection":pick}
    },"signedOutDrafts":{"account":{"drafts":{
        "new-task:archived":{"text":"","attachments":[],"modelSelection":pick},
        "receipt":{"text":"","attachments":[],"importedShareIds":["s"]}
    },"queuedMessages":[]}}});
    let migrated = migrate_document(&raw, "now", || "id".into()).expect("migrate");
    assert!(migrated["drafts"].get("e:t~queued-edit~r").is_none());
    assert!(migrated["drafts"].get("new-task:live").is_none());
    assert_eq!(
        migrated["signedOutDrafts"]["account"]["drafts"]["new-task:archived"]["modelSelection"],
        pick
    );
    assert_eq!(
        migrated["signedOutDrafts"]["account"]["drafts"]["receipt"]["importedShareIds"],
        json!(["s"])
    );
}

#[test]
fn hydrated_web_draft_restores_file_chips_and_terminal_text_defaults() {
    use t3_client::thread_views::composer::web_drafts::{hydrate_draft, partialize_state};
    let raw = json!({"prompt":"read","attachments":[],"files":[{
        "id":"f","name":"notes.txt","mimeType":"text/plain","sizeBytes":3
    }],"terminalContexts":[{"id":"terminal","text":null}]});
    let draft = hydrate_draft(&raw);
    assert_eq!(
        draft["prompt"],
        "read [notes.txt](t3-context://v1/file/file_f) "
    );
    assert_eq!(draft["terminalContexts"][0]["text"], "");
    let state = json!({"draftsByThreadKey":{"t":draft},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}});
    let saved = partialize_state(&state, &Default::default());
    assert!(
        saved["draftsByThreadKey"]["t"]["files"][0]
            .get("file")
            .is_none()
    );
}

#[test]
fn web_hydration_restores_context_chips_in_the_source_order() {
    use t3_client::thread_views::composer::web_drafts::hydrate_draft;
    let contexts = json!({"prompt":"read","attachments":[],"files":[{"id":"f","name":"notes.txt","mimeType":"text/plain","sizeBytes":3}],
        "reviewComments":[{"id":"review","sectionId":"pull-request:42","filePath":"src/page.tsx","rangeLabel":"-12 to -18","diff":"old line"}],
        "previewAnnotations":[{"id":"preview","comment":"  check\nthis ","pageTitle":"Page"}],
        "threadContexts":[{"contextId":"thread_other","label":"Other"}],
        "environmentContexts":[{"contextId":"environment_e","label":"Host"}]});
    let hydrated = hydrate_draft(&contexts);
    assert_eq!(
        hydrated["prompt"],
        "read [#42 page.tsx L12 to L18 (before)](t3-context://v1/review-comment/review-comment_review) [check this](t3-context://v1/preview-annotation/preview-annotation_preview) [Other](t3-context://v1/thread/thread_other) [Host](t3-context://v1/environment/environment_e) [notes.txt](t3-context://v1/file/file_f) "
    );
    assert_eq!(hydrate_draft(&hydrated)["prompt"], hydrated["prompt"]);
}

#[test]
fn corrupt_cloud_archives_are_retained_for_recovery() {
    use t3_client::thread_views::composer::mobile_drafts::{migrate_document, restore_cloud};
    for archive in [
        json!("unreadable"),
        json!({"drafts":{},"queuedMessages":"unreadable"}),
        json!({"drafts":"unreadable","queuedMessages":[]}),
    ] {
        let raw = json!({"schemaVersion":1,"drafts":{},"signedOutDrafts":{"account":archive}});
        assert!(restore_cloud(&raw, "account").is_err(), "{raw}");
        assert!(
            migrate_document(&raw, "now", || "id".into()).is_err(),
            "{raw}"
        );
        assert_eq!(raw["signedOutDrafts"]["account"], archive);
    }
    for (field, value) in [
        ("modelOptionMemory", json!({"codex":{"model":"unreadable"}})),
        ("modelOptionMemory", json!({"codex":false})),
        (
            "stickyModelSelection",
            json!({"instanceId":null,"model":"retained"}),
        ),
        ("cloudAccountId", json!({"invalid":true})),
    ] {
        let mut raw = json!({"schemaVersion":1,"drafts":{"t":{"text":"keep","attachments":[]}}});
        raw[field] = value.clone();
        assert!(
            migrate_document(&raw, "now", || "id".into()).is_err(),
            "{raw}"
        );
        assert!(restore_cloud(&raw, "account").is_err(), "{raw}");
        assert_eq!(raw[field], value);
    }
}
