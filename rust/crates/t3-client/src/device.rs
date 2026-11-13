//! Device protocol state and geometry, independent of capture, codecs and rendering.
use crate::platform::PlatformError;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use t3_contracts::device::DevicePlatform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFrame {
    pub sequence: u64,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Default)]
pub struct DeviceHubAccess {
    pub http_base: String,
    pub ws_base: String,
    pub query: BTreeMap<String, String>,
    pub credentials: bool,
}
pub fn form_encode(value: &str) -> String {
    let mut result = String::new();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                result.push(byte as char)
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push(HEX[(byte >> 4) as usize] as char);
                result.push(HEX[(byte & 15) as usize] as char);
            }
        }
    }
    result
}
impl DeviceHubAccess {
    pub fn with_query(&self, url: &str) -> String {
        if self.query.is_empty() {
            return url.to_owned();
        }
        let pairs = self
            .query
            .iter()
            .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{url}{}{pairs}", if url.contains('?') { "&" } else { "?" })
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Portrait,
    LandscapeLeft,
    PortraitUpsideDown,
    LandscapeRight,
}
impl Orientation {
    pub fn landscape(self) -> bool {
        matches!(self, Self::LandscapeLeft | Self::LandscapeRight)
    }
    pub fn next(self) -> Self {
        match self {
            Self::Portrait => Self::LandscapeLeft,
            Self::LandscapeLeft => Self::PortraitUpsideDown,
            Self::PortraitUpsideDown => Self::LandscapeRight,
            Self::LandscapeRight => Self::Portrait,
        }
    }
    pub fn rotation(self) -> f64 {
        use std::f64::consts::{FRAC_PI_2, PI};
        match self {
            Self::Portrait => 0.,
            Self::LandscapeLeft => -FRAC_PI_2,
            Self::LandscapeRight => FRAC_PI_2,
            Self::PortraitUpsideDown => PI,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DuoPose {
    Closed,
    Book,
    Open,
    Laptop,
    Tent,
}
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenConfig {
    pub width: f64,
    pub height: f64,
    pub orientation: Orientation,
    #[serde(default)]
    pub input_unavailable: bool,
    pub screen_id: Option<u32>,
    #[serde(default)]
    pub supports_hinge_angle: bool,
    #[serde(default)]
    pub supports_physical_orientation: bool,
    pub hinge_angle: Option<f64>,
    pub hinge_pose: Option<DuoPose>,
    #[serde(default)]
    pub table_mode: bool,
    #[serde(default)]
    pub table_mode_available: bool,
}
impl ScreenConfig {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let config: Self = serde_json::from_slice(bytes).ok()?;
        (config.width.is_finite()
            && config.height.is_finite()
            && config.width > 0.
            && config.height > 0.
            && config
                .hinge_angle
                .is_none_or(|a| a.is_finite() && (0. ..=180.).contains(&a)))
        .then_some(config)
    }
    pub fn angle(&self) -> f64 {
        self.hinge_angle
            .unwrap_or(if self.screen_id == Some(1) { 0. } else { 180. })
    }
    pub fn settled(&self) -> bool {
        (self.screen_id == Some(1)) == (self.angle() == 0.)
    }
    pub fn phone_vertical(&self) -> bool {
        if self.screen_id == Some(1) {
            !self.orientation.landscape()
        } else {
            self.orientation.landscape()
        }
    }
    pub fn display_key(&self) -> String {
        format!(
            "{:?}:{}:{}:{:?}",
            self.screen_id, self.width, self.height, self.orientation
        )
    }
}
pub fn duo_hold_orientation(vertical: bool, panel: Option<u32>) -> Orientation {
    match (panel == Some(1), vertical) {
        (true, true) => Orientation::Portrait,
        (true, false) | (false, true) => Orientation::LandscapeLeft,
        (false, false) => Orientation::PortraitUpsideDown,
    }
}
pub fn avc_codec_string(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        "avc1.42E01E".into()
    } else {
        format!("avc1.{:02x}{:02x}{:02x}", bytes[1], bytes[2], bytes[3])
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct SemuPacket<'a> {
    pub data: &'a [u8],
    pub is_key: Option<bool>,
    pub timestamp: Option<u64>,
}
pub fn parse_semu_packet(bytes: &[u8]) -> SemuPacket<'_> {
    if bytes.len() > 16 && bytes[..4] == *b"SEMU" && bytes[4] == 1 {
        let timestamp = u64::from_be_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        SemuPacket {
            data: &bytes[16..],
            is_key: Some(bytes[5] & 1 != 0),
            timestamp: (timestamp <= 9_007_199_254_740_991).then_some(timestamp),
        }
    } else {
        SemuPacket {
            data: bytes,
            is_key: None,
            timestamp: None,
        }
    }
}
pub fn scan_access_unit(bytes: &[u8]) -> (bool, Option<&[u8]>) {
    let (mut key, mut sps, mut i) = (false, None, 0);
    while i + 2 < bytes.len() {
        let code = if bytes[i..].starts_with(&[0, 0, 1]) {
            3
        } else if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else {
            0
        };
        if code != 0 {
            if let Some(&nal) = bytes.get(i + code) {
                if nal & 31 == 7 && sps.is_none() {
                    sps = Some(&bytes[i + code..]);
                }
                if nal & 31 == 5 {
                    key = true;
                }
            }
            i += code + 1;
        } else {
            i += 1;
        }
    }
    (key, sps)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvccKind {
    Description,
    Keyframe,
    Delta,
    Seed,
    End,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvccChunk {
    pub kind: AvccKind,
    pub payload: Vec<u8>,
}
pub struct AvccDemuxer {
    buffer: Vec<u8>,
    max_frame: usize,
}
impl Default for AvccDemuxer {
    fn default() -> Self {
        Self::new(16 * 1024 * 1024)
    }
}
impl AvccDemuxer {
    pub fn new(max_frame: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_frame,
        }
    }
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
    /// The caller consumes each envelope before the next one is decoded.
    pub fn push(
        &mut self,
        mut bytes: &[u8],
        mut emit: impl FnMut(AvccChunk),
    ) -> Result<(), PlatformError> {
        while !bytes.is_empty() {
            let target = if self.buffer.len() < 4 {
                4
            } else {
                let n = u32::from_be_bytes([
                    self.buffer[0],
                    self.buffer[1],
                    self.buffer[2],
                    self.buffer[3],
                ]) as usize;
                if n > self.max_frame {
                    self.reset();
                    return Err(PlatformError::TooLarge);
                }
                n.checked_add(4).ok_or(PlatformError::TooLarge)?
            };
            let n = (target - self.buffer.len()).min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            if self.buffer.len() < 4 {
                continue;
            }
            let length = u32::from_be_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            if length > self.max_frame {
                self.reset();
                return Err(PlatformError::TooLarge);
            }
            if self.buffer.len() != length + 4 {
                continue;
            }
            let kind = self.buffer.get(4).and_then(|tag| match tag {
                1 => Some(AvccKind::Description),
                2 => Some(AvccKind::Keyframe),
                3 => Some(AvccKind::Delta),
                4 => Some(AvccKind::Seed),
                7 => Some(AvccKind::End),
                _ => None,
            });
            if let Some(kind) = kind {
                emit(AvccChunk {
                    kind,
                    payload: self.buffer[5..].to_vec(),
                });
            }
            self.buffer.clear();
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamToken {
    pub session: u64,
    pub video: u64,
    pub decoder: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputToken {
    session: u64,
    input: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StreamStatus {
    #[default]
    Stopped,
    Connecting,
    Streaming,
    Failed(String),
    Unauthorized,
}
#[derive(Debug, Default)]
pub struct DeviceSession {
    pub status: StreamStatus,
    pub screen: Option<ScreenConfig>,
    pub input_connected: bool,
    pub input_admitted: bool,
    pub input_unavailable: bool,
    session: u64,
    video: u64,
    decoder: u64,
    input: u64,
    awaiting_keyframe: bool,
    first_frame_deadline: Option<u64>,
    last_frame: Option<u64>,
    timestamp: u64,
    decoded_video: bool,
}
impl DeviceSession {
    pub fn token(&self) -> StreamToken {
        StreamToken {
            session: self.session,
            video: self.video,
            decoder: self.decoder,
        }
    }
    pub fn current(&self, token: StreamToken) -> bool {
        matches!(
            self.status,
            StreamStatus::Connecting | StreamStatus::Streaming
        ) && self.token() == token
    }
    pub fn input_token(&self) -> InputToken {
        InputToken {
            session: self.session,
            input: self.input,
        }
    }
    pub fn replace_input(&mut self) -> InputToken {
        self.input = self.input.wrapping_add(1);
        self.input_admitted = false;
        self.input_unavailable = false;
        self.input_connected = false;
        self.input_token()
    }
    fn current_input(&self, token: InputToken) -> bool {
        self.input_token() == token
            && matches!(
                self.status,
                StreamStatus::Connecting | StreamStatus::Streaming
            )
    }
    pub fn input_opened(&mut self, token: InputToken, avcc: bool) -> bool {
        if !self.current_input(token) {
            return false;
        }
        self.input_connected = true;
        !avcc || self.input_admitted
    }
    pub fn start(&mut self, now: u64) -> StreamToken {
        if matches!(
            self.status,
            StreamStatus::Connecting | StreamStatus::Streaming
        ) {
            return self.token();
        }
        self.session = self.session.wrapping_add(1);
        self.status = StreamStatus::Connecting;
        self.input_admitted = false;
        self.input_unavailable = false;
        self.awaiting_keyframe = true;
        self.first_frame_deadline = Some(now.saturating_add(15_000));
        self.last_frame = None;
        self.decoded_video = false;
        self.timestamp = 0;
        self.token()
    }
    pub fn replace_video(&mut self) -> StreamToken {
        self.video = self.video.wrapping_add(1);
        self.decoder = self.decoder.wrapping_add(1);
        self.awaiting_keyframe = true;
        self.decoded_video = false;
        self.timestamp = 0;
        self.token()
    }
    pub fn stop(&mut self) {
        self.session = self.session.wrapping_add(1);
        self.replace_video();
        self.status = StreamStatus::Stopped;
        self.input_connected = false;
        self.input_admitted = false;
        self.first_frame_deadline = None;
        self.last_frame = None;
    }
    pub fn fail(&mut self, message: impl Into<String>) {
        self.stop();
        self.status = StreamStatus::Failed(message.into());
    }
    pub fn frame_presented(
        &mut self,
        token: StreamToken,
        now: u64,
        video: bool,
        success: bool,
    ) -> bool {
        if !self.current(token) || (!video && self.decoded_video) {
            return false;
        }
        if !success {
            self.fail("Could not display the device stream. Reconnect to try again.");
            return false;
        }
        self.decoded_video |= video;
        self.first_frame_deadline = None;
        self.last_frame = Some(now);
        self.status = StreamStatus::Streaming;
        true
    }
    pub fn decode_timestamp(
        &mut self,
        token: StreamToken,
        key: bool,
        queue: usize,
        pts: Option<u64>,
    ) -> Option<u64> {
        if !self.current(token) || (self.awaiting_keyframe && !key) {
            return None;
        }
        if queue > 8 {
            self.decoder = self.decoder.wrapping_add(1);
            self.awaiting_keyframe = true;
            return None;
        }
        self.awaiting_keyframe = false;
        let timestamp = pts.unwrap_or(self.timestamp);
        self.timestamp = self.timestamp.saturating_add(16_667);
        Some(timestamp)
    }
    pub fn tick(&mut self, now: u64, webrtc: bool) -> bool {
        if self.first_frame_deadline.is_some_and(|at| now >= at) {
            self.fail("No video received from the device. Reconnect to try again.");
            return false;
        }
        webrtc
            && self
                .last_frame
                .is_some_and(|at| now.saturating_sub(at) >= 15_000)
    }
    pub fn input_message(&mut self, token: InputToken, bytes: &[u8]) -> bool {
        if !self.current_input(token) {
            return false;
        }
        match bytes.split_first() {
            Some((&0x83, _)) => {
                self.input_admitted = true;
                self.input_connected = true;
                true
            }
            Some((&0x82, body)) => {
                if let Some(screen) = ScreenConfig::decode(body) {
                    self.input_unavailable = screen.input_unavailable;
                    self.screen = Some(screen);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
    pub fn can_input(&self, avcc: bool) -> bool {
        self.input_connected
            && !matches!(
                self.status,
                StreamStatus::Stopped | StreamStatus::Failed(_) | StreamStatus::Unauthorized
            )
            && (!avcc || (self.input_admitted && !self.input_unavailable))
    }
    pub fn input_closed(
        &mut self,
        token: InputToken,
        code: u16,
        ticket: bool,
        reason: &str,
    ) -> bool {
        if !self.current_input(token) {
            return false;
        }
        self.input = self.input.wrapping_add(1);
        self.input_connected = false;
        self.input_admitted = false;
        if code == 4001 {
            self.fail(format!(
                "Device lease ended: {}",
                if reason.is_empty() { "ended" } else { reason }
            ));
            return false;
        }
        if code == 1008 || code == 4401 || (code == 1006 && ticket) {
            self.stop();
            self.status = StreamStatus::Unauthorized;
            false
        } else {
            true
        }
    }
    pub fn lease_ended(&mut self, payload: &[u8]) {
        let reason = serde_json::from_slice::<Value>(payload)
            .ok()
            .and_then(|v| v.get("reason")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "ended".into());
        self.fail(format!("Device lease ended: {reason}"));
    }
}
pub fn raw_point(platform: DevicePlatform, screen: Option<&ScreenConfig>, x: f64, y: f64) -> Point {
    if platform != DevicePlatform::Ios {
        return Point { x, y };
    }
    let Some(screen) = screen else {
        return Point { x, y };
    };
    if screen.width > screen.height {
        return Point { x, y };
    }
    match screen.orientation {
        Orientation::LandscapeLeft => Point { x: y, y: 1. - x },
        Orientation::LandscapeRight => Point { x: 1. - y, y: x },
        Orientation::PortraitUpsideDown => Point {
            x: 1. - x,
            y: 1. - y,
        },
        Orientation::Portrait => Point { x, y },
    }
}
pub fn tagged_json(tag: u8, payload: Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = vec![tag];
    serde_json::to_writer(&mut out, &payload)?;
    Ok(out)
}
pub fn android_text_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    for character in text.chars() {
        if chunk.len() + character.len_utf8() > 300 {
            chunks.push(std::mem::take(&mut chunk));
            if chunks.len() == 64 {
                return chunks;
            }
        }
        chunk.push(character);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}
pub fn hid_usage(code: &str) -> Option<u8> {
    if let Some(key) = code
        .strip_prefix("Key")
        .filter(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_uppercase())
    {
        return Some(4 + key.as_bytes()[0] - b'A');
    }
    if let Some(digit) = code
        .strip_prefix("Digit")
        .filter(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_digit())
    {
        return Some(if digit == "0" {
            0x27
        } else {
            0x1e + digit.as_bytes()[0] - b'1'
        });
    }
    let usage = match code {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2a,
        "Tab" => 0x2b,
        "Space" => 0x2c,
        "Minus" => 0x2d,
        "Equal" => 0x2e,
        "BracketLeft" => 0x2f,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "Delete" => 0x4c,
        "ArrowRight" => 0x4f,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "NumpadDivide" => 0x54,
        "NumpadMultiply" => 0x55,
        "NumpadSubtract" => 0x56,
        "NumpadAdd" => 0x57,
        "NumpadEnter" => 0x58,
        "Numpad1" => 0x59,
        "Numpad2" => 0x5a,
        "Numpad3" => 0x5b,
        "Numpad4" => 0x5c,
        "Numpad5" => 0x5d,
        "Numpad6" => 0x5e,
        "Numpad7" => 0x5f,
        "Numpad8" => 0x60,
        "Numpad9" => 0x61,
        "Numpad0" => 0x62,
        "NumpadDecimal" => 0x63,
        "NumpadEqual" => 0x67,
        "ControlLeft" => 0xe0,
        "ShiftLeft" => 0xe1,
        "AltLeft" => 0xe2,
        "MetaLeft" => 0xe3,
        "ControlRight" => 0xe4,
        "ShiftRight" => 0xe5,
        "AltRight" => 0xe6,
        "MetaRight" => 0xe7,
        _ => return None,
    };
    Some(usage)
}
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "control", content = "value", rename_all = "lowercase")]
pub enum DuoCommand {
    Angle(f64),
    Pose(DuoPose),
    Table(bool),
    Physical(String),
    Orientation(Orientation),
}
#[derive(Debug, Clone, PartialEq)]
pub struct DuoRequest {
    pub request_id: u64,
    pub command: DuoCommand,
    pub deadline: u64,
}
#[derive(Debug, Default)]
pub struct DuoControl {
    pub active: Option<DuoRequest>,
    pub queued: Option<DuoCommand>,
    pub error: Option<String>,
    next_id: u64,
}
impl DuoControl {
    pub fn requested(&self) -> Option<&DuoCommand> {
        self.queued
            .as_ref()
            .or_else(|| self.active.as_ref().map(|r| &r.command))
    }
    pub fn enqueue(&mut self, command: DuoCommand, now: u64) -> Option<DuoRequest> {
        if matches!(&command,DuoCommand::Angle(a) if !a.is_finite() || !(0. ..=180.).contains(a)) {
            return None;
        }
        self.queued = Some(command);
        self.drain(now)
    }
    fn drain(&mut self, now: u64) -> Option<DuoRequest> {
        if self.active.is_some() {
            return None;
        }
        let command = self.queued.take()?;
        self.next_id = self.next_id.wrapping_add(1);
        let request = DuoRequest {
            request_id: self.next_id,
            command,
            deadline: now.saturating_add(5000),
        };
        self.active = Some(request.clone());
        self.error = None;
        Some(request)
    }
    pub fn receive(
        &mut self,
        id: u64,
        ok: bool,
        error: Option<String>,
        now: u64,
    ) -> Option<DuoRequest> {
        if self.active.as_ref().is_none_or(|r| r.request_id != id) {
            return None;
        }
        self.active = None;
        if !ok {
            self.clear(Some(error.unwrap_or_else(|| {
                "Device control failed. Its position is unknown.".into()
            })));
            return None;
        }
        self.drain(now)
    }
    pub fn tick(&mut self, now: u64) {
        if self.active.as_ref().is_some_and(|r| now >= r.deadline) {
            self.clear(Some(
                "Device control timed out. Its position is unknown.".into(),
            ));
        }
    }
    pub fn clear(&mut self, error: Option<String>) {
        self.active = None;
        self.queued = None;
        self.error = error;
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhoneNavigation {
    Orbit(Point),
    Zoom(f64),
}
pub fn phone_wheel_navigation(
    delta: Point,
    mode: u8,
    ctrl: bool,
    size: Point,
) -> Option<PhoneNavigation> {
    if ![delta.x, delta.y, size.x, size.y]
        .iter()
        .all(|v| v.is_finite())
        || size.x <= 0.
        || size.y <= 0.
        || mode > 2
    {
        return None;
    }
    let unit = match mode {
        1 => 16.,
        2 => size.y,
        _ => 1.,
    };
    Some(if ctrl {
        PhoneNavigation::Zoom((-delta.y * unit * 0.01).clamp(-1., 1.))
    } else {
        PhoneNavigation::Orbit(Point {
            x: (-delta.x * unit / size.x).clamp(-0.25, 0.25),
            y: (-delta.y * unit / size.y).clamp(-0.25, 0.25),
        })
    })
}
#[derive(Debug, Clone, PartialEq)]
pub enum InteractionEvent {
    Touch(&'static str, Point),
    Orbit(Point),
    Active(bool, bool),
}
#[derive(Default)]
pub struct PhoneInteraction {
    active: Option<(u64, Point, Option<Point>)>,
}
impl PhoneInteraction {
    pub fn begin(&mut self, id: u64, point: Point, screen: Option<Point>) -> Vec<InteractionEvent> {
        if self.active.is_some() {
            return vec![];
        }
        self.active = Some((id, point, screen));
        let mut events = vec![InteractionEvent::Active(true, screen.is_some())];
        if let Some(screen) = screen {
            events.push(InteractionEvent::Touch("begin", screen));
        }
        events
    }
    pub fn move_pointer(
        &mut self,
        id: u64,
        point: Point,
        screen: Option<Point>,
    ) -> Option<InteractionEvent> {
        let active = self.active.as_mut()?;
        if active.0 != id {
            return None;
        }
        let event = if active.2.is_some() {
            screen.map(|screen| {
                active.2 = Some(screen);
                InteractionEvent::Touch("move", screen)
            })
        } else {
            Some(InteractionEvent::Orbit(Point {
                x: point.x - active.1.x,
                y: point.y - active.1.y,
            }))
        };
        active.1 = point;
        event
    }
    pub fn end(&mut self, id: Option<u64>) -> Vec<InteractionEvent> {
        if self
            .active
            .as_ref()
            .is_none_or(|a| id.is_some_and(|id| id != a.0))
        {
            return vec![];
        }
        let Some((_, _, screen)) = self.active.take() else {
            return vec![];
        };
        let mut events = vec![];
        if let Some(screen) = screen {
            events.push(InteractionEvent::Touch("end", screen));
        }
        events.push(InteractionEvent::Active(false, screen.is_some()));
        events
    }
    pub fn can_navigate(&self) -> bool {
        self.active.is_none()
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhoneLayout {
    pub aspect: f64,
    pub rotation: f64,
    pub landscape: bool,
    pub raw_landscape: bool,
}
pub fn phone_display_layout(
    screen: Option<&ScreenConfig>,
    raw_width: f64,
    raw_height: f64,
) -> PhoneLayout {
    let width = if raw_width != 0. {
        raw_width
    } else {
        screen.map_or(900., |s| s.width)
    };
    let height = if raw_height != 0. {
        raw_height
    } else {
        screen.map_or(1950., |s| s.height)
    };
    let orientation = screen.map_or(Orientation::Portrait, |s| s.orientation);
    PhoneLayout {
        aspect: width.min(height) / width.max(height),
        rotation: orientation.rotation(),
        landscape: orientation.landscape(),
        raw_landscape: width > height,
    }
}
pub fn display_uv(u: f64, v: f64, layout: PhoneLayout) -> Point {
    if layout.raw_landscape {
        if layout.rotation > 0. {
            Point { x: 1. - v, y: u }
        } else {
            Point { x: v, y: 1. - u }
        }
    } else {
        Point { x: u, y: v }
    }
}
pub fn display_point(local: Point, width: f64, height: f64, orientation: Orientation) -> Point {
    let u = ((local.x + width / 2.) / width).clamp(0., 1.);
    let v = ((local.y + height / 2.) / height).clamp(0., 1.);
    match orientation {
        Orientation::LandscapeLeft => Point { x: v, y: u },
        Orientation::LandscapeRight => Point {
            x: 1. - v,
            y: 1. - u,
        },
        Orientation::PortraitUpsideDown => Point { x: 1. - u, y: v },
        Orientation::Portrait => Point { x: u, y: 1. - v },
    }
}
pub fn duo_raw_point(panel: u8, x: f64, y: f64) -> Point {
    if panel == 1 {
        Point { x, y }
    } else {
        Point { x: y, y: 1. - x }
    }
}
pub fn duo_frame_matches(width: f64, height: f64, screen: &ScreenConfig) -> bool {
    width > 0.
        && height > 0.
        && (width / height - screen.width / screen.height).abs() <= 1. / height + 1. / screen.height
}
pub fn is_fold_inner_aspect(aspect: f64) -> bool {
    aspect.is_finite() && aspect > 0.75 && aspect < 1.5
}
pub const DEFAULT_FOLD_INNER_ASPECT: f64 = 2076. / 2152.;
pub fn resolve_device_model(platform: DevicePlatform, name: &str) -> Option<&'static str> {
    if platform != DevicePlatform::Ios {
        return None;
    }
    match name.to_ascii_lowercase().as_str() {
        "iphone duo" => Some("iphone-duo"),
        "iphone 18 pro max" => Some("iphone-18-pro-max"),
        "iphone 18 pro" => Some("iphone-18-pro"),
        "ipad pro 13-inch (m5)" => Some("ipad-pro-13-m5"),
        _ => None,
    }
}
#[derive(Debug, Default)]
pub struct ModelSlot {
    pub source: Option<(String, String)>,
    pub installed: bool,
    generation: u64,
    disposed: bool,
}
impl ModelSlot {
    pub fn set(&mut self, source: Option<(String, String)>) -> Option<u64> {
        if self.disposed || self.source == source {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.source = source;
        self.installed = false;
        self.source.as_ref().map(|_| self.generation)
    }
    /// False means the host must dispose the late resource without installing it.
    pub fn loaded(&mut self, generation: u64) -> bool {
        if self.disposed || generation != self.generation || self.source.is_none() {
            return false;
        }
        self.installed = true;
        true
    }
    pub fn failed(&mut self, generation: u64) {
        if !self.disposed && generation == self.generation {
            self.source = None;
            self.installed = false;
        }
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.generation = self.generation.wrapping_add(1);
        self.source = None;
        self.installed = false;
    }
}
#[derive(Default)]
pub struct RenderScheduler {
    pending: bool,
    disposed: bool,
}
impl RenderScheduler {
    pub fn invalidate(&mut self) -> bool {
        if self.disposed || self.pending {
            false
        } else {
            self.pending = true;
            true
        }
    }
    pub fn frame(&mut self) -> bool {
        let pending = std::mem::take(&mut self.pending);
        pending && !self.disposed
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.pending = false;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.,
        y: 0.,
        z: 0.,
    };
    pub const X: Self = Self {
        x: 1.,
        y: 0.,
        z: 0.,
    };
    pub const Y: Self = Self {
        x: 0.,
        y: 1.,
        z: 0.,
    };
    pub const Z: Self = Self {
        x: 0.,
        y: 0.,
        z: 1.,
    };
    pub fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    pub fn cross(self, b: Self) -> Self {
        Self {
            x: self.y * b.z - self.z * b.y,
            y: self.z * b.x - self.x * b.z,
            z: self.x * b.y - self.y * b.x,
        }
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn normalized(self) -> Self {
        let n = self.length();
        if n > 0. { self * (1. / n) } else { self }
    }
    pub fn clamp_length(self, max: f64) -> Self {
        let length = self.length();
        if length > max {
            self * (max / length)
        } else {
            self
        }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self {
            x: self.x + b.x,
            y: self.y + b.y,
            z: self.z + b.z,
        }
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self {
            x: self.x - b.x,
            y: self.y - b.y,
            z: self.z - b.z,
        }
    }
}
impl std::ops::Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self {
            x: self.x * b,
            y: self.y * b,
            z: self.z * b,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}
impl Default for Quaternion {
    fn default() -> Self {
        Self::IDENTITY
    }
}
impl Quaternion {
    pub const IDENTITY: Self = Self {
        x: 0.,
        y: 0.,
        z: 0.,
        w: 1.,
    };
    pub fn normalized(self) -> Self {
        let n = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if n == 0. {
            Self::IDENTITY
        } else {
            Self {
                x: self.x / n,
                y: self.y / n,
                z: self.z / n,
                w: self.w / n,
            }
        }
    }
    pub fn inverse(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }
    pub fn compose(self, b: Self) -> Self {
        Self {
            x: self.x * b.w + self.w * b.x + self.y * b.z - self.z * b.y,
            y: self.y * b.w + self.w * b.y + self.z * b.x - self.x * b.z,
            z: self.z * b.w + self.w * b.z + self.x * b.y - self.y * b.x,
            w: self.w * b.w - self.x * b.x - self.y * b.y - self.z * b.z,
        }
    }
    pub fn from_vector(v: Vec3) -> Self {
        let angle = v.length();
        if angle < 1e-8 {
            return Self::IDENTITY;
        }
        let s = (angle / 2.).sin() / angle;
        Self {
            x: v.x * s,
            y: v.y * s,
            z: v.z * s,
            w: (angle / 2.).cos(),
        }
    }
    pub fn vector(self) -> Vec3 {
        let mut q = self.normalized();
        if q.w < 0. {
            q = Self {
                x: -q.x,
                y: -q.y,
                z: -q.z,
                w: -q.w,
            };
        }
        let v = Vec3 {
            x: q.x,
            y: q.y,
            z: q.z,
        };
        let length = v.length();
        if length < 1e-8 {
            Vec3::ZERO
        } else {
            v * (2. * length.atan2(q.w) / length)
        }
    }
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let q = Vec3 {
            x: self.x,
            y: self.y,
            z: self.z,
        };
        let t = q.cross(v) * 2.;
        v + t * self.w + q.cross(t)
    }
    pub fn angle_to(self, b: Self) -> f64 {
        2. * (self.x * b.x + self.y * b.y + self.z * b.z + self.w * b.w)
            .abs()
            .clamp(-1., 1.)
            .acos()
    }
    pub fn slerp(self, mut b: Self, t: f64) -> Self {
        let mut cosine = self.x * b.x + self.y * b.y + self.z * b.z + self.w * b.w;
        if cosine < 0. {
            cosine = -cosine;
            b = Self {
                x: -b.x,
                y: -b.y,
                z: -b.z,
                w: -b.w,
            };
        }
        let (a_weight, b_weight) = if cosine > 0.9995 {
            (1. - t, t)
        } else {
            let angle = cosine.clamp(-1., 1.).acos();
            (
                ((1. - t) * angle).sin() / angle.sin(),
                (t * angle).sin() / angle.sin(),
            )
        };
        Self {
            x: self.x * a_weight + b.x * b_weight,
            y: self.y * a_weight + b.y * b_weight,
            z: self.z * a_weight + b.z * b_weight,
            w: self.w * a_weight + b.w * b_weight,
        }
        .normalized()
    }
    pub fn from_basis(right: Vec3, up: Vec3, normal: Vec3) -> Self {
        let trace = right.x + up.y + normal.z;
        if trace > 0. {
            let s = 0.5 / (trace + 1.).sqrt();
            Self {
                x: (up.z - normal.y) * s,
                y: (normal.x - right.z) * s,
                z: (right.y - up.x) * s,
                w: 0.25 / s,
            }
        } else if right.x > up.y && right.x > normal.z {
            let s = 2. * (1. + right.x - up.y - normal.z).sqrt();
            Self {
                x: 0.25 * s,
                y: (up.x + right.y) / s,
                z: (normal.x + right.z) / s,
                w: (up.z - normal.y) / s,
            }
        } else if up.y > normal.z {
            let s = 2. * (1. + up.y - right.x - normal.z).sqrt();
            Self {
                x: (up.x + right.y) / s,
                y: 0.25 * s,
                z: (normal.y + up.z) / s,
                w: (normal.x - right.z) / s,
            }
        } else {
            let s = 2. * (1. + normal.z - right.x - up.y).sqrt();
            Self {
                x: (normal.x + right.z) / s,
                y: (normal.y + up.z) / s,
                z: 0.25 * s,
                w: (right.y - up.x) / s,
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ViewSnap {
    pub rotation: Quaternion,
    pub yaw_limit: f64,
    pub orientation: Orientation,
    pub face: String,
    pub center: Vec3,
}
pub fn nearest_device_view(rotation: Quaternion, snaps: &[ViewSnap]) -> Option<ViewSnap> {
    let mut closest = None;
    let mut distance = f64::INFINITY;
    for snap in snaps {
        let relative = rotation.compose(snap.rotation.inverse());
        let turn = 2. * relative.y.atan2(relative.w);
        let yaw = turn
            .sin()
            .atan2(turn.cos())
            .clamp(-snap.yaw_limit, snap.yaw_limit);
        let candidate = Quaternion::from_vector(Vec3::Y * yaw).compose(snap.rotation);
        let next = candidate.angle_to(rotation);
        if next < distance - 1e-8 {
            let mut snap = snap.clone();
            snap.rotation = candidate;
            closest = Some(snap);
            distance = next;
        }
    }
    closest
}
#[derive(Debug, Clone)]
pub struct RestFrame {
    pub face: String,
    pub normal: Vec3,
    pub up: Vec3,
    pub center: Vec3,
}
pub fn duo_view_snaps(frames: &[RestFrame], panel: u8) -> Vec<ViewSnap> {
    use std::f64::consts::{FRAC_PI_2, PI};
    let mut snaps = vec![];
    for frame in frames {
        let normal = frame.normal.normalized();
        if normal.dot(normal) < 0.5 {
            continue;
        }
        let right = frame.up.cross(normal).normalized();
        let up = normal.cross(right).normalized();
        let face_rotation = Quaternion::from_basis(right, up, normal).inverse();
        for (index, orientation) in [
            Orientation::Portrait,
            Orientation::LandscapeLeft,
            Orientation::PortraitUpsideDown,
            Orientation::LandscapeRight,
        ]
        .into_iter()
        .enumerate()
        {
            if (frame.face == "right" && index != 0) || (frame.face == "left" && index != 2) {
                continue;
            }
            let roll = if panel == 3 { FRAC_PI_2 } else { 0. } - index as f64 * FRAC_PI_2;
            let mut rotation = Quaternion::from_vector(Vec3::Z * roll).compose(face_rotation);
            if frame.face == "left" || frame.face == "right" {
                rotation = Quaternion::from_vector(Vec3::X * (PI / 12.)).compose(rotation);
            }
            let occluded = |rotation: Quaternion| {
                frames.iter().any(|leaf| {
                    (leaf.face == "left" || leaf.face == "right")
                        && rotation.rotate(leaf.normal).z < 0.04
                })
            };
            if panel == 3 && occluded(rotation) {
                continue;
            }
            let mut yaw_limit = if frame.face == "cover" {
                PI / 9.
            } else {
                PI / 3.
            };
            if panel == 3 {
                for sign in [-1., 1.] {
                    for step in 1..=60 {
                        if occluded(
                            Quaternion::from_vector(Vec3::Y * (sign * step as f64 * PI / 180.))
                                .compose(rotation),
                        ) {
                            yaw_limit = yaw_limit.min((step - 1) as f64 * PI / 180.);
                            break;
                        }
                    }
                }
            }
            snaps.push(ViewSnap {
                rotation,
                yaw_limit,
                orientation,
                face: frame.face.clone(),
                center: frame.center,
            });
        }
    }
    snaps
}
#[derive(Debug, Clone, Copy)]
pub struct Bounds3 {
    pub min: Vec3,
    pub max: Vec3,
}
#[derive(Debug)]
pub struct DeviceFraming {
    pub center: Vec3,
    target: Vec3,
    velocity: Vec3,
    bounds: Bounds3,
    distance: f64,
    target_distance: f64,
    distance_velocity: f64,
    tan_x: f64,
    tan_y: f64,
    at: f64,
    initialized: bool,
    moving: bool,
    held: bool,
}
impl Default for DeviceFraming {
    fn default() -> Self {
        Self {
            center: Vec3::ZERO,
            target: Vec3::ZERO,
            velocity: Vec3::ZERO,
            bounds: Bounds3 {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
            distance: 1.,
            target_distance: 1.,
            distance_velocity: 0.,
            tan_x: 1.,
            tan_y: 1.,
            at: 0.,
            initialized: false,
            moving: false,
            held: false,
        }
    }
}
impl DeviceFraming {
    pub fn advance(&mut self, now: f64, immediate: bool) -> bool {
        if self.held {
            return false;
        }
        let dt = ((now - self.at) / 1000.).max(0.);
        self.at = now;
        if !self.moving {
            return false;
        }
        let omega = 14.;
        let decay = (-omega * dt).exp();
        let error = self.center - self.target;
        let coefficient = self.velocity + error * omega;
        self.center = (error + coefficient * dt) * decay + self.target;
        self.velocity = (self.velocity - coefficient * (omega * dt)) * decay;
        let error = self.distance - self.target_distance;
        let coefficient = self.distance_velocity + omega * error;
        self.distance = self.target_distance + (error + coefficient * dt) * decay;
        self.distance_velocity = (self.distance_velocity - omega * coefficient * dt) * decay;
        self.moving = (self.center - self.target).length() > 0.0005
            || self.velocity.length() > 0.005
            || (self.distance - self.target_distance).abs() > 0.0005
            || self.distance_velocity.abs() > 0.005;
        if immediate || !self.moving {
            self.center = self.target;
            self.distance = self.target_distance;
            self.velocity = Vec3::ZERO;
            self.distance_velocity = 0.;
            self.moving = false;
        }
        true
    }
    pub fn set_bounds(
        &mut self,
        bounds: Bounds3,
        vertical: f64,
        aspect: f64,
        now: f64,
        immediate: bool,
    ) {
        if bounds.max.x < bounds.min.x || bounds.max.y < bounds.min.y || bounds.max.z < bounds.min.z
        {
            return;
        }
        self.advance(now, false);
        self.bounds = bounds;
        self.tan_y = vertical.tan();
        self.tan_x = self.tan_y * aspect;
        self.target = (bounds.min + bounds.max) * 0.5;
        self.target.z = 0.;
        let size = bounds.max - bounds.min;
        self.target_distance =
            ((size.x / self.tan_x).max(size.y / self.tan_y) * 0.565 + bounds.max.z).max(1.);
        self.moving = true;
        self.advance(now, immediate || !self.initialized);
        self.initialized = true;
    }
    pub fn distance(&self) -> f64 {
        let clearance = [
            (self.bounds.min.x - self.center.x).abs() / self.tan_x,
            (self.bounds.max.x - self.center.x).abs() / self.tan_x,
            (self.bounds.min.y - self.center.y).abs() / self.tan_y,
            (self.bounds.max.y - self.center.y).abs() / self.tan_y,
        ]
        .into_iter()
        .fold(0., f64::max);
        self.distance
            .max(clearance * 1.02 + self.bounds.max.z)
            .max(1.)
    }
    pub fn hold(&mut self, held: bool, now: f64) {
        self.held = held;
        self.at = now;
    }
    pub fn needs_frame(&self) -> bool {
        self.moving && !self.held
    }
}
#[derive(Clone, Copy)]
struct MotionSpring {
    at: f64,
    rotation: Quaternion,
    velocity: Vec3,
    steps: usize,
}
#[derive(Clone, Copy)]
struct MotionSpin {
    at: f64,
    rotation: Quaternion,
    velocity: Vec3,
    correction: Vec3,
}
#[derive(Clone, Copy)]
struct MotionDrag {
    start: Quaternion,
    rest: Quaternion,
    x: f64,
    y: f64,
    error: Vec3,
    at: f64,
}
pub struct DeviceMotion {
    pub rotation: Quaternion,
    target: Quaternion,
    velocity: Vec3,
    gesture_velocity: Vec3,
    spring: Option<MotionSpring>,
    spin: Option<MotionSpin>,
    drag: Option<MotionDrag>,
    held: bool,
    interrupted_drag: bool,
    last_input: f64,
}
impl Default for DeviceMotion {
    fn default() -> Self {
        Self {
            rotation: Quaternion::IDENTITY,
            target: Quaternion::IDENTITY,
            velocity: Vec3::ZERO,
            gesture_velocity: Vec3::ZERO,
            spring: None,
            spin: None,
            drag: None,
            held: false,
            interrupted_drag: false,
            last_input: f64::NEG_INFINITY,
        }
    }
}
impl DeviceMotion {
    fn spring(&mut self, next: Quaternion, now: f64) {
        self.target = next.normalized();
        self.spring = Some(MotionSpring {
            at: now,
            rotation: self.rotation,
            velocity: self.velocity,
            steps: 0,
        });
    }
    pub fn set_pose(&mut self, next: Quaternion, now: f64, immediate: bool) {
        self.advance(now, false);
        self.drag = None;
        self.spin = None;
        self.spring(next, now);
        if immediate {
            self.rotation = self.target;
            self.velocity = Vec3::ZERO;
            self.spring = None;
        }
    }
    pub fn advance(&mut self, now: f64, reduced: bool) -> bool {
        if self.held || !now.is_finite() {
            return false;
        }
        if let Some(spin) = self.spin {
            let seconds = ((now - spin.at) / 1000.).max(0.);
            let coast = (-1.1 * seconds).exp();
            let settle = (-2. * seconds).exp();
            let correction =
                Quaternion::from_vector(spin.correction * (1. - (1. + 2. * seconds) * settle));
            self.rotation = correction
                .compose(Quaternion::from_vector(
                    spin.velocity * ((1. - coast) / 1.1),
                ))
                .compose(spin.rotation)
                .normalized();
            self.velocity = correction.rotate(spin.velocity * coast)
                + spin.correction * (4. * seconds * settle);
            if reduced
                || (self.rotation.angle_to(self.target) < 0.01 && self.velocity.length() < 0.08)
            {
                self.spin = None;
                self.rotation = self.target;
                self.velocity = Vec3::ZERO;
            }
            return true;
        }
        if let Some(drag) = self.drag.as_mut() {
            let seconds = ((now - drag.at) / 1000.).clamp(0., 0.05);
            drag.at = now;
            let steps = (seconds * 120.).ceil() as usize;
            let dt = if steps > 0 {
                seconds / steps as f64
            } else {
                0.
            };
            for _ in 0..steps {
                let mut error = self.target.compose(self.rotation.inverse()).vector();
                let length = error.length();
                if length * length > 1e-10 {
                    let axis = error * (1. / length);
                    let turns =
                        ((drag.error.dot(axis) - length) / (2. * std::f64::consts::PI)).round();
                    error = error + axis * (turns * 2. * std::f64::consts::PI);
                }
                drag.error = error;
                let omega = 2. * std::f64::consts::PI / 0.68;
                let acceleration =
                    error.clamp_length(1.2) * (omega * omega) - self.velocity * (2. * 1.12 * omega);
                self.velocity = (self.velocity + acceleration * dt).clamp_length(9.);
                self.rotation = Quaternion::from_vector(self.velocity * dt)
                    .compose(self.rotation)
                    .normalized();
            }
            if reduced {
                self.rotation = self.target;
                self.velocity = Vec3::ZERO;
            }
            return steps > 0 || reduced;
        }
        let Some(spring) = self.spring.as_mut() else {
            return false;
        };
        let seconds = ((now - spring.at) / 1000.).max(0.);
        let steps = ((seconds * 120.).floor() as usize).min(240);
        let target = self.target;
        let integrate = |q: &mut Quaternion, speed: &mut Vec3| {
            let frequency = 2. * std::f64::consts::PI / 0.5;
            let error = target.compose(q.inverse()).vector();
            let acceleration = error * (frequency * frequency) - *speed * (2. * frequency * 0.78);
            *speed = (*speed + acceleration * (1. / 120.)).clamp_length(9.);
            *q = Quaternion::from_vector(*speed * (1. / 120.))
                .compose(*q)
                .normalized();
        };
        while spring.steps < steps {
            integrate(&mut spring.rotation, &mut spring.velocity);
            spring.steps += 1;
        }
        let mut next_rotation = spring.rotation;
        let mut next_velocity = spring.velocity;
        integrate(&mut next_rotation, &mut next_velocity);
        let fraction = ((seconds - steps as f64 / 120.) * 120.).min(1.);
        self.rotation = spring.rotation.slerp(next_rotation, fraction);
        self.velocity = spring.velocity + (next_velocity - spring.velocity) * fraction;
        if reduced
            || steps == 240
            || (self.rotation.angle_to(self.target) < 0.0005 && self.velocity.length() < 0.005)
        {
            self.rotation = self.target;
            self.velocity = Vec3::ZERO;
            self.spring = None;
        }
        true
    }
    fn begin_drag(&mut self, now: f64) {
        if self.held || self.drag.is_some() {
            return;
        }
        self.advance(now, false);
        self.drag = Some(MotionDrag {
            start: self.rotation,
            rest: self.target,
            x: 0.,
            y: 0.,
            error: Vec3::ZERO,
            at: now,
        });
        self.target = self.rotation;
        self.gesture_velocity = Vec3::ZERO;
        self.last_input = f64::NEG_INFINITY;
        self.spring = None;
        self.spin = None;
    }
    pub fn drag_active(
        &mut self,
        active: bool,
        now: f64,
        choose: impl FnOnce(Quaternion) -> Quaternion,
    ) {
        if active {
            if self.drag.is_some() || self.spin.is_some() {
                self.advance(now, false);
                self.drag = None;
                self.spin = None;
            }
            self.begin_drag(now);
        } else {
            self.advance(now, false);
            self.release(now, choose);
        }
    }
    fn release(&mut self, now: f64, choose: impl FnOnce(Quaternion) -> Quaternion) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if now - self.last_input > 100. {
            self.gesture_velocity = Vec3::ZERO;
        }
        let prediction =
            Quaternion::from_vector(self.gesture_velocity * 0.085).compose(self.target);
        self.velocity = (self.velocity * 0.2 + self.gesture_velocity * 0.8).clamp_length(22.);
        let moved = drag.x != 0. || drag.y != 0.;
        if moved && self.velocity.length() >= 4.5 {
            let projected =
                Quaternion::from_vector(self.velocity * (1. / 1.1)).compose(self.rotation);
            self.target = choose(projected).normalized();
            self.spin = Some(MotionSpin {
                at: now,
                rotation: self.rotation,
                velocity: self.velocity,
                correction: self.target.compose(projected.inverse()).vector(),
            });
            return;
        }
        self.spring(if moved { choose(prediction) } else { drag.rest }, now);
    }
    pub fn orbit(&mut self, x: f64, y: f64, now: f64) {
        if self.held || ![x, y, now].iter().all(|v| v.is_finite()) || (x == 0. && y == 0.) {
            return;
        }
        self.advance(now, false);
        self.begin_drag(now);
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let previous = self.target;
        drag.x += x;
        drag.y += y;
        self.target = Quaternion::from_vector(Vec3 {
            x: drag.y * 0.006,
            y: drag.x * 0.006,
            z: 0.,
        })
        .compose(drag.start)
        .normalized();
        let seconds = (now - self.last_input) / 1000.;
        self.gesture_velocity = self.target.compose(previous.inverse()).vector();
        self.gesture_velocity = if seconds > 0. && seconds < 0.1 {
            (self.gesture_velocity * (1. / seconds)).clamp_length(22.)
        } else {
            Vec3::ZERO
        };
        self.last_input = now;
    }
    pub fn hold(&mut self, active: bool, now: f64, choose: impl FnOnce(Quaternion) -> Quaternion) {
        if self.held == active {
            return;
        }
        if active {
            self.interrupted_drag = self.drag.is_some() || self.spin.is_some();
            self.drag = None;
            self.spin = None;
            self.spring = None;
            self.velocity = Vec3::ZERO;
        }
        self.held = active;
        if !active {
            if self.interrupted_drag {
                self.spring(choose(self.rotation), now);
            } else if self.rotation.angle_to(self.target) > 0.0005 {
                self.spring(self.target, now);
            }
            self.interrupted_drag = false;
        }
    }
    pub fn needs_frame(&self) -> bool {
        !self.held
            && (self.spring.is_some()
                || self.spin.is_some()
                || (self.drag.is_some()
                    && (self.rotation.angle_to(self.target) > 0.0005
                        || self.velocity.length() > 0.005)))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}
impl Ray {
    pub fn perspective(
        point: Point,
        vertical_fov: f64,
        aspect: f64,
        camera: Vec3,
        rotation: Quaternion,
    ) -> Self {
        let tangent = (vertical_fov / 2.).tan();
        Self {
            origin: camera,
            direction: rotation.rotate(
                Vec3 {
                    x: (point.x * 2. - 1.) * tangent * aspect,
                    y: (1. - point.y * 2.) * tangent,
                    z: -1.,
                }
                .normalized(),
            ),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct DisplaySurface {
    pub center: Vec3,
    pub rotation: Quaternion,
    pub width: f64,
    pub height: f64,
}
impl DisplaySurface {
    pub fn point(&self, ray: Ray, captured: bool) -> Option<Point> {
        let normal = self.rotation.rotate(Vec3::Z);
        let dot = normal.dot(ray.direction);
        if dot.abs() < 1e-12 || (!captured && dot >= 0.) {
            return None;
        }
        let distance = (self.center - ray.origin).dot(normal) / dot;
        if distance < 0. {
            return None;
        }
        let local = self
            .rotation
            .inverse()
            .rotate(ray.origin + ray.direction * distance - self.center);
        if !captured && (local.x.abs() > self.width / 2. || local.y.abs() > self.height / 2.) {
            return None;
        }
        Some(Point {
            x: local.x,
            y: local.y,
        })
    }
    pub fn phone_point(&self, ray: Ray, captured: bool, orientation: Orientation) -> Option<Point> {
        self.point(ray, captured)
            .map(|local| display_point(local, self.width, self.height, orientation))
    }
}
#[derive(Debug, Default)]
pub struct DuoPinch {
    angle: Option<f64>,
}
impl DuoPinch {
    pub fn begin(&mut self, contains: bool, angle: f64) -> bool {
        if !contains {
            return false;
        }
        self.angle = Some(angle.clamp(0., 180.));
        true
    }
    pub fn move_scale(&mut self, scale: f64) -> Option<f64> {
        if !scale.is_finite() {
            return None;
        }
        let angle = self.angle?;
        let next = (angle + scale * 120.).clamp(0., 180.);
        if next == angle {
            return None;
        }
        self.angle = Some(next);
        Some(next)
    }
    pub fn end(&mut self) -> bool {
        self.angle.take().is_some()
    }
}

#[derive(Debug, Default)]
pub struct DuoFrameElection {
    screen: Option<ScreenConfig>,
    ready_key: String,
    primary_key: String,
    activation_at: u64,
}
impl DuoFrameElection {
    pub fn set_screen(&mut self, screen: Option<ScreenConfig>, now: u64) -> bool {
        let changed = self.screen.as_ref().map(ScreenConfig::display_key)
            != screen.as_ref().map(ScreenConfig::display_key);
        if changed {
            self.ready_key.clear();
            self.activation_at = now;
        }
        self.screen = screen;
        changed
    }
    pub fn frame(
        &mut self,
        panel: u32,
        width: f64,
        height: f64,
        primary: bool,
        blank: bool,
        now: u64,
    ) -> bool {
        let Some(screen) = &self.screen else {
            return false;
        };
        let key = screen.display_key();
        if screen.screen_id != Some(panel)
            || (!primary && self.primary_key == key)
            || !duo_frame_matches(width, height, screen)
        {
            return false;
        }
        if !primary
            && self.ready_key.is_empty()
            && now.saturating_sub(self.activation_at) < 1500
            && blank
        {
            return false;
        }
        self.ready_key = key.clone();
        if primary {
            self.primary_key = key;
        }
        true
    }
    pub fn input_ready(&self) -> bool {
        self.screen
            .as_ref()
            .is_some_and(|s| !self.ready_key.is_empty() && self.ready_key == s.display_key())
    }
}
pub fn validate_imported_display(
    count: usize,
    matrix: &[f64; 16],
    bounds: Bounds3,
) -> Result<(), PlatformError> {
    if count != 1 {
        return Err(PlatformError::InvalidData(
            "Device model must have one device-screen mesh".into(),
        ));
    }
    let width = bounds.max.x - bounds.min.x;
    let height = bounds.max.y - bounds.min.y;
    if matrix
        .iter()
        .enumerate()
        .any(|(i, v)| !v.is_finite() || (v - if i % 5 == 0 { 1. } else { 0. }).abs() > 0.001)
        || ![width, height, bounds.min.z, bounds.max.z]
            .iter()
            .all(|v| v.is_finite())
        || width <= 0.
        || (height - 2.2).abs() > 0.001
        || (bounds.max.z - bounds.min.z).abs() > 0.001
        || (bounds.max.x + bounds.min.x).abs() > 0.001
        || (bounds.max.y + bounds.min.y).abs() > 0.001
    {
        return Err(PlatformError::InvalidData(
            "Device display is not normalized".into(),
        ));
    }
    Ok(())
}
pub fn ios_key_packet(code: &str, down: bool) -> Option<Vec<u8>> {
    tagged_json(
        6,
        json!({"type":if down {"down"}else {"up"},"usage":hid_usage(code)?}),
    )
    .ok()
}
pub fn android_key_packet(key: &str, down: bool, ctrl: bool, meta: bool) -> Option<Value> {
    if !down {
        return None;
    }
    if key == "Escape" {
        return Some(json!({"type":"back"}));
    }
    let keycode = match key {
        "ArrowUp" => 19,
        "ArrowDown" => 20,
        "ArrowLeft" => 21,
        "ArrowRight" => 22,
        "Tab" => 61,
        "Enter" => 66,
        "Backspace" => 67,
        "Delete" => 112,
        "Home" => 122,
        "End" => 123,
        "PageUp" => 92,
        "PageDown" => 93,
        _ => 0,
    };
    if keycode != 0 {
        Some(json!({"type":"key","keycode":keycode}))
    } else if key.encode_utf16().count() == 1 && !ctrl && !meta {
        Some(json!({"type":"text","text":key}))
    } else {
        None
    }
}
#[derive(Default)]
pub struct IceGatheringPolicy {
    deadline: Option<u64>,
}
impl IceGatheringPolicy {
    pub fn timeout_ms(&self) -> u64 {
        self.deadline.unwrap_or(2000)
    }
    pub fn gathered(&mut self, complete: bool, sdp: &str) -> bool {
        let external = sdp.lines().any(|line| {
            line.starts_with("a=candidate:")
                && (line.contains(" typ srflx") || line.contains(" typ relay"))
        });
        if !complete && !external && self.timeout_ms() < 4000 {
            self.deadline = Some((self.timeout_ms() * 2).min(4000));
            false
        } else {
            true
        }
    }
    pub fn received_track(&mut self) {
        self.deadline = None;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CameraShape {
    pub width: f64,
    pub height: f64,
    pub inset_x: f64,
    pub inset_y: f64,
    pub lens_radius: f64,
    pub lenses: &'static [(f64, f64)],
    pub flash: Option<(f64, f64)>,
}
#[derive(Debug, Clone, Copy)]
pub struct DeviceShape {
    pub id: &'static str,
    pub bezel: f64,
    pub body_radius: f64,
    pub screen_radius: f64,
    pub depth: f64,
    pub back_color: u32,
    pub buttons: &'static [(&'static str, f64, f64)],
    pub camera: CameraShape,
}
pub const IOS_PHONE_SHAPE: DeviceShape = DeviceShape {
    id: "ios-phone",
    bezel: 0.055,
    body_radius: 0.15,
    screen_radius: 0.105,
    depth: 0.085,
    back_color: 0x424b5d,
    buttons: &[
        ("right", 0.35, 0.3),
        ("left", 0.48, 0.18),
        ("left", 0.22, 0.18),
    ],
    camera: CameraShape {
        width: 0.39,
        height: 0.44,
        inset_x: 0.25,
        inset_y: 0.29,
        lens_radius: 0.068,
        lenses: &[(-0.08, 0.095), (0.08, -0.095)],
        flash: Some((0.085, 0.11)),
    },
};
pub const IOS_TABLET_SHAPE: DeviceShape = DeviceShape {
    id: "ios-tablet",
    bezel: 0.065,
    body_radius: 0.105,
    screen_radius: 0.045,
    depth: 0.055,
    back_color: 0x9ca5af,
    buttons: &[
        ("top", 0.5, 0.15),
        ("right", 0.78, 0.13),
        ("right", 0.58, 0.13),
    ],
    camera: CameraShape {
        width: 0.19,
        height: 0.19,
        inset_x: 0.15,
        inset_y: 0.15,
        lens_radius: 0.045,
        lenses: &[(0., 0.)],
        flash: None,
    },
};
pub const ANDROID_PHONE_SHAPE: DeviceShape = DeviceShape {
    id: "android-phone",
    bezel: 0.035,
    body_radius: 0.115,
    screen_radius: 0.08,
    depth: 0.085,
    back_color: 0x344449,
    buttons: &[("right", 0.2, 0.24), ("right", 0.65, 0.32)],
    camera: CameraShape {
        width: 0.24,
        height: 0.47,
        inset_x: 0.18,
        inset_y: 0.29,
        lens_radius: 0.056,
        lenses: &[(0., 0.11), (0., -0.11)],
        flash: Some((0.1, 0.)),
    },
};
pub const ANDROID_TABLET_SHAPE: DeviceShape = DeviceShape {
    id: "android-tablet",
    back_color: 0x697b80,
    ..IOS_TABLET_SHAPE
};
pub fn resolve_device_shape(
    platform: DevicePlatform,
    name: &str,
    portrait_aspect: f64,
) -> DeviceShape {
    let words = name
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let named_tablet = words.iter().any(|w| w == "ipad" || w == "tablet");
    let named_phone = words.iter().any(|w| w == "iphone" || w == "phone");
    let tablet = named_tablet || (!named_phone && portrait_aspect >= 0.6);
    match (platform, tablet) {
        (DevicePlatform::Ios, true) => IOS_TABLET_SHAPE,
        (DevicePlatform::Ios, false) => IOS_PHONE_SHAPE,
        (DevicePlatform::Android, true) => ANDROID_TABLET_SHAPE,
        (DevicePlatform::Android, false) => ANDROID_PHONE_SHAPE,
    }
}
pub struct AndroidFoldGeometry {
    pub inner_aspect: f64,
    angle: f64,
    captured: Option<usize>,
}
impl AndroidFoldGeometry {
    pub fn new(inner_aspect: f64, angle: f64) -> Self {
        Self {
            inner_aspect,
            angle: angle.clamp(0., 180.),
            captured: None,
        }
    }
    pub fn set_angle(&mut self, angle: f64) {
        self.angle = angle.clamp(0., 180.);
    }
    pub fn surfaces(&self) -> Vec<DisplaySurface> {
        let half_width = self.inner_aspect * (2.2 - 0.052) / 2. + 0.026;
        let pivot = Vec3::Z * 0.0425;
        let rotation =
            Quaternion::from_vector(Vec3::Y * (std::f64::consts::PI * (1. - self.angle / 180.)));
        if self.angle < 90. {
            vec![DisplaySurface {
                center: rotation.rotate(
                    Vec3 {
                        x: -half_width / 2.,
                        y: 0.,
                        z: -0.0405,
                    } - pivot,
                ) + pivot,
                rotation: rotation.compose(Quaternion::from_vector(Vec3::Y * std::f64::consts::PI)),
                width: half_width - 0.072,
                height: 2.108,
            }]
        } else {
            let width = half_width - 0.028;
            vec![
                DisplaySurface {
                    center: rotation.rotate(
                        Vec3 {
                            x: -(half_width - 0.024) / 2.,
                            y: 0.,
                            z: 0.0405,
                        } - pivot,
                    ) + pivot,
                    rotation,
                    width,
                    height: 2.148,
                },
                DisplaySurface {
                    center: Vec3 {
                        x: (half_width - 0.024) / 2.,
                        y: 0.,
                        z: 0.0405,
                    },
                    rotation: Quaternion::IDENTITY,
                    width,
                    height: 2.148,
                },
            ]
        }
    }
    pub fn screen_point(
        &mut self,
        ray: Ray,
        captured: bool,
        orientation: Orientation,
    ) -> Option<Point> {
        let surfaces = self.surfaces();
        let hit = surfaces
            .iter()
            .enumerate()
            .filter_map(|(i, surface)| {
                surface.point(ray, false).map(|point| {
                    let world = surface.rotation.rotate(Vec3 {
                        x: point.x,
                        y: point.y,
                        z: 0.,
                    }) + surface.center;
                    ((world - ray.origin).length(), i, point)
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let (index, point) = if let Some((_, index, point)) = hit {
            self.captured = Some(index);
            (index, point)
        } else if captured {
            let index = self.captured.filter(|&i| i < surfaces.len()).unwrap_or(0);
            (index, surfaces[index].point(ray, true)?)
        } else {
            self.captured = None;
            return None;
        };
        let surface = surfaces[index];
        let u = ((point.x + surface.width / 2.) / surface.width).clamp(0., 1.);
        let v = ((surface.height / 2. - point.y) / surface.height).clamp(0., 1.);
        let across = if surfaces.len() == 1 {
            u
        } else {
            (index as f64 + u) / 2.
        };
        Some(if orientation == Orientation::PortraitUpsideDown {
            Point {
                x: 1. - across,
                y: 1. - v,
            }
        } else {
            Point { x: across, y: v }
        })
    }
}

#[derive(Debug, Clone)]
pub struct DisplayTriangle {
    pub panel: Option<u32>,
    pub vertices: [Vec3; 3],
    pub uv: [Point; 3],
}
impl DisplayTriangle {
    fn intersect(&self, ray: Ray, extend: bool) -> Option<(f64, Point)> {
        let [a, b, c] = self.vertices;
        let ab = b - a;
        let ac = c - a;
        let normal = ab.cross(ac);
        let denominator = normal.dot(ray.direction);
        if denominator.abs() < 1e-12 || (!extend && denominator >= 0.) {
            return None;
        }
        let distance = normal.dot(a - ray.origin) / denominator;
        if distance < 0. {
            return None;
        }
        let point = ray.origin + ray.direction * distance - a;
        let (d00, d01, d11, d20, d21) = (
            ab.dot(ab),
            ab.dot(ac),
            ac.dot(ac),
            point.dot(ab),
            point.dot(ac),
        );
        let denominator = d00 * d11 - d01 * d01;
        if denominator.abs() < 1e-12 {
            return None;
        }
        let v = (d11 * d20 - d01 * d21) / denominator;
        let w = (d00 * d21 - d01 * d20) / denominator;
        let u = 1. - v - w;
        if !extend && (u < 0. || v < 0. || w < 0.) {
            return None;
        }
        Some((
            distance,
            Point {
                x: self.uv[0].x * u + self.uv[1].x * v + self.uv[2].x * w,
                y: self.uv[0].y * u + self.uv[1].y * v + self.uv[2].y * w,
            },
        ))
    }
}
#[derive(Default)]
pub struct DuoProjection {
    captured: Option<(String, DisplayTriangle)>,
}
impl DuoProjection {
    pub fn screen_point(
        &mut self,
        ray: Ray,
        screen: Option<&ScreenConfig>,
        ready_key: &str,
        triangles: &[DisplayTriangle],
        extend: bool,
    ) -> Option<Point> {
        let key = screen.map(ScreenConfig::display_key);
        if ready_key.is_empty() || key.as_deref() != Some(ready_key) {
            self.cancel();
            return None;
        }
        if extend
            && let Some((captured_key, triangle)) = &self.captured
            && captured_key == ready_key
        {
            let (_, uv) = triangle.intersect(ray, true)?;
            return Some(duo_raw_point(triangle.panel? as u8, uv.x, 1. - uv.y));
        }
        let (triangle, uv) = triangles
            .iter()
            .filter_map(|t| {
                t.intersect(ray, false)
                    .map(|(distance, uv)| (t, distance, uv))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(triangle, _, uv)| (triangle, uv))?;
        if triangle.panel.is_none() || triangle.panel != screen.and_then(|s| s.screen_id) {
            return None;
        }
        self.captured = Some((ready_key.into(), triangle.clone()));
        Some(duo_raw_point(triangle.panel? as u8, uv.x, 1. - uv.y))
    }
    pub fn cancel(&mut self) {
        self.captured = None;
    }
}
pub fn duo_rest_frames(
    angle: f64,
    panel: u8,
    cover: DisplaySurface,
    left: DisplaySurface,
    right: DisplaySurface,
) -> Vec<RestFrame> {
    let frame = |face: &str, surfaces: &[DisplaySurface]| {
        let mut normal = Vec3::ZERO;
        let mut up = Vec3::ZERO;
        let mut min = Vec3 {
            x: f64::INFINITY,
            y: f64::INFINITY,
            z: f64::INFINITY,
        };
        let mut max = min * -1.;
        for surface in surfaces {
            normal = normal + surface.rotation.rotate(Vec3::Z);
            up = up + surface.rotation.rotate(Vec3::Y);
            for x in [-surface.width / 2., surface.width / 2.] {
                for y in [-surface.height / 2., surface.height / 2.] {
                    let p = surface.center + surface.rotation.rotate(Vec3 { x, y, z: 0. });
                    min = Vec3 {
                        x: min.x.min(p.x),
                        y: min.y.min(p.y),
                        z: min.z.min(p.z),
                    };
                    max = Vec3 {
                        x: max.x.max(p.x),
                        y: max.y.max(p.y),
                        z: max.z.max(p.z),
                    };
                }
            }
        }
        RestFrame {
            face: face.into(),
            normal: normal.normalized(),
            up: up.normalized(),
            center: (min + max) * 0.5,
        }
    };
    if panel == 1 {
        return vec![frame("cover", &[cover])];
    }
    let mut frames = vec![frame("inside", &[left, right])];
    if angle > 20. && angle < 165. {
        frames.extend([frame("left", &[left]), frame("right", &[right])]);
    }
    frames
}

#[derive(Clone)]
pub struct DeviceTarget {
    pub platform: DevicePlatform,
    pub device_id: String,
    pub access: DeviceHubAccess,
    pub prefer_mjpeg: bool,
    pub panel_id: Option<u8>,
    pub video_only: bool,
    pub serve_sim_only: bool,
}
impl DeviceTarget {
    pub fn avcc(&self) -> bool {
        self.platform == DevicePlatform::Ios || self.serve_sim_only
    }
    pub fn url(&self, path: &str, websocket: bool) -> String {
        self.access.with_query(&format!(
            "{}/vendor/{}{}",
            if websocket {
                &self.access.ws_base
            } else {
                &self.access.http_base
            },
            if self.avcc() {
                "serve-sim"
            } else {
                "serve-emu"
            },
            path
        ))
    }
    fn encoded_id(&self) -> String {
        form_encode(&self.device_id).replace('+', "%20")
    }
    pub fn video_url(&self, mjpeg: bool) -> String {
        let id = self.encoded_id();
        let panel = self
            .panel_id
            .map(|p| format!("/panel/{p}"))
            .unwrap_or_default();
        self.url(
            &format!(
                "/helper/{id}{}/stream.{}",
                if mjpeg { "" } else { &panel },
                if mjpeg { "mjpeg" } else { "avcc" }
            ),
            false,
        )
    }
    pub fn input_url(&self, input_only: bool) -> String {
        let id = self.encoded_id();
        self.url(
            &if self.avcc() {
                format!("/helper/ws?device={id}")
            } else {
                format!(
                    "/ws?device={id}&{}",
                    if input_only {
                        "video=0"
                    } else {
                        "frame-meta=1"
                    }
                )
            },
            true,
        )
    }
    pub fn screenshot_url(&self) -> String {
        let vendor = if self.platform == DevicePlatform::Ios {
            "serve-sim"
        } else {
            "serve-emu"
        };
        self.access.with_query(&format!(
            "{}/vendor/{vendor}/api/screenshot?device={}",
            self.access.http_base,
            self.encoded_id()
        ))
    }
    pub fn webrtc_url(&self, close: bool) -> String {
        let action = if close { "close" } else { "offer" };
        let id = self.encoded_id();
        self.url(
            &if self.avcc() {
                format!("/helper/{id}/webrtc/{action}")
            } else {
                format!("/webrtc/{action}?device={id}")
            },
            false,
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTransport {
    Avcc,
    Semu,
    Mjpeg,
    WebRtc,
}
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceTransportEffect {
    PrimeHelper { timeout_ms: u64 },
    ConnectInput { input_only: bool },
    Probe { timeout_ms: u64, locked: bool },
    Video(VideoTransport),
    WebRtc(Value),
    RefreshAccess,
    Failed(String),
    Retry { at: u64, video: bool },
    FixedPanelUnavailable,
}
pub struct DeviceTransportPolicy {
    pub target: DeviceTarget,
    pub web_codecs: bool,
    pub web_rtc: bool,
    pub transport: Option<VideoTransport>,
    pub ice: IceGatheringPolicy,
}
pub fn decode_webrtc_settings(api: &Value) -> Option<Value> {
    let settings = api
        .get("streamSettings")
        .filter(|v| !v.is_null())
        .or_else(|| api.get("stream"))?;
    if settings.get("transport")?.as_str()? != "webrtc" {
        return None;
    }
    if let Some(policy) = settings.get("iceTransportPolicy")
        && !matches!(policy.as_str(), Some("all" | "relay"))
    {
        return None;
    }
    if let Some(servers) = settings.get("iceServers") {
        for server in servers.as_array()? {
            let urls = server.get("urls")?;
            if !urls.is_string()
                && !urls
                    .as_array()
                    .is_some_and(|urls| urls.iter().all(Value::is_string))
            {
                return None;
            }
            if ["username", "credential"]
                .iter()
                .any(|key| server.get(key).is_some_and(|v| !v.is_string()))
            {
                return None;
            }
        }
    }
    Some(settings.clone())
}
impl DeviceTransportPolicy {
    pub fn start(&mut self) -> Vec<DeviceTransportEffect> {
        let mut effects = vec![];
        if self.target.avcc() {
            if !self.target.video_only {
                if !self.target.serve_sim_only {
                    effects.push(DeviceTransportEffect::PrimeHelper { timeout_ms: 2000 });
                }
                effects.push(DeviceTransportEffect::ConnectInput { input_only: true });
            }
            let codecs = self.web_codecs
                && !(self.target.platform == DevicePlatform::Ios
                    && self.target.prefer_mjpeg
                    && !self.target.serve_sim_only);
            if codecs {
                self.transport = Some(VideoTransport::Avcc);
                effects.push(DeviceTransportEffect::Video(VideoTransport::Avcc));
            } else if self.target.serve_sim_only {
                effects.push(DeviceTransportEffect::Failed(
                    "This browser cannot decode this device's stream (WebCodecs unavailable)."
                        .into(),
                ));
            } else {
                self.transport = Some(VideoTransport::Mjpeg);
                effects.push(DeviceTransportEffect::Video(VideoTransport::Mjpeg));
            }
        } else {
            effects.push(DeviceTransportEffect::Probe {
                timeout_ms: 5000,
                locked: false,
            });
        }
        effects
    }
    pub fn probed(&mut self, api: Option<&Value>, locked: bool) -> Vec<DeviceTransportEffect> {
        let settings = api
            .and_then(decode_webrtc_settings)
            .or_else(|| locked.then(|| json!({"transport":"webrtc"})));
        if let Some(settings) = settings {
            if !self.web_rtc {
                return vec![DeviceTransportEffect::Failed(
                    "This browser cannot receive the device's WebRTC stream.".into(),
                )];
            }
            let mut effects = vec![];
            if !self.target.avcc() && !self.target.video_only {
                effects.push(DeviceTransportEffect::ConnectInput { input_only: true });
            }
            self.transport = Some(VideoTransport::WebRtc);
            effects.push(DeviceTransportEffect::WebRtc(settings));
            effects
        } else if !self.target.avcc() {
            if !self.web_codecs {
                return vec![DeviceTransportEffect::Failed(
                    "This browser cannot decode the Android stream (WebCodecs unavailable).".into(),
                )];
            }
            self.transport = Some(VideoTransport::Semu);
            vec![
                DeviceTransportEffect::ConnectInput { input_only: false },
                DeviceTransportEffect::Video(VideoTransport::Semu),
            ]
        } else {
            vec![DeviceTransportEffect::Failed(
                "Could not receive the device stream. Reconnect to try again.".into(),
            )]
        }
    }
    pub fn video_response(
        &self,
        status: u16,
        body: Option<&Value>,
        now: u64,
    ) -> Vec<DeviceTransportEffect> {
        if [401, 403].contains(&status) {
            return vec![DeviceTransportEffect::RefreshAccess];
        }
        if status == 409
            && self.target.panel_id.is_none()
            && body.is_some_and(|b| {
                b.get("error").and_then(Value::as_str) == Some("stream_transport_locked")
                    && b.get("transport").and_then(Value::as_str) == Some("webrtc")
            })
        {
            return vec![DeviceTransportEffect::Probe {
                timeout_ms: 5000,
                locked: true,
            }];
        }
        if self.target.panel_id.is_some() && [400, 404, 405, 410].contains(&status) {
            return vec![DeviceTransportEffect::FixedPanelUnavailable];
        }
        if !(200..300).contains(&status) {
            return vec![DeviceTransportEffect::Retry {
                at: now.saturating_add(1000),
                video: true,
            }];
        }
        vec![]
    }
    pub fn decoder_unsupported(&mut self) -> Vec<DeviceTransportEffect> {
        if self.target.serve_sim_only
            || self.target.video_only
            || self.target.platform == DevicePlatform::Android
        {
            return vec![DeviceTransportEffect::Failed(
                "This browser cannot decode this device's stream.".into(),
            )];
        }
        self.transport = Some(VideoTransport::Mjpeg);
        vec![DeviceTransportEffect::Video(VideoTransport::Mjpeg)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(panel: u32, angle: f64, orientation: Orientation) -> ScreenConfig {
        ScreenConfig {
            width: 900.,
            height: 1950.,
            orientation,
            input_unavailable: false,
            screen_id: Some(panel),
            supports_hinge_angle: true,
            supports_physical_orientation: true,
            hinge_angle: Some(angle),
            hinge_pose: None,
            table_mode: false,
            table_mode_available: false,
        }
    }
    #[test]
    fn transport_policy_preserves_webview_fallbacks_and_locked_webrtc() {
        let target = DeviceTarget {
            platform: DevicePlatform::Ios,
            device_id: "test device".into(),
            access: DeviceHubAccess {
                http_base: "https://host/api/device-hub".into(),
                ws_base: "wss://host/api/device-hub".into(),
                query: BTreeMap::from([("wsTicket".into(), "ticket".into())]),
                credentials: false,
            },
            prefer_mjpeg: true,
            panel_id: None,
            video_only: false,
            serve_sim_only: false,
        };
        assert!(
            target
                .video_url(false)
                .contains("/helper/test%20device/stream.avcc?wsTicket=ticket")
        );
        let mut policy = DeviceTransportPolicy {
            target,
            web_codecs: true,
            web_rtc: true,
            transport: None,
            ice: IceGatheringPolicy::default(),
        };
        assert!(
            policy
                .start()
                .contains(&DeviceTransportEffect::Video(VideoTransport::Mjpeg))
        );
        assert_eq!(
            policy.video_response(401, None, 0),
            vec![DeviceTransportEffect::RefreshAccess]
        );
        let locked = json!({"error":"stream_transport_locked","transport":"webrtc"});
        assert!(matches!(
            policy.video_response(409, Some(&locked), 0).as_slice(),
            [DeviceTransportEffect::Probe { .. }]
        ));
        assert!(matches!(
            policy.probed(None, true).as_slice(),
            [DeviceTransportEffect::WebRtc(_)]
        ));
        policy.target.platform = DevicePlatform::Android;
        policy.target.serve_sim_only = true;
        assert!(
            policy
                .start()
                .contains(&DeviceTransportEffect::Video(VideoTransport::Avcc))
        );
        policy.target.serve_sim_only = false;
        assert!(policy.start().contains(&DeviceTransportEffect::Probe {
            timeout_ms: 5000,
            locked: false
        }));
        assert!(
            policy
                .probed(None, false)
                .contains(&DeviceTransportEffect::Video(VideoTransport::Semu))
        );
        policy.target.panel_id = Some(3);
        assert_eq!(
            policy.video_response(404, None, 0),
            vec![DeviceTransportEffect::FixedPanelUnavailable]
        );
        assert!(
            decode_webrtc_settings(
                &json!({"streamSettings":{"transport":"webrtc","iceServers":[{"urls":42}]}})
            )
            .is_none()
        );
        assert!(decode_webrtc_settings(&json!({"stream":{"transport":"webrtc","iceServers":[{"urls":["stun:a"]}],"iceTransportPolicy":"relay"}})).is_some());
    }
    #[test]
    fn avcc_reassembles_every_split_and_bounds_hostile_lengths() {
        let data = [0, 0, 0, 5, 1, 1, 100, 0, 31, 0, 0, 0, 3, 2, 9, 8];
        for split in 0..=data.len() {
            let mut demux = AvccDemuxer::default();
            let mut chunks = vec![];
            demux
                .push(&data[..split], |c| chunks.push(c))
                .expect("first");
            demux
                .push(&data[split..], |c| chunks.push(c))
                .expect("second");
            assert_eq!(
                chunks,
                vec![
                    AvccChunk {
                        kind: AvccKind::Description,
                        payload: vec![1, 100, 0, 31]
                    },
                    AvccChunk {
                        kind: AvccKind::Keyframe,
                        payload: vec![9, 8]
                    }
                ]
            );
        }
        let mut demux = AvccDemuxer::new(8);
        assert_eq!(
            demux.push(&[255, 255, 255, 255], |_| panic!("oversized")),
            Err(PlatformError::TooLarge)
        );
        let mut chunks = vec![];
        demux
            .push(&[0, 0, 0, 0, 0, 0, 0, 1, 99, 0, 0, 0, 1, 7], |c| {
                chunks.push(c)
            })
            .expect("unknown envelopes");
        assert_eq!(
            chunks,
            vec![AvccChunk {
                kind: AvccKind::End,
                payload: vec![]
            }]
        );
    }
    #[test]
    fn semu_and_annex_b_preserve_metadata_and_truncated_packets() {
        let mut bytes = b"SEMU".to_vec();
        bytes.extend([1, 1, 0, 0]);
        bytes.extend(1234u64.to_be_bytes());
        bytes.extend([0, 0, 1, 0x65]);
        assert_eq!(
            parse_semu_packet(&bytes),
            SemuPacket {
                data: &[0, 0, 1, 0x65],
                is_key: Some(true),
                timestamp: Some(1234)
            }
        );
        bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(parse_semu_packet(&bytes).timestamp, None);
        assert_eq!(parse_semu_packet(&[1, 2]).data, &[1, 2]);
        let unit = [0, 0, 0, 1, 0x67, 0x64, 0, 31, 0, 0, 1, 0x65, 1];
        let (key, sps) = scan_access_unit(&unit);
        assert!(key);
        assert_eq!(avc_codec_string(sps.expect("sps")), "avc1.64001f");
        assert!(!scan_access_unit(&[0, 0, 1, 0x41]).0);
        assert_eq!(scan_access_unit(&[0, 0, 1]), (false, None));
        assert_eq!(avc_codec_string(&[]), "avc1.42E01E");
    }
    #[test]
    fn stream_generation_discards_delayed_decoders_seeds_and_presentation_failures() {
        let mut s = DeviceSession::default();
        let old = s.start(0);
        let current = s.replace_video();
        assert!(!s.frame_presented(old, 1, true, true));
        assert!(s.frame_presented(current, 2, true, true));
        assert!(!s.frame_presented(current, 3, false, true));
        assert!(!s.frame_presented(current, 4, true, false));
        assert!(matches!(s.status, StreamStatus::Failed(_)));
        assert!(!s.frame_presented(current, 5, true, true));
        let fresh = s.start(6);
        assert!(s.frame_presented(fresh, 7, false, true));
        assert_eq!(s.status, StreamStatus::Streaming);
        s.stop();
        assert!(!s.frame_presented(fresh, 8, true, true));
    }
    #[test]
    fn keyframes_queue_pressure_and_pts_are_handled_without_decoding() {
        let mut s = DeviceSession::default();
        let token = s.start(0);
        assert_eq!(s.decode_timestamp(token, false, 0, None), None);
        assert_eq!(s.decode_timestamp(token, true, 0, Some(100)), Some(100));
        assert_eq!(s.decode_timestamp(token, false, 0, None), Some(16_667));
        assert_eq!(s.decode_timestamp(token, true, 9, None), None);
        assert!(!s.current(token));
    }
    #[test]
    fn injected_deadlines_reconnect_stalls_and_end_leases() {
        let mut s = DeviceSession::default();
        s.start(10);
        assert!(!s.tick(15_009, false));
        s.tick(15_010, false);
        assert!(matches!(s.status, StreamStatus::Failed(_)));
        let token = s.start(20_000);
        s.frame_presented(token, 20_001, true, true);
        assert!(!s.tick(35_000, true));
        assert!(s.tick(35_001, true));
        s.lease_ended(br#"{"reason":"expired"}"#);
        assert_eq!(
            s.status,
            StreamStatus::Failed("Device lease ended: expired".into())
        );
        s.start(0);
        assert!(!s.input_closed(s.input_token(), 1006, true, ""));
        assert_eq!(s.status, StreamStatus::Unauthorized);
    }
    #[test]
    fn closing_a_stopped_input_socket_does_not_request_new_credentials() {
        let mut session = DeviceSession::default();
        session.start(0);
        let input = session.input_token();
        session.stop();
        assert!(!session.input_closed(input, 1006, true, ""));
        assert_eq!(session.status, StreamStatus::Stopped);
    }
    #[test]
    fn input_reconnect_ignores_late_close_and_admission_from_same_session() {
        let mut session = DeviceSession::default();
        session.start(0);
        let old = session.replace_input();
        let new = session.replace_input();
        assert!(session.input_opened(new, false));
        assert!(!session.input_message(old, &[0x83]));
        assert!(!session.input_closed(old, 4401, true, "expired"));
        assert!(session.can_input(false));
        assert_eq!(session.status, StreamStatus::Connecting);
        assert!(session.input_message(new, &[0x83]));
        assert!(session.can_input(true));
    }
    #[test]
    fn fold_geometry_maps_open_halves_and_closed_cover() {
        let mut fold = AndroidFoldGeometry::new(DEFAULT_FOLD_INNER_ASPECT, 180.);
        let ray = |x| Ray {
            origin: Vec3 { x, y: 0., z: 6. },
            direction: Vec3::Z * -1.,
        };
        assert!(
            (fold
                .screen_point(ray(-0.52), false, Orientation::Portrait)
                .expect("left")
                .x
                - 0.25)
                .abs()
                < 0.05
        );
        assert!(
            (fold
                .screen_point(ray(0.52), false, Orientation::Portrait)
                .expect("right")
                .x
                - 0.75)
                .abs()
                < 0.05
        );
        assert_eq!(
            fold.screen_point(ray(5.), true, Orientation::Portrait)
                .expect("captured")
                .x,
            1.
        );
        fold.set_angle(0.);
        assert!(
            (fold
                .screen_point(ray(0.52), false, Orientation::Portrait)
                .expect("cover")
                .x
                - 0.5)
                .abs()
                < 0.05
        );
        assert_eq!(
            resolve_device_shape(DevicePlatform::Ios, "iPad Pro", 0.4).id,
            "ios-tablet"
        );
        assert_eq!(
            resolve_device_shape(DevicePlatform::Android, "Phone", 0.8).id,
            "android-phone"
        );
    }
    #[test]
    fn duo_projection_extends_barycentric_input_and_rejects_stale_inactive_surfaces() {
        let mut projection = DuoProjection::default();
        let screen = screen(3, 180., Orientation::Portrait);
        let key = screen.display_key();
        let triangle = DisplayTriangle {
            panel: Some(3),
            vertices: [
                Vec3 {
                    x: -1.,
                    y: -1.,
                    z: 0.,
                },
                Vec3 {
                    x: 1.,
                    y: -1.,
                    z: 0.,
                },
                Vec3 {
                    x: -1.,
                    y: 1.,
                    z: 0.,
                },
            ],
            uv: [
                Point { x: 0., y: 0. },
                Point { x: 1., y: 0. },
                Point { x: 0., y: 1. },
            ],
        };
        let ray = |x, y| Ray {
            origin: Vec3 { x, y, z: 5. },
            direction: Vec3::Z * -1.,
        };
        let hit = projection
            .screen_point(
                ray(-0.5, -0.5),
                Some(&screen),
                &key,
                std::slice::from_ref(&triangle),
                false,
            )
            .expect("hit");
        assert_eq!(hit, Point { x: 0.75, y: 0.75 });
        let extended = projection
            .screen_point(
                ray(-0.5, -2.),
                Some(&screen),
                &key,
                std::slice::from_ref(&triangle),
                true,
            )
            .expect("extended");
        assert!(extended.x > 1.);
        assert!(
            projection
                .screen_point(
                    ray(-0.5, -0.5),
                    Some(&screen),
                    "stale",
                    std::slice::from_ref(&triangle),
                    false
                )
                .is_none()
        );
        let other = ScreenConfig {
            screen_id: Some(1),
            ..screen
        };
        assert!(
            projection
                .screen_point(
                    ray(-0.5, -0.5),
                    Some(&other),
                    &other.display_key(),
                    &[triangle],
                    false
                )
                .is_none()
        );
    }
    #[test]
    fn screen_input_admission_and_malformed_configs() {
        let mut s = DeviceSession::default();
        s.start(0);
        s.input_connected = true;
        assert!(!s.can_input(true));
        s.input_message(s.input_token(), &[0x83]);
        assert!(s.can_input(true));
        let config = tagged_json(
            0x82,
            json!({"width":900,"height":1950,"orientation":"portrait","inputUnavailable":true}),
        )
        .expect("packet");
        assert!(s.input_message(s.input_token(), &config));
        assert!(!s.can_input(true));
        assert!(
            ScreenConfig::decode(br#"{"width":0,"height":1,"orientation":"portrait"}"#).is_none()
        );
        assert!(
            ScreenConfig::decode(
                br#"{"width":1,"height":1,"orientation":"portrait","hingeAngle":181}"#
            )
            .is_none()
        );
    }
    #[test]
    fn hid_keypad_packets_and_android_utf8_chunk_limits() {
        for (code, usage) in [
            ("NumpadEnter", 0x58),
            ("Numpad0", 0x62),
            ("Numpad1", 0x59),
            ("NumpadDecimal", 0x63),
            ("KeyA", 4),
            ("Digit0", 0x27),
        ] {
            let packet = ios_key_packet(code, false).expect("HID");
            assert_eq!(packet[0], 6);
            assert_eq!(
                serde_json::from_slice::<Value>(&packet[1..]).expect("JSON"),
                json!({"type":"up","usage":usage})
            );
        }
        let text = "🧪".repeat(6000);
        let chunks = android_text_chunks(&text);
        assert_eq!(chunks.len(), 64);
        assert!(chunks.iter().all(|c| c.len() <= 300));
        assert_eq!(chunks.concat(), "🧪".repeat(64 * 75));
        assert!(android_text_chunks("").is_empty());
        assert_eq!(
            android_key_packet("Enter", true, false, false),
            Some(json!({"type":"key","keycode":66}))
        );
        assert!(android_key_packet("a", true, true, false).is_none());
    }
    #[test]
    fn duo_queue_coalesces_acknowledgments_and_keeps_failures_visible() {
        let mut c = DuoControl::default();
        let first = c.enqueue(DuoCommand::Angle(40.), 0).expect("first");
        assert_eq!(first.request_id, 1);
        assert!(c.enqueue(DuoCommand::Angle(50.), 0).is_none());
        c.enqueue(DuoCommand::Angle(60.), 0);
        let next = c.receive(1, true, None, 1).expect("coalesced");
        assert_eq!(next.command, DuoCommand::Angle(60.));
        c.enqueue(DuoCommand::Angle(100.), 1);
        c.enqueue(DuoCommand::Pose(DuoPose::Tent), 1);
        let next = c.receive(2, true, None, 2).expect("preset");
        assert_eq!(next.command, DuoCommand::Pose(DuoPose::Tent));
        assert!(c.receive(1, true, None, 2).is_none());
        c.receive(3, false, Some("denied".into()), 3);
        assert_eq!(c.error.as_deref(), Some("denied"));
        assert!(c.requested().is_none());
        c.enqueue(DuoCommand::Angle(90.), 5);
        c.tick(5005);
        assert!(c.error.as_deref().is_some_and(|s| s.contains("timed out")));
        assert!(c.enqueue(DuoCommand::Angle(f64::NAN), 0).is_none());
    }
    #[test]
    fn fold_orientation_and_pinch_do_not_follow_delayed_native_readback() {
        for panel in [1, 3] {
            for vertical in [true, false] {
                let s = screen(
                    panel,
                    if panel == 1 { 0. } else { 180. },
                    duo_hold_orientation(vertical, Some(panel)),
                );
                assert_eq!(s.phone_vertical(), vertical);
                assert!(s.settled());
            }
        }
        assert!(!screen(1, 90., Orientation::LandscapeLeft).settled());
        assert!(!screen(3, 0., Orientation::Portrait).settled());
        let mut pinch = DuoPinch::default();
        assert!(!pinch.begin(false, 90.));
        assert!(pinch.begin(true, 90.));
        assert_eq!(pinch.move_scale(0.25), Some(120.));
        assert_eq!(pinch.move_scale(0.25), Some(150.));
        assert_eq!(pinch.move_scale(2.), Some(180.));
        assert_eq!(pinch.move_scale(-3.), Some(0.));
        assert_eq!(pinch.move_scale(f64::NAN), None);
        assert!(pinch.end());
        assert!(!pinch.end());
    }
    #[test]
    fn interaction_owns_pointer_and_releases_last_valid_touch() {
        let mut i = PhoneInteraction::default();
        let p = Point { x: 0.2, y: 0.3 };
        assert_eq!(i.begin(1, p, Some(p)).len(), 2);
        assert!(i.begin(2, p, None).is_empty());
        assert!(!i.can_navigate());
        assert!(i.move_pointer(2, p, Some(p)).is_none());
        assert!(i.move_pointer(1, p, None).is_none());
        assert!(i.end(Some(2)).is_empty());
        assert_eq!(
            i.end(Some(1)),
            vec![
                InteractionEvent::Touch("end", p),
                InteractionEvent::Active(false, true)
            ]
        );
        assert!(i.can_navigate());
        let n = phone_wheel_navigation(
            Point { x: 10., y: 20. },
            1,
            false,
            Point { x: 400., y: 800. },
        );
        assert_eq!(
            n,
            Some(PhoneNavigation::Orbit(Point { x: -0.25, y: -0.25 }))
        );
        assert!(phone_wheel_navigation(p, 3, true, p).is_none());
    }
    #[test]
    fn projection_preserves_orientations_rejects_rear_and_clamps_captured_drags() {
        let surface = DisplaySurface {
            center: Vec3::ZERO,
            rotation: Quaternion::IDENTITY,
            width: 1.,
            height: 2.2,
        };
        for (orientation, expected) in [
            (Orientation::Portrait, Point { x: 0.25, y: 0.75 }),
            (Orientation::LandscapeLeft, Point { x: 0.25, y: 0.25 }),
            (Orientation::LandscapeRight, Point { x: 0.75, y: 0.75 }),
            (Orientation::PortraitUpsideDown, Point { x: 0.75, y: 0.25 }),
        ] {
            let ray = Ray {
                origin: Vec3 {
                    x: -0.25,
                    y: -0.55,
                    z: 5.,
                },
                direction: Vec3 {
                    x: 0.,
                    y: 0.,
                    z: -1.,
                },
            };
            assert_eq!(surface.phone_point(ray, false, orientation), Some(expected));
        }
        let ray = Ray {
            origin: Vec3 {
                x: 2.,
                y: 0.,
                z: 5.,
            },
            direction: Vec3 {
                x: 0.,
                y: 0.,
                z: -1.,
            },
        };
        assert!(
            surface
                .phone_point(ray, false, Orientation::Portrait)
                .is_none()
        );
        assert_eq!(
            surface.phone_point(ray, true, Orientation::Portrait),
            Some(Point { x: 1., y: 0.5 })
        );
        let rear = DisplaySurface {
            rotation: Quaternion::from_vector(Vec3::Y * std::f64::consts::PI),
            ..surface
        };
        assert!(
            rear.point(
                Ray {
                    origin: Vec3::Z * 5.,
                    direction: Vec3::Z * -1.
                },
                false
            )
            .is_none()
        );
    }
    #[test]
    fn nearest_snap_preserves_yaw_and_quaternion_sign() {
        let snaps = (0..4)
            .map(|i| ViewSnap {
                rotation: Quaternion::from_vector(
                    Vec3::Z * (-(i as f64) * std::f64::consts::FRAC_PI_2),
                ),
                yaw_limit: std::f64::consts::PI / 3.,
                orientation: [
                    Orientation::Portrait,
                    Orientation::LandscapeLeft,
                    Orientation::PortraitUpsideDown,
                    Orientation::LandscapeRight,
                ][i],
                face: "inside".into(),
                center: Vec3::ZERO,
            })
            .collect::<Vec<_>>();
        let released = Quaternion::from_vector(Vec3::Y * 0.4).compose(snaps[1].rotation);
        let nearest = nearest_device_view(released, &snaps).expect("snap");
        assert_eq!(nearest.orientation, Orientation::LandscapeLeft);
        assert!(nearest.rotation.angle_to(released) < 1e-6);
        let negative = Quaternion {
            x: -released.x,
            y: -released.y,
            z: -released.z,
            w: -released.w,
        };
        assert!(
            nearest_device_view(negative, &snaps)
                .expect("sign")
                .rotation
                .angle_to(nearest.rotation)
                < 1e-6
        );
        assert!(
            nearest_device_view(Quaternion::from_vector(Vec3::Y * 3.), &snaps)
                .expect("limit")
                .rotation
                .angle_to(Quaternion::IDENTITY)
                <= std::f64::consts::PI / 3. + 1e-6
        );
    }
    #[test]
    fn leaf_snap_and_motion_resume_interrupted_rest_without_a_drag() {
        let frames = [
            RestFrame {
                face: "inside".into(),
                normal: Vec3::Z,
                up: Vec3::Y,
                center: Vec3::ZERO,
            },
            RestFrame {
                face: "right".into(),
                normal: Vec3 {
                    x: 0.6,
                    y: 0.,
                    z: 0.8,
                },
                up: Vec3::Y,
                center: Vec3::X,
            },
        ];
        let snaps = duo_view_snaps(&frames, 3);
        let seat = snaps.iter().find(|s| s.face == "right").expect("seat");
        let released = Quaternion::from_vector(Vec3 {
            x: 0.04,
            y: 0.05,
            z: 0.,
        })
        .compose(seat.rotation);
        assert_eq!(
            nearest_device_view(released, &snaps).expect("nearest").face,
            "right"
        );
        let mut motion = DeviceMotion::default();
        let rest = Quaternion::from_vector(Vec3::Y * 0.8);
        motion.set_pose(rest, 0., false);
        motion.advance(50., false);
        motion.drag_active(true, 50., |_| panic!("no selection"));
        motion.advance(60., false);
        motion.drag_active(false, 60., |_| panic!("no selection"));
        motion.advance(2100., false);
        assert!(motion.rotation.angle_to(rest) < 1e-6);
        assert!(!motion.needs_frame());
    }
    #[test]
    fn model_generations_and_scheduler_reject_late_work() {
        let mut slot = ModelSlot::default();
        let old = slot
            .set(Some(("pro".into(), "/a".into())))
            .expect("request");
        assert!(slot.set(Some(("pro".into(), "/a".into()))).is_none());
        let new = slot
            .set(Some(("max".into(), "/b".into())))
            .expect("replacement");
        assert!(!slot.loaded(old));
        assert!(slot.loaded(new));
        slot.failed(old);
        assert!(slot.installed);
        slot.dispose();
        assert!(!slot.loaded(new));
        let mut scheduler = RenderScheduler::default();
        assert!(scheduler.invalidate());
        assert!(!scheduler.invalidate());
        assert!(scheduler.frame());
        assert!(scheduler.invalidate());
        scheduler.dispose();
        assert!(!scheduler.frame());
    }
    #[test]
    fn primary_elected_feed_wins_and_fixed_shutdown_blanks_are_bounded() {
        let mut election = DuoFrameElection::default();
        election.set_screen(Some(screen(1, 90., Orientation::Portrait)), 0);
        assert!(!election.frame(1, 900., 1950., false, true, 10));
        assert!(election.frame(1, 900., 1950., true, true, 10));
        assert!(!election.frame(1, 900., 1950., false, false, 20));
        assert!(!election.frame(3, 900., 1950., true, false, 20));
        assert!(election.input_ready());
        election.set_screen(Some(screen(3, 90., Orientation::Portrait)), 100);
        assert!(!election.input_ready());
        assert!(election.frame(3, 900., 1950., false, true, 1600));
    }
    #[test]
    fn framing_settles_and_ice_deadlines_remain_bounded() {
        let mut framing = DeviceFraming::default();
        framing.set_bounds(
            Bounds3 {
                min: Vec3 {
                    x: -1.,
                    y: -2.,
                    z: 0.,
                },
                max: Vec3 {
                    x: 1.,
                    y: 2.,
                    z: 0.1,
                },
            },
            0.3,
            0.7,
            0.,
            false,
        );
        assert!(framing.distance() > 1.);
        assert!(!framing.needs_frame());
        let mut ice = IceGatheringPolicy::default();
        assert!(!ice.gathered(false, ""));
        assert_eq!(ice.timeout_ms(), 4000);
        assert!(ice.gathered(false, ""));
        ice.received_track();
        assert_eq!(ice.timeout_ms(), 2000);
        assert!(ice.gathered(false, "a=candidate:1 typ relay"));
    }
}
