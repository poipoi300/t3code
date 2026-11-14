#![allow(clippy::unwrap_used)]
use t3_terminal::*;

#[test]
fn bounded_history_matches_reference_across_unicode_chunks_and_clear() {
    for lines in [0, 1, 2, 10] {
        for bytes in [0, 1, 3, 17, 128] {
            let mut history = BoundedTerminalHistory::new(lines, bytes);
            let mut full = String::new();
            for chunk in [
                "one\n",
                "\n",
                "café\t界🙂",
                "e\u{301}\r\n",
                "\x1b[31mred\x1b[0m\n",
                "long partial ",
                "tail",
            ] {
                full.push_str(chunk);
                history.append(chunk);
                let expected = if lines == 0 || bytes == 0 {
                    if bytes > 0 && chunk.ends_with('\n') {
                        "\n".to_owned()
                    } else {
                        String::new()
                    }
                } else {
                    let mut retained = full.clone();
                    let count = retained.bytes().filter(|b| *b == b'\n').count()
                        + usize::from(!retained.ends_with('\n'));
                    for _ in 0..count.saturating_sub(lines) {
                        let end = retained.find('\n').unwrap() + 1;
                        retained.drain(..end);
                    }
                    retained_terminal_history(&retained, bytes)
                };
                assert_eq!(
                    history.value(),
                    expected,
                    "lines={lines},bytes={bytes},chunk={chunk:?}"
                );
                full = expected;
            }
            history.clear();
            assert_eq!(history.value(), "");
        }
    }
}

#[test]
fn history_sanitizer_ports_csi_dcs_osc_and_split_query_cases() {
    let cases = [
        ("A\x1b[0\0cB", "A\0B"),
        ("A\x1b[0\x7fcB", "AB"),
        ("A\x1b[?2026\0$pB", "A\0B"),
        ("AB\x1b[5\rnC", "AB\rC"),
        ("AB\x1b[5\x10nC", "ABC"),
        ("A\x1b[0écB", "AB"),
        ("A\x1b[嚠B", "A\u{fffd}B"),
        ("A\x1b[\u{90}\u{9b}6nB", "A\x1bP\u{9c}B"),
        ("A\x1b[5\x1b[6nB", "AB"),
        ("A\x1b[界nB", "A\x1b[界nB"),
        (
            "before \x1b[?2026$pafter \x1bP$q m\x1b\\after \x1b[?31uafter \x1bP+q544e\x1b\\after\n",
            "before after after after after\n",
        ),
        (
            "A\x1b]11;?\u{9c}HIDDEN\x1b\\B\nC\x1bP$qm\u{9c}SHOWN\x1b\\D\n",
            "AB\nCSHOWN\x1b\\D\n",
        ),
        (
            "\x1b[!p\x1b[\"p\x1b[4 q\x1b[u",
            "\x1b[!p\x1b[\"p\x1b[4 q\x1b[u",
        ),
        (
            "\x1b]52;c;YWJj\x07\x1bPplain\x07payload\x1b\\",
            "\x1b]52;c;YWJj\x07\x1bPplain\x07payload\x1b\\",
        ),
        (
            "café\t界🙂\u{9c}\u{85} â€\u{9d}\x1b[5ne\u{301}\r\n",
            "café\t界🙂\u{9c}\u{85} â€\u{9d}e\u{301}\r\n",
        ),
    ];
    for (input, expected) in cases {
        for split in input.char_indices().map(|(i, _)| i).chain([input.len()]) {
            let mut sanitizer = HistorySanitizer::default();
            let mut output = sanitizer.push(&input[..split]);
            output.push_str(&sanitizer.push(&input[split..]));
            assert_eq!(output, expected, "input={input:?},split={split}");
        }
    }
}

#[test]
fn cuts_drop_control_sequence_tails_and_remember_state_across_appends() {
    for (text, bytes, expected) in [
        ("prefix\x1b[31mOK", 6, "OK"),
        ("prefix\x1b]0;HIDDEN\x07OK", 10, "OK"),
        ("prefix\x1b]0;🙂HIDDEN\x07OK", 9, "OK"),
        ("prefix\x1bP🙂HELLO\x1b\\OK", 9, "HELLO\x1b\\OK"),
    ] {
        for split in text.char_indices().map(|(at, _)| at).chain([text.len()]) {
            let mut history = BoundedTerminalHistory::new(5000, bytes);
            history.append(&text[..split]);
            history.append(&text[split..]);
            assert_eq!(history.value(), expected, "{text:?} split={split}");
        }
        assert_eq!(retained_terminal_history(text, bytes), expected);
    }
    let mut history = BoundedTerminalHistory::new(1, 8);
    history.append("prefix\x1b]0;this is still hidden");
    assert_eq!(history.value(), "");
    history.append("more hidden\x07OK");
    assert_eq!(history.value(), "OK");
    history.clear();
    history.append("VISIBLE");
    assert_eq!(history.value(), "VISIBLE");
}

