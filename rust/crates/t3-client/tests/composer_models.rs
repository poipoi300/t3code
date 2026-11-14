#![allow(clippy::unwrap_used)]
#[path = "support/fake_server.rs"]
mod fake_server;
use serde_json::json;
use t3_client::thread_views::composer::{
    model_search::{PickerSearchItem, rank_model_picker_search, score_model_picker_search},
    model_traits::{ModelTraitsInput, plan_model_trait_change, resolve_model_traits},
};
use t3_contracts::model::{
    ProviderOptionDescriptor, ProviderOptionSelection, ProviderOptionSelectionValue,
};

fn model_runtime() -> (
    fake_server::Harness,
    t3_client::composition::ClientRuntime<fake_server::TestPlatform>,
    t3_client::types::ThreadKey,
    serde_json::Value,
) {
    use futures::{FutureExt, StreamExt};
    use std::rc::Rc;
    let mut h = fake_server::Harness::new("env");
    h.server().state.borrow_mut().permissions = Some(vec![
        "orchestration:read".into(),
        "orchestration:operate".into(),
        "settings:write".into(),
        "providers:manage".into(),
    ]);
    let mut config = fake_server::config("env");
    config["settings"] =
        serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap();
    config["settings"]["favorites"] = json!([]);
    config["settings"]["providerModelPreferences"] = json!({});
    config["environment"]["capabilities"]["modelCatalogRefresh"] = json!(true);
    config["providers"] = json!([{"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":fake_server::NOW,"models":[{"slug":"gpt-5.4","name":"GPT","isCustom":false,"capabilities":null},{"slug":"alternate","name":"Alternate","isCustom":false,"capabilities":null}],"slashCommands":[],"skills":[]}]);
    let source = config.clone();
    h.server().state.borrow_mut().extra_rpc =
        Some(Rc::new(move |request| match request.tag.as_str() {
            "server.getConfig" => {
                let config = source.clone();
                Some(t3_rpc::HandlerOutput::Unary(
                    async move { t3_rpc::Exit::success(config) }.boxed_local(),
                ))
            }
            "subscribeServerConfig" => {
                let config = source.clone();
                Some(t3_rpc::HandlerOutput::Stream(
                    futures::stream::once(async move {
                        Ok(vec![json!({"version":1,"type":"snapshot","config":config})])
                    })
                    .chain(futures::stream::pending())
                    .boxed_local(),
                ))
            }
            _ => None,
        }));
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
    let key = t3_client::types::ThreadKey {
        environment: t3_client::types::EnvironmentId("env".into()),
        thread: t3_client::types::ThreadId("t1".into()),
    };
    runtime.open_thread(&key);
    h.settle();
    (h, runtime, key, config)
}

#[test]
fn favorites_publish_optimistically_roll_back_refusal_and_send_explicit_desired_state() {
    use futures::{FutureExt, StreamExt};
    use t3_contracts::provider_instance::ProviderInstanceId;
    let (mut h, runtime, key, config) = model_runtime();
    let models = runtime.composer_models(&key);
    let mut changes = models.changes();
    let _ = changes.next().now_or_never();
    h.server().state.borrow_mut().rpc_script.entry("server.updateSettings".into()).or_default().push_back(Err(json!({"_tag":"ServerSettingsError","settingsPath":"settings.json","operation":"normalize","message":"Favorites refused","detail":"read-only mount"})));
    let mut rejected =
        models.toggle_model_favorite(ProviderInstanceId("codex".into()), "gpt-5.4".into());
    assert!(rejected.as_mut().now_or_never().is_none());
    assert_eq!(models.snapshot().favorites.len(), 1);
    assert_eq!(changes.next().now_or_never(), Some(Some(())));
    let error = h.run(rejected).unwrap_err();
    assert!(matches!(error, t3_client::ClientError::Rejected { .. }));
    assert!(error.to_string().contains("read-only mount"));
    assert!(models.snapshot().favorites.is_empty());
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Ok(config["settings"].clone()));
    h.run(models.toggle_model_favorite(ProviderInstanceId("codex".into()), "gpt-5.4".into()))
        .unwrap();
    assert_eq!(models.snapshot().favorites.len(), 1);
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.updateSettings".into())
        .or_default()
        .push_back(Ok(config["settings"].clone()));
    h.run(models.toggle_model_favorite(ProviderInstanceId("codex".into()), "gpt-5.4".into()))
        .unwrap();
    assert!(models.snapshot().favorites.is_empty());
    let requests = h
        .server()
        .state
        .borrow()
        .rpc_calls
        .iter()
        .filter(|(tag, _)| tag == "server.updateSettings")
        .map(|(_, input)| input["patch"]["setModelFavorites"][0]["favorite"].clone())
        .collect::<Vec<_>>();
    assert_eq!(requests, vec![json!(true), json!(true), json!(false)]);
    let mut canceled =
        models.toggle_model_favorite(ProviderInstanceId("codex".into()), "gpt-5.4".into());
    assert!(canceled.as_mut().now_or_never().is_none());
    assert_eq!(models.snapshot().favorites.len(), 1);
    drop(canceled);
    assert!(models.snapshot().favorites.is_empty());
}

