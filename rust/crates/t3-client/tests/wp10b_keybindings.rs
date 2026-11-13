#![allow(clippy::unwrap_used)]
use std::{cell::RefCell, rc::Rc};
use t3_client::view_logic::shell::{keybindings::*, shortcuts::*};
use t3_contracts::keybindings::{KeybindingRule, ResolvedKeybindingRule};

fn bindings(rows: &[(&str, &str, Option<&str>)]) -> Vec<ResolvedKeybindingRule> {
    compile_config(
        &rows
            .iter()
            .map(|(key, command, when)| {
                serde_json::from_value::<KeybindingRule>(
                    serde_json::json!({"key":key,"command":command,"when":when}),
                )
                .unwrap()
            })
            .map(|mut rule| {
                if rule.when == Some(None) {
                    rule.when = None;
                }
                rule
            })
            .collect::<Vec<_>>(),
    )
}
fn event(key: &str, code: &str, modifiers: Modifiers) -> KeyEvent {
    KeyEvent {
        key: key.into(),
        code: code.into(),
        modifiers,
        ..Default::default()
    }
}
fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Default::default()
    }
}
fn meta() -> Modifiers {
    Modifiers {
        meta: true,
        ..Default::default()
    }
}
fn command<'a>(
    e: &KeyEvent,
    rules: &'a [ResolvedKeybindingRule],
    platform: &str,
    context: &MatchContext,
) -> Option<&'a str> {
    resolve_shortcut_command(e, rules, platform, context).map(|c| c.0.as_str())
}

