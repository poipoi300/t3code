//! Router/history decisions. The host executes navigation and supplies history availability.
use crate::types::{EnvironmentId, SyncStatus, ThreadId, ThreadKey};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteTarget {
    Server(ThreadKey),
    Draft(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderState {
    Loading,
    Ready,
    Missing,
}
pub fn snapshot_authoritative(status: Option<SyncStatus>) -> bool {
    status == Some(SyncStatus::Live)
}
pub fn render_state(
    bootstrap: bool,
    exists: bool,
    deleted: bool,
    detail_status: SyncStatus,
    detail_archived_or_deleted: Option<bool>,
    draft: bool,
) -> RenderState {
    if draft || (exists && !deleted) {
        return RenderState::Ready;
    }
    if !bootstrap {
        return RenderState::Loading;
    }
    if exists || detail_status == SyncStatus::Deleted {
        return RenderState::Missing;
    }
    match detail_archived_or_deleted {
        None => RenderState::Loading,
        Some(true) => RenderState::Missing,
        Some(false) => RenderState::Ready,
    }
}
pub fn route_target(
    environment: Option<&str>,
    thread: Option<&str>,
    draft: Option<&str>,
) -> Option<RouteTarget> {
    if let (Some(e), Some(t)) = (
        environment.filter(|s| !s.is_empty()),
        thread.filter(|s| !s.is_empty()),
    ) {
        return Some(RouteTarget::Server(ThreadKey {
            environment: EnvironmentId(e.into()),
            thread: ThreadId(t.into()),
        }));
    }
    draft
        .filter(|s| !s.is_empty())
        .map(|s| RouteTarget::Draft(s.into()))
}
pub fn active_thread<'a>(
    target: Option<&'a RouteTarget>,
    promoted: Option<&'a ThreadKey>,
    redirect: Option<&'a ThreadKey>,
) -> Option<&'a ThreadKey> {
    match target {
        Some(RouteTarget::Server(key)) => Some(key),
        Some(RouteTarget::Draft(_)) => promoted.or(redirect),
        None => None,
    }
}
pub fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
fn decode_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut result = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let part = bytes.get(i + 1..i + 3)?;
            let high = char::from(part[0]).to_digit(16)?;
            let low = char::from(part[1]).to_digit(16)?;
            result.push((high * 16 + low) as u8);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).ok()
}
pub fn is_thread_path(path: &str, key: &ThreadKey) -> bool {
    let parts = path
        .strip_suffix('/')
        .unwrap_or(path)
        .split('/')
        .collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].is_empty()
        && decode_component(parts[1]).as_deref() == Some(&key.environment.0)
        && decode_component(parts[2]).as_deref() == Some(&key.thread.0)
}
/// Historical localStorage format only; runtime maps use structured identity.
pub fn legacy_thread_key(key: &ThreadKey) -> String {
    format!("{}:{}", key.environment.0, key.thread.0)
}
pub fn parse_legacy_thread_key(key: &str) -> Option<ThreadKey> {
    crate::environment::parse_scoped_thread_key(key).map(|scoped| ThreadKey {
        environment: scoped.environment_id,
        thread: scoped.thread_id,
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryAvailability {
    pub back: bool,
    pub forward: bool,
}
impl HistoryAvailability {
    pub fn can_navigate(self, direction: Direction) -> bool {
        match direction {
            Direction::Back => self.back,
            Direction::Forward => self.forward,
        }
    }
}