#[test]
fn model_mutations_keep_failed_picks_refresh_only_catalogs_and_recheck_permissions() {
    let (mut h, runtime, key, _) = model_runtime();
    let models = runtime.composer_models(&key);
    let selection = t3_contracts::model_selection::ModelSelection {
        instance_id: t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        model: "alternate".into(),
        options: None,
    };
    let error = h
        .run(models.set_model_selection(selection.clone()))
        .unwrap_err();
    assert!(matches!(error, t3_client::ClientError::Rejected { .. }));
    assert_eq!(models.snapshot().selection, selection);
    assert!(matches!(
        models.snapshot().state,
        t3_client::thread_views::composer::model_service::ModelCatalogState::Ready
    ));
    h.server()
        .state
        .borrow_mut()
        .rpc_script
        .entry("server.refreshProviders".into())
        .or_default()
        .push_back(Ok(json!({"providers":[]})));
    h.run(models.refresh_model_catalog()).unwrap();
    assert!(
        h.server()
            .state
            .borrow()
            .rpc_calls
            .iter()
            .any(|(tag, input)| tag == "server.refreshProviders"
                && input["modelCatalogOnly"] == true
                && input.get("refreshModels").is_none())
    );
    h.server().state.borrow_mut().permissions = Some(vec![]);
    h.run(runtime.reconnect(&key.environment)).unwrap();
    h.settle();
    let before = h.server().state.borrow().rpc_calls.len();
    assert!(matches!(
        h.run(models.set_model_selection(selection)),
        Err(t3_client::ClientError::PermissionDenied { .. })
    ));
    assert!(matches!(
        h.run(models.refresh_model_catalog()),
        Err(t3_client::ClientError::PermissionDenied { .. })
    ));
    assert_eq!(h.server().state.borrow().rpc_calls.len(), before);
}

fn model(name: &str, favorite: bool) -> PickerSearchItem {
    PickerSearchItem {
        driver_kind: "claudeAgent".into(),
        provider_display_name: "Claude".into(),
        name: name.into(),
        short_name: None,
        sub_provider: None,
        is_favorite: favorite,
    }
}

#[test]
fn model_mentions_port_word_boundaries_separators_and_file_link_encoding() {
    use t3_client::thread_views::composer::suggestions::{
        rank_model_mentions, serialize_file_link,
    };
    let models = vec![
        PickerSearchItem {
            name: "GPT-6.1".into(),
            driver_kind: "codex".into(),
            provider_display_name: "Traex".into(),
            ..Default::default()
        },
        PickerSearchItem {
            name: "GPT-6.1".into(),
            driver_kind: "opencode".into(),
            provider_display_name: "OpenCode".into(),
            sub_provider: Some("Traex".into()),
            ..Default::default()
        },
        PickerSearchItem {
            name: "Claude Opus 5.5".into(),
            driver_kind: "opencode".into(),
            provider_display_name: "OpenCode".into(),
            sub_provider: Some("Anthropic".into()),
            ..Default::default()
        },
        PickerSearchItem {
            name: "Claude Opus 5.5".into(),
            driver_kind: "claudeAgent".into(),
            provider_display_name: "Claude".into(),
            ..Default::default()
        },
    ];
    for query in ["", "-", "---", ":/-/:", "src", "g", "agent", "pus"] {
        assert!(rank_model_mentions(&models, query).is_empty(), "{query}");
    }
    for query in ["opencode:traex", "opencode/gpt", "opencode-gpt"] {
        assert_eq!(rank_model_mentions(&models, query), vec![1], "{query}");
    }
    let mut ranked = rank_model_mentions(&models, "gpt-6.1");
    ranked.sort();
    assert_eq!(ranked, vec![0, 1]);
    assert_eq!(
        serialize_file_link("src/a [b](c)#?.rs"),
        "[a \\[b\\](c)#?.rs](src/a%20%5Bb%5D%28c%29%23%3F.rs)"
    );
}

#[test]
fn suggestion_queries_and_selection_are_fenced_by_generation_and_current_source() {
    use t3_client::thread_views::composer::{
        policy::detect_trigger,
        suggestions::{SuggestionOutcome, SuggestionRequest},
    };
    let (mut h, runtime, key, _) = model_runtime();
    let drafts = runtime.composer_drafts();
    drafts.set_prompt(&key, "/mo".into(), 3).unwrap();
    let session = runtime.composer_suggestions(&key, None);
    let request = SuggestionRequest {
        trigger: detect_trigger("/mo", 3).unwrap(),
        generation: 1,
    };
    h.run(session.query(Some(request.clone()))).unwrap();
    assert_eq!(session.snapshot().unwrap().items[0].id, "slash:model");
    assert!(matches!(
        session.select(&request, "slash:model", "changed", 7),
        Err(t3_client::ClientError::Cancelled)
    ));
    assert!(matches!(
        session.select(&request, "slash:model", "/mo", 3).unwrap(),
        SuggestionOutcome::OpenModelPicker { .. }
    ));
    assert!(drafts.view(&key).prompt.is_empty());
    drafts.set_prompt(&key, "/mo".into(), 3).unwrap();
    let newer = SuggestionRequest {
        generation: 2,
        ..request.clone()
    };
    h.run(session.query(Some(newer))).unwrap();
    assert!(matches!(
        session.select(&request, "slash:model", "/mo", 3),
        Err(t3_client::ClientError::Cancelled)
    ));
    h.run(session.query(None)).unwrap();
    assert!(session.snapshot().is_none());
}

