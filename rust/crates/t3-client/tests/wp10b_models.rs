#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use t3_client::view_logic::shell::{models::*, skill_search::*};
use t3_contracts::{
    base_schemas::{EnvironmentId, ProjectId},
    model_selection::ModelSelection,
    provider_instance::ProviderInstanceId,
    server::*,
    settings::ServerSettings,
};
fn provider(id: &str, driver: &str, models: &[&str]) -> ServerProvider {
    serde_json::from_value(json!({"instanceId":id,"driver":driver,"enabled":true,"installed":true,"version":null,"status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-01-01T00:00:00.000Z","models":models.iter().map(|s|json!({"slug":s,"name":s,"isCustom":false,"capabilities":{}})).collect::<Vec<_>>(),"slashCommands":[],"skills":[]})).unwrap()
}
fn settings(v: serde_json::Value) -> ServerSettings {
    serde_json::from_value(v).unwrap()
}
fn selection(id: &str, model: &str) -> ModelSelection {
    serde_json::from_value(json!({"instanceId":id,"model":model})).unwrap()
}
#[test]
fn instance_readiness_availability_error_and_requested_selection() {
    let mut profiles = vec![
        provider("disabled", "codex", &["m"]),
        provider("warning", "codex", &["w"]),
        provider("ready", "codex", &["r"]),
        provider("unavailable", "codex", &["u"]),
    ];
    profiles[0].enabled = false;
    profiles[1].status = ServerProviderState::Error;
    profiles[3].availability = Some(Some(ServerProviderAvailability::Unavailable));
    let entries = derive_entries(&profiles);
    assert!(!entries[0].ready());
    assert!(!entries[0].visible());
    assert_eq!(
        selectable_entry(&entries, None)
            .unwrap()
            .snapshot
            .instance_id
            .0,
        "ready"
    );
    assert_eq!(
        selectable_entry(&entries, Some(&ProviderInstanceId("warning".into())))
            .unwrap()
            .snapshot
            .instance_id
            .0,
        "warning"
    );
    assert_eq!(
        default_selection(&profiles, Some(&selection("missing", "x")))
            .unwrap()
            .model,
        "r"
    );
    let mut saved = selection("warning", "custom");
    saved.options = Some(serde_json::from_value(json!([{"id":"effort","value":"high"}])).unwrap());
    assert_eq!(default_selection(&profiles, Some(&saved)), Some(saved));
    assert!(default_selection(&profiles[..2], None).is_none());
    assert!(default_selection(&[], None).is_none());
}
#[test]
fn project_settings_override_streamed_enabled_flags_and_default_group_order() {
    let providers = [
        provider("custom", "claudeAgent", &["custom-m"]),
        provider("claudeAgent", "claudeAgent", &["stock"]),
        provider("codex", "codex", &["gpt"]),
    ];
    let settings = settings(
        json!({"providerInstances":{"claudeAgent":{"driver":"claudeAgent","enabled":false},"custom":{"driver":"claudeAgent","enabled":true}},"projectSettingsOverrides":{"p":{"providerInstances":{"claudeAgent":{"enabled":true}}}}}),
    );
    let entries = entries_for(&providers, &settings, None);
    assert_eq!(
        entries
            .iter()
            .map(|e| e.snapshot.instance_id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["claudeAgent", "custom", "codex"]
    );
    assert!(!entries[0].enabled);
    assert_eq!(
        project_default(&providers, &settings, None)
            .unwrap()
            .instance_id
            .0,
        "custom"
    );
    assert_eq!(
        project_default(&providers, &settings, Some(&ProjectId("p".into())))
            .unwrap()
            .instance_id
            .0,
        "claudeAgent"
    );
    assert!(!project_instance_enabled(
        &settings,
        None,
        &ProviderInstanceId("unknown".into())
    ));
    assert!(project_instance_enabled(
        &settings,
        None,
        &ProviderInstanceId("codex".into())
    ));
}
#[test]
fn instance_branding_stays_scoped_to_environment_and_local_config_clears_registry_icons() {
    let p = [provider("constructor", "acpRegistry", &[])];
    let a = settings(
        json!({"providerInstances":{"constructor":{"driver":"acpRegistry","config":{"agentId":" first ","registryIconUrl":" https://a "}}}}),
    );
    let b = settings(
        json!({"providerInstances":{"constructor":{"driver":"acpRegistry","config":{"agentId":"second","registryIconUrl":"https://b"}}}}),
    );
    let e1 = EnvironmentId("a".into());
    let e2 = EnvironmentId("b".into());
    let entries =
        entries_by_environment([(&e1, p.as_slice(), Some(&a)), (&e2, p.as_slice(), Some(&b))]);
    assert_eq!(
        entries[&e1][&p[0].instance_id].acp_agent.as_deref(),
        Some("first")
    );
    assert_eq!(
        entries[&e2][&p[0].instance_id].acp_agent.as_deref(),
        Some("second")
    );
    let local = settings(
        json!({"providerInstances":{"constructor":{"driver":"acpRegistry","config":{"source":"local","agentId":"old","registryIconUrl":"old"}}}}),
    );
    assert_eq!(entries_for(&p, &local, None)[0].acp_agent, None);
}
#[test]
fn instance_defaults_use_own_catalog_and_declared_default() {
    let mut p = provider("custom", "claudeAgent", &["first", "second"]);
    p.models[1].is_default = Some(Some(true));
    assert_eq!(
        default_instance_model(std::slice::from_ref(&p), &p.instance_id).as_deref(),
        Some("second")
    );
    p.models.clear();
    assert_eq!(
        default_instance_model(std::slice::from_ref(&p), &p.instance_id).as_deref(),
        Some("claude-opus-5-5")
    );
    assert!(default_instance_model(&[], &p.instance_id).is_none());
}
#[test]
fn custom_models_are_instance_scoped_not_limited_to_32_and_custom_aliases_win() {
    let mut server = settings(
        json!({"providerInstances":{"claudeAgent":{"driver":"claudeAgent","config":{"customModels":[]}},"custom":{"driver":"claudeAgent","config":{"customModels":["5.4",{"slug":" duplicate ","name":" Friendly "},"duplicate"]}}}}),
    );
    let stock = provider("claudeAgent", "claudeAgent", &["stock"]);
    let custom = provider("custom", "claudeAgent", &["builtin"]);
    let s = Settings {
        server: &server,
        legacy_preferences: None,
    };
    assert_eq!(options_for(&s, &stock, None).len(), 1);
    let options = options_for(&s, &custom, None);
    assert_eq!(
        options.iter().map(|o| o.slug.as_str()).collect::<Vec<_>>(),
        vec!["builtin", "5.4", "duplicate"]
    );
    assert_eq!(options[2].name, "Friendly");
    assert_eq!(
        resolve_for_instance(
            &s,
            std::slice::from_ref(&custom),
            &custom.instance_id,
            Some("5.4"),
            false
        )
        .as_deref(),
        Some("5.4")
    );
    server
        .provider_instances
        .get_mut(&custom.instance_id)
        .unwrap()
        .config =
        Some(json!({"customModels":(0..70).map(|i|format!("model-{i}")).collect::<Vec<_>>()}));
    let s = Settings {
        server: &server,
        legacy_preferences: None,
    };
    assert_eq!(options_for(&s, &custom, None).len(), 71);
    assert_eq!(
        resolve_for_instance(
            &s,
            std::slice::from_ref(&custom),
            &custom.instance_id,
            Some("model-69"),
            false
        )
        .as_deref(),
        Some("model-69")
    );
}
#[test]
fn stale_server_custom_models_hidden_models_and_preference_order() {
    let mut p = provider("codex", "codex", &["one", "two", "old-custom"]);
    p.models[2].is_custom = true;
    let server = settings(
        json!({"providerInstances":{"codex":{"driver":"codex","config":{"customModels":["new-custom"]}}},"providerModelPreferences":{"codex":{"hiddenModels":["one","new-custom"],"modelOrder":["new-custom","two"]}}}),
    );
    let s = Settings {
        server: &server,
        legacy_preferences: None,
    };
    let options = options_for(&s, &p, None);
    assert_eq!(
        options.iter().map(|o| o.slug.as_str()).collect::<Vec<_>>(),
        vec!["new-custom", "two"]
    );
    assert_eq!(
        resolve_for_instance(
            &s,
            std::slice::from_ref(&p),
            &p.instance_id,
            Some("one"),
            false
        )
        .as_deref(),
        Some("new-custom")
    );
    assert!(!options.iter().any(|o| o.slug == "old-custom"));
}
#[test]
fn dynamic_catalog_removal_preserves_only_explicit_unhidden_choices() {
    for driver in ["opencode", "cursor", "antigravity"] {
        let p = provider(driver, driver, &["current"]);
        let server = settings(
            json!({"providerModelPreferences":{driver:{"hiddenModels":["hidden"]}},"providerInstances":{driver:{"driver":driver,"config":{"customModels":["ignored"]}}}}),
        );
        let s = Settings {
            server: &server,
            legacy_preferences: None,
        };
        assert_eq!(
            resolve_for_instance(
                &s,
                std::slice::from_ref(&p),
                &p.instance_id,
                Some("missing"),
                true
            )
            .as_deref(),
            Some("missing")
        );
        assert_eq!(
            resolve_for_instance(
                &s,
                std::slice::from_ref(&p),
                &p.instance_id,
                Some("missing"),
                false
            )
            .as_deref(),
            Some("current")
        );
        assert!(
            !options_for(&s, &p, Some("hidden"))
                .iter()
                .any(|o| o.slug == "hidden")
        );
        assert!(
            options_for(&s, &p, Some("missing"))
                .last()
                .unwrap()
                .is_unavailable
        );
        if driver == "antigravity" {
            assert!(
                !options_for(&s, &p, None)
                    .iter()
                    .any(|o| o.slug == "ignored")
            );
            assert_eq!(
                resolve_for_instance(
                    &s,
                    std::slice::from_ref(&p),
                    &p.instance_id,
                    Some("antigravity-default"),
                    true
                )
                .as_deref(),
                Some("current")
            );
            let mut empty = p.clone();
            empty.models.clear();
            assert_eq!(
                resolve_for_instance(
                    &s,
                    std::slice::from_ref(&empty),
                    &empty.instance_id,
                    Some("antigravity-default"),
                    true
                ),
                None
            );
        }
    }
    let p = provider("codex", "codex", &["stock"]);
    let server = settings(json!({}));
    assert_eq!(
        resolve_for_instance(
            &Settings {
                server: &server,
                legacy_preferences: None
            },
            std::slice::from_ref(&p),
            &p.instance_id,
            Some("missing"),
            true
        )
        .as_deref(),
        Some("stock")
    );
}
#[test]
fn effective_defaults_keep_options_and_mixed_automatic_explicit_choices_distinct() {
    let a = provider("codex", "codex", &["one"]);
    let b = provider("claudeAgent", "claudeAgent", &["two"]);
    let server = settings(json!({}));
    assert!(defaults_mixed(
        None,
        &[
            DefaultTarget {
                project: None,
                settings: &server,
                providers: std::slice::from_ref(&a)
            },
            DefaultTarget {
                project: None,
                settings: &server,
                providers: std::slice::from_ref(&b)
            }
        ]
    ));
    let explicit = settings(json!({"defaultModelSelection":{"instanceId":"codex","model":"one"}}));
    assert!(defaults_mixed(
        None,
        &[
            DefaultTarget {
                project: None,
                settings: &server,
                providers: std::slice::from_ref(&a)
            },
            DefaultTarget {
                project: None,
                settings: &explicit,
                providers: std::slice::from_ref(&a)
            }
        ]
    ));
    assert!(!defaults_mixed(None, &[]));
    let original: ModelSelection = serde_json::from_value(
        json!({"instanceId":"codex","model":"gone","options":[{"id":"effort","value":"high"}]}),
    )
    .unwrap();
    let result = effective_default(
        &Settings {
            server: &server,
            legacy_preferences: None,
        },
        &[a],
        &original,
    );
    assert_eq!(result.model, "one");
    assert_eq!(result.options, original.options);
}
#[test]
fn text_generation_uses_cheap_defaults_and_excludes_incapable_instances_and_models() {
    let mut p = provider("codex", "codex", &["chat", "gpt-6-luna", "extension"]);
    p.models[0].is_default = Some(Some(true));
    p.models[2].supports_text_generation = Some(Some(false));
    let mut disabled = provider("incapable", "claudeAgent", &["x"]);
    disabled.supports_text_generation = Some(Some(false));
    let server = settings(
        json!({"textGenerationModelSelection":{"instanceId":"incapable","model":"x"},"providerInstances":{"codex":{"driver":"codex","config":{"customModels":[{"slug":"extension","name":"renamed"},"unknown"]}}}}),
    );
    let s = Settings {
        server: &server,
        legacy_preferences: None,
    };
    let result = text_generation_selection(&s, &[disabled, p.clone()]);
    assert_eq!(result.instance_id.0, "codex");
    assert_eq!(result.model, "gpt-6-luna");
    let options = options_by_instance(&s, &[p], None, true);
    let options = &options[&ProviderInstanceId("codex".into())];
    assert!(!options.iter().any(|o| o.slug == "extension"));
    assert!(options.iter().any(|o| o.slug == "unknown"));
    assert_eq!(text_generation_selection(&s, &[]), no_provider_selection());
}
#[test]
fn text_generation_plan_choice_and_dispatch_normalization() {
    let mut p = provider("codex", "codex", &["m"]);
    p.models[0].capabilities=Some(serde_json::from_value(json!({"optionDescriptors":[{"type":"select","id":"agent","label":"Agent","options":[{"id":"build","label":"Build","isDefault":true},{"id":"plan","label":"Plan"}]},{"type":"select","id":"effort","label":"Effort","options":[{"id":"low","label":"Low","isDefault":true},{"id":"high","label":"High"}]},{"type":"boolean","id":"fastMode","label":"Fast","currentValue":true}]})).unwrap());
    let server = settings(
        json!({"textGenerationModelSelection":{"instanceId":"codex","model":"m","options":[{"id":"agent","value":"plan"},{"id":"effort","value":"unsupported"},{"id":"unknown","value":"keep?"}]}}),
    );
    let result = text_generation_selection(
        &Settings {
            server: &server,
            legacy_preferences: None,
        },
        std::slice::from_ref(&p),
    );
    let options = result.options.unwrap();
    assert!(selects_plan(Some(&options)));
    assert!(
        options
            .0
            .iter()
            .any(|o| o.id == "effort" && serde_json::to_value(&o.value).unwrap() == "low")
    );
    assert!(!options.0.iter().any(|o| o.id == "unknown"));
    assert!(
        options
            .0
            .iter()
            .any(|o| o.id == "fastMode" && serde_json::to_value(&o.value).unwrap() == false)
    );
    let caps = model_capabilities(&p, Some("m"), false);
    let descriptors = caps.option_descriptors.flatten().unwrap();
    let t3_contracts::model::ProviderOptionDescriptor::Select(agent) = &descriptors[0] else {
        panic!()
    };
    assert_eq!(agent.options.len(), 1);
}
#[test]
fn missing_dynamic_catalog_dispatch_keeps_explicit_options() {
    let p = provider("cursor", "cursor", &[]);
    let options = serde_json::from_value(
        json!([{"id":"custom","value":"yes"},{"id":"agent","value":"plan"}]),
    )
    .unwrap();
    assert_eq!(
        dispatch_options(&p, "gone", Some(&options), false)
            .unwrap()
            .0
            .len(),
        1
    );
    assert_eq!(
        dispatch_options(&p, "gone", Some(&options), true),
        Some(options)
    );
}
#[test]
fn model_order_favorites_short_names_and_picker_mounts() {
    let mut items = vec![
        ModelOption {
            slug: "b".into(),
            ..Default::default()
        },
        ModelOption {
            slug: "a".into(),
            ..Default::default()
        },
        ModelOption {
            slug: "c".into(),
            ..Default::default()
        },
    ];
    sort_models(
        &mut items,
        |o| &o.slug,
        &["c".into()],
        &BTreeSet::from(["a".into()]),
        true,
    );
    assert_eq!(
        items.iter().map(|o| o.slug.as_str()).collect::<Vec<_>>(),
        vec!["a", "c", "b"]
    );
    for (name, driver, expected) in [
        ("Claude Sonnet 4.6", "claudeAgent", Some("Sonnet 4.6")),
        ("GPT-6", "codex", None),
        ("Gemini 4 Pro", "codex", None),
        ("Vendor: Claude Sonnet 4.6", "codex", Some("Sonnet 4.6")),
    ] {
        assert_eq!(short_name(name, None, driver).as_deref(), expected);
    }
    let visibility = PickerVisibility::default();
    let a = visibility.register();
    let b = visibility.register();
    assert!(visibility.is_open());
    assert_eq!(visibility.revision(), 1);
    drop(a);
    assert!(visibility.is_open());
    drop(b);
    assert!(!visibility.is_open());
    assert_eq!(visibility.revision(), 2);
}
#[test]
fn skill_currency_aliases_fuzzy_ranking_and_first_invocable_definition() {
    let make = |name: &str, path: &str| {
        serde_json::from_value::<ServerProviderSkill>(
            json!({"name":name,"path":path,"enabled":true}),
        )
        .unwrap()
    };
    let review = make("review", "/review");
    for prefix in ["$", "€", "£", "¥", "₹", "₩", "₿", "𑿝"] {
        assert_eq!(
            search_skills(
                std::slice::from_ref(&review),
                &format!("{prefix}review"),
                Some(1),
                |a, b| a.cmp(b)
            ),
            vec![&review]
        );
    }
    let skills = [
        make("gh-fix-ci", "/1"),
        make("github", "/2"),
        make("agent-browser", "/3"),
        make("gh-fix-ci", "/4"),
    ];
    assert_eq!(
        search_skills(&skills, "gfc", None, |a, b| a.cmp(b)),
        vec![&skills[0]]
    );
    assert_eq!(
        search_skills(&skills, "", Some(1), |a, b| a.cmp(b)).len(),
        3
    );
    assert!(search_skills(&skills, "g", Some(0), |a, b| a.cmp(b)).is_empty());
}
#[test]
fn composer_catalog_refresh_draft_thread_precedence_and_optionless_threads() {
    let config = settings(json!({}));
    let settings = Settings {
        server: &config,
        legacy_preferences: None,
    };
    for (driver, catalog, missing, expected) in [
        (
            "claudeAgent",
            "claude-sonnet-4-6",
            "removed-model",
            "claude-sonnet-4-6",
        ),
        ("cursor", "new", "stored-old", "stored-old"),
        ("opencode", "new", "stored-old", "stored-old"),
        ("antigravity", "new", "stored-old", "stored-old"),
    ] {
        let providers = [provider(driver, driver, &[catalog])];
        let id = ProviderInstanceId(driver.into());
        let mut saved = selection(driver, missing);
        saved.options =
            Some(serde_json::from_value(json!([{"id":"effort","value":"high"}])).unwrap());
        let drafts = BTreeMap::from([(id.clone(), saved.clone())]);
        for as_draft in [true, false] {
            let result = composer_model_state(ComposerModelInput {
                draft_selections: as_draft.then_some(&drafts),
                providers: &providers,
                driver,
                selected_instance: Some(&id),
                requested_instance: Some(&id),
                thread_selection: (!as_draft).then_some(&saved),
                project_selection: None,
                settings: &settings,
            });
            assert_eq!(result.selected_model, expected, "{driver} draft={as_draft}");
            assert_eq!(result.options[&id], saved.options.clone().unwrap());
        }
    }
    let providers = [provider("codex", "codex", &["gpt-5.5"])];
    let id = ProviderInstanceId("codex".into());
    let thread = selection("codex", "gpt-5.5");
    let mut project = thread.clone();
    project.options =
        Some(serde_json::from_value(json!([{"id":"effort","value":"high"}])).unwrap());
    for exists in [false, true] {
        let result = composer_model_state(ComposerModelInput {
            draft_selections: None,
            providers: &providers,
            driver: "codex",
            selected_instance: Some(&id),
            requested_instance: Some(&id),
            thread_selection: exists.then_some(&thread),
            project_selection: Some(&project),
            settings: &settings,
        });
        assert_eq!(result.options.is_empty(), exists);
    }
}
#[test]
fn composer_antigravity_empty_accounts_and_custom_instance_drafts_do_not_borrow_catalogs() {
    let config = settings(
        json!({"providerInstances":{"custom":{"driver":"claudeAgent","enabled":true,"config":{"customModels":["proxy/model"]}}}}),
    );
    let settings = Settings {
        server: &config,
        legacy_preferences: None,
    };
    let providers = [
        provider("antigravity", "antigravity", &[]),
        provider("claudeAgent", "claudeAgent", &["claude-sonnet-4-6"]),
        provider("custom", "claudeAgent", &["claude-opus-4-6"]),
    ];
    let id = ProviderInstanceId("antigravity".into());
    let state = composer_model_state(ComposerModelInput {
        draft_selections: None,
        providers: &providers,
        driver: "antigravity",
        selected_instance: Some(&id),
        requested_instance: Some(&id),
        thread_selection: None,
        project_selection: None,
        settings: &settings,
    });
    assert_eq!(state.selected_model, "");
    let default = ProviderInstanceId("claudeAgent".into());
    let custom = ProviderInstanceId("custom".into());
    let drafts = BTreeMap::from([
        (
            default.clone(),
            selection("claudeAgent", "claude-sonnet-4-6"),
        ),
        (custom.clone(), selection("custom", "proxy/model")),
    ]);
    let state = composer_model_state(ComposerModelInput {
        draft_selections: Some(&drafts),
        providers: &providers,
        driver: "claudeAgent",
        selected_instance: Some(&custom),
        requested_instance: Some(&custom),
        thread_selection: None,
        project_selection: None,
        settings: &settings,
    });
    assert_eq!(state.selected_model, "proxy/model");
    let drafts = BTreeMap::from([(default, selection("claudeAgent", "claude-sonnet-4-6"))]);
    let state = composer_model_state(ComposerModelInput {
        draft_selections: Some(&drafts),
        providers: &providers,
        driver: "claudeAgent",
        selected_instance: Some(&custom),
        requested_instance: Some(&custom),
        thread_selection: None,
        project_selection: None,
        settings: &settings,
    });
    assert_eq!(state.selected_model, "claude-opus-4-6");
}
#[test]
fn fallback_instance_options_follow_model_identity_without_overwriting_own_choices() {
    let config = settings(
        json!({"providerInstances":{"requested":{"driver":"claudeAgent","enabled":false,"config":{"customModels":["sonnet"]}},"fallback":{"driver":"claudeAgent","enabled":true}}}),
    );
    let settings = Settings {
        server: &config,
        legacy_preferences: None,
    };
    let providers = [
        provider("requested", "claudeAgent", &["claude-sonnet-4-6"]),
        provider("fallback", "claudeAgent", &["claude-sonnet-4-6"]),
        provider("codex", "codex", &["gpt-5.5"]),
    ];
    let requested = ProviderInstanceId("requested".into());
    let fallback = ProviderInstanceId("fallback".into());
    let mut source = selection("requested", "claude-sonnet-4-6");
    source.options = Some(serde_json::from_value(json!([{"id":"effort","value":"high"}])).unwrap());
    for (model, own, carry) in [
        ("claude-sonnet-4-6", false, true),
        ("claude-opus-4-6", false, false),
        ("sonnet", false, false),
        ("claude-sonnet-4-6", true, false),
    ] {
        let mut source = source.clone();
        source.model = model.into();
        let mut drafts = BTreeMap::from([(requested.clone(), source.clone())]);
        if own {
            let mut own = selection("fallback", "claude-sonnet-4-6");
            own.options =
                Some(serde_json::from_value(json!([{"id":"effort","value":"low"}])).unwrap());
            drafts.insert(fallback.clone(), own);
        }
        let state = composer_model_state(ComposerModelInput {
            draft_selections: Some(&drafts),
            providers: &providers,
            driver: "claudeAgent",
            selected_instance: Some(&fallback),
            requested_instance: Some(&requested),
            thread_selection: None,
            project_selection: None,
            settings: &settings,
        });
        assert_eq!(
            state.options.get(&fallback) == source.options.as_ref(),
            carry,
            "{model} own={own}"
        );
        if own {
            assert_eq!(
                serde_json::to_value(&state.options[&fallback]).unwrap()[0]["value"],
                "low"
            );
        }
    }
    let drafts = BTreeMap::from([(requested.clone(), source)]);
    let id = ProviderInstanceId("codex".into());
    let state = composer_model_state(ComposerModelInput {
        draft_selections: Some(&drafts),
        providers: &providers,
        driver: "codex",
        selected_instance: Some(&id),
        requested_instance: Some(&requested),
        thread_selection: None,
        project_selection: None,
        settings: &settings,
    });
    assert!(!state.options.contains_key(&id));
}
