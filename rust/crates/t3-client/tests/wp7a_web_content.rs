use serde_json::{Value, json};
use t3_client::thread_views::composer::{web_content::*, web_models::set_prompt};
use t3_contracts::chat_attachment::PROVIDER_SEND_TURN_MAX_ATTACHMENTS;

fn file(id: &str, name: &str, mime: &str, bytes: Value) -> Value {
    json!({"id":id,"name":name,"mimeType":mime,"sizeBytes":3,"type":"file","file":bytes})
}
#[test]
fn image_dedup_and_caps_release_only_unowned_previews() {
    let mut state = json!({"draftsByThreadKey":{}});
    let image = |id: &str, preview: &str| json!({"id":id,"name":"same","mimeType":"image/png","sizeBytes":3,"previewUrl":preview});
    let changes = add_images(
        &mut state,
        "e:t",
        &[
            image("a", "blob:retained"),
            image("b", "blob:retained"),
            image("c", "blob:unused"),
        ],
        false,
    );
    assert_eq!(changes.accepted_ids, vec!["a"]);
    assert_eq!(changes.release_previews, vec!["blob:unused"]);
    assert!(
        add_images(&mut state, "e:t", &[image("a", "blob:retained")], true)
            .accepted_ids
            .is_empty()
    );
    let more = (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS)
        .map(|index| image(&format!("extra-{index}"), &format!("blob:{index}")))
        .collect::<Vec<_>>();
    let changes = add_images(&mut state, "e:t", &more, true);
    assert_eq!(
        changes.accepted_ids.len(),
        PROVIDER_SEND_TURN_MAX_ATTACHMENTS - 1
    );
    assert_eq!(changes.release_previews.len(), 1);
    let previews = remove_image(&mut state, "e:t", "a");
    assert_eq!(previews, vec!["blob:retained"]);
}
#[test]
fn missing_file_references_reattach_at_the_cap_in_input_order() {
    let mut state = json!({"draftsByThreadKey":{}});
    set_prompt(
        &mut state,
        "e:t",
        "look [movie](t3-context://v1/file/file_old-z) [raw](t3-context://v1/file/file_old-a)",
    );
    let marker = file(
        "old-z",
        "movie.mp4",
        "application/octet-stream",
        Value::Null,
    );
    let raw = file("old-a", "raw.dng", "image/dng", Value::Null);
    state["draftsByThreadKey"]["e:t"]["files"] = json!([marker.clone(), raw]);
    state["draftsByThreadKey"]["e:t"]["images"] = json!(
        (0..PROVIDER_SEND_TURN_MAX_ATTACHMENTS - 2)
            .map(|i| json!({"id":i}))
            .collect::<Vec<_>>()
    );
    let movie = file("new-movie", "movie.mp4", "video/mp4", json!("host:movie"));
    let raw = file("new-raw", "raw.dng", "image/x-adobe-dng", json!("host:raw"));
    assert!(reattach_marker_matches(&marker, &movie));
    assert!(add_files(&mut state, "e:t", &[movie, raw], false, true).is_empty());
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["files"][0]["id"],
        "new-movie"
    );
    let prompt = state["draftsByThreadKey"]["e:t"]["prompt"]
        .as_str()
        .expect("valid fixture");
    assert!(prompt.contains("file_new-movie"));
    assert!(prompt.contains("file_new-raw"));
    assert!(!prompt.contains("file_old"));
    assert!(remove_file(&mut state, "e:t", "new-movie"));
    assert!(
        !state["draftsByThreadKey"]["e:t"]["prompt"]
            .as_str()
            .expect("valid fixture")
            .contains("file_new-movie")
    );
}
#[test]
fn stale_upload_verification_cannot_clear_reuploaded_or_local_files() {
    let mut state = json!({"draftsByThreadKey":{"e:t":{"files":[file("f","file.txt","text/plain",Value::Null)]}}});
    assert!(set_file_upload(&mut state, "e:t", "f", "e", "old"));
    assert!(set_file_upload(&mut state, "e:t", "f", "e", "new"));
    assert!(!mark_file_upload_missing(
        &mut state, "e:t", "f", "e", "old"
    ));
    state["draftsByThreadKey"]["e:t"]["files"][0]["file"] = json!("host:bytes");
    assert!(!mark_file_upload_missing(
        &mut state, "e:t", "f", "e", "new"
    ));
    state["draftsByThreadKey"]["e:t"]["files"][0]["file"] = Value::Null;
    assert!(mark_file_upload_missing(&mut state, "e:t", "f", "e", "new"));
    assert!(needs_reattach(
        &state["draftsByThreadKey"]["e:t"]["files"][0]
    ));
}
#[test]
fn moving_content_keeps_overflow_and_unreachable_files_and_reuploads_cross_environment_bytes() {
    let mut local = file("local", "local.txt", "text/plain", json!("host:bytes"));
    local["uploadEnvironmentId"] = json!("old");
    local["uploadedAttachmentId"] = json!("old-upload");
    let mut remote = file("remote", "remote.txt", "text/plain", Value::Null);
    remote["uploadEnvironmentId"] = json!("old");
    remote["uploadedAttachmentId"] = json!("remote-upload");
    let mut state = json!({"draftsByThreadKey":{"from":{"prompt":"move [file](t3-context://v1/file/file_local)","images":[{"id":"image","previewUrl":"blob:retained"}],"files":[local,remote],"nonPersistedImageIds":["image"],"persistedAttachments":[{"id":"image","dataUrl":"data:image/png;base64,eA=="}]},"to":{"prompt":"existing","images":[],"files":[],"modelSelectionByProvider":{"codex":{"instanceId":"codex","model":"chosen"}},"activeProvider":"codex"}}});
    assert!(move_prompt_and_images(
        &mut state,
        "from",
        "to",
        Some("new")
    ));
    assert_eq!(
        state["draftsByThreadKey"]["from"]["files"][0]["id"],
        "remote"
    );
    assert_eq!(
        state["draftsByThreadKey"]["to"]["images"][0]["previewUrl"],
        "blob:retained"
    );
    assert!(
        state["draftsByThreadKey"]["to"]["files"][0]
            .get("uploadedAttachmentId")
            .is_none()
    );
    assert_eq!(
        state["draftsByThreadKey"]["to"]["files"][0]["file"],
        "host:bytes"
    );
    assert_eq!(
        state["draftsByThreadKey"]["to"]["persistedAttachments"][0]["id"],
        "image"
    );
    assert_eq!(state["draftsByThreadKey"]["to"]["activeProvider"], "codex");
    assert_eq!(state["draftsByThreadKey"]["to"]["prompt"], "move ");
    let before = state.clone();
    assert!(!move_prompt_and_images(
        &mut state,
        "from",
        "from",
        Some("new")
    ));
    assert_eq!(state, before);
}
#[test]
fn persisted_image_readback_tracks_concurrent_removal_and_new_images_without_dropping_live_bytes() {
    let captured = json!({"id":"a","name":"a.png","mimeType":"image/png","sizeBytes":1,"dataUrl":"data:image/png;base64,eA=="});
    let mut state = json!({"draftsByThreadKey":{"e:t":{"prompt":"keep","images":[{"id":"a","file":"host:a"}],"nonPersistedImageIds":["a"]}}});
    assert!(sync_persisted_attachments(
        &mut state,
        "e:t",
        std::slice::from_ref(&captured)
    ));
    let bytes=serde_json::to_vec(&json!({"version":9,"state":{"draftsByThreadKey":{"e:t":{"prompt":"keep","attachments":[captured.clone()]}}}})).expect("json");
    let persisted = persisted_attachment_ids(Some(&bytes), "e:t");
    assert!(persisted.contains("a"));
    state["draftsByThreadKey"]["e:t"]["images"] = json!([{"id":"b","file":"host:bytes-b"}]);
    assert!(verify_persisted_attachments(
        &mut state,
        "e:t",
        &[captured],
        &persisted
    ));
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["nonPersistedImageIds"],
        json!(["b"])
    );
    assert!(
        state["draftsByThreadKey"]["e:t"]["persistedAttachments"]
            .as_array()
            .expect("attachments")
            .is_empty()
    );
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["images"][0]["file"],
        "host:bytes-b"
    );
    assert!(persisted_attachment_ids(Some(b"unreadable"), "e:t").is_empty());
}
