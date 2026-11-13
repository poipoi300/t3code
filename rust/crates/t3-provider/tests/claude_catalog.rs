#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_provider::{catalog::Manifest, claude_catalog::*};
#[test]
fn aliases_runtime_effort_and_context_suffixes_follow_the_manifest_and_keep_unknown_models_opaque()
{
    let manifest=Manifest::parse(&json!({"version":1,"currentModels":{},"providers":{"claudeAgent":{"profiles":{"synthetic":{"capabilities":{"optionDescriptors":[{"id":"effort","label":"Reasoning","type":"select","options":[{"id":"extreme","label":"Extreme","isDefault":true}]},{"id":"contextWindow","label":"Context","type":"select","options":[{"id":"large","label":"Large","isDefault":true},{"id":"small","label":"Small"}]}]},"adapter":{"claudeCode":{"effortMap":{"extreme":"high","none":null},"modelSuffixes":{"contextWindow":{"large":"[large]"}},"contextWindowTokens":{"large":1000000,"small":32000}}}},"fixed":{"adapter":{"claudeCode":{"fixedContextWindowTokens":64000}}}},"models":[{"slug":"claude-synthetic-next","name":"Synthetic","status":"current","profile":"synthetic","aliases":["synthetic"]},{"slug":"fixed","name":"Fixed","status":"current","profile":"fixed"}]}}}).to_string()).unwrap();
    let catalog = manifest.catalog("claudeAgent", 0).unwrap();
    assert_eq!(resolve_slug(&catalog, "SYNTHETIC"), "claude-synthetic-next");
    assert_eq!(
        resolve_reported_slug(&catalog, "claude-synthetic-next-20261113"),
        "claude-synthetic-next"
    );
    assert_eq!(
        resolve_effort(&catalog, Some("synthetic"), Some("bogus")),
        Some("extreme".into())
    );
    assert_eq!(
        normalize_effort(&catalog, "synthetic", Some("extreme")),
        Some("high".into())
    );
    assert_eq!(normalize_effort(&catalog, "synthetic", Some("none")), None);
    let mut selection =
        serde_json::from_value(json!({"instanceId":"claudeAgent","model":"synthetic"})).unwrap();
    assert_eq!(
        api_model_id(&catalog, &selection),
        "claude-synthetic-next[large]"
    );
    assert_eq!(
        context_window_tokens(&catalog, Some(&selection)),
        Some(1000000.)
    );
    selection.options =
        Some(serde_json::from_value(json!([{"id":"contextWindow","value":"small"}])).unwrap());
    assert_eq!(
        context_window_tokens(&catalog, Some(&selection)),
        Some(32000.)
    );
    selection.model = "fixed".into();
    assert_eq!(
        context_window_tokens(&catalog, Some(&selection)),
        Some(64000.)
    );
    selection.model = "opaque[wide]".into();
    assert_eq!(api_model_id(&catalog, &selection), "opaque[wide]");
    assert_eq!(context_window_tokens(&catalog, Some(&selection)), None);
}
