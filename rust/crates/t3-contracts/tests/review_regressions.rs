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

#[test]
fn handoff_validates_js_string_limits_and_reconnect_registration_on_decode() {
    use t3_contracts::provider_setup::ChatGptHandoffInput;
    let valid = json!({"instanceId":"codex","environmentId":"remote","attemptId":"😀".repeat(64),
        "returnUrl":"x".repeat(4096),"profile":{"clientId":"oaiapp_a-B_0",
        "redirectUri":"http://localhost:99999/auth/callback","idTokenHint":"x".repeat(16384)}});
    assert!(serde_json::from_value::<ChatGptHandoffInput>(valid.clone()).is_ok());
    for (path, bad) in [
        ("/attemptId", "😀".repeat(65)),
        ("/returnUrl", "x".repeat(4097)),
        ("/profile/clientId", "oaiapp_é".into()),
        ("/profile/clientId", "oaiapp_".into()),
        (
            "/profile/redirectUri",
            "http://localhost:01/auth/callback".into(),
        ),
        (
            "/profile/redirectUri",
            "https://example.org:1/auth/callback".into(),
        ),
        ("/profile/idTokenHint", "x".repeat(16385)),
    ] {
        let mut input = valid.clone();
        *input.pointer_mut(path).expect("fixture field") = json!(bad);
        assert!(
            serde_json::from_value::<ChatGptHandoffInput>(input).is_err(),
            "{path}"
        );
    }
}

#[test]
fn transfer_credentials_enforce_token_limits_and_require_nullable_refresh_token() {
    use t3_contracts::provider_setup::ChatGptTransferredCredentials;
    let sample = json!({"clientId":"app","accessToken":"a","idToken":"i","refreshToken":null,"issuer":"i",
        "expiresAt":100,"earliestRefreshAt":null,"scopes":[],"subject":"subject","email":null});
    assert!(serde_json::from_value::<ChatGptTransferredCredentials>(sample.clone()).is_ok());
    for key in ["accessToken", "idToken", "refreshToken"] {
        for (value, valid) in [
            ("😀".repeat(8192), true),
            ("a".repeat(16385), false),
            (String::new(), key == "refreshToken"),
        ] {
            let mut input = sample.clone();
            input[key] = json!(value);
            assert_eq!(
                serde_json::from_value::<ChatGptTransferredCredentials>(input).is_ok(),
                valid,
                "{key}"
            );
        }
    }
    let mut missing = sample;
    missing
        .as_object_mut()
        .expect("object")
        .remove("refreshToken");
    assert!(serde_json::from_value::<ChatGptTransferredCredentials>(missing).is_err());
}
