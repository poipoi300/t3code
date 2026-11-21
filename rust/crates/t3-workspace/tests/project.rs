use std::{fs, path::Path};
use t3_workspace::{paths, project::*};
fn put(root: &Path, path: &str, text: &str) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().expect("test fixture")).expect("test fixture");
    fs::write(p, text).expect("test fixture");
}

#[tokio::test]
async fn project_file_jsonc_schema_trimming_and_failures() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    assert!(load_project_file(root).await.is_none());
    put(root, "t3.json", "{ not json");
    assert!(load_project_file(root).await.is_none());
    let source = r#"{
      // preserve URLs in strings
      "$schema": "https://t3.codes/schema/t3.json",
      "iconPath": " assets/logo.svg ",
      "defaultThreadEnvMode": "worktree",
      "worktreeSubmodules": "none",
      "scripts": [{"name":" Dev ","command":" bun dev ","runOnWorktreeCreate":true,},],
    }"#;
    put(root, "t3.json", source);
    let file = load_project_file(root).await.expect("test fixture");
    assert_eq!(
        file.icon_path.as_ref().expect("test fixture").0,
        "assets/logo.svg"
    );
    assert_eq!(
        file.scripts.as_ref().expect("test fixture")[0].command.0,
        "bun dev"
    );
    let scripts = scripts_from_file(&file, ["id".into()]);
    assert_eq!(scripts[0].name.0, "Dev");
    assert!(setup_script(&scripts).is_some());
    assert_eq!(
        scripts[0].icon,
        t3_contracts::project::ProjectScriptIcon::Play
    );
    for invalid in [
        "{unquoted: true}",
        "{'scripts':[]}",
        r#"{"scripts": [{"name":"", "command":"x"}]}"#,
        r#"{"iconPath":null}"#,
        r#"{"defaultThreadEnvMode":"invalid"}"#,
    ] {
        assert!(parse_project_file(invalid).is_none(), "{invalid}");
    }
    let many =
        serde_json::json!({"scripts":vec![serde_json::json!({"name":"x","command":"x"});51]});
    assert!(parse_project_file(&many.to_string()).is_none());
}

#[tokio::test]
async fn favicon_precedence_overrides_project_file_well_known_and_sources() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let resolver = FaviconResolver::new(root.into());
    put(root, "assets/logo.svg", "logo");
    put(root, "public/favicon.png", "png");
    put(root, "favicon.ico", "ico");
    put(root, "favicon.svg", "svg");
    put(root, "custom/icon.svg", "custom");
    put(root, "t3.json", r#"{"iconPath":"custom/icon.svg"}"#);
    let root_str = root.to_string_lossy();
    assert_eq!(
        resolver
            .resolve(&root_str, Some("assets/logo.svg"))
            .await
            .expect("test fixture"),
        Some(root.join("assets/logo.svg"))
    );
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("custom/icon.svg"))
    );
    put(root, "t3.json", r#"{"iconPath":"../escape"}"#);
    resolver.invalidate(root).await;
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("favicon.svg"))
    );
    let external = tempfile::NamedTempFile::new().expect("test fixture");
    assert_eq!(
        resolver
            .resolve(&root_str, Some(&external.path().to_string_lossy()))
            .await
            .expect("test fixture"),
        Some(external.path().into())
    );
    fs::remove_file(root.join("favicon.svg")).expect("test fixture");
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("favicon.ico"))
    );
    for p in ["favicon.ico", "public/favicon.png", "assets/logo.svg"] {
        fs::remove_file(root.join(p)).expect("test fixture");
    }
    resolver.invalidate(root).await;
    put(root, "public/icons/site.svg", "svg");
    put(
        root,
        "index.html",
        r#"<link href=" /icons/site.svg?version=1" rel="icon">"#,
    );
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("public/icons/site.svg"))
    );
}

