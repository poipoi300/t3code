#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_provider::{discovery::Driver, settings::decode_packaged};

#[test]
fn packaged_settings_match_binary_defaults_trimmed_fields_and_registry_default_enablement() {
    for driver in [
        Driver::Grok,
        Driver::Fx,
        Driver::Kiro,
        Driver::Pi,
        Driver::Muse,
        Driver::OpenCode,
    ] {
        let config = decode_packaged(driver, Some(&json!({"binaryPath":"  "}))).unwrap();
        assert_eq!(config["binaryPath"], json!(driver.binary().unwrap()));
        assert_eq!(config["enabled"], json!(false));
    }
    let local=decode_packaged(Driver::AcpRegistry,Some(&json!({"source":"local","commandPath":" ./tool ","commandArgs":[" raw argument "],"customModels":[" raw-model "]}))).unwrap();
    assert_eq!(local["enabled"], json!(true));
    assert_eq!(local["commandPath"], json!("./tool"));
    assert_eq!(local["commandArgs"], json!([" raw argument "]));
    assert_eq!(local["customModels"], json!([" raw-model "]));
    assert!(decode_packaged(Driver::AcpRegistry, Some(&json!({"source":" local "}))).is_err());
    assert!(decode_packaged(Driver::AcpRegistry, Some(&json!({"distribution":" auto "}))).is_err());
}

#[test]
fn packaged_config_decoders_reject_wrong_field_types_and_keep_secret_references() {
    for bad in [
        json!({"enabled":"yes"}),
        json!({"binaryPath":42}),
        json!({"customModels":null}),
        json!({"serverPassword":42}),
    ] {
        assert!(decode_packaged(Driver::OpenCode, Some(&bad)).is_err());
    }
    let secret = json!({"provider":"vault","reference":"op://v/i/f"});
    let config = decode_packaged(
        Driver::OpenCode,
        Some(&json!({"serverPassword":secret,"serverUrl":" http://localhost:42 "})),
    )
    .unwrap();
    assert_eq!(config["serverPassword"], secret);
    assert_eq!(config["serverUrl"], json!("http://localhost:42"));
}
