use serde_json::json;
use t3_contracts::{
    base_schemas::ThreadId, provider_instance::ProviderInstanceEnvironmentVariable,
};

#[test]
fn old_environment_settings_apply_defaults_and_secret_sensitivity() {
    let variable: ProviderInstanceEnvironmentVariable =
        serde_json::from_value(json!({"name":"A"})).expect("old settings");
    assert_eq!(variable.value, "");
    assert!(!variable.sensitive);
    let variable: ProviderInstanceEnvironmentVariable = serde_json::from_value(json!({"name":"A", "value":"ignored", "sensitive":false, "secret":{"provider":"vault", "reference":"key"}})).expect("secret settings");
    assert!(variable.sensitive);
    assert_eq!(variable.value, "");
}

#[test]
fn ids_validate_and_normalize_at_decode() {
    assert!(serde_json::from_value::<ThreadId>(json!("  ")).is_err());
    let id: ThreadId = serde_json::from_value(json!(" thread:1 ")).expect("id");
    assert_eq!(id.0, "thread:1");
}

#[test]
fn javascript_numbers_keep_their_text_and_reject_overflow() {
    for raw in ["5.108443427076897e-154", "123456789012345680000"] {
        let number: t3_contracts::wire::JsNumber = serde_json::from_str(raw).expect("number");
        assert_eq!(serde_json::to_string(&number).expect("encode"), raw);
    }
    assert!(serde_json::from_str::<t3_contracts::wire::JsNumber>("1e999").is_err());
}

#[test]
fn secret_debug_is_redacted() {
    let variable: ProviderInstanceEnvironmentVariable =
        serde_json::from_value(json!({"name":"A", "value":"MARKER_SECRET", "sensitive":true}))
            .expect("settings");
    assert!(!format!("{variable:?}").contains("MARKER_SECRET"));
}