#[test]
fn persisted_log_tails_scan_the_prefix_before_restoring_and_compacting() {
    let temp = tempfile::tempdir().expect("temp");
    let store = HistoryStore::new(temp.path(), 5, 8);
    for persisted in [
        "\x1b]0;x\x07ABCD",
        "名\x1b]0;名\x07ABCD",
        "\x1b]0;x\x07\x1b[31mABCD",
    ] {
        store.persist("t", "term-1", persisted).expect("persist");
        assert_eq!(store.read("t", "term-1").expect("read"), "ABCD");
        assert_eq!(
            std::fs::read_to_string(store.path("t", "term-1")).expect("compacted"),
            "ABCD"
        );
    }
    let persisted = "\x1b]0;".to_owned() + &"x".repeat(65_530) + "名\x07ABCD";
    store.persist("t", "term-1", &persisted).expect("persist");
    assert_eq!(store.read("t", "term-1").expect("read"), "ABCD");
}

#[test]
fn legacy_history_migrates_and_long_names_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let store = HistoryStore::new(temp.path(), 2, 64);
    std::fs::write(temp.path().join("thread_1.log"), "one\ntwo\nthree\n").unwrap();
    assert_eq!(store.read("thread/1", "term-1").unwrap(), "two\nthree\n");
    assert_eq!(
        store.path("thread/1", "term-1").file_name().unwrap(),
        "terminal_dGhyZWFkLzE.log"
    );
    assert!(!temp.path().join("thread_1.log").exists());
    assert_eq!(
        std::fs::read_to_string(store.path("thread/1", "term-1")).unwrap(),
        "two\nthree\n"
    );
    for (thread, terminal) in [
        ("界".repeat(200), "term-1".into()),
        ("a".repeat(200), "z".repeat(128)),
    ] {
        assert_eq!(store.read(&thread, &terminal).unwrap(), "");
        let path = store.path(&thread, &terminal);
        assert!(path.file_name().unwrap().len() <= 143);
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("terminal_sha256.")
        );
        store.persist(&thread, &terminal, "hé🙂\n").unwrap();
        assert_eq!(store.read(&thread, &terminal).unwrap(), "hé🙂\n");
        store.delete(&thread, None).unwrap();
        assert!(!path.exists());
    }
}

#[test]
fn environment_and_shell_choices_match_platform_rules() {
    let env = Environment::from([
        ("SystemRoot".into(), "D:\\Windows".into()),
        ("Path".into(), "C:\\tools".into()),
        ("Tmux".into(), "bad".into()),
        ("VITE_URL".into(), "bad".into()),
        ("T3CODE_TOKEN".into(), "bad".into()),
        ("MY_TOOL".into(), "yes".into()),
    ]);
    let shells = shell_candidates(Platform::Windows, &env, Some("missing.exe"));
    assert_eq!(
        shells[1],
        ShellCandidate {
            shell: "pwsh.exe".into(),
            args: vec!["-NoLogo".into()]
        }
    );
    assert_eq!(
        shells[2].shell,
        "D:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    );
    let runtime = Environment::from([
        ("PATH".into(), "D:\\user".into()),
        ("codex_home".into(), "~/agent".into()),
    ]);
    let result = spawn_environment(
        &env,
        Some(&runtime),
        Platform::Windows,
        std::path::Path::new("C:/home"),
        "1",
        &["E:\\managed".into()],
    );
    assert_eq!(result["Path"], "D:\\user;E:\\managed");
    assert!(!result.contains_key("PATH"));
    assert!(!result.contains_key("T3CODE_TOKEN"));
    assert!(!result.contains_key("Tmux"));
    assert_eq!(result["MY_TOOL"], "yes");
    assert_eq!(result["TERM"], "xterm-256color");
    assert_eq!(result["fish_features"], "no-query-term");
    assert_eq!(result["TERM_PROGRAM"], "t3code");
    assert_eq!(
        result["codex_home"],
        std::path::Path::new("C:/home")
            .join("agent")
            .to_str()
            .unwrap()
    );
    assert_eq!(result["COLORTERM"], "truecolor");
    assert_eq!(
        shell_candidates(Platform::Mac, &Environment::new(), Some("/bin/bash"))[0].args,
        ["-l"]
    );
    assert!(
        shell_candidates(Platform::Posix, &Environment::new(), Some("/bin/bash"))[0]
            .args
            .is_empty()
    );
    assert_eq!(retained_terminal_history("a🙂\u{feff}", 5), "\u{feff}");
}

#[test]
fn wire_shapes_preserve_omitted_nullable_and_event_fields() {
    let input: TerminalOpenInput = serde_json::from_value(
        serde_json::json!({"threadId":"t","terminalId":"term-1","cwd":"C:/","worktreePath":null}),
    )
    .unwrap();
    assert_eq!(input.worktree_path, Some(None));
    assert_eq!(
        serde_json::to_value(input).unwrap(),
        serde_json::json!({"threadId":"t","terminalId":"term-1","cwd":"C:/","worktreePath":null})
    );
    let event:TerminalAttachStreamEvent=serde_json::from_value(serde_json::json!({"type":"exited","threadId":"t","terminalId":"term-1","sequence":2,"exitCode":7,"exitSignal":null})).unwrap();
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        serde_json::json!({"type":"exited","threadId":"t","terminalId":"term-1","sequence":2,"exitCode":7,"exitSignal":null})
    );
}

