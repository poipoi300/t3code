#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_provider::{
    antigravity_callback::*,
    antigravity_profile::*,
    antigravity_release,
    environment::{Environment, Platform},
    sensitive::Sensitive,
};
#[test]
fn release_manifest_pins_the_official_target_version_and_digest() {
    for (platform, arch, folder, name) in [
        (
            Platform::Windows,
            "arm64",
            "windows-arm",
            "cli_windows_arm64.exe",
        ),
        (Platform::Linux, "x64", "linux-x64", "cli_linux_x64.tar.gz"),
        (
            Platform::Macos,
            "arm64",
            "darwin-arm",
            "cli_mac_arm64.tar.gz",
        ),
    ] {
        let manifest = json!({"version":"1.5.3","url":format!("https://storage.googleapis.com/antigravity-public/antigravity-cli/1.5.3-123/{folder}/{name}"),"sha512":"A".repeat(128)});
        let release = antigravity_release::parse(&manifest, platform, arch, false).unwrap();
        assert_eq!(release.id(), Some("a".repeat(64).as_str()));
        assert!(release.harness.is_none());
        let mut private = release.clone();
        private.url.push_str("?token=secret-never-format");
        assert!(!format!("{private:?}").contains("secret-never-format"));
        for (url, version) in [
            (
                format!("{}?token=secret", manifest["url"].as_str().unwrap()),
                "1.5.3",
            ),
            (
                manifest["url"]
                    .as_str()
                    .unwrap()
                    .replace("storage.googleapis.com", "evil.invalid"),
                "1.5.3",
            ),
            (
                manifest["url"]
                    .as_str()
                    .unwrap()
                    .replace("1.5.3-", "1.4.0-"),
                "1.4.0",
            ),
        ] {
            let mut invalid = manifest.clone();
            invalid["url"] = json!(url);
            invalid["version"] = json!(version);
            assert!(antigravity_release::parse(&invalid, platform, arch, false).is_none());
        }
    }
}

