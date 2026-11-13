use crate::base_schemas::TrimmedNonEmptyString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorLaunchStyle {
    DirectPath,
    Goto,
    LineColumn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorId {
    Cursor,
    Trae,
    Kiro,
    Vscode,
    VscodeInsiders,
    Vscodium,
    Zed,
    Antigravity,
    Idea,
    Aqua,
    Clion,
    Datagrip,
    Dataspell,
    Goland,
    Phpstorm,
    Pycharm,
    Rider,
    Rubymine,
    Rustrover,
    Webstorm,
    FileManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileManagerRevealKind {
    Finder,
    FileExplorer,
    Files,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchEditorInput {
    pub cwd: TrimmedNonEmptyString,
    pub editor: EditorId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub project_root: Option<Option<TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reveal: Option<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteOpenTargetKind {
    Tailscale,
    Mdns,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteOpenTarget {
    pub kind: RemoteOpenTargetKind,
    pub host: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub username: Option<TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherUnknownEditorError {
    ExternalLauncherUnknownEditorError { editor: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherUnsupportedEditorError {
    ExternalLauncherUnsupportedEditorError { editor: EditorId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherCommandNotFoundError {
    ExternalLauncherCommandNotFoundError { editor: EditorId, command: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherUnsupportedTargetError {
    ExternalLauncherUnsupportedTargetError { editor: EditorId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherBrowserSpawnError {
    ExternalLauncherBrowserSpawnError {
        command: String,
        args: Vec<String>,
        cause: Value,
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ExternalLauncherEditorSpawnError {
    ExternalLauncherEditorSpawnError {
        command: String,
        args: Vec<String>,
        cause: Value,
        editor: EditorId,
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExternalLauncherError {
    UnknownEditor(ExternalLauncherUnknownEditorError),
    UnsupportedEditor(ExternalLauncherUnsupportedEditorError),
    CommandNotFound(ExternalLauncherCommandNotFoundError),
    UnsupportedTarget(ExternalLauncherUnsupportedTargetError),
    BrowserSpawn(ExternalLauncherBrowserSpawnError),
    EditorSpawn(ExternalLauncherEditorSpawnError),
}