#[test]
fn terminal_word_line_delete_and_clear_chords() {
    for platform in ["Win32", "Linux x86_64", "MacIntel"] {
        for (key, expected) in [("ArrowLeft", "\u{1b}b"), ("ArrowRight", "\u{1b}f")] {
            assert_eq!(
                terminal_navigation_shortcut_data(
                    &event(
                        key,
                        "",
                        Modifiers {
                            alt: true,
                            ..Default::default()
                        }
                    ),
                    platform
                ),
                Some(expected)
            );
            assert_eq!(
                terminal_navigation_shortcut_data(&event(key, "", ctrl()), platform),
                None
            );
            assert_eq!(
                terminal_navigation_shortcut_data(
                    &event(
                        key,
                        "",
                        Modifiers {
                            alt: true,
                            shift: true,
                            ..Default::default()
                        }
                    ),
                    platform
                ),
                None
            );
        }
    }
    for (key, data) in [("ArrowLeft", "\u{1}"), ("ArrowRight", "\u{5}")] {
        assert_eq!(
            terminal_navigation_shortcut_data(&event(key, "", meta()), "MacIntel"),
            Some(data)
        );
    }
    assert_eq!(
        terminal_delete_shortcut_data(&event("Backspace", "", meta()), "MacIntel"),
        Some("\u{15}")
    );
    assert_eq!(
        terminal_delete_shortcut_data(&event("Backspace", "", meta()), "Linux"),
        None
    );
    assert!(is_terminal_clear_shortcut(&event("l", "", ctrl()), "Linux"));
    assert!(is_terminal_clear_shortcut(
        &event("k", "", meta()),
        "MacIntel"
    ));
    assert!(!is_terminal_clear_shortcut(
        &event("k", "", meta()),
        "Linux"
    ));
}
#[test]
fn default_contexts_reserve_terminal_and_native_editing() {
    let rules = default_keybindings();
    for platform in ["MacIntel", "Win32", "Linux"] {
        let m = if is_mac_platform(platform) {
            meta()
        } else {
            ctrl()
        };
        for (key, shift, normal, terminal) in [
            ("w", false, Some("rightPanel.close"), Some("terminal.close")),
            ("d", false, Some("diff.toggle"), Some("terminal.split")),
            (
                "d",
                true,
                Some("composer.dictation"),
                Some("terminal.splitVertical"),
            ),
            ("n", false, Some("chat.new"), Some("terminal.new")),
            ("s", true, Some("thread.settle"), None),
            ("b", true, Some("threadPanel.toggle"), None),
            ("[", false, Some("navigation.back"), None),
            ("]", false, Some("navigation.forward"), None),
        ] {
            let e = event(key, "", Modifiers { shift, ..m });
            assert_eq!(
                command(&e, &rules, platform, &MatchContext::default()),
                normal
            );
            assert_eq!(
                command(
                    &e,
                    &rules,
                    platform,
                    &MatchContext::default().with("terminalFocus", true)
                ),
                terminal
            );
        }
        let undo = event("z", "", m);
        assert_eq!(
            command(&undo, &rules, platform, &MatchContext::default()),
            Some("thread.undo")
        );
        for key in ["editableFocus", "terminalFocus"] {
            assert_eq!(
                command(
                    &undo,
                    &rules,
                    platform,
                    &MatchContext::default().with(key, true)
                ),
                None
            );
        }
    }
}
#[test]
fn order_and_when_both_control_precedence_and_labels() {
    let rules = bindings(&[
        ("mod+d", "diff.toggle", None),
        (
            "ctrl+d",
            "terminal.split",
            Some("terminalOpen && terminalFocus"),
        ),
    ]);
    let e = event("d", "", ctrl());
    for (open, focus, winner) in [
        (false, true, "diff.toggle"),
        (true, false, "diff.toggle"),
        (true, true, "terminal.split"),
    ] {
        let c = MatchContext::default()
            .with("terminalOpen", open)
            .with("terminalFocus", focus);
        assert_eq!(command(&e, &rules, "Linux", &c), Some(winner));
        assert_eq!(
            shortcut_label_for_command(&rules, Some(winner), "Linux", &c).as_deref(),
            Some("Ctrl+D")
        );
        assert_eq!(
            shortcut_label_for_command(
                &rules,
                Some(if winner == "diff.toggle" {
                    "terminal.split"
                } else {
                    "diff.toggle"
                }),
                "Linux",
                &c
            ),
            None
        );
    }
    let reverse = rules.into_iter().rev().collect::<Vec<_>>();
    assert_eq!(
        command(
            &e,
            &reverse,
            "Linux",
            &MatchContext::default()
                .with("terminalOpen", true)
                .with("terminalFocus", true)
        ),
        Some("diff.toggle")
    );
    let jumps = bindings(&[
        ("mod+shift+1", "thread.jump.1", None),
        ("mod+shift+1", "thread.jump.7", None),
    ]);
    assert_eq!(
        shortcut_label_for_command(
            &jumps,
            Some("thread.jump.1"),
            "MacIntel",
            &MatchContext::default()
        ),
        None
    );
    assert_eq!(
        shortcut_label_for_command(
            &jumps,
            Some("thread.jump.7"),
            "MacIntel",
            &MatchContext::default()
        )
        .as_deref(),
        Some("⇧⌘1")
    );
}
#[test]
fn effective_preview_shortcuts_only_include_unclaimed_contextual_keys() {
    let rules = bindings(&[
        ("mod+shift+t", "view.reopenClosed", None),
        ("mod+shift+t", "preview.refresh", Some("previewFocus")),
        ("mod+alt+t", "view.reopenClosed", Some("previewFocus")),
    ]);
    for (focus, expected) in [(true, "⌥⌘T"), (false, "⇧⌘T")] {
        let c = MatchContext::default().with("previewFocus", focus);
        let shortcuts =
            effective_shortcuts_for_command(&rules, "view.reopenClosed", "MacIntel", &c);
        assert_eq!(shortcuts.len(), 1);
        assert_eq!(format_shortcut_label(&shortcuts[0], "MacIntel"), expected);
    }
}
#[test]
fn jump_hints_and_model_jumps_respect_desktop_picker_and_terminal() {
    let rules = default_keybindings();
    for desktop in [false, true] {
        for picker in [false, true] {
            let c = MatchContext::new(desktop)
                .with("modelPickerOpen", picker)
                .with("usagePageOpen", true);
            assert_eq!(
                command(&event("3", "", meta()), &rules, "MacIntel", &c),
                if !desktop {
                    None
                } else if picker {
                    Some("modelPicker.jump.3")
                } else {
                    Some("thread.jump.3")
                }
            );
            assert_eq!(
                should_show_thread_jump_hints(meta(), &rules, "MacIntel", &c),
                desktop && !picker
            );
        }
    }
    let c = MatchContext::new(true).with("terminalFocus", true);
    assert!(!should_show_thread_jump_hints(
        meta(),
        &rules,
        "MacIntel",
        &c
    ));
    let unrestricted = bindings(&[("mod+1", "thread.jump.1", None)]);
    assert!(!should_show_thread_jump_hints(
        meta(),
        &unrestricted,
        "MacIntel",
        &c
    ));
    assert!(should_show_thread_jump_hints(
        meta(),
        &unrestricted,
        "MacIntel",
        &MatchContext::default()
    ));
    for i in 0..9 {
        assert_eq!(
            thread_jump_index_from_command(&thread_jump_command_for_index(i).unwrap()),
            Some(i)
        );
        assert_eq!(
            model_picker_jump_index_from_command(&model_picker_jump_command_for_index(i).unwrap()),
            Some(i)
        );
    }
    assert_eq!(thread_jump_command_for_index(9), None);
}
#[test]
fn active_scripts_and_primary_app_bindings_use_separate_sources() {
    let primary = bindings(&[
        ("mod+r", "script.setup.run", None),
        ("mod+k", "commandPalette.toggle", None),
        ("mod+j", "terminal.toggle", None),
    ]);
    let active = bindings(&[
        ("mod+t", "script.setup.run", None),
        ("mod+k", "script.deploy.run", None),
        ("mod+x", "terminal.toggle", None),
        ("mod+r", "script.deploy.run", None),
        ("mod+r", "terminal.new", Some("terminalFocus")),
    ]);
    for (key, terminal, expected) in [
        ("t", false, Some("script.setup.run")),
        ("k", false, Some("commandPalette.toggle")),
        ("r", false, Some("script.deploy.run")),
        ("r", true, None),
        ("j", false, Some("terminal.toggle")),
        ("x", false, None),
    ] {
        assert_eq!(
            resolve_chat_shortcut_command(
                &event(key, "", ctrl()),
                &primary,
                &active,
                "Linux",
                &MatchContext::default().with("terminalFocus", terminal)
            )
            .map(|c| c.0.as_str()),
            expected
        );
    }
    assert!(
        resolve_chat_shortcut_command(
            &event("r", "", ctrl()),
            &primary,
            &[],
            "Linux",
            &MatchContext::default()
        )
        .is_none()
    );
}
#[test]
fn layouts_keep_latin_letters_and_recover_non_latin_and_punctuation() {
    let rules = bindings(&[
        ("mod+d", "diff.toggle", None),
        ("mod+m", "thread.pin", None),
        ("mod+;", "sidebar.toggle", None),
        ("mod+alt+b", "rightPanel.toggle", None),
        ("mod+shift+'", "terminal.toggle", None),
    ]);
    for (key, code, m, expected) in [
        ("в", "KeyD", meta(), Some("diff.toggle")),
        ("a", "KeyD", meta(), None),
        ("d", "KeyL", meta(), Some("diff.toggle")),
        ("m", "Semicolon", meta(), Some("thread.pin")),
        (
            "∫",
            "KeyB",
            Modifiers {
                alt: true,
                ..meta()
            },
            Some("rightPanel.toggle"),
        ),
        (
            "@",
            "Quote",
            Modifiers {
                shift: true,
                ..meta()
            },
            Some("terminal.toggle"),
        ),
        (
            "\"",
            "Digit2",
            Modifiers {
                shift: true,
                ..meta()
            },
            None,
        ),
    ] {
        assert_eq!(
            command(
                &event(key, code, m),
                &rules,
                "MacIntel",
                &MatchContext::default()
            ),
            expected
        );
    }
}
#[test]
fn zoom_plus_uses_layout_unless_shift_alt_or_number_row() {
    let defaults = default_keybindings();
    assert_eq!(
        command(
            &event("+", "BracketRight", ctrl()),
            &defaults,
            "Linux",
            &MatchContext::default()
        ),
        None
    );
    assert_eq!(
        command(
            &event("+", "Minus", ctrl()),
            &defaults,
            "Linux",
            &MatchContext::default().with("previewFocus", true)
        ),
        Some("preview.zoomIn")
    );
    assert_eq!(
        command(
            &event("+", "Digit1", ctrl()),
            &defaults,
            "Linux",
            &MatchContext::new(true)
        ),
        Some("thread.jump.1")
    );
    for (code, m, key) in [
        (
            "BracketRight",
            Modifiers {
                alt: true,
                ..Default::default()
            },
            "alt+]",
        ),
        (
            "Equal",
            Modifiers {
                shift: true,
                ..ctrl()
            },
            "ctrl+shift+=",
        ),
    ] {
        assert_eq!(
            command(
                &event("+", code, m),
                &bindings(&[(key, "terminal.toggle", None)]),
                "Linux",
                &MatchContext::default()
            ),
            Some("terminal.toggle")
        );
    }
}
#[test]
fn altgr_text_is_not_an_app_shortcut_but_firefox_reporting_still_works() {
    let rules = bindings(&[("mod+alt+e", "composer.effort", Some("!terminalFocus"))]);
    for platform in ["Win32", "Linux", "MacIntel"] {
        let m = Modifiers {
            alt: true,
            ..if is_mac_platform(platform) {
                meta()
            } else {
                ctrl()
            }
        };
        let mut e = event("€", "KeyE", m);
        e.alt_graph = true;
        assert_eq!(
            command(&e, &rules, platform, &MatchContext::default()).is_some(),
            is_mac_platform(platform)
        );
        e.alt_graph = false;
        assert_eq!(
            command(&e, &rules, platform, &MatchContext::default()),
            Some("composer.effort")
        );
        e.alt_graph = true;
        e.key = "e".into();
        assert_eq!(
            command(&e, &rules, platform, &MatchContext::default()),
            Some("composer.effort")
        );
    }
}
#[test]
fn rich_text_selection_and_layout_control_bold_yield() {
    for (key, code, expected) in [
        ("и", "KeyB", true),
        ("x", "KeyB", false),
        ("b", "KeyN", true),
    ] {
        let e = event(key, code, ctrl());
        assert_eq!(is_rich_text_bold_shortcut(&e, Some(false)), expected);
        assert!(!is_rich_text_bold_shortcut(&e, Some(true)));
        assert!(!is_rich_text_bold_shortcut(&e, None));
    }
}
#[test]
fn saved_custom_commands_replace_all_their_defaults_and_fill_new_commands() {
    let custom = bindings(&[("mod+alt+shift+b", "threadPanel.toggle", None)]);
    let merged = merge_with_defaults(&custom);
    assert_eq!(
        command(
            &event(
                "b",
                "",
                Modifiers {
                    alt: true,
                    shift: true,
                    ..meta()
                }
            ),
            &merged,
            "MacIntel",
            &MatchContext::default()
        ),
        Some("threadPanel.toggle")
    );
    assert_eq!(
        command(
            &event(
                "b",
                "",
                Modifiers {
                    shift: true,
                    ..meta()
                }
            ),
            &merged,
            "MacIntel",
            &MatchContext::default()
        ),
        None
    );
    let mut older = default_keybindings();
    older.retain(|b| {
        !matches!(
            b.command.0.as_str(),
            "pullRequest.copyNumber" | "thread.copyReference"
        )
    });
    older.extend(bindings(&[(
        "mod+shift+8",
        "thread.copyReference",
        Some("!terminalFocus"),
    )]));
    let merged = merge_with_defaults(&older);
    for (key, expected) in [
        ("k", Some("pullRequest.copyNumber")),
        ("8", Some("thread.copyReference")),
        ("c", None),
        ("y", None),
    ] {
        assert_eq!(
            command(
                &event(
                    key,
                    "",
                    Modifiers {
                        shift: true,
                        ..meta()
                    }
                ),
                &merged,
                "MacIntel",
                &MatchContext::default()
            ),
            expected
        );
    }
}
#[test]
fn composer_send_preference_adjusts_chord_without_rewriting_saved_rule() {
    let rules = default_keybindings();
    for platform in ["MacIntel", "Win32", "Linux"] {
        for requires in [false, true] {
            let m = Modifiers {
                shift: true,
                ..if is_mac_platform(platform) {
                    meta()
                } else {
                    ctrl()
                }
            };
            let c = MatchContext::default()
                .with("composerFocus", true)
                .with("turnRunning", true)
                .with("composerRequiresModifier", requires);
            assert_eq!(
                command(&event("Enter", "", m), &rules, platform, &c),
                Some(if requires {
                    "composer.sendAlternate"
                } else {
                    "thread.steerQueuedMessage"
                })
            );
            let label =
                shortcut_label_for_command(&rules, Some("composer.sendAlternate"), platform, &c)
                    .unwrap();
            assert_eq!(
                label,
                format_shortcut_label(
                    &parse_shortcut(if requires {
                        "mod+shift+enter"
                    } else {
                        "mod+enter"
                    })
                    .unwrap(),
                    platform
                )
            );
            assert_eq!(
                command(
                    &event("Enter", "", m),
                    &rules,
                    platform,
                    &c.with("composerFocus", false)
                ),
                Some("thread.steerQueuedMessage")
            );
        }
    }
}
#[test]
fn usage_period_shifted_digits_are_contextual_and_do_not_take_plain_jumps() {
    let rules = default_keybindings();
    let e = event(
        "!",
        "Digit1",
        Modifiers {
            shift: true,
            ..ctrl()
        },
    );
    assert_eq!(command(&e, &rules, "Linux", &MatchContext::default()), None);
    assert_eq!(
        command(
            &e,
            &rules,
            "Linux",
            &MatchContext::default().with("usagePageOpen", true)
        ),
        Some("usage.period.day")
    );
}
#[test]
fn editable_focus_defaults_from_host_but_explicit_context_wins() {
    for cmd in [
        "commandPalette.toggle",
        "sidebar.toggle",
        "thread.next",
        "thread.jump.1",
    ] {
        let rules = bindings(&[("mod+k", cmd, Some("!editableFocus"))]);
        let mut e = event("k", "", ctrl());
        e.editable_focus = true;
        assert_eq!(command(&e, &rules, "Linux", &MatchContext::default()), None);
        assert_eq!(
            command(
                &e,
                &rules,
                "Linux",
                &MatchContext::default().with("editableFocus", false)
            ),
            Some(cmd)
        );
        e.editable_focus = false;
        assert_eq!(
            command(&e, &rules, "Linux", &MatchContext::default()),
            Some(cmd)
        );
        assert_eq!(
            command(
                &e,
                &rules,
                "Linux",
                &MatchContext::default().with("editableFocus", true)
            ),
            None
        );
    }
    let mut e = event("k", "", ctrl());
    e.editable_focus = true;
    assert_eq!(
        command(
            &e,
            &bindings(&[("mod+k", "sidebar.toggle", Some("editableFocus"))]),
            "Linux",
            &MatchContext::default()
        ),
        Some("sidebar.toggle")
    );
    assert_eq!(
        command(
            &e,
            &bindings(&[("mod+k", "sidebar.toggle", Some("true"))]),
            "Linux",
            &MatchContext::default()
        ),
        Some("sidebar.toggle")
    );
}
#[test]
fn physical_modifiers_ignore_poisoned_flags_and_clear_when_released() {
    let mut state = ModifierState::default();
    for (key, kind, expected) in [
        (
            "Meta",
            "keydown",
            Modifiers {
                meta: true,
                ..Default::default()
            },
        ),
        (
            "Shift",
            "keydown",
            Modifiers {
                meta: true,
                shift: true,
                ..Default::default()
            },
        ),
        (
            "Meta",
            "keyup",
            Modifiers {
                shift: true,
                ..Default::default()
            },
        ),
        ("Shift", "keyup", Modifiers::default()),
    ] {
        state.keyboard(&KeyEvent {
            key: key.into(),
            event_type: Some(kind.into()),
            modifiers: meta(),
            ..Default::default()
        });
        assert_eq!(state.modifiers, expected);
    }
    state.keyboard(&event("Enter", "", meta()));
    assert_eq!(state.modifiers, Modifiers::default());
    state.modifiers = meta();
    state.keyboard(&event("a", "", Modifiers::default()));
    assert_eq!(state.modifiers, Modifiers::default());
    state.ignore_editable = true;
    state.modifiers = meta();
    state.focus(true);
    assert_eq!(state.modifiers, Modifiers::default());
}
#[test]
fn parsing_precedence_depth_limits_and_shortcut_aliases() {
    let c = MatchContext::default()
        .with("a", true)
        .with("b", false)
        .with("c", true);
    assert!(evaluate_when(
        &parse_when("!b && (a || false) && c").unwrap(),
        &c
    ));
    assert!(parse_when("a &&").is_none());
    assert!(parse_when("a & b").is_none());
    assert!(parse_when(&"!".repeat(65)).is_none());
    assert!(parse_when(&format!("{}a{}", "(".repeat(65), ")".repeat(65))).is_none());
    for (input, key) in [
        ("mod++", "+"),
        ("mod+space", " "),
        ("option+esc", "escape"),
        ("\u{feff}ctrl + a\u{feff}", "a"),
    ] {
        assert_eq!(parse_shortcut(input).unwrap().key, key);
    }
    assert!(parse_shortcut("mod+a+b").is_none());
    assert!(parse_shortcut("mod+ +b").is_none());
}
#[test]
fn toast_cooldown_deduplicates_only_identical_consecutive_decisions() {
    let update = |message: Option<&str>| {
        serde_json::from_value(serde_json::json!({"version":1,"type":"keybindingsUpdated","payload":{"keybindings":[],"issues":message.into_iter().map(|m|serde_json::json!({"kind":"keybindings.invalid-entry","message":m,"index":0})).collect::<Vec<_>>()}})).unwrap()
    };
    let mut controller = UpdateToastController::default();
    for (time, message, expected) in [
        (
            0,
            Some("unknown command"),
            Some(UpdateToast::InvalidConfiguration("unknown command".into())),
        ),
        (100, Some("unknown command"), None),
        (
            200,
            Some("bad shortcut"),
            Some(UpdateToast::InvalidConfiguration("bad shortcut".into())),
        ),
        (
            5000,
            Some("bad shortcut"),
            Some(UpdateToast::InvalidConfiguration("bad shortcut".into())),
        ),
        (5100, None, Some(UpdateToast::Success)),
        (5200, None, None),
    ] {
        assert_eq!(controller.handle(Some(&update(message)), time), expected);
    }
    assert_eq!(controller.handle(None, 6000), None);
}

