use crate::base_schemas::TrimmedNonEmptyString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_WHEN_EXPRESSION_DEPTH: usize = 64;
pub const MAX_SCRIPT_ID_LENGTH: usize = 24;
pub const MAX_KEYBINDINGS_COUNT: usize = 256;
pub const STATIC_KEYBINDING_COMMANDS: &[&str] = &[
    "thread.jump.1",
    "thread.jump.2",
    "thread.jump.3",
    "thread.jump.4",
    "thread.jump.5",
    "thread.jump.6",
    "thread.jump.7",
    "thread.jump.8",
    "thread.jump.9",
    "modelPicker.jump.1",
    "modelPicker.jump.2",
    "modelPicker.jump.3",
    "modelPicker.jump.4",
    "modelPicker.jump.5",
    "modelPicker.jump.6",
    "modelPicker.jump.7",
    "modelPicker.jump.8",
    "modelPicker.jump.9",
    "thread.stop",
    "thread.steerQueuedMessage",
    "thread.editQueuedMessage",
    "thread.previous",
    "thread.next",
    "thread.copyReference",
    "thread.settle",
    "thread.pin",
    "thread.regenerateTitle",
    "thread.undo",
    "modelPicker.toggle",
    "modelPicker.previousProvider",
    "modelPicker.nextProvider",
    "workspace.focusNext",
    "workspace.focusPrevious",
    "sidebar.toggle",
    "navigation.back",
    "navigation.forward",
    "terminal.toggle",
    "terminal.split",
    "terminal.splitVertical",
    "terminal.new",
    "terminal.close",
    "rightPanel.toggle",
    "rightPanel.new",
    "threadPanel.toggle",
    "rightPanel.toggleMaximized",
    "rightPanel.close",
    "view.reopenClosed",
    "pullRequest.copyNumber",
    "diff.toggle",
    "preview.toggle",
    "preview.refresh",
    "preview.focusUrl",
    "preview.zoomIn",
    "preview.zoomOut",
    "preview.resetZoom",
    "preview.find",
    "commandPalette.toggle",
    "filePicker.toggle",
    "projectSearch.toggle",
    "usage.open",
    "theme.select",
    "appearance.cycle",
    "themeEditor.toggle",
    "composer.stash",
    "composer.sendAlternate",
    "composer.sendBackground",
    "composer.sendAndNewThread",
    "composer.dictation",
    "composer.host",
    "composer.cycleHost",
    "composer.effort",
    "composer.increaseEffort",
    "composer.decreaseEffort",
    "composer.mode",
    "composer.workspace",
    "composer.previousWorktree",
    "composer.branch",
    "chat.new",
    "chat.newLocal",
    "chat.newWithoutProject",
    "chat.find",
    "editor.openFavorite",
    "usage.cost",
    "usage.tokens",
    "usage.limits",
    "usage.period.day",
    "usage.period.week",
    "usage.period.month",
    "usage.period.quarter",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeybindingCommand(pub String);

impl TryFrom<String> for KeybindingCommand {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if STATIC_KEYBINDING_COMMANDS.contains(&value.as_str()) || is_script_command(&value) {
            Ok(Self(value))
        } else {
            Err("unknown keybinding command")
        }
    }
}
impl From<KeybindingCommand> for String {
    fn from(value: KeybindingCommand) -> Self {
        value.0
    }
}
fn is_script_command(value: &str) -> bool {
    let Some(id) = value
        .strip_prefix("script.")
        .and_then(|v| v.strip_suffix(".run"))
    else {
        return false;
    };
    !id.is_empty()
        && id.len() <= MAX_SCRIPT_ID_LENGTH
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && id.as_bytes()[0] != b'-'
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ScriptRunCommandPattern(pub String);
impl TryFrom<String> for ScriptRunCommandPattern {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if is_script_command(&value) {
            Ok(Self(value))
        } else {
            Err("invalid script command")
        }
    }
}
impl From<ScriptRunCommandPattern> for String {
    fn from(value: ScriptRunCommandPattern) -> Self {
        value.0
    }
}

pub type KeybindingValue = TrimmedNonEmptyString;
pub type KeybindingWhen = TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeybindingRule {
    pub key: KeybindingValue,
    pub command: KeybindingCommand,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub when: Option<Option<KeybindingWhen>>,
}
pub type KeybindingsConfig = Vec<KeybindingRule>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingShortcut {
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub mod_key: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewForwardedShortcut {
    pub command: KeybindingCommand,
    pub shortcut: KeybindingShortcut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum KeybindingWhenNode {
    Identifier {
        name: String,
    },
    Not {
        node: Box<KeybindingWhenNode>,
    },
    And {
        left: Box<KeybindingWhenNode>,
        right: Box<KeybindingWhenNode>,
    },
    Or {
        left: Box<KeybindingWhenNode>,
        right: Box<KeybindingWhenNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedKeybindingRule {
    pub command: KeybindingCommand,
    pub shortcut: KeybindingShortcut,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub when_ast: Option<Option<KeybindingWhenNode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResolvedKeybindingsConfig(
    #[serde(deserialize_with = "crate::wire::deserialize_lenient_array")]
    pub  Vec<ResolvedKeybindingRule>,
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum KeybindingsConfigError {
    KeybindingsConfigParseError {
        #[serde(rename = "configPath")]
        config_path: String,
        detail: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        cause: Option<Option<Value>>,
    },
}