#[test]
fn model_search_ports_exact_fuzzy_favorite_and_catalog_tie_order() {
    let mut models = vec![
        model("Claude Opus 5.5", false),
        model("Claude Opus 5", false),
        model("Claude Opus 4.8", false),
        model("Claude Opus 4.7", false),
        model("Claude Opus 4.6", false),
        model("Claude Opus 4.5", false),
    ];
    assert_eq!(
        rank_model_picker_search(&models, "opus 4"),
        vec![2, 3, 4, 5]
    );
    assert_eq!(rank_model_picker_search(&models, "opus 5"), vec![0, 1, 5]);
    assert_eq!(rank_model_picker_search(&models, "opus 5."), vec![0]);
    assert_eq!(rank_model_picker_search(&models, "claude opus 5")[0], 1);
    models[5].is_favorite = true;
    assert_eq!(&rank_model_picker_search(&models, "opus")[..3], &[5, 0, 1]);
    let exact = model("Opus 4.7", false);
    assert!(
        score_model_picker_search(&exact, "opus 4.7").unwrap()
            < score_model_picker_search(&models[5], "opus 4.7").unwrap_or(i64::MAX)
    );
    let mut copilot = model("Claude Opus 4.7", false);
    copilot.sub_provider = Some("GitHub Copilot".into());
    assert!(
        score_model_picker_search(&copilot, "copilot opus").unwrap()
            < score_model_picker_search(&copilot, "coplt op").unwrap()
    );
    assert_eq!(
        rank_model_picker_search(&models, "absent"),
        Vec::<usize>::new()
    );
    assert_eq!(
        rank_model_picker_search(&models, "  "),
        vec![5, 0, 1, 2, 3, 4]
    );
}

#[test]
fn traits_port_pi_thinking_fallback_without_materializing_a_selection() {
    for (current, default, value, label, id) in [
        (None, None, None, "Thinking", "thinking"),
        (None, Some("high"), Some("high"), "High", "thinking"),
        (Some("low"), None, Some("low"), "Low", "thinking"),
        (Some("max"), Some("high"), Some("max"), "Max", "thinking"),
        (None, None, None, "", "effort"),
    ] {
        let mut raw = json!({"type":"select","id":id,"label":"Thinking","options":[{"id":"low","label":"Low"},{"id":"high","label":"High"},{"id":"max","label":"Max"}]});
        if let Some(current) = current {
            raw["currentValue"] = json!(current);
        }
        if let Some(default) = default {
            for option in raw["options"].as_array_mut().unwrap() {
                if option["id"] == default {
                    option["isDefault"] = json!(true);
                }
            }
        }
        let descriptors = vec![serde_json::from_value::<ProviderOptionDescriptor>(raw).unwrap()];
        let input = ModelTraitsInput {
            descriptors: &descriptors,
            selection: None,
            reported: None,
            prompt: "",
            allow_prompt_injected_effort: true,
            unavailable: false,
        };
        let view = resolve_model_traits(&input);
        assert_eq!(view.summary, label);
        assert_eq!(view.reasoning_value.as_deref(), value);
    }
}

#[test]
fn trait_change_rejects_body_ultrathink_and_strips_only_a_controlled_prefix() {
    let descriptors = vec![serde_json::from_value::<ProviderOptionDescriptor>(json!({"type":"select","id":"effort","label":"Effort","currentValue":"high","promptInjectedValues":["ultrathink"],"options":[{"id":"low","label":"Low"},{"id":"high","label":"High"},{"id":"ultrathink","label":"Ultrathink"}]})).unwrap()];
    let change = ProviderOptionSelection {
        id: "effort".into(),
        value: ProviderOptionSelectionValue::String("low".into()),
    };
    let mut input = ModelTraitsInput {
        descriptors: &descriptors,
        selection: None,
        reported: None,
        prompt: "Ultrathink:\nquestion",
        allow_prompt_injected_effort: true,
        unavailable: false,
    };
    let planned = plan_model_trait_change(&input, change.clone()).unwrap();
    assert_eq!(planned.prompt, "question");
    input.prompt = "Keep ultrathink in this body";
    assert!(plan_model_trait_change(&input, change).is_none());
}
