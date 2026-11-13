use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use t3_config::{config::*, environment::*, home::*};

fn inputs(root: &std::path::Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags::default(),
        env: Environment::new(),
        bootstrap: None,
        home: root.join("account"),
        process_cwd: root.join("cwd"),
        headless: false,
        force_auto_bootstrap: None,
        service_managed: false,
        cli_log_level: None,
        automatic_port: Some(8123),
        persisted_observability: BTreeMap::new(),
        static_dir: None,
    }
}
#[test]
fn path_and_dev_home_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut i = inputs(root);
    let c = resolve_config(&i).unwrap();
    assert_eq!(c.paths.base_dir, root.join("account/.t3"));
    assert_eq!(c.paths.state_dir, root.join("account/.t3/userdata"));
    assert_eq!(c.port, 8123);
    i.flags.dev_url = Some("http://localhost:5173".into());
    assert_eq!(
        resolve_config(&i).unwrap().paths.state_dir,
        root.join("account/.t3/dev")
    );
    i.env.insert("T3CODE_HOME".into(), "~/custom".into());
    assert_eq!(
        resolve_config(&i).unwrap().paths.state_dir,
        root.join("account/custom/userdata")
    );
    i.flags.base_dir = Some(" ".into());
    assert_eq!(
        resolve_config(&i).unwrap().paths.state_dir,
        root.join("account/.t3/dev")
    );
    let worktree = root.join("worktree/.t3");
    assert_eq!(
        resolve_dev_home(
            None,
            Some(&worktree),
            Some("~/shared"),
            &i.home,
            &i.process_cwd
        ),
        (worktree.clone(), true)
    );
    assert_eq!(
        resolve_dev_home(
            Some(" "),
            Some(&worktree),
            Some("~/shared"),
            &i.home,
            &i.process_cwd
        )
        .0,
        worktree
    );
    assert_eq!(
        resolve_dev_home(
            Some("~/explicit"),
            Some(&worktree),
            Some("~/shared"),
            &i.home,
            &i.process_cwd
        )
        .0,
        i.home.join("explicit")
    );
    assert_eq!(expand_home("~\\x", &i.home), i.home.join("x"));
    assert_eq!(
        resolve_path("a/../b", &i.home, &i.process_cwd),
        i.process_cwd.join("b")
    );
    assert!(resolve_worktrees_directory("relative", root, &i.home).is_none());
}
#[test]
fn boolean_and_bootstrap_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let mut i = inputs(temp.path());
    i.bootstrap = Some(serde_json::from_value(json!({"mode":"desktop","port":4000,"host":"127.1.2.3","noBrowser":true,"desktopBootstrapToken":"secret","tailscaleServeEnabled":true,"tailscaleServePort":8443,"t3Home":temp.path().join("bootstrap")})).unwrap());
    let c = resolve_config(&i).unwrap();
    assert_eq!(c.mode, RuntimeMode::Desktop);
    assert_eq!(c.port, 4000);
    assert!(!c.needs_automatic_port);
    for key in [
        "T3CODE_NO_BROWSER",
        "T3CODE_TAILSCALE_SERVE",
        "T3CODE_AUTO_BOOTSTRAP_PROJECT_FROM_CWD",
        "T3CODE_LOG_WS_EVENTS",
    ] {
        i.env.insert(key.into(), "true".into());
    }
    i.env.insert("T3CODE_PORT".into(), "5000".into());
    i.flags.port = Some(6000);
    i.flags.no_browser = Some(false);
    i.flags.tailscale_serve_enabled = Some(false);
    i.flags.auto_bootstrap_project_from_cwd = Some(false);
    i.flags.log_websocket_events = Some(false);
    let c = resolve_config(&i).unwrap();
    assert_eq!(c.port, 6000);
    assert!(!c.no_browser);
    assert!(!c.tailscale_serve_enabled);
    assert!(!c.auto_bootstrap_project_from_cwd);
    assert!(!c.log_websocket_events);
    i.headless = true;
    assert!(resolve_config(&i).unwrap().no_browser);
    i.force_auto_bootstrap = Some(true);
    assert!(resolve_config(&i).unwrap().auto_bootstrap_project_from_cwd);
    i.env.insert("T3CODE_PORT".into(), "invalid".into());
    assert!(resolve_config(&i).is_err());
}
#[test]
fn lan_and_web_dev_auth() {
    let temp = tempfile::tempdir().unwrap();
    let mut i = inputs(temp.path());
    i.service_managed = true;
    assert!(resolve_config(&i).unwrap().private_network_only);
    i.flags.lan = Some(false);
    assert_eq!(resolve_config(&i).unwrap().host, None);
    i.flags.lan = Some(true);
    assert_eq!(resolve_config(&i).unwrap().host.as_deref(), Some("0.0.0.0"));
    i.flags.host = Some("127.0.0.1".into());
    assert!(!resolve_config(&i).unwrap().private_network_only);
    i.env.insert("T3CODE_DEV_AUTH_TOKEN".into(), "short".into());
    assert!(resolve_config(&i).is_ok());
    i.flags.dev_url = Some("http://localhost:1234".into());
    assert!(resolve_config(&i).is_err());
    i.env.insert(
        "T3CODE_DEV_AUTH_TOKEN".into(),
        format!(" {} ", "x".repeat(32)),
    );
    assert_eq!(
        resolve_config(&i).unwrap().dev_auth_token(),
        Some("x".repeat(32).as_str())
    );
    i.flags.mode = Some(RuntimeMode::Desktop);
    assert!(resolve_config(&i).unwrap().dev_auth_token().is_none());
}
#[test]
fn observability_sources_and_disable() {
    let temp = tempfile::tempdir().unwrap();
    let mut i = inputs(temp.path());
    i.persisted_observability
        .insert("traces".into(), "https://saved/v1/traces".into());
    assert_eq!(
        resolve_config(&i).unwrap().telemetry.traces.unwrap().source,
        "settings"
    );
    i.env.insert(
        "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
        "https://collector/base?key=x".into(),
    );
    let endpoint = resolve_config(&i).unwrap().telemetry.traces.unwrap();
    assert_eq!(endpoint.url, "https://collector/base/v1/traces?key=x");
    assert_eq!(endpoint.export.protocol, "http/protobuf");
    i.env
        .insert("T3CODE_OTLP_TRACES_URL".into(), "https://t3/traces".into());
    assert_eq!(
        resolve_config(&i).unwrap().telemetry.traces.unwrap().url,
        "https://t3/traces"
    );
    i.env.insert("OTEL_SDK_DISABLED".into(), "true".into());
    assert!(resolve_config(&i).unwrap().telemetry.traces.is_none());
    i.env
        .insert("T3CODE_OTEL_SDK_DISABLED".into(), "false".into());
    assert!(resolve_config(&i).unwrap().telemetry.traces.is_some());
    let h = parse_otlp_headers(" auth = a%20b==,x=value ").unwrap();
    assert_eq!(h["auth"], "a b==");
    assert_eq!(h["x"], "value");
    assert!(parse_otlp_headers("x=%zz").is_err());
}

