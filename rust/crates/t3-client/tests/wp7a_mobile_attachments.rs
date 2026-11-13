use serde_json::{Value, json};
use t3_client::thread_views::composer::{
    mobile_attachments::*, mobile_content::Insertion, mobile_drafts::restore_file_references,
};
fn file(id: &str, name: &str, mime: &str) -> Value {
    json!({"type":"file","id":id,"name":name,"mimeType":mime,"sizeBytes":3,"fileUri":format!("file:///{id}")})
}
#[test]
fn append_replaces_captured_file_selection_before_counting_caps_and_keeps_rejected_cleanup() {
    let old = file("old", "old.txt", "text/plain");
    let draft = json!({"text":"[old](t3-context://v1/file/old)","context":{"version":1,"records":[context_record(&old)]},"attachments":[old]});
    let selection = Insertion {
        text: draft["text"].as_str().expect("text").into(),
        start: 0,
        end: draft["text"].as_str().expect("text").len(),
    };
    let next = file("next", "n[ew]\n.txt", "text/plain");
    let extra = file("extra", "extra.txt", "text/plain");
    let result = append(
        &draft,
        &[next.clone(), extra.clone()],
        AppendOptions {
            append_reference: true,
            insertion: Some(&selection),
            max_attachments: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(result.rejected, 1);
    assert_eq!(result.draft["attachments"], json!([next]));
    assert_eq!(
        result.draft["text"],
        "[n ew .txt](t3-context://v1/file/next) "
    );
    assert_eq!(
        result.cleanup,
        json!([extra, old]).as_array().expect("array").clone()
    );
    assert!(result.cursor.is_some());
    let removed = remove(&result.draft, "next");
    assert_eq!(removed.draft["text"], " ");
    assert!(removed.draft.get("context").is_none());
    assert_eq!(removed.cleanup, vec![next]);
}
#[test]
fn overflow_preserves_recovery_attachments_but_context_limits_still_apply() {
    let all: Vec<_> = (0..101)
        .map(|id| file(&id.to_string(), "file.txt", "text/plain"))
        .collect();
    let draft = json!({"text":"","attachments":all});
    let incoming = file("extra", "extra.txt", "text/plain");
    let capped = append(&draft, &[incoming.clone()], Default::default());
    assert_eq!(capped.rejected, 1);
    assert_eq!(capped.draft, draft);
    let allowed = append(
        &draft,
        &[incoming],
        AppendOptions {
            allow_overflow: true,
            append_reference: true,
            ..Default::default()
        },
    );
    assert_eq!(allowed.rejected, 0);
    assert_eq!(
        allowed.draft["attachments"]
            .as_array()
            .expect("array")
            .len(),
        102
    );
    let count = t3_contracts::composer_context::COMPOSER_CONTEXT_MAX_RECORDS;
    let full = json!({"text":(0..count).map(|id|format!("[f](t3-context://v1/file/{id})")).collect::<Vec<_>>().join(" "),"attachments":[],"context":{"version":1,"records":(0..count).map(|id|json!({"contextId":id.to_string()})).collect::<Vec<_>>()}});
    assert_eq!(
        append(
            &full,
            &[file("f", "f", "text/plain")],
            AppendOptions {
                append_reference: true,
                ..Default::default()
            }
        )
        .rejected,
        1
    );
}
#[test]
fn upload_completion_changes_only_current_stamps_and_never_resurrects_removed_files() {
    let original = file("f", "new-name.txt", "text/plain");
    let draft = json!({"text":"typed","attachments":[original]});
    let mut completed = file("f", "stale-name.txt", "text/plain");
    completed["uploadedAttachmentId"] = json!("uploaded");
    completed["uploadEnvironmentId"] = json!("env");
    let (next, previous) = set_upload(&draft, &completed);
    assert_eq!(previous, Some(original));
    assert_eq!(next["attachments"][0]["name"], "new-name.txt");
    assert_eq!(next["attachments"][0]["uploadedAttachmentId"], "uploaded");
    let removed = remove(&next, "f");
    assert_eq!(
        set_upload(&removed.draft, &completed),
        (removed.draft, None)
    );
}
#[test]
fn generic_picker_media_and_unsupported_images_follow_source_inference() {
    let image = file("image", "p.PNG ", "application/octet-stream");
    assert_eq!(context_record(&image)["kind"], "image");
    let draft = json!({"text":"","attachments":[image,file("v","v.mp4","application/octet-stream"),file("s","s.svg","image/svg+xml"),file("ts","source.ts","video/mp2t")]});
    let restored = restore_file_references(&draft);
    assert_eq!(
        restored["context"]["records"]
            .as_array()
            .expect("records")
            .len(),
        2
    );
    assert!(restored["text"].as_str().expect("text").contains("/file/s"));
    assert!(
        restored["text"]
            .as_str()
            .expect("text")
            .contains("/file/ts")
    );
    let label = "😀".repeat(110);
    let record = context_record(&file("f", &label, "text/plain"));
    assert_eq!(
        record["label"]
            .as_str()
            .expect("label")
            .encode_utf16()
            .count(),
        200
    );
}
