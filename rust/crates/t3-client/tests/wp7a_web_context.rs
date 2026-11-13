use serde_json::{Value, json};
use t3_client::thread_views::composer::{
    mobile_content::Insertion,
    web_context::{self, Kind, Placement},
    web_models::set_prompt,
};
fn terminal(id: &str, start: f64, end: f64) -> Value {
    json!({"id":id,"threadId":"other","terminalId":" tty ","terminalLabel":" Shell ","lineStart":start,"lineEnd":end,"createdAt":"then","text":"\n\nline\r\nnext\n"})
}
fn review(id: &str, text: &str) -> Value {
    json!({"id":id,"sectionId":"changes","sectionTitle":"Changes","filePath":"src.rs","rangeLabel":"line 1","text":text,"diff":"+ line","startIndex":0,"endIndex":1})
}
#[test]
fn terminal_normalization_dedup_and_reference_replacement_share_the_live_thread_scope() {
    let mut state = json!({"draftsByThreadKey":{}});
    set_prompt(&mut state, "e:t", "look");
    web_context::set(
        &mut state,
        "e:t",
        Kind::Terminal,
        &[
            terminal("first", 0.2, 0.0),
            terminal("rejected", 1.0, 1.0),
            terminal("rejected", 2.0, 3.0),
        ],
        "thread",
    );
    let contexts = &state["draftsByThreadKey"]["e:t"]["terminalContexts"];
    assert_eq!(contexts.as_array().expect("valid fixture").len(), 2);
    assert_eq!(contexts[1]["id"], "rejected");
    assert_eq!(contexts[0]["threadId"], "thread");
    assert_eq!(contexts[0]["terminalId"], "tty");
    assert_eq!(contexts[0]["text"], "line\nnext");
    assert!(
        state["draftsByThreadKey"]["e:t"]["prompt"]
            .as_str()
            .expect("valid fixture")
            .contains("Shell line 1")
    );
    web_context::set(
        &mut state,
        "e:t",
        Kind::Terminal,
        &[terminal("next", 4.0, 5.0)],
        "thread",
    );
    let prompt = state["draftsByThreadKey"]["e:t"]["prompt"]
        .as_str()
        .expect("valid fixture");
    assert!(!prompt.contains("terminal_first"));
    assert!(prompt.contains("terminal_next"));
    assert!(web_context::remove(
        &mut state,
        "e:t",
        Kind::Terminal,
        "next"
    ));
    assert_eq!(state["draftsByThreadKey"]["e:t"]["prompt"], "look");
}
#[test]
fn review_insertion_replaces_the_captured_selection_and_allows_repeated_references() {
    let mut state = json!({"draftsByThreadKey":{}});
    set_prompt(&mut state, "e:t", "😀 chosen tail");
    let insertion = Insertion {
        text: "😀 chosen tail".into(),
        start: 3,
        end: 9,
    };
    let caret = web_context::add(
        &mut state,
        "e:t",
        Kind::Review,
        &[review("comment", "first")],
        "t",
        Placement {
            insertion: Some(&insertion),
            ..Default::default()
        },
    )
    .expect("valid fixture");
    let prompt = state["draftsByThreadKey"]["e:t"]["prompt"]
        .as_str()
        .expect("valid fixture");
    assert!(prompt.starts_with("😀 ["));
    assert!(prompt.ends_with(" tail"));
    assert_eq!(caret, prompt.encode_utf16().count() - 4);
    web_context::add(
        &mut state,
        "e:t",
        Kind::Review,
        &[review("comment", "updated")],
        "t",
        Placement {
            allow_duplicate_reference: true,
            ..Default::default()
        },
    );
    let prompt = state["draftsByThreadKey"]["e:t"]["prompt"]
        .as_str()
        .expect("valid fixture");
    assert_eq!(prompt.matches("t3-context://v1/review-comment/").count(), 2);
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["reviewComments"]
            .as_array()
            .expect("valid fixture")
            .len(),
        1
    );
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["reviewComments"][0]["text"],
        "updated"
    );
    assert!(web_context::remove(
        &mut state,
        "e:t",
        Kind::Review,
        "comment"
    ));
    assert!(
        !state["draftsByThreadKey"]["e:t"]["prompt"]
            .as_str()
            .expect("valid fixture")
            .contains("t3-context")
    );
}
#[test]
fn clearing_prompt_and_images_keeps_context_records_reply_and_model_settings() {
    use t3_client::thread_views::composer::web_content::clear_prompt_and_images;
    let mut state = json!({"draftsByThreadKey":{"e:t":{"prompt":"typed","images":[{"id":"image","previewUrl":"blob:release"}],"files":[{"id":"file"}],"modelSelectionByProvider":{"codex":{"instanceId":"codex","model":"chosen"}},"activeProvider":"codex","replyTo":{"future":true}}}});
    web_context::add(
        &mut state,
        "e:t",
        Kind::Review,
        &[review("comment", "keep")],
        "t",
        Placement::default(),
    );
    assert_eq!(
        clear_prompt_and_images(&mut state, "e:t"),
        vec!["blob:release"]
    );
    let draft = &state["draftsByThreadKey"]["e:t"];
    assert!(
        draft["prompt"]
            .as_str()
            .expect("prompt")
            .contains("t3-context://v1/review-comment/")
    );
    assert!(!draft["prompt"].as_str().expect("prompt").contains("typed"));
    assert_eq!(draft["reviewComments"][0]["text"], "keep");
    assert_eq!(draft["replyTo"]["future"], true);
    assert_eq!(draft["activeProvider"], "codex");
    assert!(draft["files"].as_array().expect("files").is_empty());
}
