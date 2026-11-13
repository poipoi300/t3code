#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::path::Path;
use t3_contracts::acp_registry::AcpRegistryDistribution;
use t3_provider::{
    discovery::*,
    environment::{Environment, Platform},
    registry::*,
};

fn agent(id: &str, name: &str) -> Agent {
    serde_json::from_value(json!({"id":id,"name":name,"version":"1.2.3+rev","description":"Coding assistant","authors":["Someone"],"distribution":{"binary":{"windows-x86_64":{"archive":"https://example.org/a.zip","cmd":"bin/agent.exe","sha256":"0000000000000000000000000000000000000000000000000000000000000000","args":["acp"],"env":{"CONFIG":"registry"}}},"npx":{"package":"@example/agent@1.2.3"},"uvx":{"package":"agent==1.2.3"}}})).unwrap()
}

#[test]
fn registry_envelope_discards_invalid_agents_and_rejects_unsafe_distribution_specs() {
    let valid = agent("assistant", "Assistant");
    valid.validate().unwrap();
    let mut entries = vec![serde_json::to_value(&valid).unwrap()];
    for invalid in [
        json!({"id":"../outside"}),
        json!({"id":"invalid","name":"No version"}),
    ] {
        entries.push(invalid);
    }
    let index = RegistryIndex::parse(
        json!({"version":"1","agents":entries})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(index.agents.len(), 1);
    for package in [
        "agent@latest",
        "agent@^1.2.3",
        "agent@1.2",
        "https://example.org/script",
    ] {
        let mut bad = valid.clone();
        bad.distribution.npx.as_mut().unwrap().package = package.into();
        assert!(bad.validate().is_err());
    }
    for url in [
        "http://example.org/a",
        "https://user:secret@example.org/a",
        "file:///tmp/a",
    ] {
        assert!(!https_url(url));
    }
    let huge = json!({"version":"1","agents":vec![json!(null);513]});
    assert!(RegistryIndex::parse(huge.to_string().as_bytes()).is_err());
    assert!(RegistryIndex::parse(&[0xff]).is_err());
}

#[test]
fn distribution_order_prefers_binary_then_npm_then_uv_and_windows_arm_falls_back_to_x64() {
    let agent = agent("assistant", "Assistant");
    for target in ["windows-x86_64", "windows-aarch64"] {
        let distribution = resolve_distribution(&agent, Preference::Auto, Some(target)).unwrap();
        assert_eq!(distribution.kind, AcpRegistryDistribution::Binary);
        assert_eq!(distribution.args, ["acp"]);
    }
    assert_eq!(
        resolve_distribution(&agent, Preference::Auto, Some("linux-x86_64"))
            .unwrap()
            .kind,
        AcpRegistryDistribution::Npx
    );
    assert_eq!(
        resolve_distribution(&agent, Preference::Uvx, None)
            .unwrap()
            .kind,
        AcpRegistryDistribution::Uvx
    );
    assert!(resolve_distribution(&agent, Preference::Binary, Some("linux-x86_64")).is_none());
    assert_eq!(
        platform_target(Platform::Windows, "arm64").as_deref(),
        Some("windows-aarch64")
    );
    assert!(platform_target(Platform::Linux, "ia32").is_none());
}

#[test]
fn registry_search_ranks_identity_before_authors_and_filters_missing_package_managers() {
    let mut exact = agent("assistant", "Assistant");
    exact.distribution.binary = None;
    let prefix = agent("assistant-extra", "Assistant Extra");
    let mut author = agent("other", "Other");
    author.authors = vec!["Assistant".into()];
    let index = RegistryIndex {
        version: "1".into(),
        agents: vec![author, prefix, exact],
    };
    let result = search(
        &index,
        "assistant",
        Some(3),
        Some("windows-x86_64"),
        true,
        false,
    );
    assert_eq!(
        result
            .agents
            .iter()
            .map(|a| a.id.0.as_str())
            .collect::<Vec<_>>(),
        ["assistant", "assistant-extra", "other"]
    );
    let result = search(
        &index,
        "assistant",
        None,
        Some("windows-x86_64"),
        false,
        false,
    );
    assert_eq!(result.agents.len(), 2);
    assert!(!result.agents.iter().any(|a| a.id.0 == "assistant"));
    assert_eq!(
        search(
            &index,
            "coding assistant",
            Some(1),
            Some("windows-x86_64"),
            true,
            true
        )
        .agents
        .len(),
        1
    );
}

#[test]
fn archive_paths_reject_traversal_roots_and_drive_paths_on_every_platform() {
    for unsafe_path in [
        "../outside",
        "./../outside",
        "/absolute",
        "C:\\outside",
        "bin/../outside",
        "bin/./agent",
        "\\\\server\\share",
    ] {
        assert!(
            normalize_command_path(unsafe_path).is_none(),
            "{unsafe_path}"
        );
    }
    assert_eq!(
        normalize_command_path("./bin\\agent.exe"),
        Some(vec!["bin".into(), "agent.exe".into()])
    );
    assert!(validate_archive_entries("./\nbin/agent\nlib/data\n"));
    assert!(!validate_archive_entries("bin/agent\n../../outside"));
    assert!(verify_checksum(
        b"abc",
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    ));
    assert!(!verify_checksum(b"abc", Some("0000")));
    let agent = agent("assistant", "Assistant");
    let root = install_root(Path::new("tools"), &agent, "windows-x86_64").unwrap();
    assert!(root.to_string_lossy().contains("1.2.3%2Brev"));
}

#[test]
fn launch_overrides_keep_registry_args_env_and_package_installs_use_managed_prefixes() {
    let agent = agent("assistant", "Assistant");
    let distribution =
        resolve_distribution(&agent, Preference::Auto, Some("windows-x86_64")).unwrap();
    let launch = launch_description(
        &distribution,
        Path::new("installed.exe"),
        Some(Path::new("override.exe")),
        &Environment::from([("CONFIG".into(), "host".into())]),
    );
    assert_eq!(launch.executable, Path::new("override.exe"));
    assert_eq!(launch.args, ["acp"]);
    assert_eq!(launch.environment["CONFIG"], "registry");
    let npm = resolve_distribution(&agent, Preference::Npx, None).unwrap();
    let install =
        package_install_description(&npm, Path::new("managed"), &Environment::new()).unwrap();
    assert_eq!(
        install.args,
        ["install", "--global", "@example/agent@1.2.3"]
    );
    assert_eq!(install.environment["npm_config_prefix"], "managed");
    let uv = resolve_distribution(&agent, Preference::Uvx, None).unwrap();
    let install =
        package_install_description(&uv, Path::new("managed"), &Environment::new()).unwrap();
    assert_eq!(install.args, ["tool", "install", "--force", "agent==1.2.3"]);
    assert_eq!(install.environment["UV_TOOL_DIR"], "managed");
}

#[test]
fn opencode_server_generation_probes_require_json_identity_and_handle_auth_errors() {
    assert!(
        classify_opencode_response("/api/info", 200, Some("text/html"), "<html>ok</html>")
            .unwrap()
            .is_none()
    );
    assert!(
        classify_opencode_response(
            "/api/info",
            200,
            Some("application/json"),
            r#"{"version":"2.2.5"}"#
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        classify_opencode_response(
            "/api/info",
            200,
            Some("Application/JSON; charset=utf-8"),
            r#"{"version":"2.2.5","pid":42}"#
        )
        .unwrap()
        .unwrap()
        .generation,
        2
    );
    assert_eq!(
        classify_opencode_response(
            "/global/health",
            200,
            Some("application/json"),
            r#"{"version":"1.14.19","healthy":true}"#
        )
        .unwrap()
        .unwrap()
        .generation,
        1
    );
    assert!(classify_opencode_response("/api/info", 401, None, "").is_err());
}

#[test]
fn managed_release_pins_first_installs_never_downgrades_and_prefers_newest() {
    assert_eq!(
        managed_release(None, Some("1.2.0"), Some("2.0.0")),
        Some("1.2.0")
    );
    assert_eq!(
        managed_release(Some("1.3.0"), Some("1.2.0"), Some("1.4.0")),
        Some("1.4.0")
    );
    assert_eq!(
        managed_release(Some("1.4.0"), Some("1.2.0"), Some("1.3.0")),
        None
    );
}
