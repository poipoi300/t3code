use crate::types::{EnvironmentId, ThreadId};
use t3_contracts::checkpoint_diff::OrchestrationGetTurnDiffResult;
use t3_contracts::peer_link::{PeerLinkStatus, PeerLinkSummary};

pub type CheckpointDiffResult = OrchestrationGetTurnDiffResult;
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckpointDiffState {
    pub data: Option<CheckpointDiffResult>,
    pub error: Option<String>,
    pub pending: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointDiffTarget {
    pub environment: Option<EnvironmentId>,
    pub thread: Option<ThreadId>,
    pub from_turn_count: Option<u64>,
    pub to_turn_count: Option<u64>,
    pub ignore_whitespace: bool,
    pub cache_scope: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerPathKind {
    File,
    Directory,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerPathSearchEntry {
    pub path: String,
    pub kind: ComposerPathKind,
    pub parent_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerPathSearchTarget {
    pub environment: Option<EnvironmentId>,
    pub cwd: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerLinkHealth {
    Expired,
    Unreachable { detail: Option<String> },
    Failing { detail: String },
    Reachable { expires_in_days: Option<i64> },
}
pub fn peer_link_health(link: &PeerLinkSummary, now_epoch_ms: i64) -> PeerLinkHealth {
    let expires = crate::time::epoch_ms(&link.expires_at.0);
    if link.status == PeerLinkStatus::Expired
        || expires.is_some_and(|expires| expires <= now_epoch_ms)
    {
        return PeerLinkHealth::Expired;
    }
    if link.status == PeerLinkStatus::Unreachable {
        return PeerLinkHealth::Unreachable {
            detail: link.last_error.clone(),
        };
    }
    if let Some(error) = &link.last_error {
        return PeerLinkHealth::Failing {
            detail: error.clone(),
        };
    }
    let days = expires.map(|expires| {
        expires
            .saturating_sub(now_epoch_ms)
            .saturating_add(86_399_999)
            / 86_400_000
    });
    PeerLinkHealth::Reachable {
        expires_in_days: days.filter(|days| *days <= 5),
    }
}
pub struct EnvironmentCatalogEntry {
    pub registration: crate::environment::EnvironmentRegistration,
    pub unsupported_reason: Option<String>,
}
#[derive(Default)]
pub struct EnvironmentCatalogState {
    pub ready: bool,
    pub entries: Vec<EnvironmentCatalogEntry>,
}
pub fn enabled_environment_ids(
    catalog: &EnvironmentCatalogState,
) -> impl Iterator<Item = &EnvironmentId> {
    catalog
        .entries
        .iter()
        .filter(|entry| entry.registration.enabled && entry.unsupported_reason.is_none())
        .map(|entry| &entry.registration.id)
}