fn dispatch_event(key: &str) -> DispatchEvent {
    DispatchEvent {
        key: event(key, "", Modifiers::default()),
        default_prevented: false,
        propagation_stopped: false,
        inside_confirmation: false,
    }
}
#[test]
fn dispatcher_newest_scope_declines_guards_cleanup_raw_and_observers() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let handler = |name: &'static str, answer, prevent| {
        let calls = calls.clone();
        Box::new(move |event: &mut DispatchEvent| {
            calls.borrow_mut().push(name);
            event.default_prevented |= prevent;
            answer
        }) as Handler<'static>
    };
    let rules = bindings(&[("k", "chat.new", None)]);
    let mut d = Dispatcher::default();
    d.register(CommandScope {
        commands: std::collections::BTreeMap::from([(
            "chat.new".into(),
            handler("lower", true, false),
        )]),
        ..Default::default()
    });
    let upper = d.register(CommandScope {
        commands: std::collections::BTreeMap::from([(
            "chat.new".into(),
            handler("upper", false, true),
        )]),
        ..Default::default()
    });
    d.register(CommandScope {
        when: Box::new(|_| false),
        commands: std::collections::BTreeMap::from([(
            "chat.new".into(),
            handler("guarded", true, false),
        )]),
        ..Default::default()
    });
    assert!(d.dispatch(
        Phase::Bubble,
        &mut dispatch_event("k"),
        &rules,
        "Linux",
        &MatchContext::default()
    ));
    assert_eq!(*calls.borrow(), ["upper"]);
    calls.borrow_mut().clear();
    d.unregister(upper);
    let declined = d.register(CommandScope {
        commands: std::collections::BTreeMap::from([(
            "chat.new".into(),
            handler("declined", false, false),
        )]),
        ..Default::default()
    });
    assert!(d.dispatch(
        Phase::Bubble,
        &mut dispatch_event("k"),
        &rules,
        "Linux",
        &MatchContext::default()
    ));
    assert_eq!(*calls.borrow(), ["declined", "lower"]);
    calls.borrow_mut().clear();
    d.unregister(declined);
    let observed = Rc::new(RefCell::new(Vec::new()));
    let observed_copy = observed.clone();
    d.register(CommandScope {
        observe: Some(Box::new(move |_, used| {
            observed_copy.borrow_mut().push(used)
        })),
        ..Default::default()
    });
    d.register(CommandScope {
        on_keydown: Some(Box::new(|e| e.key.key == "Escape")),
        ..Default::default()
    });
    for key in ["Escape", "k", "x"] {
        d.dispatch(
            Phase::Bubble,
            &mut dispatch_event(key),
            &rules,
            "Linux",
            &MatchContext::default(),
        );
    }
    assert_eq!(*observed.borrow(), [true, true, false]);
    let mut confirm = dispatch_event("k");
    confirm.inside_confirmation = true;
    calls.borrow_mut().clear();
    assert!(d.has_command(&confirm, &rules, "Linux", &MatchContext::default()));
    d.dispatch(
        Phase::Bubble,
        &mut confirm,
        &rules,
        "Linux",
        &MatchContext::default(),
    );
    assert!(confirm.default_prevented);
    assert!(calls.borrow().is_empty());
}
#[test]
fn menu_newest_registration_wins_and_route_guards_read_current_data() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut d = Dispatcher::default();
    for name in ["one", "two"] {
        let calls = calls.clone();
        d.register_menu(
            "new".into(),
            Box::new(move || calls.borrow_mut().push(name)),
        );
    }
    d.dispatch_menu("new");
    assert_eq!(*calls.borrow(), ["two"]);
    let mut c = ChatRouteContext {
        chat_route: true,
        open_projects: 2,
        total_projects: 2,
        ..Default::default()
    };
    assert_eq!(
        chat_route_command("chat.new", &c),
        Some(Some(RouteAction::PickNewThreadProject))
    );
    assert_eq!(
        chat_route_command("chat.newLocal", &c),
        Some(Some(RouteAction::StartLocalThread))
    );
    c.open_projects = 1;
    assert_eq!(
        chat_route_command("chat.new", &c),
        Some(Some(RouteAction::StartLocalThread))
    );
    c.open_projects = 0;
    assert_eq!(
        chat_route_command("chat.new", &c),
        Some(Some(RouteAction::PickNewThreadProject))
    );
    c.selection_present = true;
    assert_eq!(
        chat_route_escape("Escape", &c),
        Some(RouteAction::ClearSelection)
    );
    c.modal_page = true;
    assert_eq!(chat_route_escape("Escape", &c), None);
    for composing in [false, true] {
        assert_eq!(
            palette_escape(true, true, true, "Escape", composing, 0),
            !composing
        );
    }
    assert!(!palette_escape(true, true, true, "Escape", false, 229));
    assert!(!palette_escape(false, true, true, "Escape", false, 0));
    assert_eq!(
        palette_command(false, "commandPalette.toggle", false, "system"),
        None
    );
    for command in ["appearance.cycle", "theme.select"] {
        assert_eq!(palette_command(true, command, true, "system"), Some(None));
    }
    for (mode, next) in [("system", "light"), ("light", "dark"), ("dark", "system")] {
        assert_eq!(
            palette_command(true, "appearance.cycle", false, mode),
            Some(Some(PaletteCommandAction::SetAppearance(next)))
        );
    }
    assert_eq!(
        palette_command(true, "theme.select", false, "system"),
        Some(Some(PaletteCommandAction::ChangeTheme))
    );
    assert_eq!(
        palette_command(true, "themeEditor.toggle", true, "system"),
        Some(Some(PaletteCommandAction::ToggleThemeEditor))
    );
    assert_eq!(
        palette_command(true, "usage.open", false, "system"),
        Some(Some(PaletteCommandAction::OpenUsage))
    );
    assert_eq!(
        palette_command(true, "filePicker.toggle", false, "system"),
        Some(Some(PaletteCommandAction::ToggleMode(
            t3_client::view_logic::shell::palette::OverlayMode::Files
        )))
    );
}