#[test]
fn model_thinking_observations_do_not_copy_another_models_selected_level_or_reuse_duplicate_names()
{
    use t3_contracts::model::ProviderOptionDescriptor;
    use t3_provider::{antigravity_catalog::*, catalog::descriptors};
    let setup = json!({"configOptions":[{"id":"model","type":"select","currentValue":"one","options":[{"value":"one","name":"Same"},{"value":"two","name":"Same"},{"value":"one","name":"Duplicate"}]},{"id":"thinking","name":"Thinking","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]}]});
    let mut levels = ThinkingLevels::new();
    observe(setup["configOptions"].as_array().unwrap(), &mut levels);
    let first = models(&setup, &levels);
    assert_eq!(first.len(), 2);
    assert_eq!(first[1].name.0, "Same (two)");
    assert!(
        matches!(&descriptors(first[1].capabilities.as_ref().unwrap())[0],ProviderOptionDescriptor::Select(descriptor)if descriptor.current_value.is_none())
    );
    assert!(
        matches!(&descriptors(first[0].capabilities.as_ref().unwrap())[0],ProviderOptionDescriptor::Select(descriptor)if descriptor.current_value==Some(Some("high".into())))
    );
    levels.insert("two".into(), None);
    let observed = models(&setup, &levels);
    assert!(descriptors(observed[1].capabilities.as_ref().unwrap()).is_empty());
}
#[test]
fn skill_roots_keep_native_name_precedence_and_ignore_bodies_beyond_the_scan_budget() {
    use t3_provider::antigravity_skills;
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("project");
    std::fs::create_dir_all(&workspace).unwrap();
    for (relative, name, description) in [
        (".gemini/config/skills/one/SKILL.md", "same", "User wins"),
        (
            "project/.gemini/skills/one/SKILL.md",
            "same",
            "Project loses",
        ),
        (
            "project/.agents/skills/z/SKILL.md",
            "agent",
            "Project agent",
        ),
        (
            ".gemini/antigravity-cli/skills/a/SKILL.md",
            "cli",
            "CLI skill",
        ),
    ] {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(
                "---\nname: {name}\ndescription: {description}\n---\n{}",
                "x".repeat(1_100_000)
            ),
        )
        .unwrap();
    }
    let skills = antigravity_skills::discover(&workspace, home).unwrap();
    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.name.0.as_str())
            .collect::<Vec<_>>(),
        ["agent", "cli", "same"]
    );
    let value = serde_json::to_value(&skills).unwrap();
    assert_eq!(value[2]["description"], "User wins");
    let path = home.join("project/.agents/skills/budget/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "-".repeat(1_000_001)).unwrap();
    assert!(antigravity_skills::discover(&workspace, home).is_ok());
    let path = home.join("project/.agents/skills/budget/SKILL.md");
    std::fs::write(path, "x".repeat(1_000_001)).unwrap();
    assert!(antigravity_skills::discover(&workspace, home).is_err());
}

#[test]
fn disposable_session_cleanup_requires_its_uuid_and_matching_temporary_workspace() {
    use t3_provider::antigravity_session_files;
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path();
    let cwd = profile.join("unique-workspace");
    let session = uuid::Uuid::new_v4().to_string();
    let conversations = profile.join("antigravity-acp/conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let base = conversations.join(&session);
    let metadata = base.with_extension("meta");
    let database = base.with_extension("db");
    std::fs::write(&metadata, json!({"cwd":"another-workspace"}).to_string()).unwrap();
    std::fs::write(&database, "keep").unwrap();
    antigravity_session_files::remove(profile, Some(&session), &cwd);
    assert!(database.exists());
    std::fs::write(&metadata, json!({"cwd":cwd}).to_string()).unwrap();
    antigravity_session_files::remove(profile, Some("../outside"), &cwd);
    assert!(database.exists());
    antigravity_session_files::remove(profile, Some(&session), &cwd);
    assert!(!database.exists());
    assert!(!metadata.exists());
}
fn auth_url(port: u16) -> String {
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut()
        .append_pair("state", "secret-never-format")
        .append_pair("redirect_uri", &format!("http://127.0.0.1:{port}/"))
        .append_pair("response_type", "code");
    url.into()
}
#[tokio::test]
async fn google_callbacks_are_single_response_bound_to_the_exact_flow_and_forwarded_only_to_loopback()
 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let pending = authorization(&auth_url(port)).unwrap();
    assert!(!format!("{pending:?}").contains("secret-never-format"));
    let callback = format!(
        "http://127.0.0.1:{port}/?state=secret-never-format&code=secret-code&iss=https%3A%2F%2Faccounts.google.com"
    );
    for invalid in [
        format!("{callback}&state=secret-never-format"),
        format!("{callback}&error=denied"),
        format!("{callback}&code=again"),
        callback.replace("127.0.0.1", "localhost"),
        callback.replace("state=secret-never-format", "state=stale"),
        format!("{callback}#fragment"),
    ] {
        let error = validate(&pending, &invalid).unwrap_err();
        assert!(!format!("{error:?}: {error}").contains("secret"));
    }
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 2048];
        let read = socket.read(&mut bytes).await.unwrap();
        assert!(
            std::str::from_utf8(&bytes[..read])
                .unwrap()
                .starts_with("GET /?state=")
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    forward(&validate(&pending, &callback).unwrap())
        .await
        .unwrap();
    server.await.unwrap();
    assert!(authorization(&auth_url(80)).is_err());
}
#[tokio::test]
async fn private_profile_clears_inherited_auth_uses_file_settings_and_proves_browser_suppression() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let auth = AuthConfig {
        method: "agent-platform".into(),
        api_key: Sensitive::new("secret-never-format".into()),
        gcp_project: "project".into(),
        gcp_location: "region".into(),
    };
    let environment = Environment::from([
        ("google_api_key".into(), "inherited-secret".into()),
        ("GEMINI_HOME".into(), "wrong".into()),
        ("GOOGLE_CLOUD_PROJECT".into(), "wrong".into()),
    ]);
    let (profile_path, runtime) = instance_directories(root, "Work");
    assert_ne!(
        instance_directories(root, "work"),
        (profile_path.clone(), runtime.clone())
    );
    let profile = prepare(PrepareProfile {
        directory: &profile_path,
        temp_directory: Some(&runtime),
        user_home: root,
        browser_helper: std::path::Path::new(env!("CARGO_BIN_EXE_antigravity-browser-helper")),
        environment: &environment,
        platform: Platform::current(),
        auth: &auth,
    })
    .await
    .unwrap();
    let settings = std::fs::read_to_string(profile.acp_directory.join("settings.json")).unwrap();
    assert!(!settings.contains("secret"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&settings).unwrap(),
        json!({"auth":{"type":"agent-platform"},"gcp":{"project":"project","location":"region"}})
    );
    let environment = profile.environment(&environment, &auth, None);
    assert_eq!(environment["GOOGLE_API_KEY"], "secret-never-format");
    assert!(!environment.contains_key("google_api_key"));
    assert!(!environment.contains_key("GOOGLE_CLOUD_PROJECT"));
    assert!(!format!("{auth:?} {profile:?}").contains("secret"));
    let spawn = profile.spawn(
        std::path::Path::new("agy"),
        None,
        root,
        &environment,
        &auth,
        None,
    );
    assert_eq!(
        spawn.args,
        [
            "acp",
            "--gemini-dir",
            profile_path.to_str().unwrap(),
            "--app-data-dir",
            "antigravity-acp"
        ]
    );
    assert!(!format!("{spawn:?}").contains("secret"));
    assert!(auth.issue().is_none());
    assert!(!auth.uses_browser());
}

#[test]
fn runtime_retirement_moves_before_deletion_and_preserves_other_instances() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path();
    let runtime = parent.join("instance");
    std::fs::create_dir_all(runtime.join("run-orphan/_MEI123")).unwrap();
    std::fs::write(runtime.join("run-orphan/_MEI123/payload"), "stale").unwrap();
    std::fs::create_dir_all(parent.join("instance.old-unfinished/runtime")).unwrap();
    std::fs::create_dir(parent.join("other-instance")).unwrap();
    let retired = t3_provider::antigravity_session_files::retire(&runtime);
    assert!(!runtime.exists());
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 3);
    std::fs::create_dir(&runtime).unwrap();
    std::fs::write(runtime.join("new-live-data"), "keep").unwrap();
    retired.remove();
    assert!(parent.join("other-instance").exists());
    assert!(runtime.join("new-live-data").exists());
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 2);
}
