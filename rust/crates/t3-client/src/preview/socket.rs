//! Transport for the server browser's JPEG and input channel.

use futures::{
    StreamExt,
    channel::mpsc,
    future::{Either, select},
};
use serde::Serialize;
use t3_contracts::preview::{
    PREVIEW_VIEWPORT_MAX_AREA, PREVIEW_VIEWPORT_MAX_DIMENSION, PreviewTabId,
};
use t3_rpc::Frame;

const MAX_EVENT_BYTES: usize = 256 * 1024;

use crate::{
    ClientError,
    authorization::AuthorizationError,
    platform::{Platform, PlatformError, WebSocket},
};

use super::{PreviewEvent, PreviewSession, browser::BrowserPreview};

#[derive(Debug, thiserror::Error)]
pub enum BrowserTransportError {
    #[error(transparent)]
    Access(#[from] ClientError),
    #[error(transparent)]
    Authorization(#[from] AuthorizationError),
    #[error(transparent)]
    Network(#[from] PlatformError),
    #[error("Browser unavailable")]
    Closed,
}

pub struct BrowserSocket {
    protocol: PreviewSession,
    socket: Box<dyn WebSocket>,
}

impl<P: Platform + Clone + 'static> BrowserPreview<P> {
    pub async fn connect_stream(
        &self,
        tab: PreviewTabId,
        dimensions: (u32, u32),
    ) -> Result<BrowserSocket, BrowserTransportError> {
        let endpoint = self.endpoint()?;
        let connection = self
            .runtime
            .client(&self.thread.environment)
            .ok_or(ClientError::Disconnected)?;
        let http = connection
            .pull_request_http()
            .ok_or(ClientError::Disconnected)?;
        let (target, socket) = http
            .open_preview_socket(self.thread.thread.0.clone(), tab.0, frame_cap(dimensions))
            .await?;
        if endpoint.grants_scope(t3_contracts::auth::AuthEnvironmentScope::PreviewOperate)
            != Some(true)
        {
            return Err(ClientError::Disconnected.into());
        }
        Ok(BrowserSocket {
            protocol: PreviewSession::new(target),
            socket,
        })
    }
}

impl BrowserSocket {
    pub async fn run(
        mut self,
        mut inputs: mpsc::Receiver<BrowserInput>,
        mut on_event: impl FnMut(PreviewEvent),
    ) -> Result<(), BrowserTransportError> {
        let generation = self.protocol.generation();
        for event in self.protocol.opened(generation) {
            on_event(event);
        }
        loop {
            let event = match select(self.socket.receive(), inputs.next()).await {
                Either::Left((result, _)) => Either::Left(result?),
                Either::Right((input, _)) => Either::Right(input),
            };
            let events = match event {
                Either::Left(Some(Frame::Text(text))) if text.len() <= MAX_EVENT_BYTES => {
                    self.protocol.text(generation, &text)
                }
                Either::Left(Some(Frame::Text(_))) => continue,
                Either::Left(Some(Frame::Binary(bytes))) => self.protocol.binary(generation, bytes),
                Either::Left(None) => return Err(BrowserTransportError::Closed),
                Either::Right(Some(input)) => {
                    if !input.valid() {
                        continue;
                    }
                    let value = serde_json::to_value(input).map_err(ClientError::from)?;
                    self.protocol.send(value).into_iter().collect()
                }
                Either::Right(None) => {
                    self.socket.close().await?;
                    return Ok(());
                }
            };
            for event in events {
                if let PreviewEvent::Send(value) = event {
                    self.socket.send(Frame::Text(value.to_string())).await?;
                } else {
                    on_event(event);
                }
            }
        }
    }
}

fn frame_cap((width, height): (u32, u32)) -> (u32, u32) {
    (width.clamp(1, 3840), height.clamp(1, 2160))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn from_page(width: f64, height: f64) -> Option<Self> {
        if !width.is_finite()
            || !height.is_finite()
            || width.fract() != 0.0
            || height.fract() != 0.0
            || width < 1.0
            || height < 1.0
            || width > PREVIEW_VIEWPORT_MAX_DIMENSION as f64
            || height > PREVIEW_VIEWPORT_MAX_DIMENSION as f64
            || width * height > PREVIEW_VIEWPORT_MAX_AREA as f64
        {
            return None;
        }
        Some(Self {
            width: width as u32,
            height: height as u32,
        })
    }

    /// Follow the viewer in fill mode, within the worker's supported fill bounds.
    pub fn for_container(width: f32, height: f32) -> Option<Self> {
        if !width.is_finite() || !height.is_finite() || width < 100.0 || height < 100.0 {
            return None;
        }
        Some(Self {
            width: width.round().min(3840.0) as u32,
            height: height.round().min(2160.0) as u32,
        })
    }

    pub fn project(self, (width, height): (f32, f32), (x, y): (f32, f32)) -> Option<PagePoint> {
        let width = f64::from(width);
        let height = f64::from(height);
        let page_width = f64::from(self.width);
        let page_height = f64::from(self.height);
        let scale = (width / page_width).min(height / page_height);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let x = (f64::from(x) - (width - page_width * scale) / 2.0) / scale;
        let y = (f64::from(y) - (height - page_height * scale) / 2.0) / scale;
        if !(0.0..page_width).contains(&x) || !(0.0..page_height).contains(&y) {
            return None;
        }
        Some(PagePoint { x, y, scale })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PagePoint {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
}

impl MouseButton {
    pub fn mask(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAction {
    Down,
    Up,
    Move,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Down,
    Up,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserInput {
    TakeControl,
    ReleaseControl,
    Resize {
        width: u32,
        height: u32,
    },
    Navigate {
        url: String,
    },
    History {
        delta: i8,
    },
    Reload,
    StopLoading,
    Probe {
        x: f64,
        y: f64,
    },
    Text {
        text: String,
    },
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        button: MouseButton,
        buttons: u8,
        click_count: u32,
        modifiers: u8,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: u8,
    },
    Key {
        action: KeyAction,
        key: String,
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        modifiers: u8,
    },
}

impl BrowserInput {
    fn valid(&self) -> bool {
        match self {
            Self::Resize { width, height } => {
                (100..=3840).contains(width) && (100..=2160).contains(height)
            }
            Self::Navigate { url } => {
                url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
            }
            Self::History { delta } => matches!(delta, -1 | 1),
            Self::Probe { x, y } | Self::Mouse { x, y, .. } => x.is_finite() && y.is_finite(),
            Self::Text { text } => !text.is_empty(),
            Self::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                ..
            } => [x, y, delta_x, delta_y]
                .into_iter()
                .all(|value| value.is_finite()),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_resize_is_bounded_before_serialization() {
        assert_eq!(Viewport::for_container(f32::NAN, 600.0), None);
        assert_eq!(Viewport::for_container(99.0, 600.0), None);
        assert_eq!(
            Viewport::for_container(100_000.0, 100_000.0),
            Some(Viewport {
                width: 3840,
                height: 2160
            })
        );
        assert!(
            !BrowserInput::Resize {
                width: u32::MAX,
                height: u32::MAX
            }
            .valid()
        );
        assert!(
            BrowserInput::Resize {
                width: 390,
                height: 650
            }
            .valid()
        );
    }

    #[test]
    fn projection_rejects_letterboxing_and_excessive_reported_viewports() {
        assert!(Viewport::from_page(3840.0, 3840.0).is_none());
        assert!(Viewport::from_page(f64::INFINITY, 1200.0).is_none());
        let page = Viewport::from_page(800.0, 600.0).expect("valid viewer viewport");
        assert!(page.project((400.0, 600.0), (200.0, 50.0)).is_none());
        let center = page
            .project((400.0, 600.0), (200.0, 300.0))
            .expect("inside the frame");
        assert_eq!((center.x, center.y, center.scale), (400.0, 300.0, 0.5));
    }
}
