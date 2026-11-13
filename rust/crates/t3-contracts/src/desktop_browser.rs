use crate::base_schemas::TrimmedNonEmptyString;
use crate::wire::JsNumber;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum DesktopBrowserEvent {
    #[serde(rename = "attached")]
    Attached {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
    },
    #[serde(rename = "detached")]
    Detached {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
    },
    #[serde(rename = "cdp")]
    Cdp {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
        message: String,
    },
    #[serde(rename = "loadFailed")]
    LoadFailed {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
        url: String,
        code: i64,
        description: String,
    },
    #[serde(rename = "shown")]
    Shown {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
        shown: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopBrowserPointerPhase {
    #[serde(rename = "move")]
    Move,
    #[serde(rename = "click")]
    Click,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum DesktopBrowserCommand {
    #[serde(rename = "cdp")]
    Cdp {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
        message: String,
    },
    #[serde(rename = "release")]
    Release {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
    },
    #[serde(rename = "pointer")]
    Pointer {
        thread_id: TrimmedNonEmptyString,
        tab_id: TrimmedNonEmptyString,
        phase: DesktopBrowserPointerPhase,
        x: JsNumber,
        y: JsNumber,
    },
    #[serde(rename = "turn")]
    Turn {
        thread_id: TrimmedNonEmptyString,
        active: bool,
    },
}
