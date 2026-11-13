use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_client::thread_views::composer::{web_models::set_prompt, web_sessions::*};
use t3_contracts::environment::{ScopedProjectRef, ScopedThreadRef};
fn project(environment: &str, id: &str) -> ScopedProjectRef {
    serde_json::from_value(json!({"environmentId":environment,"projectId":id}))
        .expect("valid fixture")
}
fn reference(environment: &str, id: &str) -> ScopedThreadRef {
    serde_json::from_value(json!({"environmentId":environment,"threadId":id}))
        .expect("valid fixture")
}
fn empty() -> Value {
    json!({"draftsByThreadKey":{},"draftThreadsByThreadKey":{},"logicalProjectDraftThreadKeyByLogicalProjectKey":{}})
}
#[test]
fn remapping_keeps_populated_or_previewed_drafts_and_clears_only_cross_environment_uploads() {
    let mut state = empty();
    let previews = BTreeSet::new();
    let p = project("old", "p");
    set_logical_project(
        &mut state,
        " logical ",
        &p,
        "first",
        &json!({}),
        "now",
        &previews,
    );
    set_prompt(&mut state, "first", "typed");
    set_logical_project(
        &mut state,
        "logical",
        &p,
        "second",
        &json!({}),
        "later",
        &previews,
    );
    assert!(state["draftThreadsByThreadKey"].get("first").is_some());
    assert_eq!(by_project(&state, &p).expect("valid fixture").0, "second");
    set_logical_project(
        &mut state,
        "logical",
        &p,
        "third",
        &json!({}),
        "later",
        &BTreeSet::from(["old:second".into()]),
    );
    assert!(state["draftThreadsByThreadKey"].get("second").is_some());
    state["logicalProjectDraftThreadKeyByLogicalProjectKey"]["stale"] = json!("third");
    state["draftsByThreadKey"]["third"] = json!({"prompt":"text","files":[{"id":"local","file":"host:bytes","uploadedAttachmentId":"old-upload","uploadEnvironmentId":"old"},{"id":"new","file":"host:new","uploadedAttachmentId":"new-upload","uploadEnvironmentId":"new"}]});
    set_logical_project(
        &mut state,
        "next",
        &project("new", "p"),
        "third",
        &json!({}),
        "new-time",
        &previews,
    );
    assert!(
        state["logicalProjectDraftThreadKeyByLogicalProjectKey"]
            .get("stale")
            .is_none()
    );
    assert_eq!(
        state["draftThreadsByThreadKey"]["third"]["createdAt"],
        "later"
    );
    assert!(
        state["draftsByThreadKey"]["third"]["files"][0]
            .get("uploadedAttachmentId")
            .is_none()
    );
    assert_eq!(
        state["draftsByThreadKey"]["third"]["files"][1]["uploadedAttachmentId"],
        "new-upload"
    );
}
#[test]
fn session_context_resets_project_checkout_and_resolves_reserved_refs_without_conflating_draft_ids()
{
    let mut state = empty();
    set_logical_project(
        &mut state,
        "logical",
        &project("e", "p"),
        "draft-id",
        &json!({"threadId":"reserved","branch":"branch","worktreePath":"/tree","checkoutPullRequest":{"url":"url","branch":"pr"},"loadBalancedEnvironmentId":"chosen","startFromOrigin":true}),
        "original",
        &BTreeSet::new(),
    );
    assert_eq!(
        resolve(&state, Target::Thread(&reference("e", "reserved"))).as_deref(),
        Some("draft-id")
    );
    assert!(update_context(
        &mut state,
        Target::Draft("draft-id"),
        Some(&project("other", "q")),
        &json!({"createdAt":""}),
        "now"
    ));
    let session = &state["draftThreadsByThreadKey"]["draft-id"];
    assert_eq!(session["branch"], Value::Null);
    assert_eq!(session["worktreePath"], Value::Null);
    assert!(session.get("checkoutPullRequest").is_none());
    assert_eq!(session["loadBalancedEnvironmentId"], Value::Null);
    assert_eq!(session["startFromOrigin"], true);
    assert_eq!(session["createdAt"], "original");
    assert!(update_context(
        &mut state,
        Target::Draft("draft-id"),
        None,
        &json!({"branch":"manual"}),
        "now"
    ));
    assert_eq!(
        state["draftThreadsByThreadKey"]["draft-id"]["environmentSelection"],
        "manual"
    );
    assert_eq!(
        by_logical_project(&state, " logical ")
            .expect("valid fixture")
            .0,
        "draft-id"
    );
    assert!(clear_project(&mut state, &project("e", "p")).is_empty());
    assert!(state["draftThreadsByThreadKey"].get("draft-id").is_some());
}
#[test]
fn promotion_waits_for_ownership_background_completion_and_current_destination() {
    let mut state = empty();
    set_logical_project(
        &mut state,
        "logical",
        &project("e", "p"),
        "draft",
        &json!({"threadId":"reserved"}),
        "now",
        &BTreeSet::new(),
    );
    set_prompt(&mut state, "draft", "send");
    let mut tracker = PromotionTracker::default();
    let shells = BTreeMap::from([(
        "e:reserved".into(),
        json!({"latestRun":{"startedAt":"now"}}),
    )]);
    assert!(tracker.observe(&mut state, &shells).is_empty());
    assert!(by_logical_project(&state, "logical").is_none());
    state["backgroundSubmissionThreadKeys"] = json!({"e:reserved":true});
    assert!(tracker.observe(&mut state, &shells).is_empty());
    state["backgroundSubmissionThreadKeys"]["e:reserved"] = json!(false);
    let ticket = tracker
        .observe(&mut state, &shells)
        .pop()
        .expect("valid fixture");
    assert!(tracker.observe(&mut state, &shells).is_empty());
    set_logical_project(
        &mut state,
        "logical",
        &project("e", "p"),
        "draft",
        &json!({"threadId":"new"}),
        "now",
        &BTreeSet::new(),
    );
    assert!(!tracker.complete(&mut state, ticket));
    assert_eq!(state["draftsByThreadKey"]["draft"]["prompt"], "send");
    let shells = BTreeMap::from([("e:new".into(), json!({"latestUserMessageAt":"now"}))]);
    tracker.observe(&mut state, &shells);
    let ticket = tracker
        .observe(&mut state, &shells)
        .pop()
        .expect("valid fixture");
    assert!(tracker.complete(&mut state, ticket));
    assert_eq!(state["draftsByThreadKey"]["e:new"]["prompt"], "send");
    assert_eq!(state["promotedDraftRedirects"]["draft"]["threadId"], "new");
    tracker.dispose();
    assert!(tracker.observe(&mut state, &shells).is_empty());
}
