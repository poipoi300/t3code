#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_provider::{
    environment::{Environment, Platform},
    maintenance::LatestVersions,
    maintenance_resolution::*,
    sensitive::*,
    transport::*,
};
use tokio::sync::Mutex;
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        1000
    }
}
#[derive(Default)]
struct Probe {
    resolved: BTreeMap<String, String>,
    canonical: BTreeMap<String, String>,
    replies: BTreeMap<String, String>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}
#[async_trait]
impl MaintenanceProbe for Probe {
    fn resolve(&self, command: &str, _: &Environment) -> Option<String> {
        self.resolved.get(command).cloned()
    }
    fn canonical(&self, path: &str) -> Option<String> {
        Some(
            self.canonical
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.into()),
        )
    }
    async fn run(
        &self,
        command: &str,
        args: &[String],
        _: &Environment,
        _: &[i32],
    ) -> Option<String> {
        self.calls.lock().await.push((command.into(), args.into()));
        self.replies
            .get(&format!("{command}|{}", args.join("|")))
            .cloned()
    }
}
#[derive(Default)]
struct Http {
    replies: Mutex<BTreeMap<String, Value>>,
    calls: Mutex<Vec<HttpRequest>>,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        let reply = self
            .replies
            .lock()
            .await
            .get(&request.url)
            .cloned()
            .ok_or(ServiceError::Network)?;
        self.calls.lock().await.push(request);
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(reply),
        })
    }
}
fn context(driver: &str, _path: &str) -> MaintenanceContext {
    MaintenanceContext {
        driver: driver.into(),
        binary: "provider".into(),
        environment: BTreeMap::from([("SECRET".into(), "secret-never-format".into())]),
        host_environment: BTreeMap::new(),
        home: "C:/fixture/home".into(),
        cwd: "C:/fixture/state".into(),
        platform: Platform::Windows,
        shared_codex_home: "C:/fixture/shared-home".into(),
    }
}
fn build_resolver(
    context: MaintenanceContext,
    probe: Arc<Probe>,
    http: Arc<Http>,
) -> NativeMaintenance {
    NativeMaintenance::new(
        context,
        probe,
        http,
        Arc::new(Clock),
        Arc::new(LatestVersions::default()),
    )
}
#[tokio::test]
async fn native_codex_updates_use_the_executables_home_and_its_own_release_channel() {
    let path = "C:/fixture/standalone-home/packages/standalone/codex.exe";
    let probe = Arc::new(Probe {
        resolved: BTreeMap::from([("provider".into(), path.into())]),
        ..Default::default()
    });
    let http = Arc::new(Http::default());
    http.replies.lock().await.insert(
        "https://releases.openai.com/codex/channels/latest".into(),
        json!({"tag_name":"rust-v0.167.0"}),
    );
    let resolver = build_resolver(context("codex", path), probe, http.clone());
    let caps = resolver.capabilities(false).await.unwrap();
    let action = caps.update.as_ref().unwrap();
    assert_eq!(action.lock_key, "codex-native");
    assert_eq!(action.args, ["update"]);
    assert_eq!(
        action.environment["CODEX_HOME"],
        "C:/fixture/standalone-home"
    );
    assert_eq!(
        resolver.latest(false).await.unwrap(),
        Some("0.167.0".into())
    );
    assert_eq!(http.calls.lock().await.len(), 1);
    assert!(!format!("{caps:?} {action:?}").contains("secret-never-format"));
    assert_eq!(
        codex_standalone_home("C:\\packages\\standalone\\codex.exe"),
        Some("C:\\".into())
    );
    assert_eq!(
        codex_standalone_home("/packages/standalone/codex"),
        Some("/".into())
    );
    assert_eq!(
        codex_standalone_home("C:/İstanbul/packages/standalone/codex.exe"),
        Some("C:/İstanbul".into())
    );
}
#[tokio::test]
async fn package_manager_ownership_is_proven_and_project_node_modules_stay_manual() {
    for (path, lock, command) in [
        (
            "C:/prefix/lib/node_modules/@openai/codex/bin/codex.js",
            Some("npm-global:c:/prefix"),
            "npm",
        ),
        (
            "C:/project/node_modules/@openai/codex/bin/codex.js",
            None,
            "",
        ),
        ("C:/Users/u/.bun/bin/codex.exe", Some("bun-global"), "bun"),
        (
            "C:/Users/u/AppData/Local/pnpm/codex.exe",
            Some("pnpm-global"),
            "pnpm",
        ),
        (
            "C:/Users/u/.vite-plus/bin/codex.exe",
            Some("vite-plus-global"),
            "vp",
        ),
        (
            "C:/yarn/global/node_modules/@openai/codex/bin/codex.js",
            Some("yarn-global"),
            "yarn",
        ),
    ] {
        let probe = Arc::new(Probe {
            resolved: BTreeMap::from([("provider".into(), path.into())]),
            replies: BTreeMap::from([("pnpm|--version".into(), "10.4.0".into())]),
            ..Default::default()
        });
        let resolver = build_resolver(context("codex", path), probe, Arc::new(Http::default()));
        let caps = resolver.capabilities(false).await.unwrap();
        assert_eq!(
            caps.update.as_ref().map(|a| a.lock_key.as_str()),
            lock,
            "{path}"
        );
        if let Some(action) = caps.update {
            assert_eq!(action.executable, command);
            if command == "pnpm" {
                assert!(action.args.contains(&"--allow-build=@openai/codex".into()));
            }
        }
    }
}
#[tokio::test]
async fn mise_only_upgrades_active_latest_installs_and_keeps_pinned_paths_manual() {
    let resolved = "C:/fixture/mise/installs/codex/latest/bin/codex.exe";
    let real = "C:/fixture/mise/installs/codex/0.166.0/bin/codex.exe";
    let probe=Arc::new(Probe {resolved:BTreeMap::from([("provider".into(),resolved.into()),("mise".into(),"C:/fixture/bin/mise.exe".into())]),canonical:BTreeMap::from([(resolved.into(),real.into())]),replies:BTreeMap::from([("C:/fixture/bin/mise.exe|which|codex".into(),real.into()),("C:/fixture/bin/mise.exe|ls|--json|--installed|codex".into(),json!({"codex":[{"install_path":"C:/fixture/mise/installs/codex/0.166.0","requested_version":"latest","active":true}]}).to_string()),("C:/fixture/bin/mise.exe|latest|codex".into(),"0.167.0".into())]),..Default::default()});
    let resolver = build_resolver(context("codex", resolved), probe, Arc::new(Http::default()));
    assert_eq!(
        resolver
            .capabilities(false)
            .await
            .unwrap()
            .update
            .unwrap()
            .command,
        "mise upgrade codex"
    );
    assert_eq!(
        resolver.latest(false).await.unwrap(),
        Some("0.167.0".into())
    );
    let probe=Arc::new(Probe {resolved:BTreeMap::from([("provider".into(),real.into()),("mise".into(),"C:/fixture/bin/mise.exe".into())]),replies:BTreeMap::from([("C:/fixture/bin/mise.exe|which|codex".into(),real.into()),("C:/fixture/bin/mise.exe|ls|--json|--installed|codex".into(),json!([{"install_path":"C:/fixture/mise/installs/codex/0.166.0","requested_version":"0.166.0","active":true}]).to_string())]),..Default::default()});
    assert!(
        build_resolver(context("codex", real), probe, Arc::new(Http::default()))
            .capabilities(false)
            .await
            .unwrap()
            .update
            .is_none()
    );
    for malformed in [
        json!({"active":true}),
        json!({"install_path":123}),
        json!({"install_path":"other","active":"yes"}),
        json!({"install_path":"other","requested_version":42}),
    ] {
        let listing = json!([
            {"install_path":"C:/fixture/mise/installs/codex/0.166.0","requested_version":"latest","active":true},
            malformed
        ]);
        let probe = Arc::new(Probe {
            resolved: BTreeMap::from([
                ("provider".into(), resolved.into()),
                ("mise".into(), "mise-fixture".into()),
            ]),
            canonical: BTreeMap::from([(resolved.into(), real.into())]),
            replies: BTreeMap::from([
                ("mise-fixture|which|codex".into(), real.into()),
                (
                    "mise-fixture|ls|--json|--installed|codex".into(),
                    listing.to_string(),
                ),
            ]),
            ..Default::default()
        });
        assert!(
            build_resolver(context("codex", resolved), probe, Arc::new(Http::default()))
                .capabilities(false)
                .await
                .unwrap()
                .update
                .is_none()
        );
    }
}

