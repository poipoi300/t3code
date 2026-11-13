#![allow(clippy::unwrap_used)]
use futures::executor::block_on;
use serde_json::json;
use std::collections::BTreeSet;
use t3_client::{
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::shell::{diff_state::*, stash},
};
fn key(e: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(e.into()),
        thread: ThreadId("t".into()),
    }
}
#[test]
fn tool_call_selection_survives_checkpoint_reconciliation_then_returns_to_git() {
    let mut s = DiffStore::default();
    let k = key("e");
    let target = Selection::ToolCall {
        thread_id: ThreadId("child".into()),
        item_id: "edit".into(),
        revision: "live".into(),
    };
    s.by_thread.insert(k.clone(), target.clone());
    s.reconcile_turn(&k, &[]);
    assert_eq!(s.by_thread[&k], target);
    s.select_git(k.clone(), false);
    assert_eq!(s.by_thread[&k], Selection::default());
    s.select_base(k.clone(), Some(" main "));
    s.select_commit(k.clone(), "abc".into());
    s.by_thread.insert(k.clone(), target);
    s.select_git(k.clone(), true);
    s.select_git(k.clone(), false);
    assert_eq!(
        s.by_thread[&k],
        Selection::Branch {
            base_ref: Some("main".into())
        }
    );
    assert_eq!(s.by_thread.get(&key("other")), None);
}
#[test]
fn tool_call_revision_refresh_matches_source_identity_and_completed_detail() {
    let target = Selection::ToolCall {
        thread_id: ThreadId("child".into()),
        item_id: "edit".into(),
        revision: "live".into(),
    };
    let mut row = json!({"position":0,"visibility":"local","sourceThreadId":"child","sourceItemId":"edit","item":{
  "id":"edit","threadId":"child","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
  "ordinal":1,"status":"running","title":null,"startedAt":"2026-10-28T00:00:00.000Z","completedAt":null,"updatedAt":"2026-10-28T00:00:00.000Z","type":"file_change","fileName":"a.ts"}});
    let running = serde_json::from_value(row.clone()).unwrap();
    assert_eq!(refresh_tool_call_target(target.clone(), &[running]), target);
    row["item"]["status"] = json!("completed");
    row["item"]["completedAt"] = json!("2026-10-28T00:00:00.000Z");
    let completed = serde_json::from_value(row.clone()).unwrap();
    assert!(
        matches!(refresh_tool_call_target(target.clone(),&[completed]),Selection::ToolCall{revision,..}if revision=="2026-10-28T00:00:00.000Z")
    );
    row["sourceThreadId"] = json!("other");
    assert_eq!(
        refresh_tool_call_target(target.clone(), &[serde_json::from_value(row).unwrap()]),
        target
    );
    assert_eq!(refresh_tool_call_target(target.clone(), &[]), target);
}
#[test]
fn diff_selection_persistence_and_session_fold_choices_have_separate_lifetimes() {
    let mut s = DiffStore::default();
    let k = key("e");
    let defaults = BTreeSet::from(["a".into(), "b".into()]);
    s.update_collapsed("scope".into(), &defaults, |current| {
        current
            .iter()
            .filter(|s| s.as_str() != "a")
            .cloned()
            .collect()
    });
    s.update_collapsed("scope".into(), &BTreeSet::new(), |current| {
        assert_eq!(current, &BTreeSet::from(["b".into()]));
        BTreeSet::new()
    });
    s.update_collapsed("scope".into(), &defaults, Clone::clone);
    assert!(s.collapsed["scope"].is_empty());
    assert_eq!(defaults.len(), 2);
    s.ignore_whitespace = Some(true);
    s.select_base(k.clone(), Some(" main "));
    s.select_turn(k.clone(), "old".into(), Some(" a.rs "));
    s.select_turn(k.clone(), "old".into(), None);
    s.reconcile_turn(&k, &["new".into()]);
    assert_eq!(
        s.by_thread[&k],
        Selection::Turn {
            turn_id: "new".into(),
            file_path: None,
            reveal_request_id: 2
        }
    );
    let mut restored = DiffStore::decode(&s.encode().unwrap()).unwrap();
    assert_eq!(restored.by_thread, s.by_thread);
    assert!(restored.collapsed.is_empty());
    assert_eq!(restored.ignore_whitespace, None);
    restored.select_git(k.clone(), false);
    assert_eq!(
        restored.by_thread[&k],
        Selection::Branch {
            base_ref: Some("main".into())
        }
    );
    restored.remove(&k);
    assert!(restored.by_thread.is_empty());
    assert!(restored.branch_base.is_empty());
}
fn entry(id: &str) -> stash::Entry {
    serde_json::from_value(json!({"id":id,"createdAt":"2026-01-01T00:00:00Z","prompt":"@[citation] investigate","attachments":[],"droppedImageNames":[],
 "citations":[{"id":"c","unknownFutureField":42}],"boundComments":[{"id":"b","filePath":"a.rs"}],"selectionMentions":[{"id":"s","text":"quoted"}]})).unwrap()
}
fn image(name: &str, data: String) -> stash::SavedImage {
    serde_json::from_value(
        json!({"id":name,"name":name,"mimeType":"image/png","sizeBytes":1,"dataUrl":data}),
    )
    .unwrap()
}
#[test]
fn stash_attachment_budget_counts_utf16_and_preserves_input_order() {
    let (kept, dropped) = stash::partition_attachments(vec![
        image("big", "a".repeat(stash::MAX_ATTACHMENT_CHARS - 2)),
        image("unicode", "😀".into()),
        image("overflow", "x".into()),
    ]);
    assert_eq!(
        kept.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["big", "unicode"]
    );
    assert_eq!(dropped, ["overflow"]);
}
#[test]
fn stash_round_trip_preserves_composer_records_uploaded_files_and_unknown_metadata() {
    let mut e = entry("one");
    e.files=Some(vec![serde_json::from_value(json!({"id":"file","name":"a.rs","mimeType":"text/plain","sizeBytes":5,"attachmentId":"uploaded","environmentId":"remote","future":true})).unwrap()]);
    let raw = stash::encode(std::slice::from_ref(&e)).unwrap();
    let restored = stash::decode(&raw).unwrap();
    assert_eq!(restored, [e]);
    assert!(!raw.contains("fileContents"));
    assert!(stash::decode("{broken").is_err());
    assert!(stash::decode(r#"{"version":1,"state":{"entries":[{"id":"bad"}]}}"#).is_err());
}
#[test]
fn interrupted_image_serialization_settles_as_unreadable_without_losing_prompt() {
    let mut e = entry("one");
    e.pending_image_count = Some(2.0);
    let raw = stash::encode(&[e]).unwrap();
    let restored = stash::decode(&raw).unwrap();
    assert_eq!(restored[0].pending_image_count, Some(0.0));
    assert_eq!(
        restored[0].unreadable_image_names.as_ref().unwrap(),
        &[
            "image 1 (not saved before reload)",
            "image 2 (not saved before reload)"
        ]
    );
    assert_eq!(restored[0].prompt, "@[citation] investigate");
}
#[test]
fn stash_eviction_and_failed_vs_memory_only_writes_are_explicit() {
    block_on(async {
        let mut s = stash::Stash {
            entries: (0..20).map(|i| entry(&i.to_string())).collect(),
        };
        let before = s.entries.clone();
        let failed = s
            .stash(entry("new"), None, async |_| stash::WriteOutcome {
                written: false,
                durable: false,
            })
            .await;
        assert!(failed.evicted.is_none());
        assert_eq!(s.entries, before);
        let memory = s
            .stash(entry("new"), None, async |_| stash::WriteOutcome {
                written: true,
                durable: false,
            })
            .await;
        assert_eq!(memory.evicted.unwrap().id, "19");
        assert!(memory.write.written);
        assert!(!memory.write.durable);
        assert_eq!(s.entries.len(), 20);
    });
}
#[test]
fn stash_merges_latest_cross_tab_entries_and_takes_finalized_metadata() {
    block_on(async {
        let mut s = stash::Stash {
            entries: vec![entry("one")],
        };
        let stored = vec![entry("other-tab"), entry("one")];
        let ok = async |_: &[stash::Entry]| stash::WriteOutcome {
            written: true,
            durable: true,
        };
        s.stash(entry("new"), Some(&stored), ok).await;
        assert_eq!(
            s.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["new", "other-tab", "one"]
        );
        assert_eq!(
            s.finalize_images(
                "one",
                vec![image("saved", "url".into())],
                vec!["dropped".into()],
                vec!["unreadable".into()],
                None,
                ok
            )
            .await,
            (true, true)
        );
        let (taken, durable) = s.take("one", None, ok).await;
        let taken = taken.unwrap();
        assert!(durable);
        assert_eq!(taken.attachments[0].name, "saved");
        assert_eq!(taken.dropped_image_names, ["dropped"]);
        assert_eq!(
            taken.unreadable_image_names,
            Some(vec!["unreadable".into()])
        );
        assert_eq!(
            s.finalize_images("one", vec![], vec![], vec![], None, ok)
                .await,
            (false, true)
        );
        let (taken, durable) = s
            .take("new", None, async |_| stash::WriteOutcome {
                written: false,
                durable: false,
            })
            .await;
        assert!(taken.is_some());
        assert!(!durable);
        let authoritative = vec![entry("other-tab")];
        s.stash(entry("last"), Some(&authoritative), ok).await;
        assert_eq!(
            s.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["last", "other-tab"]
        );
    });
}
