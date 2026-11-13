use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewCaptureFailureTag {
    #[serde(rename = "PreviewCaptureFailure")]
    PreviewCaptureFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewCaptureFailureReason {
    #[serde(rename = "renderer-unavailable")]
    RendererUnavailable,
    #[serde(rename = "capture-unavailable")]
    CaptureUnavailable,
    #[serde(rename = "capture-timeout")]
    CaptureTimeout,
    #[serde(rename = "recording-conflict")]
    RecordingConflict,
    #[serde(rename = "capture-failed")]
    CaptureFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewCaptureFailureStage {
    #[serde(rename = "screenshot")]
    Screenshot,
    #[serde(rename = "capture-page")]
    CapturePage,
    #[serde(rename = "recording-start")]
    RecordingStart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCaptureFailure {
    #[serde(rename = "_tag")]
    pub _tag: PreviewCaptureFailureTag,
    pub reason: PreviewCaptureFailureReason,
    pub stage: PreviewCaptureFailureStage,
}

pub type PreviewCaptureFailureEncoded = PreviewCaptureFailure;