#[test]
fn debug_output_redacts_configuration_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let mut i = inputs(temp.path());
    let secret = "sensitive-debug-regression-token";
    i.env.insert("T3CODE_DEV_AUTH_TOKEN".into(), secret.into());
    i.env.insert(
        "T3CODE_OTLP_HEADERS".into(),
        format!("authorization={secret}"),
    );
    i.env.insert(
        "T3CODE_OTLP_TRACES_URL".into(),
        "https://collector/traces".into(),
    );
    i.bootstrap = Some(serde_json::from_value(json!({"mode":"desktop","port":4000,"host":"127.0.0.1","noBrowser":true,"tailscaleServeEnabled":false,"tailscaleServePort":443,"desktopBootstrapToken":secret})).unwrap());
    let telemetry = resolve_config(&i).unwrap().telemetry;
    let endpoint = telemetry.traces.as_ref().unwrap();
    for debug in [
        format!("{i:?}"),
        format!("{telemetry:?}"),
        format!("{endpoint:?}"),
        format!("{:?}", endpoint.export),
    ] {
        assert!(!debug.contains(secret), "credential in Debug output");
    }
}
#[tokio::test]
async fn linked_worktree_identity_and_environment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("repo/src/sub")).unwrap();
    std::fs::write(
        root.join("repo/.git"),
        "gitdir: ../main/.git/worktrees/repo\n",
    )
    .unwrap();
    assert_eq!(
        resolve_worktree_home(&root.join("repo/src/sub")),
        Some(root.join("repo/.t3"))
    );
    let layout = HomeLayout::derive(root.join("home"), false, true);
    let (a, b) = tokio::join!(environment_id(&layout), environment_id(&layout));
    assert_eq!(a.unwrap(), b.unwrap());
    std::fs::write(layout.environment_id(), " \n").unwrap();
    let recovered = environment_id(&layout).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(layout.environment_id_recovery())
            .unwrap()
            .trim(),
        recovered.0
    );
    let mut env = Environment::new();
    hydrate_posix_home(&mut env, "/account");
    assert_eq!(env["HOME"], "/account");
    env.insert("HOME".into(), "/keep".into());
    hydrate_posix_home(&mut env, "/other");
    assert_eq!(env["HOME"], "/keep");
    assert_eq!(
        parse_machine_info_value(
            "# comment\nPRETTY_HOSTNAME=\"Machine Name\"",
            "PRETTY_HOSTNAME"
        )
        .as_deref(),
        Some("Machine Name")
    );
    assert_eq!(environment_label(None, Some(" host "), "cwd"), "host");
    assert_eq!(PathBuf::from(&layout.state_dir), root.join("home/userdata"));
}
#[test]
fn terminal_paths_and_pending_upload_sweep() {
    use std::time::{Duration, UNIX_EPOCH};
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    home.ensure_directories().unwrap();
    assert_eq!(
        home.terminal_history("thread", "term-1"),
        home.terminal_logs().join("terminal_dGhyZWFk.log")
    );
    assert_eq!(
        home.terminal_history("thread", "other"),
        home.terminal_logs().join("terminal_dGhyZWFk_b3RoZXI.log")
    );
    assert!(
        home.terminal_history(&"long".repeat(100), "term-1")
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("terminal_sha256.")
    );
    let pending = home
        .attachments()
        .join("pending-00000000-0000-4000-8000-000000000000-png.png");
    let partial = home.attachments().join("unfinished.part");
    let stored = home
        .attachments()
        .join("thread-00000000-0000-4000-8000-000000000000.png");
    for file in [&pending, &partial, &stored] {
        let file = std::fs::File::create(file).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(100))
            .unwrap();
    }
    assert_eq!(
        home.sweep_stale_pending_attachments(UNIX_EPOCH + Duration::from_secs(100000)),
        2
    );
    assert!(stored.exists());
    assert!(!pending.exists());
    assert!(!partial.exists());
}
