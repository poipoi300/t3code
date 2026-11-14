use crate::{Error, Result, lock};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

pub const TAB_GONE_CODE: u16 = 4404;
pub const MAX_SOCKET_BUFFER_BYTES: usize = 8 * 1024 * 1024;
pub const VIEWER_OUTPUT_LIMIT: usize = 64;

#[derive(Debug, Clone)]
pub enum ViewerOutput {
    Frame { data: Vec<u8>, frame_id: u64 },
    Message(Value),
    Gone,
    Reconnect,
}
impl ViewerOutput {
    fn state_kind(&self) -> Option<&str> {
        match self {
            Self::Message(value) => value["type"].as_str(),
            Self::Gone => Some("gone"),
            Self::Reconnect => Some("reconnect"),
            _ => None,
        }
    }
}
#[derive(Default)]
struct Buffer {
    items: VecDeque<ViewerOutput>,
    closed: bool,
}
#[derive(Clone, Default)]
pub struct ViewerQueue {
    buffer: Arc<Mutex<Buffer>>,
    ready: Arc<Notify>,
}
impl ViewerQueue {
    /// Returns dropped frame ids; every one must be acknowledged to release Chromium.
    pub fn push(&self, output: ViewerOutput) -> Vec<u64> {
        let mut buffer = lock(&self.buffer);
        let mut dropped = Vec::new();
        if buffer.closed {
            if let ViewerOutput::Frame { frame_id, .. } = output {
                dropped.push(frame_id);
            }
            return dropped;
        }
        if buffer.items.len() < VIEWER_OUTPUT_LIMIT {
            buffer.items.push_back(output);
        } else if let ViewerOutput::Frame { frame_id, .. } = output {
            dropped.push(frame_id);
        } else if matches!(
            output.state_kind(),
            Some("gone" | "reconnect" | "control" | "viewport" | "fileChooser")
        ) {
            buffer.items.push_back(output);
            let all = std::mem::take(&mut buffer.items);
            let control = all
                .iter()
                .rfind(|item| item.state_kind() == Some("control"))
                .cloned();
            let viewport = all
                .iter()
                .rfind(|item| item.state_kind() == Some("viewport"))
                .cloned();
            let mut rest = VecDeque::new();
            for item in all {
                match item {
                    ViewerOutput::Frame { frame_id, .. } => dropped.push(frame_id),
                    item if matches!(item.state_kind(), Some("control" | "viewport")) => {}
                    item => rest.push_back(item),
                }
            }
            let keep = VIEWER_OUTPUT_LIMIT
                - usize::from(control.is_some())
                - usize::from(viewport.is_some());
            while rest.len() > keep {
                rest.pop_front();
            }
            rest.extend(control);
            rest.extend(viewport);
            buffer.items = rest;
        }
        self.ready.notify_one();
        dropped
    }
    pub async fn recv(&self) -> Option<ViewerOutput> {
        loop {
            let ready = self.ready.notified();
            {
                let mut buffer = lock(&self.buffer);
                if let Some(item) = buffer.items.pop_front() {
                    return Some(item);
                }
                if buffer.closed {
                    return None;
                }
            }
            ready.await;
        }
    }
    pub fn close(&self) {
        lock(&self.buffer).closed = true;
        self.ready.notify_waiters();
    }
}

/// Transport-neutral WebSocket pacing: one frame on the wire, one newest frame waiting.
#[derive(Default)]
pub struct FrameFlow {
    in_flight: Option<u64>,
    pending: Option<(Vec<u8>, u64)>,
}
impl FrameFlow {
    pub fn offer(&mut self, data: Vec<u8>, id: u64) -> (Option<Vec<u8>>, Option<u64>) {
        if self.in_flight.is_none() {
            self.in_flight = Some(id);
            return (Some(data), None);
        }
        let replaced = self.pending.replace((data, id)).map(|(_, id)| id);
        (None, replaced)
    }
    pub fn ack(&mut self) -> (Option<Vec<u8>>, Option<u64>) {
        let ack = self.in_flight.take();
        let next = self.pending.take().map(|(data, id)| {
            self.in_flight = Some(id);
            data
        });
        (next, ack)
    }
    pub fn drain(&mut self) -> Vec<u64> {
        self.in_flight
            .take()
            .into_iter()
            .chain(self.pending.take().map(|(_, id)| id))
            .collect()
    }
}
pub fn check_socket_buffer(queued: usize, next: usize) -> Result<()> {
    if queued.saturating_add(next).saturating_add(14) > MAX_SOCKET_BUFFER_BYTES {
        return Err(Error::Io(
            "Slow preview viewer exceeded socket buffer limit".into(),
        ));
    }
    Ok(())
}
pub fn parse_driver_output(value: Value) -> Result<ViewerOutput> {
    Ok(match value["type"].as_str() {
        Some("frame") => ViewerOutput::Frame {
            data: STANDARD
                .decode(
                    value["data"]
                        .as_str()
                        .ok_or_else(|| Error::InvalidInput("Missing frame data".into()))?,
                )
                .map_err(|e| Error::InvalidInput(e.to_string()))?,
            frame_id: value["frameId"]
                .as_u64()
                .ok_or_else(|| Error::InvalidInput("Missing frame id".into()))?,
        },
        Some("gone") => ViewerOutput::Gone,
        Some("reconnect") => ViewerOutput::Reconnect,
        Some(
            "viewport" | "control" | "clipboard" | "download" | "fileChooser" | "fileChooserClosed"
            | "popup" | "pointer" | "probe",
        ) => ViewerOutput::Message(value),
        _ => return Err(Error::InvalidInput("Unknown viewer output".into())),
    })
}

pub fn control_message(
    can_operate: bool,
    controller: &str,
    generation: u64,
    dialog: Value,
) -> ViewerOutput {
    ViewerOutput::Message(
        json!({"type":"control","canOperate":can_operate,"controller":controller,"generation":generation,"dialog":dialog}),
    )
}

pub fn int_param(value: Option<&str>, fallback: u32, max: u32) -> u32 {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n.round().min(max as f64) as u32)
        .unwrap_or(fallback)
}

pub fn host_setup(error: &Error) -> Option<t3_contracts::preview::PreviewStreamHostSetup> {
    use t3_contracts::preview::{PreviewHostSetupNeed, PreviewStreamHostSetup};
    let Error::Operation {
        tag,
        detail: Some(detail),
        ..
    } = error
    else {
        return None;
    };
    let need = match tag.as_str() {
        "PreviewBrowserSandboxError" => PreviewHostSetupNeed::Sandbox,
        "PreviewBrowserLibrariesError" => PreviewHostSetupNeed::Libraries,
        _ => return None,
    };
    Some(PreviewStreamHostSetup {
        need,
        command: detail["setupCommand"].as_str()?.into(),
    })
}
