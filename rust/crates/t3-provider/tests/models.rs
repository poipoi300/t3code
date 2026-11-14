#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_contracts::{model::ProviderOptionDescriptor, server::ServerProviderAuthStatus};
use t3_provider::{catalog::descriptors, environment::Environment, models::*};

#[test]
fn cursor_parameter_options_are_stably_ordered_mapped_and_deduplicated() {
    let models=cursor_models(&[json!({"id":"opus","displayName":"Opus","parameters":[{"id":"fast","values":[{"value":"true"},{"value":"false"}]},{"id":"context","displayName":"Context","values":[{"value":"200k"},{"value":"1m"}]},{"id":"effort","values":[{"value":"low"},{"value":"high"}]},{"id":"fastMode","values":[{"value":"ignored"}]}],"variants":[{"isDefault":true,"params":[{"id":"fast","value":"true"},{"id":"effort","value":"high"}]}]}),json!({"id":"opus","displayName":"Duplicate"}),json!({"id":"  ","displayName":"Blank"})]).unwrap();
    assert_eq!(models.len(), 1);
    let options = descriptors(models[0].capabilities.as_ref().unwrap());
    assert!(
        matches!(&options[0],ProviderOptionDescriptor::Select(d) if d.id == "effort" && d.current_value == Some(Some("high".into())))
    );
    assert!(matches!(&options[1],ProviderOptionDescriptor::Select(d) if d.id == "contextWindow"));
    assert!(
        matches!(&options[2],ProviderOptionDescriptor::Boolean(d) if d.id == "fastMode" && d.current_value == Some(Some(true)))
    );
    assert_eq!(options.len(), 3);
}

#[test]
fn muse_distinguishes_unknown_variants_from_an_explicit_empty_effort_list() {
    assert!(descriptors(&muse_capabilities(Some(&json!({"variants":[]})))).is_empty());
    for model in [None, Some(json!({"variants":"unknown"}))] {
        let caps = muse_capabilities(model.as_ref());
        assert!(
            matches!(&descriptors(&caps)[0],ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("high".into())))
        );
    }
    let catalog = json!({"providerId":"meta","models":[{"modelId":"muse-1-pro","displayLabel":"","providerId":"meta","isDefault":true,"variants":["ultra","bogus","low"],"defaultReasoningEffort":"ultra","reasoningEffortVariants":[{"tier":"ultra","description":"More reasoning"}]},{"modelId":"muse-1-pro","displayLabel":"Duplicate","providerId":"meta","isDefault":false},{"modelId":"foreign","displayLabel":"Foreign","providerId":"other","isDefault":false}]});
    for field in [
        "variants",
        "defaultReasoningEffort",
        "reasoningEffortVariants",
    ] {
        let mut invalid = catalog.clone();
        invalid["models"][0][field] = serde_json::Value::Null;
        assert!(
            muse_models(&invalid).is_err(),
            "{field} must reject explicit null"
        );
    }
    let mut invalid = catalog.clone();
    invalid["models"][0]["reasoningEffortVariants"][0]["description"] = serde_json::Value::Null;
    assert!(muse_models(&invalid).is_err());
    let models = muse_models(&catalog).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].name.0, "Muse 1 Pro");
    assert!(
        matches!(&descriptors(models[0].capabilities.as_ref().unwrap())[0],ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("ultra".into())) && d.options.len() == 2)
    );
    assert!(muse_models(&json!({"providerId":"other","models":[]})).is_err());
    assert!(muse_models(&json!({"providerId":"meta","models":[{"modelId":"m","displayLabel":"M","providerId":"meta","isDefault":true,"variants":[42]}]})).is_err());
}

#[test]
fn grok_effort_order_defaults_and_unfamiliar_tokens_follow_advertised_metadata() {
    let caps = grok_capabilities(
        &json!({"_meta":{"reasoningEffort":"high","reasoningEfforts":[{"value":"max","isDefault":true},{"value":"low"},{"value":"high","default":true},{"value":"high"},{"value":"invalid token"}]}}),
    );
    assert!(
        matches!(&descriptors(&caps)[0],ProviderOptionDescriptor::Select(d) if d.options.iter().map(|o|o.id.as_str()).collect::<Vec<_>>() == ["low","high","max"] && d.current_value == Some(Some("high".into())) && d.options[1].is_default == Some(Some(true)) && d.options[2].is_default.is_none())
    );
    let custom = grok_capabilities(
        &json!({"_meta":{"reasoningEfforts":[{"value":"high"},{"value":"future-level"},{"value":"low"}]}}),
    );
    assert!(
        matches!(&descriptors(&custom)[0],ProviderOptionDescriptor::Select(d) if d.options[0].id == "high" && d.options[1].id == "future-level")
    );
    assert!(
        descriptors(&grok_capabilities(
            &json!({"_meta":{"supportsReasoningEffort":false}})
        ))
        .is_empty()
    );
}

