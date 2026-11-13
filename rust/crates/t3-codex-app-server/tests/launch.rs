use std::collections::BTreeMap;
use t3_codex_app_server::launch::*;

fn input(platform: Platform) -> LaunchInput {
    LaunchInput {
        platform,
        binary_path: String::new(),
        resolved_executable: None,
        home_directory: if platform == Platform::Windows {
            "C:\\Users\\test"
        } else {
            "/home/test"
        }
        .into(),
        effective_codex_home: None,
        launch_args: None,
        host_environment: BTreeMap::new(),
        instance_environment: vec![],
        cwd: None,
        extend_env: None,
    }
}

#[test]
fn launch_args_match_typescript_tokenization_and_precedence() {
    for (source, expected) in [
        (
            r#"--config model=\"gpt-6.1-sol\" --strict-config"#,
            vec!["--config", "model=\"gpt-6.1-sol\"", "--strict-config"],
        ),
        (
            "-c thread_unload_delay_secs=1800",
            vec!["-c", "thread_unload_delay_secs=1800"],
        ),
        (
            r#"--config model="gpt 5" '' "" C:\tools\codex"#,
            vec!["--config", "model=gpt 5", "", "", "C:\\tools\\codex"],
        ),
        ("'unterminated value", vec!["unterminated value"]),
    ] {
        let mut i = input(Platform::Posix);
        i.launch_args = Some(source.into());
        let spec = app_server_launch(i);
        assert_eq!(
            spec.args,
            [
                vec!["app-server", "-c", "thread_unload_delay_secs=0"],
                expected
            ]
            .concat()
        );
        assert_eq!(spec.command, "codex");
        assert_eq!(spec.cwd, None);
        assert!(!spec.shell);
        assert!(spec.shared);
    }
    let mut i = input(Platform::Posix);
    i.launch_args = Some("--strict-config".into());
    i.host_environment
        .insert("T3CODE_CODEX_LAUNCH_ARGS".into(), " --enable foo ".into());
    assert_eq!(&app_server_launch(i.clone()).args[3..], ["--enable", "foo"]);
    i.host_environment
        .insert("T3CODE_CODEX_LAUNCH_ARGS".into(), "   ".into());
    assert_eq!(&app_server_launch(i).args[3..], ["--strict-config"]);
}

#[test]
fn launch_preserves_instance_environment_home_resolution_and_probe_cwd() {
    let mut posix = input(Platform::Posix);
    posix.binary_path = "~\\tools\\codex".into();
    assert_eq!(app_server_launch(posix).command, "/home/test/tools\\codex");
    let mut i = input(Platform::Windows);
    i.host_environment.insert("Path".into(), "inherited".into());
    i.host_environment
        .insert("CODEX_HOME".into(), "inherited-home".into());
    i.instance_environment = vec![
        ("path".into(), "configured".into()),
        ("codex_home".into(), "~/private".into()),
        ("codex_internal_originator_override".into(), "Custom".into()),
    ];
    i.binary_path = "~\\bin\\codex.exe".into();
    let spec = app_server_launch(i.clone());
    assert_eq!(spec.command, "C:\\Users\\test\\bin\\codex.exe");
    assert_eq!(spec.environment["PATH"], "configured");
    assert!(!spec.environment.contains_key("Path"));
    assert_eq!(spec.environment["CODEX_HOME"], "C:\\Users\\test\\private");
    assert_eq!(
        spec.environment["CODEX_INTERNAL_ORIGINATOR_OVERRIDE"],
        "Custom"
    );
    i.effective_codex_home = Some("~/effective".into());
    i.cwd = Some("C:/workspace".into());
    i.extend_env = Some(true);
    let spec = app_server_launch(i);
    assert_eq!(spec.environment["CODEX_HOME"], "C:\\Users\\test\\effective");
    assert_eq!(spec.cwd, Some("C:/workspace".into()));
    assert_eq!(spec.extend_env, Some(true));
}

#[test]
fn windows_native_and_batch_resolution_match_the_shared_shell_contract() {
    let mut i = input(Platform::Windows);
    i.resolved_executable = Some("C:\\Program Files\\Codex\\codex.exe".into());
    let spec = app_server_launch(i.clone());
    assert!(!spec.shell);
    assert_eq!(spec.command, "C:\\Program Files\\Codex\\codex.exe");
    i.resolved_executable = Some("C:\\Program Files\\npm\\codex.CMD".into());
    i.launch_args = Some(r#"--config 'key=a&b' ''"#.into());
    let spec = app_server_launch(i);
    assert!(spec.shell);
    assert!(spec.args.is_empty());
    assert_eq!(
        spec.command,
        "^\"C:\\Program^ Files\\npm\\codex.CMD^\" ^\"app-server^\" ^\"-c^\" ^\"thread_unload_delay_secs=0^\" ^\"--config^\" ^\"key=a^&b^\" ^\"^\""
    );
}

#[test]
fn windows_batch_escaping_matches_the_typescript_shell_regression_vectors() {
    let mut i = input(Platform::Windows);
    i.resolved_executable = Some(r"C:\Program Files\npm & tools\codex.cmd".into());
    i.launch_args = Some(
        r#"'value & calc' '%PATH%' 'quote"value' 'C:\dir with space\' 'slash\"quote' 'x|y<z>^'"#
            .into(),
    );
    let spec = app_server_launch(i);
    assert!(spec.shell);
    assert!(spec.args.is_empty());
    assert_eq!(
        spec.command,
        [
            r#"^"C:\Program^ Files\npm^ ^&^ tools\codex.cmd^""#,
            r#"^"app-server^""#,
            r#"^"-c^""#,
            r#"^"thread_unload_delay_secs=0^""#,
            r#"^"value^ ^&^ calc^""#,
            r#"^"^%PATH^%^""#,
            r#"^"quote\^"value^""#,
            r#"^"C:\dir^ with^ space\\^""#,
            r#"^"slash\\\^"quote^""#,
            r#"^"x^|y^<z^>^^^""#,
        ]
        .join(" ")
    );
}
