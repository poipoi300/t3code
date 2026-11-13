use serde_json::{Value, json};
use std::collections::BTreeSet;
use t3_client::thread_views::composer::{web_drafts::partialize_state, web_models::*};
use t3_contracts::{model::ProviderOptionSelections, model_selection::ModelSelection};

fn pick(model: &str, options: Option<Value>) -> ModelSelection {
    let mut value = json!({"instanceId":"secondary","model":model});
    if let Some(options) = options {
        value["options"] = options;
    }
    serde_json::from_value(value).expect("valid fixture")
}
#[test]
fn explicit_picks_keep_traits_and_only_the_exact_acknowledged_pick_releases() {
    let mut state = json!({"draftsByThreadKey":{}});
    let mut picks = ModelPicks::default();
    let options = json!([{"id":"fastMode","value":false}]);
    let first = picks
        .set(
            &mut state,
            "e:t",
            Some(&pick("old", Some(options.clone()))),
            false,
            true,
        )
        .expect("valid fixture");
    let newer = picks
        .set(&mut state, "e:t", Some(&pick("new", None)), false, true)
        .expect("valid fixture");
    assert_eq!(
        newer.options.as_ref().map(|options| json!(options)),
        Some(options)
    );
    assert!(!picks.release(&mut state, "e:t", &first));
    let same_value_new_pick = picks
        .set(&mut state, "e:t", Some(newer.as_ref()), false, true)
        .expect("valid fixture");
    assert!(!picks.release(&mut state, "e:t", &newer));
    set_prompt(&mut state, "e:t", "typing");
    assert!(picks.release(&mut state, "e:t", &same_value_new_pick));
    assert_eq!(state["draftsByThreadKey"]["e:t"]["prompt"], "typing");
    assert_eq!(
        state["draftsByThreadKey"]["e:t"]["activeProvider"],
        Value::Null
    );
    assert!(!picks.release(&mut state, "e:t", &same_value_new_pick));
}

#[test]
fn sticky_traits_option_memory_explicit_clear_and_content_clear_remain_independent() {
    let mut picks = ModelPicks::default();
    let mut state = json!({"draftsByThreadKey":{},"stickyModelSelectionByProvider":{},"stickyOptionsByModelByProvider":{}});
    set_sticky(
        &mut state,
        &pick("old", Some(json!([{"id":"fastMode","value":true}]))),
    );
    set_sticky(&mut state, &pick("new", None));
    picks.apply_sticky(&mut state, "draft");
    assert_eq!(
        state["draftsByThreadKey"]["draft"]["modelSelectionByProvider"]["secondary"]["options"][0]
            ["value"],
        true
    );
    let options: ProviderOptionSelections =
        serde_json::from_value(json!([{"id":"effort","value":"high"}])).expect("valid fixture");
    picks.set_provider_options(
        &mut state,
        "draft",
        ProviderOptions {
            instance_id: "secondary",
            fallback_model: "fallback",
            remembered_model: Some("remembered"),
            options: Some(&options),
            persist_sticky: true,
            activate_instance: true,
        },
    );
    assert_eq!(
        state["stickyOptionsByModelByProvider"]["secondary"]["remembered"],
        json!(options)
    );
    assert_eq!(
        state["stickyModelSelectionByProvider"]["secondary"]["model"],
        "new"
    );
    set_prompt(&mut state, "draft", " ");
    clear_content(&mut state, "draft");
    assert_eq!(
        state["draftsByThreadKey"]["draft"]["modelSelectionExplicit"],
        true
    );
    picks.set_provider_options(
        &mut state,
        "draft",
        ProviderOptions {
            instance_id: "secondary",
            fallback_model: "fallback",
            remembered_model: Some("remembered"),
            options: None,
            persist_sticky: true,
            activate_instance: true,
        },
    );
    assert!(
        state["stickyOptionsByModelByProvider"]["secondary"]
            .get("remembered")
            .is_none()
    );
    assert!(
        state["stickyModelSelectionByProvider"]["secondary"]
            .get("options")
            .is_none()
    );
    picks.apply_sticky(&mut state, "draft");
    assert!(
        state["draftsByThreadKey"]["draft"]
            .get("modelSelectionExplicit")
            .is_none()
    );
    assert_eq!(
        partialize_state(&state, &BTreeSet::new())["stickyModelSelectionByProvider"]["secondary"]["model"],
        "new"
    );
}
#[test]
fn reconciliation_requires_live_authoritative_agreement_and_no_pending_command() {
    use t3_client::thread_views::composer::mobile_models::MobileModelPicks;
    let mut document = json!({"schemaVersion":1,"drafts":{}});
    let mut picks = MobileModelPicks::default();
    let pending = pick("chosen", Some(json!([{"id":"fastMode","value":false}])));
    let server = pick("chosen", None);
    let old = pick("old", None);
    let first = picks.set(&mut document, "e:t", pending.clone());
    for (live, in_flight, authoritative) in [
        (false, false, &server),
        (true, true, &server),
        (true, false, &old),
    ] {
        assert!(!picks.reconcile(
            &mut document,
            "e:t",
            &first,
            Observation {
                live,
                pending: in_flight,
                visible: Some(&server),
                authoritative: Some(authoritative)
            }
        ));
    }
    let newer = picks.set(&mut document, "e:t", pending);
    assert!(!picks.reconcile(
        &mut document,
        "e:t",
        &first,
        Observation {
            live: true,
            pending: false,
            visible: Some(&server),
            authoritative: Some(&server)
        }
    ));
    assert!(picks.reconcile(
        &mut document,
        "e:t",
        &newer,
        Observation {
            live: true,
            pending: false,
            visible: Some(&server),
            authoritative: Some(&server)
        }
    ));
    assert!(document["drafts"].get("e:t").is_none());
    let seed = picks.set(&mut document, "new-task:draft", server.clone());
    assert!(!picks.reconcile(
        &mut document,
        "new-task:draft",
        &seed,
        Observation {
            live: true,
            pending: false,
            visible: Some(&server),
            authoritative: Some(&server)
        }
    ));
}
#[test]
fn mobile_option_memory_keeps_explicit_false_and_preserves_unchanged_pick_identity() {
    use std::rc::Rc;
    use t3_client::thread_views::composer::mobile_models::{
        remember_options, with_remembered_options,
    };
    let mut document = json!({"schemaVersion":1,"drafts":{}});
    let remembered = pick("chosen", Some(json!([{"id":"fastMode","value":false}])));
    assert!(remember_options(&mut document, &remembered));
    let bare = Rc::new(pick("chosen", None));
    assert!(!remember_options(&mut document, &bare));
    let restored = with_remembered_options(&document, bare.clone());
    assert!(!Rc::ptr_eq(&bare, &restored));
    assert_eq!(*restored, remembered);
    assert!(Rc::ptr_eq(
        &restored,
        &with_remembered_options(&document, restored.clone())
    ));
    assert!(pick_matches_thread(&restored, &bare));
    assert!(!pick_matches_thread(
        &restored,
        &pick("chosen", Some(json!([{"id":"fastMode","value":true}])))
    ));
}
