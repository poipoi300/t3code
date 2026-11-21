// The manual roots in these two launchers must claim the schemes in the shared NSH manifest.
fn check(source: &str) {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../shells/nsh.json")).expect("shell manifest");
    let schemes = manifest["schemes"].as_array().expect("scheme list");
    assert!(!schemes.is_empty());
    let declaration = format!(
        "platform::AppSource::Rust(platform::Root::of::<t3_ui::App>().url_schemes({}))",
        serde_json::Value::Array(schemes.clone())
    );
    let source: String = source.split_whitespace().collect();
    assert!(
        source.contains(&declaration),
        "missing manifest schemes on the app root"
    );
}

#[test]
fn desktop_claims_manifest_schemes() {
    check(include_str!("../../../shells/desktop/src/main.rs"));
}

#[test]
fn gpui_claims_manifest_schemes() {
    check(include_str!("../../../shells/gpui/src/main.rs"));
}
