//! Transport for the server browser's JPEG and input channel.

use futures::{
    Future, Stream, StreamExt,
    future::{Either, select},
};
use serde::Serialize;
use t3_contracts::preview::{
    PREVIEW_VIEWPORT_MAX_AREA, PREVIEW_VIEWPORT_MAX_DIMENSION, PreviewTabId,
};
use t3_rpc::Frame;

use crate::{
    ClientError,
    authorization::AuthorizationError,
    platform::{Platform, PlatformError, SocketClose, WebSocket},
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
    #[error("Browser socket send timed out")]
    Stalled,
}

impl BrowserTransportError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self,
            Self::Access(ClientError::PermissionDenied { .. })
                | Self::Authorization(
                    AuthorizationError::CredentialMissing
                        | AuthorizationError::CredentialRejected
                        | AuthorizationError::HttpStatus(401 | 403)
                        | AuthorizationError::Remote {
                            status: 401 | 403,
                            ..
                        },
                )
                | Self::Network(
                    PlatformError::Denied(_)
                        | PlatformError::SocketHandshakeClosed {
                            close: SocketClose { code: 1006, .. },
                            http_status: None,
                        }
                        | PlatformError::SocketHandshakeClosed {
                            close: SocketClose {
                                code: 1008 | 4401,
                                ..
                            },
                            ..
                        }
                        | PlatformError::SocketHandshakeClosed {
                            http_status: Some(401 | 403),
                            ..
                        },
                )
        )
    }
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
    pub async fn run<F: Future<Output = ()>>(
        mut self,
        mut inputs: impl Stream<Item = BrowserInput> + Unpin,
        send_deadline: impl Fn() -> F,
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
                Either::Left(Some(Frame::Text(text))) => self.protocol.text(generation, &text),
                Either::Left(Some(Frame::Binary(bytes))) => self.protocol.binary(generation, bytes),
                Either::Left(None) => {
                    if let Some(close) = self.socket.close_info() {
                        for event in self
                            .protocol
                            .closed(generation, close.code, &close.reason, 0)
                        {
                            on_event(event);
                        }
                    }
                    return Err(BrowserTransportError::Closed);
                }
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
                    match select(
                        self.socket.send(Frame::Text(value.to_string())),
                        Box::pin(send_deadline()),
                    )
                    .await
                    {
                        Either::Left((sent, _)) => sent?,
                        Either::Right(_) => return Err(BrowserTransportError::Stalled),
                    }
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
    fn handshake_abnormal_close_is_unauthorized_unless_http_explains_it() {
        let close = |code, http_status| {
            BrowserTransportError::Network(PlatformError::SocketHandshakeClosed {
                close: SocketClose {
                    code,
                    reason: String::new(),
                },
                http_status,
            })
        };
        assert!(close(1006, None).is_unauthorized());
        assert!(close(1006, Some(403)).is_unauthorized());
        assert!(!close(1006, Some(404)).is_unauthorized());
    }

    #[cfg(feature = "testing")]
    fn test_socket(wire: &crate::preview::test_support::BrowserWire) -> BrowserSocket {
        use crate::{platform::SocketRequest, preview::PreviewTarget};
        BrowserSocket {
            socket: wire
                .connect(SocketRequest {
                    url: "wss://example.test/api/preview-stream/ws".into(),
                    protocols: vec![],
                })
                .expect("fixture socket"),
            protocol: PreviewSession::new(PreviewTarget {
                access: Default::default(),
                thread_id: "thread".into(),
                tab_id: "tab".into(),
                max_width: 800.0,
                max_height: 600.0,
                interactive: true,
            }),
        }
    }

    #[cfg(feature = "testing")]
    #[test]
    fn read_socket_reports_peer_close_codes() {
        use crate::preview::test_support::BrowserWire;
        use futures::{executor::block_on, future::pending, stream};

        for (code, expected) in [
            (1008, PreviewEvent::Unauthorized),
            (4401, PreviewEvent::Unauthorized),
            (4404, PreviewEvent::Gone),
            (1006, PreviewEvent::RetryAt(500)),
        ] {
            let wire = BrowserWire::default();
            assert!(wire.peer_close(code, ""));
            let mut events = Vec::new();
            let result = block_on(test_socket(&wire).run(stream::pending(), pending, |event| {
                events.push(event);
            }));
            assert!(matches!(result, Err(BrowserTransportError::Closed)));
            assert!(events.contains(&expected), "{code}: {events:?}");
        }
    }

    #[cfg(feature = "testing")]
    #[test]
    fn send_deadline_only_aborts_a_pending_write() {
        use crate::preview::test_support::BrowserWire;
        use futures::{executor::block_on, future::ready, stream};

        for stalls in [false, true] {
            let wire = BrowserWire::default();
            assert!(wire.text(serde_json::json!({
                "type": "control", "controller": "unclaimed", "generation": 1,
                "canOperate": true, "dialog": null
            })));
            if stalls {
                wire.stall_sends();
            }
            let result = block_on(test_socket(&wire).run(
                stream::iter([BrowserInput::TakeControl]),
                || ready(()),
                |_| {},
            ));
            if stalls {
                assert!(matches!(result, Err(BrowserTransportError::Stalled)));
                assert!(wire.sent().is_empty());
            } else {
                assert!(result.is_ok());
                assert_eq!(wire.sent(), [serde_json::json!({"type":"takeControl"})]);
            }
        }
    }

    #[test]
    fn navigation_input_only_accepts_http_and_https() {
        for url in ["http://localhost:3000/", "https://example.test/"] {
            assert!(BrowserInput::Navigate { url: url.into() }.valid());
        }
        for url in [
            "/relative",
            "file:///tmp/example",
            "javascript:alert(1)",
            "data:text/plain,x",
            "view-source:https://example.test/",
            "chrome://settings/",
        ] {
            assert!(!BrowserInput::Navigate { url: url.into() }.valid(), "{url}");
        }
    }

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
