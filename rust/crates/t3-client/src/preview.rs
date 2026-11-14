//! Browser preview protocol policy. The host owns JPEG decoding and presentation.
pub mod file_path;
pub mod gesture;
pub mod open;
use crate::device::{DeviceHubAccess, form_encode};
use serde_json::{Value, json};
use t3_contracts::preview::{
    PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE, PreviewHostSetupNeed, PreviewStreamHostSetup,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewState {
    Closed,
    Connecting,
    Ready { url: String },
    Failed(String),
}

#[derive(Clone)]
pub struct PreviewTarget {
    pub access: DeviceHubAccess,
    pub thread_id: String,
    pub tab_id: String,
    pub max_width: f64,
    pub max_height: f64,
    pub interactive: bool,
}
impl PreviewTarget {
    pub fn websocket_url(&self) -> String {
        let mut url = format!(
            "{}/ws?threadId={}&tabId={}&maxWidth={}&maxHeight={}",
            self.access.ws_base,
            form_encode(&self.thread_id),
            form_encode(&self.tab_id),
            self.max_width.round().max(1.),
            self.max_height.round().max(1.)
        );
        if !self.interactive {
            url.push_str("&interactive=false");
        }
        self.access.with_query(&url)
    }
    pub fn resource_url(&self, upload: bool, id: &str) -> String {
        self.access.with_query(&format!(
            "{}/{}?threadId={}&tabId={}&{}={}",
            self.access.http_base,
            if upload { "upload" } else { "download" },
            form_encode(&self.thread_id),
            form_encode(&self.tab_id),
            if upload { "chooser" } else { "id" },
            form_encode(id)
        ))
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewControl {
    pub can_operate: bool,
    pub controller: String,
    pub generation: f64,
    pub dialog: Value,
}
impl PreviewControl {
    fn decode(v: &Value) -> Option<Self> {
        let dialog = v.get("dialog")?;
        if !dialog.is_null()
            && !(dialog.get("type")?.is_string()
                && dialog.get("message")?.is_string()
                && dialog.get("defaultValue")?.is_string())
        {
            return None;
        }
        let controller = v.get("controller")?.as_str()?;
        if !["agent", "you", "another-viewer", "unclaimed"].contains(&controller) {
            return None;
        }
        Some(Self {
            can_operate: v.get("canOperate")?.as_bool()?,
            controller: controller.into(),
            generation: v.get("generation")?.as_f64()?,
            dialog: dialog.clone(),
        })
    }
    pub fn label(&self) -> &'static str {
        if !self.can_operate {
            "Read-only"
        } else {
            match self.controller.as_str() {
                "you" => "You have control",
                "agent" => "Agent has control",
                "another-viewer" => "Another viewer has control",
                _ => "Watching",
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewEvent {
    Send(Value),
    Frame(Vec<u8>),
    Connected(bool),
    Viewport {
        width: f64,
        height: f64,
    },
    Probe {
        x: f64,
        y: f64,
        editable: bool,
    },
    Pointer {
        phase: String,
        x: f64,
        y: f64,
        sequence: f64,
    },
    Control(PreviewControl),
    Clipboard(String),
    Popup(String),
    Download {
        file_name: String,
        size_bytes: f64,
        url: String,
    },
    FileChooser(Option<PreviewFileChooser>),
    Unauthorized,
    Gone,
    HostSetup(PreviewStreamHostSetup),
    RetryAt(u64),
}
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewFileChooser {
    pub multiple: bool,
    pub accept: String,
    pub upload_url: String,
    pub credentials: bool,
}
pub struct PreviewSession {
    pub state: PreviewState,
    pub control: Option<PreviewControl>,
    target: PreviewTarget,
    generation: u64,
    stopped: bool,
    opened: bool,
    failures: u32,
    chooser: Option<String>,
}
impl PreviewSession {
    pub fn new(target: PreviewTarget) -> Self {
        Self {
            target,
            state: PreviewState::Connecting,
            control: None,
            generation: 1,
            stopped: false,
            opened: false,
            failures: 0,
            chooser: None,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    fn current(&self, generation: u64) -> bool {
        !self.stopped && self.generation == generation
    }
    pub fn reconnect(&mut self) -> Option<u64> {
        if self.stopped {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.opened = false;
        self.state = PreviewState::Connecting;
        Some(self.generation)
    }
    pub fn opened(&mut self, generation: u64) -> Vec<PreviewEvent> {
        if !self.current(generation) {
            return vec![];
        }
        self.opened = true;
        self.state = PreviewState::Ready {
            url: self.target.websocket_url(),
        };
        vec![PreviewEvent::Connected(true)]
    }
    pub fn binary(&mut self, generation: u64, bytes: Vec<u8>) -> Vec<PreviewEvent> {
        if !self.current(generation) {
            return vec![];
        }
        self.failures = 0;
        vec![
            PreviewEvent::Send(json!({"type":"ack"})),
            PreviewEvent::Frame(bytes),
        ]
    }
    pub fn text(&mut self, generation: u64, text: &str) -> Vec<PreviewEvent> {
        if !self.current(generation) {
            return vec![];
        }
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return vec![];
        };
        self.message(&v).into_iter().collect()
    }
    fn message(&mut self, v: &Value) -> Option<PreviewEvent> {
        Some(match v.get("type")?.as_str()? {
            "control" => {
                let control = PreviewControl::decode(v)?;
                self.control = Some(control.clone());
                PreviewEvent::Control(control)
            }
            "fileChooser" => {
                let id = v.get("id")?.as_str()?;
                let multiple = v.get("multiple")?.as_bool()?;
                let accept = v.get("accept")?.as_str()?.to_owned();
                self.chooser = Some(id.into());
                PreviewEvent::FileChooser(Some(PreviewFileChooser {
                    multiple,
                    accept,
                    upload_url: self.target.resource_url(true, id),
                    credentials: self.target.access.credentials,
                }))
            }
            "fileChooserClosed" => {
                if self.chooser.as_deref() != Some(v.get("id")?.as_str()?) {
                    return None;
                }
                self.chooser = None;
                PreviewEvent::FileChooser(None)
            }
            "pointer" => {
                let phase = v.get("phase")?.as_str()?;
                if !["move", "click"].contains(&phase) {
                    return None;
                }
                PreviewEvent::Pointer {
                    phase: phase.into(),
                    x: v.get("x")?.as_f64()?,
                    y: v.get("y")?.as_f64()?,
                    sequence: v.get("sequence")?.as_f64()?,
                }
            }
            "popup" => PreviewEvent::Popup(v.get("tabId")?.as_str()?.into()),
            "clipboard" => PreviewEvent::Clipboard(v.get("text")?.as_str()?.into()),
            "download" => PreviewEvent::Download {
                file_name: v.get("fileName")?.as_str()?.into(),
                size_bytes: v.get("sizeBytes")?.as_f64()?,
                url: self.target.resource_url(false, v.get("id")?.as_str()?),
            },
            "viewport" => {
                let width = v.get("width")?.as_f64()?;
                let height = v.get("height")?.as_f64()?;
                self.failures = 0;
                PreviewEvent::Viewport { width, height }
            }
            "probe" => PreviewEvent::Probe {
                x: v.get("x")?.as_f64()?,
                y: v.get("y")?.as_f64()?,
                editable: v.get("editable")?.as_bool()?,
            },
            _ => return None,
        })
    }
    pub fn send(&self, input: Value) -> Option<PreviewEvent> {
        if self.stopped || !self.opened {
            return None;
        }
        let control = self.control.as_ref()?;
        if !control.can_operate
            || (input.get("type")?.as_str()? != "takeControl" && control.controller != "you")
        {
            return None;
        }
        Some(PreviewEvent::Send(input))
    }
    pub fn stop_loading(&self) -> Option<PreviewEvent> {
        self.send(json!({"type":"stopLoading"}))
    }
    pub fn closed(&mut self, generation: u64, code: u16, reason: &str, now: u64) -> Vec<PreviewEvent> {
        if !self.current(generation) {
            return vec![];
        }
        let mut events = self.withdraw();
        self.control = None;
        if self.opened {
            events.push(PreviewEvent::Connected(false));
        }
        let opened = std::mem::take(&mut self.opened);
        // Invalidate every callback from the closed socket before requesting a retry.
        self.generation = self.generation.wrapping_add(1);
        match code {
            4404 => {
                self.stopped = true;
                self.state = PreviewState::Closed;
                events.push(PreviewEvent::Gone);
            }
            PREVIEW_STREAM_HOST_SETUP_CLOSE_CODE => {
                self.stopped = true;
                self.state = PreviewState::Failed("Browser host setup required".into());
                events.push(PreviewEvent::HostSetup(decode_host_setup(reason)));
            }
            1008 | 4401 => {
                self.stopped = true;
                self.state = PreviewState::Failed("Unauthorized".into());
                events.push(PreviewEvent::Unauthorized);
            }
            1006 if !opened => {
                self.stopped = true;
                self.state = PreviewState::Failed("Unauthorized".into());
                events.push(PreviewEvent::Unauthorized);
            }
            _ => {
                let delay = (500u64.saturating_mul(1u64 << self.failures.min(5))).min(10_000);
                self.failures = self.failures.saturating_add(1);
                self.state = PreviewState::Connecting;
                events.push(PreviewEvent::RetryAt(now.saturating_add(delay)));
            }
        }
        events
    }
    fn withdraw(&mut self) -> Vec<PreviewEvent> {
        if self.chooser.take().is_some() {
            vec![PreviewEvent::FileChooser(None)]
        } else {
            vec![]
        }
    }
    pub fn stop(&mut self) -> Vec<PreviewEvent> {
        self.stopped = true;
        self.opened = false;
        self.control = None;
        self.state = PreviewState::Closed;
        self.generation = self.generation.wrapping_add(1);
        self.withdraw()
    }
}
pub fn preview_modifiers(alt: bool, ctrl: bool, meta: bool, shift: bool) -> u8 {
    (alt as u8) | ((ctrl as u8) << 1) | ((meta as u8) << 2) | ((shift as u8) << 3)
}
pub fn decode_host_setup(reason: &str) -> PreviewStreamHostSetup {
    serde_json::from_str(reason).unwrap_or_else(|_| PreviewStreamHostSetup {
        need: PreviewHostSetupNeed::Sandbox,
        command: "sudo npx t3 browser setup".into(),
    })
}
pub fn host_setup_message(setup: &PreviewStreamHostSetup) -> &'static str {
    match setup.need {
        PreviewHostSetupNeed::Sandbox => {
            "This server's host blocks the sandbox its browser runs in. Run this once on the host, then try again:"
        }
        PreviewHostSetupNeed::Libraries => {
            "This server's host is missing libraries its browser needs. Run this once on the host, then try again:"
        }
    }
}
#[derive(Default)]
pub struct PreviewFrameQueue {
    decoding: bool,
    waiting: Option<Vec<u8>>,
    stopped: bool,
}
impl PreviewFrameQueue {
    pub fn paint(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if self.stopped {
            return None;
        }
        if self.decoding {
            self.waiting = Some(bytes);
            None
        } else {
            self.decoding = true;
            Some(bytes)
        }
    }
    pub fn decoded(&mut self) -> Option<Vec<u8>> {
        self.decoding = false;
        if self.stopped {
            return None;
        }
        let next = self.waiting.take();
        self.decoding = next.is_some();
        next
    }
    pub fn can_present(&self) -> bool {
        !self.stopped
    }
    pub fn stop(&mut self) {
        self.stopped = true;
        self.waiting = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> PreviewSession {
        PreviewSession::new(PreviewTarget {
            access: DeviceHubAccess {
                http_base: "https://test/api/preview-stream".into(),
                ws_base: "wss://test/api/preview-stream".into(),
                query: std::collections::BTreeMap::from([("wsTicket".into(), "ticket".into())]),
                credentials: false,
            },
            thread_id: "thread".into(),
            tab_id: "tab".into(),
            max_width: 800.,
            max_height: 600.,
            interactive: true,
        })
    }
    fn control(
        session: &mut PreviewSession,
        operate: bool,
        owner: &str,
        dialog: Value,
    ) -> Vec<PreviewEvent> {
        session.text(session.generation(),&json!({"type":"control","canOperate":operate,"controller":owner,"generation":1,"dialog":dialog}).to_string())
    }
    #[test]
    fn malformed_dialog_never_grants_control() {
        let mut s = session();
        s.opened(1);
        assert!(
            control(
                &mut s,
                true,
                "you",
                json!({"type":"prompt","message":42,"defaultValue":""})
            )
            .is_empty()
        );
        assert!(s.send(json!({"type":"reload"})).is_none());
    }
    #[test]
    fn read_only_stream_acknowledges_frames_but_refuses_every_mutation() {
        let mut s = session();
        s.opened(1);
        control(&mut s, false, "agent", Value::Null);
        assert_eq!(
            s.binary(1, vec![1, 2]),
            vec![
                PreviewEvent::Send(json!({"type":"ack"})),
                PreviewEvent::Frame(vec![1, 2])
            ]
        );
        for kind in ["takeControl", "resize", "reload"] {
            assert!(s.send(json!({"type":kind})).is_none());
        }
    }
    #[test]
    fn release_revokes_input_and_takeover_requires_permission() {
        let mut s = session();
        s.opened(1);
        control(
            &mut s,
            true,
            "you",
            json!({"type":"prompt","message":"Name?","defaultValue":"initial"}),
        );
        assert!(s.send(json!({"type":"dialog","accept":true})).is_some());
        assert!(s.send(json!({"type":"releaseControl"})).is_some());
        control(&mut s, true, "agent", Value::Null);
        assert!(s.send(json!({"type":"text","text":"late"})).is_none());
        assert!(s.send(json!({"type":"takeControl"})).is_some());
    }
    #[test]
    fn picker_ids_are_matched_and_withdrawn_on_disconnect_or_stop() {
        let mut s = session();
        s.opened(1);
        let offer =
            json!({"type":"fileChooser","id":"c1","multiple":true,"accept":".txt"}).to_string();
        assert!(matches!(
            s.text(1, &offer).as_slice(),
            [PreviewEvent::FileChooser(Some(_))]
        ));
        assert!(
            s.text(1, r#"{"type":"fileChooserClosed","id":"old"}"#)
                .is_empty()
        );
        let closed = s.closed(1, 1012, "", 0);
        assert!(closed.contains(&PreviewEvent::FileChooser(None)));
        assert!(closed.contains(&PreviewEvent::Connected(false)));
        let generation = s.reconnect().expect("retry allowed");
        s.text(generation, &offer);
        assert_eq!(s.stop(), vec![PreviewEvent::FileChooser(None)]);
    }
    #[test]
    fn download_and_upload_urls_carry_access_and_cookie_policy() {
        let mut s = session();
        let events = s.text(
            1,
            r#"{"type":"download","id":"d1","fileName":"a b.csv","sizeBytes":3}"#,
        );
        assert_eq!(events,vec![PreviewEvent::Download {file_name:"a b.csv".into(),size_bytes:3.,url:"https://test/api/preview-stream/download?threadId=thread&tabId=tab&id=d1&wsTicket=ticket".into()}]);
        assert!(
            s.target
                .resource_url(true, "a b")
                .ends_with("chooser=a+b&wsTicket=ticket")
        );
    }
    #[test]
    fn pointer_validation_and_old_socket_callbacks() {
        let mut s = session();
        assert!(
            s.text(
                1,
                r#"{"type":"pointer","phase":"drag","x":1,"y":2,"sequence":1}"#
            )
            .is_empty()
        );
        assert_eq!(
            s.text(
                1,
                r#"{"type":"pointer","phase":"click","x":140,"y":50,"sequence":2}"#
            ),
            vec![PreviewEvent::Pointer {
                phase: "click".into(),
                x: 140.,
                y: 50.,
                sequence: 2.
            }]
        );
        s.reconnect();
        assert!(s.binary(1, vec![1]).is_empty());
        assert!(s.closed(1, 4401, "", 0).is_empty());
    }
    #[test]
    fn terminal_close_codes_and_backoff_are_injected() {
        for (code, expected) in [
            (4404, PreviewEvent::Gone),
            (4401, PreviewEvent::Unauthorized),
            (1008, PreviewEvent::Unauthorized),
            (1006, PreviewEvent::Unauthorized),
        ] {
            let mut s = session();
            assert!(s.closed(1, code, "", 0).contains(&expected));
            assert!(s.reconnect().is_none());
        }
        let mut s = session();
        assert!(matches!(
            s.closed(1, 4503, "invalid", 0).as_slice(),
            [PreviewEvent::HostSetup(_)]
        ));
        let mut s = session();
        s.opened(1);
        assert!(
            s.closed(1, 1006, "", 100)
                .contains(&PreviewEvent::RetryAt(600))
        );
        let generation = s.reconnect().expect("retry");
        s.opened(generation);
        assert!(
            s.closed(generation, 1012, "", 1000)
                .contains(&PreviewEvent::RetryAt(2000))
        );
    }
    #[test]
    fn painter_coalesces_to_latest_waiting_frame_and_stops_late_output() {
        let mut queue = PreviewFrameQueue::default();
        assert_eq!(queue.paint(vec![1]), Some(vec![1]));
        assert_eq!(queue.paint(vec![2]), None);
        assert_eq!(queue.paint(vec![3]), None);
        assert_eq!(queue.decoded(), Some(vec![3]));
        queue.stop();
        assert!(!queue.can_present());
        assert_eq!(queue.decoded(), None);
        assert_eq!(queue.paint(vec![4]), None);
    }
}