#[test]
fn environment_normalizes_paths_and_case_insensitive_windows_shell_keys() {
    let env = Environment::from([
        ("systemroot".into(), "D:\\OS".into()),
        ("comspec".into(), "E:\\cmd.exe".into()),
    ]);
    let shells = shell_candidates(Platform::Windows, &env, None);
    assert_eq!(
        shells[1].shell,
        "D:\\OS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    );
    assert!(
        shells
            .iter()
            .position(|s| s.shell == "E:\\cmd.exe")
            .unwrap()
            < shells
                .iter()
                .position(|s| s.shell == "D:\\OS\\System32\\cmd.exe")
                .unwrap()
    );
    let base = Environment::from([("PATH".into(), " /bin :/bin:: /usr/bin ".into())]);
    let runtime = Environment::from([("CODEX_HOME".into(), "~//agent".into())]);
    let result = spawn_environment(
        &base,
        Some(&runtime),
        Platform::Posix,
        std::path::Path::new("/home/test"),
        "test",
        &["/bin".into(), "/managed".into()],
    );
    assert_eq!(result["PATH"], "/bin:/usr/bin:/managed");
    assert_eq!(
        result["CODEX_HOME"],
        std::path::Path::new("/home/test")
            .join("agent")
            .to_str()
            .unwrap()
    );
    assert_eq!(result["TERM"], "xterm-256color");
}

#[test]
fn appimage_scrubbing_colorterm_override_and_posix_term() {
    let base = Environment::from([
        ("APPIMAGE".into(), "/tmp/App.AppImage".into()),
        ("APPDIR".into(), "/tmp/app".into()),
        ("ARGV0".into(), "bad".into()),
        ("OWD".into(), "bad".into()),
        ("PATH".into(), "/tmp/app/bin:/usr/bin:/tmp/app".into()),
        ("LD_LIBRARY_PATH".into(), "/tmp/app/lib:/usr/lib".into()),
        ("TERM".into(), "dumb".into()),
    ]);
    let runtime = Environment::from([("COLORTERM".into(), "".into())]);
    let env = spawn_environment(
        &base,
        Some(&runtime),
        Platform::Posix,
        std::path::Path::new("/home"),
        "test",
        &[],
    );
    for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        assert!(!env.contains_key(key));
    }
    assert_eq!(env["PATH"], "/usr/bin");
    assert_eq!(env["LD_LIBRARY_PATH"], "/usr/lib");
    assert_eq!(env["COLORTERM"], "");
    assert_eq!(env["TERM"], "xterm-256color");
}

#[test]
fn history_splits_large_multibyte_chunks_at_utf8_boundaries() {
    let mut history = BoundedTerminalHistory::new(5000, 100000);
    let mut full = String::new();
    for chunk in [
        "界🙂x".repeat(10000),
        "é\n".repeat(7000),
        "tail🙂".repeat(20000),
    ] {
        full.push_str(&chunk);
        history.append(&chunk);
        let lines =
            full.bytes().filter(|b| *b == b'\n').count() + usize::from(!full.ends_with('\n'));
        for _ in 0..lines.saturating_sub(5000) {
            let end = full.find('\n').unwrap() + 1;
            full.drain(..end);
        }
        full = retained_terminal_history(&full, 100000);
        assert_eq!(history.value(), full);
    }
}

#[test]
fn canonical_types_reject_missing_nullable_snapshot_keys() {
    let value = serde_json::json!({"threadId":"t","terminalId":"term-1","cwd":"/","status":"exited","history":"","exitCode":null,"exitSignal":null,"label":"Terminal 1","updatedAt":"now","worktreePath":null});
    assert!(serde_json::from_value::<TerminalSessionSnapshot>(value.clone()).is_err());
    let mut complete = value;
    complete["pid"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<TerminalSessionSnapshot>(complete).is_ok());
}
