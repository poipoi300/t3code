use serde_json::json;
use t3_forge::{http::ApiCredential, settings};
#[test]
fn unresolved_and_redacted_settings_cannot_fall_back_to_a_different_environment_account() {
    let environment = t3_process::Environment::from([(
        "T3CODE_BITBUCKET_ACCESS_TOKEN".into(),
        Some("different-account".into()),
    )]);
    for secret in [
        json!({"provider":"one-password","reference":"private-reference"}),
        json!("••••••"),
    ] {
        let raw = serde_json::from_value(json!({"accessToken":secret})).unwrap();
        let error = settings::bitbucket(&raw, &environment).err().unwrap();
        assert!(!format!("{error:?}").contains("private-reference"));
        let raw = serde_json::from_value(json!({"tokens":{"github.com":secret}})).unwrap();
        assert!(settings::github(&raw, &Default::default()).is_err());
        let raw = serde_json::from_value(json!({"token":secret})).unwrap();
        assert!(settings::gitcafe(&raw, Default::default()).is_err());
    }
    let raw = serde_json::from_value(json!({"accessToken":"injected\r\nheader"})).unwrap();
    assert!(
        settings::bitbucket(&raw, &Default::default())
            .unwrap()
            .is_none()
    );
    let raw = serde_json::from_value(
        json!({"accessToken":"saved","apiToken":{"provider":"one-password","reference":"unused"}}),
    )
    .unwrap();
    assert!(
        matches!(settings::bitbucket(&raw,&environment).unwrap(),Some(ApiCredential::Bearer(value))if value=="saved")
    );
    let raw =
        serde_json::from_value(json!({"email":"fixture@example.test","apiToken":"api-token"}))
            .unwrap();
    assert!(
        matches!(settings::bitbucket(&raw,&environment).unwrap(),Some(ApiCredential::Basic{username,password})if username=="fixture@example.test"&&password=="api-token")
    );
    let raw=serde_json::from_value(json!({"tokens":{" GITHUB.COM ":"saved"},"accounts":{"GITHUB.COM":"fixture"},"disabledHosts":[" Example.COM "]})).unwrap();
    let github = settings::github(&raw, &Default::default()).unwrap();
    assert_eq!(github.tokens["github.com"], "saved");
    assert_eq!(github.accounts["github.com"], "fixture");
    assert!(github.disabled_hosts.contains("example.com"));
}