#[test]
fn source_icons_metadata_and_long_hostile_inputs() {
    assert_eq!(
        extract_icon_href(r#"<link href="/icon.svg?v=1" rel="shortcut icon">"#),
        Some("/icon.svg".into())
    );
    assert_eq!(
        extract_icon_href(r#"{rel: 'icon', href: '/logo.svg'}"#),
        Some("/logo.svg".into())
    );
    assert_eq!(extract_icon_href(&"{href: 'icon' }".repeat(2000)), None);
}

#[tokio::test(start_paused = true)]
async fn negative_and_positive_favicon_cache_expiry() {
    let temp = tempfile::tempdir().expect("test fixture");
    let root = temp.path();
    let resolver = FaviconResolver::new(root.into());
    let root_str = root.to_string_lossy();
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        None
    );
    put(root, "favicon.png", "png");
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        None
    );
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("favicon.png"))
    );
    put(root, "favicon.svg", "svg");
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("favicon.png"))
    );
    tokio::time::advance(std::time::Duration::from_secs(601)).await;
    assert_eq!(
        resolver
            .resolve(&root_str, None)
            .await
            .expect("test fixture"),
        Some(root.join("favicon.svg"))
    );
}

#[tokio::test(start_paused = true)]
async fn missing_workspace_errors_obey_the_negative_cache_window() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("missing");
    let resolver = FaviconResolver::new(temp.path().into());
    assert!(
        resolver
            .resolve(&root.to_string_lossy(), None)
            .await
            .is_err()
    );
    put(&root, "favicon.svg", "svg");
    assert!(
        resolver
            .resolve(&root.to_string_lossy(), None)
            .await
            .is_err()
    );
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    assert_eq!(
        resolver
            .resolve(&root.to_string_lossy(), None)
            .await
            .expect("after ttl"),
        Some(root.join("favicon.svg"))
    );
}