#[test]
fn kiro_whoami_reads_first_line_and_never_infers_auth_from_a_failed_command() {
    let env = Environment::new();
    assert_eq!(kiro_auth_from_whoami(Some(0),Some("\u{feff}{\"email\":\"ada@example.com\",\"accountType\":\"IdentityCenter\"}\nProfile: work"),&env).unwrap().email.unwrap().unwrap().0,"ada@example.com");
    assert!(
        kiro_auth_from_whoami(
            Some(0),
            Some(r#"{"accountType":4,"email":"must-not-retain@example.invalid"}"#),
            &env
        )
        .unwrap()
        .email
        .is_none()
    );
    let auth = kiro_auth_from_whoami(
        Some(0),
        Some("{\"email\":\"ada@example.com\",\"accountType\":\"IdentityCenter\"}\nProfile: work"),
        &env,
    )
    .unwrap();
    assert_eq!(auth.status, ServerProviderAuthStatus::Authenticated);
    assert_eq!(auth.email.unwrap().unwrap().0, "ada@example.com");
    assert_eq!(
        kiro_auth_from_whoami(Some(1), Some("{\"account\":null}"), &env)
            .unwrap()
            .status,
        ServerProviderAuthStatus::Unauthenticated
    );
    let env = Environment::from([("KIRO_API_KEY".into(), "key".into())]);
    assert_eq!(
        kiro_auth_from_whoami(Some(1), Some("failed"), &env)
            .unwrap()
            .status,
        ServerProviderAuthStatus::Unknown
    );
    assert_eq!(
        kiro_auth_from_whoami(Some(0), Some("{}"), &env)
            .unwrap()
            .r#type
            .unwrap()
            .unwrap()
            .0,
        "api_key"
    );
}

#[test]
fn kiro_default_model_alias_has_no_effort_and_known_models_keep_exact_effort_defaults() {
    for invalid in [
        json!({"models":[{"model_id":"future","model_name":null}]}),
        json!({"models":[{"model_id":"future"}],"default_model":null}),
    ] {
        assert!(
            kiro_models_from_list(0, &invalid.to_string())
                .unwrap()
                .is_none()
        );
    }
    let models=kiro_models_from_list(0,&json!({"default_model":"claude-opus-5.5","models":[{"model_id":"claude-opus-5.5","model_name":"Opus"},{"model_id":"claude-opus-4.7"}]}).to_string()).unwrap().unwrap();
    assert_eq!(models[0].slug.0, "default");
    assert!(descriptors(models[0].capabilities.as_ref().unwrap()).is_empty());
    assert_eq!(
        models[0].aliases.as_ref().unwrap().as_ref().unwrap()[0].0,
        "claude-opus-5.5"
    );
    assert!(
        matches!(&descriptors(models[1].capabilities.as_ref().unwrap())[0],ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("xhigh".into())))
    );
    let models = kiro_models_from_list(0, "{\"models\":[{\"model_id\":\"future\"}]}")
        .unwrap()
        .unwrap();
    assert_eq!(models[0].slug.0, "default");
    assert_eq!(models.len(), 2);
    assert!(descriptors(models[1].capabilities.as_ref().unwrap()).is_empty());
    assert!(kiro_models_from_list(1, "{}").unwrap().is_none());
}

#[test]
fn kiro_agent_picker_requires_multiple_choices_and_preserves_descriptions() {
    assert!(kiro_agent_descriptor(&json!({"configOptions":[{"id":"mode","type":"select","options":[{"value":"one","name":"One"},{"value":"two","name":"Two"}]},{"id":3,"type":"boolean"}]})).is_none());
    assert!(kiro_agent_descriptor(&json!({"configOptions":[{"id":"mode","type":"select","options":[{"value":"default","name":"Default"}]}]})).is_none());
    let descriptor=kiro_agent_descriptor(&json!({"configOptions":[{"id":"mode","type":"select","currentValue":"work","options":[{"value":"default","name":"Default"},{"value":" work ","name":"Work","description":" Company agent "}]}]})).unwrap();
    assert!(
        matches!(descriptor,ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("work".into())) && d.options[1].description == Some(Some("Company agent".into())))
    );
}

#[test]
fn cursor_sdk_selection_fills_catalog_defaults_but_only_sends_fast_when_selected() {
    let models=cursor_models(&[json!({"id":"grok-4.5","displayName":"Grok 4.5","parameters":[{"id":"effort","values":[{"value":"low"},{"value":"high"}]},{"id":"context","values":[{"value":"1m"}]},{"id":"fast","values":[{"value":"false"},{"value":"true"}]}],"variants":[{"isDefault":true,"params":[{"id":"effort","value":"high"},{"id":"context","value":"1m"},{"id":"fast","value":"true"}]}]})]).unwrap();
    for (options, expected) in [
        (
            json!([]),
            json!([{"id":"effort","value":"high"},{"id":"context","value":"1m"},{"id":"fast","value":"false"}]),
        ),
        (
            json!([{"id":"effort","value":"low"},{"id":"fastMode","value":true}]),
            json!([{"id":"effort","value":"low"},{"id":"fast","value":"true"},{"id":"context","value":"1m"}]),
        ),
    ] {
        let selection = serde_json::from_value(
            json!({"instanceId":"cursor","model":"grok-4.5","options":options}),
        )
        .unwrap();
        assert_eq!(
            cursor_sdk_model_selection(&selection, &models),
            json!({"id":"grok-4.5","params":expected})
        );
        assert_eq!(
            cursor_context_window_tokens(&selection, &models),
            Some(1_000_000)
        );
    }
    let selection = serde_json::from_value(json!({"instanceId":"cursor","model":"auto"})).unwrap();
    assert_eq!(
        cursor_sdk_model_selection(&selection, &models),
        json!({"id":"default"})
    );
    for (value, expected) in [
        (json!(" 256K "), Some(256_000)),
        (json!("2m"), Some(2_000_000)),
        (json!(true), None),
        (json!("0k"), None),
        (json!("2🙂"), None),
        (json!("9007199254740991m"), None),
    ] {
        let selection=serde_json::from_value(json!({"instanceId":"cursor","model":"missing","options":[{"id":"contextWindow","value":value}]})).unwrap();
        assert_eq!(cursor_context_window_tokens(&selection, &models), expected);
    }
}
