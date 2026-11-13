use serde_json::json;
use t3_config::{home::HomeLayout, keybindings::KeybindingsService, settings::SettingsService};

// FileSystem.ts readFileString uses TextDecoder; schemaJson.ts accepts comments/trailing commas.
// serverSettings.ts loadSettingsFromDisk and keybindings.ts loadRuntimeCustomKeybindingsConfig
// recover from parse/schema failures, but propagate filesystem failures.
#[tokio::test]
async fn typescript_file_encoding_and_shape_table() {
    let defaults_temp = tempfile::tempdir().unwrap();
    let defaults_home = HomeLayout::derive(defaults_temp.path(), false, true);
    let mut settings_defaults = serde_json::to_value(
        SettingsService::new(defaults_home.settings())
            .snapshot()
            .await
            .unwrap(),
    )
    .unwrap();
    settings_defaults["continueThreadsAfterServerUpdate"] = json!(false);
    let keybindings_defaults = KeybindingsService::new(defaults_home.keybindings())
        .snapshot()
        .await
        .unwrap()
        .keybindings;
    for settings in [true, false] {
        let valid = if settings {
            r#"{"threadTitleInstructions":"loaded"}"#
        } else {
            r#"[{"key":"alt+q","command":"chat.new"}]"#
        };
        let utf16 = |big_endian: bool| {
            let mut bytes = if big_endian {
                vec![0xfe, 0xff]
            } else {
                vec![0xff, 0xfe]
            };
            for unit in valid.encode_utf16() {
                bytes.extend(if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            bytes
        };
        let commented = if settings {
            "{/* block */\r\n// line\r\n\"threadTitleInstructions\":\"loaded\",}\r\n"
        } else {
            "[/* block */\r\n// line\r\n{\"key\":\"alt+q\",\"command\":\"chat.new\",},]\r\n"
        };
        let mut lossy_string = valid
            .replace(if settings { "loaded" } else { "alt+q" }, "INVALID")
            .into_bytes();
        let offset = lossy_string
            .windows(7)
            .position(|w| w == b"INVALID")
            .unwrap();
        lossy_string.splice(offset..offset + 7, [0xff]);
        let cases = vec![
            ("utf8", valid.as_bytes().to_vec(), true),
            ("bom", format!("\u{feff}{valid}").into_bytes(), true),
            ("bom only", "\u{feff}".as_bytes().to_vec(), false),
            (
                "double bom",
                format!("\u{feff}\u{feff}{valid}").into_bytes(),
                false,
            ),
            ("crlf", format!("\r\n{valid}\r\n").into_bytes(), true),
            ("newline", format!("{valid}\n").into_bytes(), true),
            (
                "comments and trailing commas",
                commented.as_bytes().to_vec(),
                true,
            ),
            ("empty", vec![], false),
            ("whitespace", b" \r\n\t".to_vec(), false),
            (
                "comments only",
                b"/* user notes */ // more notes\n".to_vec(),
                false,
            ),
            ("utf16le", utf16(false), false),
            ("utf16be", utf16(true), false),
            ("invalid utf8 outside string", vec![0xff, b'{', b'}'], false),
            ("invalid utf8 inside string", lossy_string, true),
            (
                "half written",
                valid.as_bytes()[..valid.len() / 2].to_vec(),
                false,
            ),
            ("null", b"null".to_vec(), false),
            (
                "wrong root",
                if settings {
                    b"[]".to_vec()
                } else {
                    b"{}".to_vec()
                },
                false,
            ),
            (
                "large (8 MiB padding)",
                format!("{}{valid}", " ".repeat(8 * 1024 * 1024)).into_bytes(),
                true,
            ),
        ];
        for (name, bytes, loads) in cases {
            let temp = tempfile::tempdir().unwrap();
            let home = HomeLayout::derive(temp.path(), false, true);
            std::fs::create_dir_all(&home.state_dir).unwrap();
            let path = if settings {
                home.settings()
            } else {
                home.keybindings()
            };
            std::fs::write(&path, &bytes).unwrap();
            if settings {
                let service = SettingsService::new(&path);
                let state = service
                    .reload()
                    .await
                    .unwrap_or_else(|e| panic!("settings {name}: {e}"));
                assert_eq!(state.parse_failure.is_none(), loads, "settings {name}");
                if loads {
                    let expected = if name == "invalid utf8 inside string" {
                        "\u{fffd}"
                    } else {
                        "loaded"
                    };
                    assert_eq!(
                        serde_json::to_value(&state.settings).unwrap()["threadTitleInstructions"],
                        expected,
                        "{name}"
                    );
                } else {
                    assert_eq!(
                        serde_json::to_value(&state.settings).unwrap(),
                        settings_defaults,
                        "settings defaults for {name}"
                    );
                    assert!(
                        service
                            .update(
                                &serde_json::from_value(json!({"threadTitleInstructions":"edit"}))
                                    .unwrap()
                            )
                            .await
                            .is_err()
                    );
                }
            } else {
                let service = KeybindingsService::new(&path);
                let state = service
                    .reload()
                    .await
                    .unwrap_or_else(|e| panic!("keybindings {name}: {e}"));
                assert_eq!(state.issues.is_empty(), loads, "keybindings {name}");
                if loads {
                    let expected = if name == "invalid utf8 inside string" {
                        "\u{fffd}"
                    } else {
                        "q"
                    };
                    assert!(
                        state
                            .keybindings
                            .0
                            .iter()
                            .any(|r| r.shortcut.key == expected),
                        "{name}"
                    );
                } else {
                    assert_eq!(
                        state.keybindings, keybindings_defaults,
                        "keybindings defaults for {name}"
                    );
                    assert!(
                        matches!(
                            state.issues.as_slice(),
                            [t3_config::keybindings::ConfigIssue::MalformedConfig(_)]
                        ),
                        "{name}"
                    );
                    service.sync_defaults().await.unwrap();
                    assert!(
                        service
                            .upsert(
                                serde_json::from_value(json!({"key":"alt+x","command":"chat.new"}))
                                    .unwrap(),
                                None
                            )
                            .await
                            .is_err()
                    );
                }
            }
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "file changed: settings={settings}, {name}"
            );
        }
    }
}

#[test]
fn persisted_settings_accept_typescript_generated_schema_samples() {
    for (index, line) in include_str!("../../../fixtures/samples/ServerSettings.jsonl")
        .lines()
        .enumerate()
    {
        t3_config::settings::decode_persisted(serde_json::from_str(line).unwrap())
            .unwrap_or_else(|e| panic!("TypeScript ServerSettings sample {index}: {e}"));
    }
}

#[tokio::test]
async fn filesystem_read_errors_are_not_parse_failures() {
    let temp = tempfile::tempdir().unwrap();
    let home = HomeLayout::derive(temp.path(), false, true);
    for path in [home.settings(), home.keybindings()] {
        std::fs::create_dir_all(&path).unwrap();
    }
    assert!(
        SettingsService::new(home.settings())
            .reload()
            .await
            .is_err()
    );
    assert!(
        KeybindingsService::new(home.keybindings())
            .reload()
            .await
            .is_err()
    );
    assert!(home.settings().is_dir());
    assert!(home.keybindings().is_dir());
}