#[test]
fn script_override_inheritance_runtime_env_and_completion_protocol() {
    let file=parse_project_file(r#"{"scripts":[{"name":"Setup","command":"exit 2","runOnWorktreeCreate":true},{"name":"Settle","command":"cleanup","runOnSettle":true}]}"#).expect("test fixture");
    let scripts = scripts_from_file(&file, ["setup".into(), "settle".into()]);
    let mut settings = ScriptSettings {
        defaults: vec![scripts[1].clone()],
        ..Default::default()
    };
    assert_eq!(
        resolve_scripts(&settings, "project", &scripts),
        scripts.as_slice()
    );
    settings.legacy_overrides.insert("project".into(), None);
    assert_eq!(
        resolve_scripts(&settings, "project", &scripts),
        settings.defaults.as_slice()
    );
    settings.overrides.insert("project".into(), vec![]);
    assert!(resolve_scripts(&settings, "project", &scripts).is_empty());
    assert_eq!(
        settle_script(&scripts).expect("test fixture").id.0,
        "settle"
    );
    let env = script_runtime_env("project", Some("tree"));
    assert_eq!(env["T3CODE_PROJECT_ROOT"], "project");
    assert_eq!(env["T3CODE_WORKTREE_PATH"], "tree");
    assert_eq!(script_terminal_input("run"), " run\r");
    assert!(completion_shell("cmd.exe").is_err());
    assert_eq!(
        completion_shell("C:\\Program Files\\PowerShell\\7\\pwsh").expect("test fixture"),
        CompletionShell::PowerShell
    );
    for shell in ["C:/Users/john.exec/bin/pwsh.exe", "C:/pwsh.exe"] {
        assert_eq!(
            completion_shell(shell).expect("shell"),
            CompletionShell::PowerShell
        );
    }
    assert_eq!(
        completion_shell("/Applications/My Shells/fish").expect("shell"),
        CompletionShell::Fish
    );
    for shell in ["C:/tools.exe.d/cmd.exe", "C:/Program Files/x/cmd"] {
        assert!(completion_shell(shell).is_err(), "{shell}");
    }
    for shell in [
        CompletionShell::PowerShell,
        CompletionShell::Posix,
        CompletionShell::Fish,
        CompletionShell::Nushell,
    ] {
        let wrapper =
            wrap_script("echo a\n# trailing comment", shell, "abc123").expect("test fixture");
        assert!(wrapper.contains("__T3_SETUP_DONE___abc123:"));
    }
    let wrapper =
        wrap_script("echo test", CompletionShell::PowerShell, "abc123").expect("test fixture");
    let mut output = ScriptOutput::new("abc123", &wrapper).expect("test fixture");
    assert!(output.feed(&(wrapper + "\r\n")).is_empty());
    assert_eq!(
        output.feed("\x1b[31mred\x1b[0m\rprogress\n"),
        vec!["red", "progress"]
    );
    assert!(output.feed("__T3_SETUP_DONE___abc").is_empty());
    assert!(!output.settled);
    assert!(output.feed("123:2\r\n").is_empty());
    assert!(output.settled);
    assert_eq!(output.exit_code, Some(2));
    let mut output = ScriptOutput::new("secret", "").expect("test fixture");
    assert_eq!(
        output.feed("__T3_SETUP_DONE___spoof:0\n"),
        vec!["__T3_SETUP_DONE___spoof:0"]
    );
    assert!(!output.settled);
    output.closed();
    assert!(output.settled);
    assert_eq!(output.exit_code, None);
}

#[tokio::test]
async fn completion_wrapper_reports_the_native_shell_exit_code() {
    let temp = tempfile::tempdir().expect("test fixture");
    let (program, command, flags): (&str, &str, &[&str]) = if cfg!(windows) {
        (
            "powershell.exe",
            "& $env:ComSpec /d /c exit 7",
            &["-NoProfile", "-NonInteractive", "-Command"],
        )
    } else {
        ("/bin/sh", "exit 7", &["-c"])
    };
    let shell = completion_shell(program).expect("test fixture");
    let wrapper = wrap_script(command, shell, "real123")
        .expect("test fixture")
        .replace('\r', "\n");
    let mut options = t3_process::ProcessOptions::new(program)
        .args(flags.iter().copied().chain([wrapper.as_str()]));
    options.cwd = Some(temp.path().into());
    options.check_exit = false;
    let result = t3_process::run(options).await.expect("test fixture");
    let mut output = ScriptOutput::new("real123", "").expect("test fixture");
    output.feed(&result.stdout_text());
    assert_eq!(output.exit_code, Some(7));
    assert!(
        paths::normalize_root(&temp.path().to_string_lossy(), temp.path(), false)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn favicon_invalidation_normalizes_the_cache_key() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path();
    let root = home.join("project");
    fs::create_dir(&root).expect("project");
    let resolver = FaviconResolver::new(home.into());
    assert_eq!(
        resolver.resolve("~/project", None).await.expect("lookup"),
        None
    );
    put(&root, "favicon.svg", "svg");
    resolver.invalidate(&root).await;
    assert_eq!(
        resolver.resolve("~/project", None).await.expect("lookup"),
        Some(root.join("favicon.svg"))
    );
}
#[tokio::test]
async fn favicon_source_inspection_is_bounded() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    put(root, "icons/icon.svg", "svg");
    let mut source = " ".repeat(1024 * 1024 + 1);
    source.push_str(r#"<link rel="icon" href="/icons/icon.svg">"#);
    put(root, "index.html", &source);
    let resolver = FaviconResolver::new(root.into());
    assert_eq!(
        resolver
            .resolve(root.to_str().expect("path"), None)
            .await
            .expect("lookup"),
        None
    );
}

#[tokio::test]
async fn posix_completion_wrapper_reports_a_real_bash_exit_code() {
    #[cfg(windows)]
    let program = "C:/Program Files/Git/bin/bash.exe";
    #[cfg(unix)]
    let program = "bash";
    let temp = tempfile::tempdir().expect("temp");
    let wrapper = wrap_script(
        "printf 'bash output\\n'; (exit 7)",
        CompletionShell::Posix,
        "bash123",
    )
    .expect("wrapper")
    .replace('\r', "\n");
    let mut options =
        t3_process::ProcessOptions::new(program).args(["--noprofile", "--norc", "-c", &wrapper]);
    options.cwd = Some(temp.path().into());
    let result = t3_process::run(options).await.expect("bash");
    let mut output = ScriptOutput::new("bash123", "").expect("output");
    output.feed(&result.stdout_text());
    assert_eq!(output.exit_code, Some(7));
}
