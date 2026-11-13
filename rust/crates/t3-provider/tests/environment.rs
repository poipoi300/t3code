#![allow(clippy::unwrap_used)]

use serde_json::json;
use std::path::Path;
use t3_contracts::provider_instance::*;
use t3_provider::environment::*;

fn vars(value: serde_json::Value) -> ProviderInstanceEnvironment {
    serde_json::from_value(value).unwrap()
}

#[test]
fn configured_homes_expand_but_inherited_and_custom_values_stay_literal() {
    let base = Environment::from([
        ("CODEX_HOME".into(), "~/.inherited".into()),
        ("PATH".into(), "/bin".into()),
    ]);
    let configured = vars(json!([
        {"name":"CLAUDE_CONFIG_DIR","value":"~/.account","sensitive":false},
        {"name":"GROK_HOME","value":"~/.account","sensitive":false},
        {"name":"CUSTOM_VALUE","value":"~/.account","sensitive":false}
    ]));
    let merged =
        merge_stored_environment(&configured, &base, Path::new("/home/ada"), Platform::Linux);
    assert_eq!(merged["CODEX_HOME"], "~/.inherited");
    assert_eq!(
        merged["GROK_HOME"],
        Path::new("/home/ada").join(".account").to_string_lossy()
    );
    assert_eq!(merged["CUSTOM_VALUE"], "~/.account");
    assert_eq!(base.len(), 2);
}

#[test]
fn windows_removes_all_inherited_spellings_uppercases_and_preserves_last_empty_value() {
    let base = Environment::from([
        ("HTTPS_PROXY".into(), "old".into()),
        ("https_proxy".into(), "older".into()),
        ("Https_Proxy".into(), "oldest".into()),
    ]);
    let configured = vars(json!([
        {"name":"Https_Proxy","value":"gateway","sensitive":false},
        {"name":"https_proxy","value":"","sensitive":false},
        {"name":"Codex_Home","value":"~/corp","sensitive":false}
    ]));
    let merged = merge_stored_environment(
        &configured,
        &base,
        Path::new("/home/ada"),
        Platform::Windows,
    );
    assert_eq!(merged.len(), 2);
    assert_eq!(merged["HTTPS_PROXY"], "");
    assert!(!merged.contains_key("https_proxy"));
    assert_eq!(
        merged["CODEX_HOME"],
        Path::new("/home/ada").join("corp").to_string_lossy()
    );
    let posix =
        merge_stored_environment(&configured, &base, Path::new("/home/ada"), Platform::Linux);
    assert_eq!(posix["HTTPS_PROXY"], "old");
    assert_eq!(posix["Codex_Home"], "~/corp");
}

#[test]
fn unresolved_references_are_skipped_only_by_the_low_level_merge() {
    let configured = vars(
        json!([{"name":"OPENAI_API_KEY","value":"","sensitive":true,"secret":{"provider":"vault","reference":"op://vault/item/field"}}]),
    );
    let base = Environment::from([("OPENAI_API_KEY".into(), "inherited".into())]);
    assert_eq!(
        merge_stored_environment(
            &configured,
            &base,
            Path::new("/home/ada"),
            Platform::Windows
        ),
        base
    );
}

#[test]
fn claude_clears_host_session_identity_before_applying_instance_overrides() {
    let base = Environment::from([
        ("CLAUDECODE".into(), "1".into()),
        ("CLAUDE_EFFORT".into(), "max".into()),
        ("ELECTRON_RUN_AS_NODE".into(), "1".into()),
        ("HOME".into(), "same".into()),
    ]);
    let variables: ResolvedProviderInstanceEnvironment =
        serde_json::from_value(json!([{"name":"CLAUDE_EFFORT","value":"high","sensitive":false}]))
            .unwrap();
    let env = claude_process_environment(
        &base,
        &variables,
        Path::new("/home/ada"),
        Platform::Windows,
        Some("settings-entrypoint"),
    );
    assert!(!env.contains_key("CLAUDECODE"));
    assert!(!env.contains_key("ELECTRON_RUN_AS_NODE"));
    assert_eq!(env["HOME"], "same");
    assert_eq!(env["CLAUDE_EFFORT"], "high");
    assert_eq!(env["CLAUDE_CODE_ENTRYPOINT"], "settings-entrypoint");
}

#[test]
fn codex_overlay_uses_shared_continuation_identity_and_private_effective_home() {
    let cwd = std::env::current_dir().unwrap();
    let home = cwd.join("home");
    let direct = codex_home_layout("~/shared", "", &home, &cwd);
    let overlay = codex_home_layout("~/shared", "~/work-auth", &home, &cwd);
    assert_eq!(direct.continuation_key, overlay.continuation_key);
    assert!(overlay.auth_overlay);
    assert_ne!(direct.effective_home, overlay.effective_home);
    let inherited = Environment::from([("CLAUDE_CONFIG_DIR".into(), "~/.literal".into())]);
    assert_eq!(
        claude_home("", &inherited, &home, &cwd),
        cwd.join("~/.literal")
    );
}