#[tokio::test]
async fn pnpm_latest_only_lists_the_installed_version_when_outdated_has_no_eligible_update() {
    for (outdated, version, lists) in [
        (
            json!({"@openai/codex":{"latest":"0.167.0"}}),
            Some("0.167.0"),
            0,
        ),
        (json!({}), Some("0.166.0"), 1),
        (json!([]), None, 0),
    ] {
        let path = "C:/Users/u/AppData/Local/pnpm/codex.exe";
        let probe = Arc::new(Probe {
            resolved: BTreeMap::from([
                ("provider".into(), path.into()),
                ("pnpm".into(), "pnpm-fixture".into()),
            ]),
            replies: BTreeMap::from([
                ("pnpm|--version".into(), "10.5.0".into()),
                (
                    "pnpm-fixture|outdated|-g|--reporter=silent|--format|json|@openai/codex".into(),
                    outdated.to_string(),
                ),
                (
                    "pnpm-fixture|ls|-g|--reporter=silent|--depth|0|--json|@openai/codex".into(),
                    json!([{"dependencies":{"@openai/codex":{"version":"0.166.0"}}}]).to_string(),
                ),
            ]),
            ..Default::default()
        });
        let resolver = build_resolver(
            context("codex", path),
            probe.clone(),
            Arc::new(Http::default()),
        );
        assert_eq!(resolver.latest(false).await.unwrap().as_deref(), version);
        assert_eq!(
            probe
                .calls
                .lock()
                .await
                .iter()
                .filter(|(_, args)| args.first().is_some_and(|arg| arg == "ls"))
                .count(),
            lists
        );
    }
}
#[tokio::test]
async fn claude_native_channel_uses_the_updaters_host_settings_and_sorted_managed_drop_ins() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("host-config");
    let program = temp.path().join("programs");
    std::fs::create_dir_all(&config).unwrap();
    let drop_ins = program.join("ClaudeCode/managed-settings.d");
    std::fs::create_dir_all(&drop_ins).unwrap();
    std::fs::write(
        config.join("settings.json"),
        "{\"autoUpdatesChannel\":\"stable\",}",
    )
    .unwrap();
    std::fs::write(
        drop_ins.join("10-a.json"),
        "{\"autoUpdatesChannel\":\"latest\"}",
    )
    .unwrap();
    std::fs::write(
        drop_ins.join("20-b.json"),
        "{\"autoUpdatesChannel\":\"stable\"}",
    )
    .unwrap();
    let path = "C:/Users/u/.local/bin/claude.exe";
    let mut context = context("claudeAgent", path);
    context.host_environment = BTreeMap::from([
        (
            "CLAUDE_CONFIG_DIR".into(),
            config.to_string_lossy().into_owned(),
        ),
        (
            "PROGRAMFILES".into(),
            program.to_string_lossy().into_owned(),
        ),
    ]);
    let probe = Arc::new(Probe {
        resolved: BTreeMap::from([("provider".into(), path.into())]),
        ..Default::default()
    });
    let http = Arc::new(Http::default());
    http.replies.lock().await.insert(
        "https://downloads.claude.ai/claude-code-releases/stable".into(),
        json!("2.1.301\n"),
    );
    http.replies.lock().await.insert(
        "https://downloads.claude.ai/claude-code-releases/latest".into(),
        json!("2.1.302\n"),
    );
    let resolver = build_resolver(context, probe, http);
    assert_eq!(
        resolver
            .capabilities(false)
            .await
            .unwrap()
            .update
            .unwrap()
            .lock_key,
        "claudeAgent-native"
    );
    assert_eq!(
        resolver.latest(false).await.unwrap(),
        Some("2.1.301".into())
    );
    std::fs::write(
        drop_ins.join("20-b.json"),
        "{\"autoUpdatesChannel\":\"latest\"}",
    )
    .unwrap();
    assert_eq!(
        resolver.latest(false).await.unwrap(),
        Some("2.1.302".into())
    );
}
