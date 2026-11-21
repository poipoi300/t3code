use std::{collections::BTreeMap, path::Path, sync::Arc};
use t3_host::{service_launcher::*, triage::*};
fn plan(v: &serde_json::Value) -> ServicePlan {
    ServicePlan {
        program: serde_json::from_value(v["program"].clone()).unwrap(),
        base_dir: v["baseDir"].as_str().unwrap().into(),
        log_path: v["logPath"].as_str().unwrap().into(),
        install_bin_dir: v
            .get("installBinDir")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    }
}
#[test]
fn service_definitions_are_byte_identical_to_typescript() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("service-definitions.json")).unwrap();
    let p = plan(&fixture["plan"]);
    assert_eq!(
        render_systemd(&p).unwrap(),
        fixture["systemd"].as_str().unwrap()
    );
    assert_eq!(
        render_launchd(&p, "/home/test", "/usr/bin:/x&y"),
        fixture["launchd"].as_str().unwrap()
    );
    assert_eq!(
        render_windows_task(
            &plan(&fixture["windows"]),
            "DOMAIN\\user&other",
            "C:\\Users\\test",
            "C:\\Windows\\System32\\conhost.exe"
        )
        .unwrap(),
        fixture["task"].as_str().unwrap()
    );
}
#[test]
fn service_executable_injection_and_protected_environment_are_refused() {
    let mut p = ServicePlan {
        program: vec!["/a/t3".into()],
        base_dir: "/a".into(),
        log_path: "/a/log".into(),
        install_bin_dir: None,
    };
    for value in ["/a/quote\"/t3", "/a\nExecStart=evil", "C:\\a\\t3"] {
        p.program[0] = value.into();
        assert!(render_systemd(&p).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("service.env"),"PATH=evil\nT3CODE_HOME=other\nT3_BOOT_SERVICE_UNIT=other\nT3_SERVICE_LAUNCHER_CONTEXT=evil\nexport T3CODE_PORT=1234 # comment\nCUSTOM='a # b'\nMULTILINE=\"first\nsecond\"\nESCAPED=\"a\\nb\"\nLITERAL=\"literal $HOME ${USER} $(whoami) # token\"\nEMPTY=\ninvalid-name=no\n").unwrap();
    let inherited = BTreeMap::from([("PATH".into(), "original".into())]);
    let env = read_service_environment(temp.path(), &inherited).unwrap();
    assert_eq!(env["PATH"], "original");
    assert_eq!(env["T3CODE_PORT"], "1234");
    assert_eq!(env["CUSTOM"], "a # b");
    assert_eq!(env["MULTILINE"], "first\nsecond");
    assert_eq!(env["ESCAPED"], "a\nb");
    assert_eq!(env["LITERAL"], "literal $HOME ${USER} $(whoami) # token");
    assert_eq!(env["EMPTY"], "");
    assert!(!env.contains_key("T3CODE_HOME"));
    assert!(!env.contains_key("invalid-name"));
}
#[test]
fn source_ref_prefers_peeled_tag_then_main_and_rejects_unsafe_commit_names() {
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let c = "c".repeat(40);
    let listed =
        format!("{a}\trefs/tags/v1.0.0\n{b}\trefs/tags/v1.0.0^{{}}\n{c}\trefs/heads/main\n");
    assert_eq!(pick_source_ref(&listed, "1.0.0").unwrap().commit, b);
    assert_eq!(pick_source_ref(&listed, "nightly").unwrap().commit, c);
    assert!(pick_source_ref("../../evil\trefs/heads/main", "nightly").is_none());
    assert!(pick_source_ref("", "1").is_none());
}

#[test]
fn service_state_validation_and_version_precedence_match_launcher_protocol() {
    for version in [
        "0.0.0",
        "1.2.3",
        "1.2.3-alpha.1",
        "1.2.3-0",
        "1.2.3+001",
        "9999999999999999999999999.0.0",
    ] {
        assert!(is_exact_service_version(version));
    }
    for version in ["latest", "01.2.3", "1.2.3-01", "1.2.3-alpha..1", "1.2.3+."] {
        assert!(!is_exact_service_version(version));
    }
    assert!(
        compare_exact_service_versions("9999999999999999999999999.0.0", "2.0.0")
            .unwrap()
            .is_gt()
    );
    assert!(
        compare_exact_service_versions("1.2.3+one", "1.2.3+two")
            .unwrap()
            .is_eq()
    );
    assert!(
        compare_exact_service_versions("2.0.0-alpha.2", "2.0.0-alpha.beta")
            .unwrap()
            .is_lt()
    );
    for status in ["pending", "failed", "rolled-back"] {
        let state = serde_json::json!({"protocol":4,"activeVersion":"1.0.0","update":{"id":"u","fromVersion":"0.9.0","targetVersion":"2.0.0","dbPath":"/db","status":status}});
        assert!(parse_service_state(&state.to_string()).is_err());
    }
    assert!(
        parse_service_state(r#"{"protocol":4,"activeVersion":"1.0.0","update":null}"#).is_err()
    );
    let state = parse_service_state(r#"{"protocol":2,"activeVersion":"1.0.0"}"#).unwrap();
    assert_eq!(state.protocol, 4);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime/service-state.json");
    write_service_state(&path, &state).unwrap();
    assert_eq!(read_service_state(&path).unwrap(), state);
    assert!(std::fs::read_to_string(path).unwrap().ends_with('\n'));
}

#[cfg(windows)]
#[test]
fn triage_context_and_seed_prompts_match_typescript_for_cli_app_and_source_states() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("triage-contexts.json")).unwrap();
    let p = Path::new("C:\\T3 home\\userdata");
    let paths = TriagePaths {
        base_dir: "C:\\T3 home".into(),
        state_dir: p.into(),
        db_path: p.join("statev2.sqlite"),
        settings_path: p.join("settings.json"),
        logs_dir: p.join("logs"),
        server_trace_path: p.join("logs").join("server.trace.ndjson"),
        provider_event_log_path: p.join("logs").join("provider").join("events.log"),
        terminal_logs_dir: p.join("logs").join("terminals"),
        provider_status_cache_dir: Path::new("C:\\T3 home").join("caches"),
        secrets_dir: p.join("secrets"),
    };
    let service = TriageService::new(
        paths,
        TriageMachine {
            os: "win32 x64 (test)".into(),
            runtime_version: "not applicable (native Rust server)".into(),
            server_version: "1.0.0".into(),
            pid: 0,
        },
        Arc::new(Time),
    );
    for case in cases {
        let input = &case["input"];
        let raw = &input["entry"];
        let entry = if raw["kind"] == "cli" {
            TriageEntry::Cli {
                cli_version: raw["cliVersion"].as_str().unwrap().into(),
                launched_as: raw["launchedAs"].as_str().unwrap().into(),
            }
        } else {
            TriageEntry::App {
                client: raw["client"].as_str().unwrap().into(),
            }
        };
        let source = input.get("sourceClone").map(|s| {
            if s["_tag"] == "failed" {
                SourceClone::Failed {
                    reason: s["reason"].as_str().unwrap().into(),
                }
            } else {
                let source = SourceRef {
                    reference: s["ref"].as_str().unwrap().into(),
                    commit: s["commit"].as_str().unwrap().into(),
                };
                let directory = s["directory"].as_str().unwrap().into();
                if s["_tag"] == "cloned" {
                    SourceClone::Cloned { source, directory }
                } else {
                    SourceClone::Cloning { source, directory }
                }
            }
        });
        assert_eq!(
            service.build_context(
                input["generatedAt"].as_str().unwrap(),
                &entry,
                "running test",
                source.as_ref()
            ),
            case["context"].as_str().unwrap()
        );
        assert_eq!(
            build_seed_prompt(Path::new("context.md"), &entry),
            case["seed"].as_str().unwrap()
        );
    }
}
struct Time;
impl t3_host::background::Clock for Time {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
async fn git(cwd: &Path, args: &[&str]) {
    let mut options = t3_process::ProcessOptions::new("git").args(args.iter().copied());
    options.cwd = Some(cwd.into());
    options.env.insert(
        "GIT_CONFIG_GLOBAL".into(),
        Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
    );
    options
        .env
        .insert("GIT_CONFIG_NOSYSTEM".into(), Some("1".into()));
    t3_process::run(options).await.unwrap();
}
#[tokio::test]
async fn triage_writes_playbook_and_atomically_publishes_exact_local_source_commit() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("source.txt"), "source").unwrap();
    git(&repo, &["add", "."]).await;
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    )
    .await;
    git(&repo, &["tag", "v1.0.0"]).await;
    let state = temp.path().join("state with spaces");
    let base = temp.path().join("home with spaces");
    let paths = TriagePaths {
        base_dir: base.clone(),
        state_dir: state.clone(),
        db_path: state.join("statev2.sqlite"),
        settings_path: state.join("settings.json"),
        logs_dir: state.join("logs"),
        server_trace_path: state.join("logs/server.trace.ndjson"),
        provider_event_log_path: state.join("logs/provider/events.log"),
        terminal_logs_dir: state.join("logs/terminals"),
        provider_status_cache_dir: base.join("caches"),
        secrets_dir: state.join("secrets"),
    };
    let abandoned = base.join("source-staging").join("clone-abandoned");
    std::fs::create_dir_all(&abandoned).unwrap();
    std::fs::write(abandoned.join("partial"), "unfinished").unwrap();
    let service = TriageService::with_repository(
        paths,
        TriageMachine {
            os: "win32 x64 (test)".into(),
            runtime_version: "rust test".into(),
            server_version: "1.0.0".into(),
            pid: 123,
        },
        Arc::new(Time),
        repo.to_string_lossy().into_owned(),
    )
    .with_git_environment(BTreeMap::from([
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
        ),
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
    ]));
    let result = service
        .prepare(
            serde_json::from_value(serde_json::json!({"client":{"label":"Test client"}})).unwrap(),
        )
        .await
        .unwrap();
    assert!(result.prompt.starts_with("Read [triage-playbook.md]("));
    assert!(result.prompt.contains("state%20with%20spaces"));
    let session = std::fs::read_dir(state.join("triage"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let playbook = std::fs::read_to_string(session.join("triage-playbook.md")).unwrap();
    assert!(playbook.ends_with(TRIAGE_PLAYBOOK));
    let context = std::fs::read_to_string(session.join("context.md")).unwrap();
    assert!(context.contains("NEVER read this"));
    assert!(context.contains("this session runs inside it"));
    service.drain().await;
    assert!(!abandoned.exists());
    let source = std::fs::read_dir(base.join("source"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read_to_string(source.join("source.txt")).unwrap(),
        "source"
    );
}

#[tokio::test]
async fn stopping_during_backup_aborts_copy_preserves_pending_and_starts_no_trial() {
    use async_trait::async_trait;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use t3_host::service_launcher::*;
    struct Copy {
        started: tokio::sync::Notify,
    }
    #[async_trait]
    impl BackupCopy for Copy {
        async fn try_clone(
            &self,
            _: &std::path::Path,
            destination: &std::path::Path,
            cancel: t3_process::CancellationToken,
        ) -> t3_host::Result<bool> {
            std::fs::write(destination, "partial").unwrap();
            self.started.notify_one();
            cancel.cancelled().await;
            Ok(false)
        }
    }
    struct Trial(AtomicUsize);
    #[async_trait]
    impl TrialHost for Trial {
        async fn start_trial(&self, _: &ServiceUpdate) -> t3_host::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("userdata/state.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, "database").unwrap();
    let state:ServiceState=serde_json::from_value(serde_json::json!({"protocol":4,"activeVersion":"1.0.0","update":{"id":"update-1","fromVersion":"1.0.0","targetVersion":"1.1.0","dbPath":db,"status":"pending"}})).unwrap();
    let state_path = temp.path().join("runtime/service-state.json");
    write_service_state(&state_path, &state).unwrap();
    let copy = Arc::new(Copy {
        started: tokio::sync::Notify::new(),
    });
    let trial = Arc::new(Trial(AtomicUsize::new(0)));
    let cancel = t3_process::CancellationToken::new();
    let task = {
        let base = temp.path().to_owned();
        let copy = copy.clone();
        let trial = trial.clone();
        let cancel = cancel.clone();
        let state = state.clone();
        tokio::spawn(async move {
            prepare_and_start_trial(&base, &state, copy.as_ref(), trial.as_ref(), cancel).await
        })
    };
    copy.started.notified().await;
    cancel.cancel();
    assert!(!task.await.unwrap().unwrap());
    assert_eq!(trial.0.load(Ordering::SeqCst), 0);
    assert_eq!(read_service_state(&state_path).unwrap(), state);
    assert!(!temp.path().join("runtime/db-backup/update-1").exists());
    assert!(
        !temp
            .path()
            .join("runtime/db-backup/update-1.staging")
            .exists()
    );
    assert_eq!(std::fs::read_to_string(db).unwrap(), "database");
}
