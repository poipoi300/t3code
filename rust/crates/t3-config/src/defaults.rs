use t3_contracts::keybindings::KeybindingRule;
pub(crate) fn keybindings() -> Vec<KeybindingRule> {
    vec![
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+b".into()),
            command: t3_contracts::keybindings::KeybindingCommand("sidebar.toggle".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("f6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("workspace.focusNext".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("shift+f6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("workspace.focusPrevious".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+f6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("workspace.focusNext".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+f6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("workspace.focusPrevious".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+[".into()),
            command: t3_contracts::keybindings::KeybindingCommand("navigation.back".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+]".into()),
            command: t3_contracts::keybindings::KeybindingCommand("navigation.forward".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+j".into()),
            command: t3_contracts::keybindings::KeybindingCommand("terminal.toggle".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+b".into()),
            command: t3_contracts::keybindings::KeybindingCommand("rightPanel.toggle".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+b".into()),
            command: t3_contracts::keybindings::KeybindingCommand("threadPanel.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+t".into()),
            command: t3_contracts::keybindings::KeybindingCommand("rightPanel.new".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "rightPanelOpen && !terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+d".into()),
            command: t3_contracts::keybindings::KeybindingCommand("terminal.split".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+d".into()),
            command: t3_contracts::keybindings::KeybindingCommand("terminal.splitVertical".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+d".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.dictation".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+n".into()),
            command: t3_contracts::keybindings::KeybindingCommand("terminal.new".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+w".into()),
            command: t3_contracts::keybindings::KeybindingCommand("terminal.close".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+w".into()),
            command: t3_contracts::keybindings::KeybindingCommand("rightPanel.close".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+t".into()),
            command: t3_contracts::keybindings::KeybindingCommand("view.reopenClosed".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+d".into()),
            command: t3_contracts::keybindings::KeybindingCommand("diff.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+j".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.toggle".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+r".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.refresh".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+l".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.focusUrl".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+=".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.zoomIn".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod++".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.zoomIn".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+-".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.zoomOut".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+0".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.resetZoom".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+f".into()),
            command: t3_contracts::keybindings::KeybindingCommand("preview.find".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+k".into()),
            command: t3_contracts::keybindings::KeybindingCommand("commandPalette.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+p".into()),
            command: t3_contracts::keybindings::KeybindingCommand("filePicker.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+f".into()),
            command: t3_contracts::keybindings::KeybindingCommand("projectSearch.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+u".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.open".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+a".into()),
            command: t3_contracts::keybindings::KeybindingCommand("theme.select".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+shift+a".into()),
            command: t3_contracts::keybindings::KeybindingCommand("appearance.cycle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+shift+t".into()),
            command: t3_contracts::keybindings::KeybindingCommand("themeEditor.toggle".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+s".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.stash".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+enter".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "thread.steerQueuedMessage".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("alt+arrowup".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "thread.editQueuedMessage".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "composerFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+enter".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.sendAlternate".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "composerFocus && turnRunning".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+enter".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.sendBackground".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "composerFocus && draftThreadRoute".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+enter".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.sendBackground".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "composerFocus && draftThreadRoute".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+enter".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "composer.sendAndNewThread".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "composerFocus && !draftThreadRoute".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+n".into()),
            command: t3_contracts::keybindings::KeybindingCommand("chat.new".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+o".into()),
            command: t3_contracts::keybindings::KeybindingCommand("chat.new".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+n".into()),
            command: t3_contracts::keybindings::KeybindingCommand("chat.newLocal".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+alt+n".into()),
            command: t3_contracts::keybindings::KeybindingCommand("chat.newWithoutProject".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+f".into()),
            command: t3_contracts::keybindings::KeybindingCommand("chat.find".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus && !previewFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+m".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.toggle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+h".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.host".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+e".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.effort".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+a".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.mode".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+x".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.workspace".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+g".into()),
            command: t3_contracts::keybindings::KeybindingCommand("composer.branch".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+l".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "composer.previousWorktree".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+k".into()),
            command: t3_contracts::keybindings::KeybindingCommand("pullRequest.copyNumber".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+arrowup".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "modelPicker.previousProvider".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+arrowdown".into()),
            command: t3_contracts::keybindings::KeybindingCommand(
                "modelPicker.nextProvider".into(),
            ),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+o".into()),
            command: t3_contracts::keybindings::KeybindingCommand("editor.openFavorite".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+[".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.previous".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+]".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.next".into()),
            when: None,
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+c".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.copyReference".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+s".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.settle".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+p".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.pin".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+z".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.undo".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!terminalFocus && !editableFocus".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+1".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.1".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+2".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.2".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+3".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.3".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+4".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.4".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+5".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.5".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.6".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+7".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.7".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+8".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.8".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+9".into()),
            command: t3_contracts::keybindings::KeybindingCommand("thread.jump.9".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "!modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+1".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.1".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+2".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.2".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+3".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.3".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+4".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.4".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+5".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.5".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+6".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.6".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+7".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.7".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+8".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.8".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+9".into()),
            command: t3_contracts::keybindings::KeybindingCommand("modelPicker.jump.9".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "modelPickerOpen && isDesktop".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("c".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.cost".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("t".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.tokens".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("l".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.limits".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+1".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.period.day".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+2".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.period.week".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+3".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.period.month".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
        KeybindingRule {
            key: t3_contracts::base_schemas::TrimmedNonEmptyString("mod+shift+4".into()),
            command: t3_contracts::keybindings::KeybindingCommand("usage.period.quarter".into()),
            when: Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                "usagePageOpen".into(),
            ))),
        },
    ]
}
