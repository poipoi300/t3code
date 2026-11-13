use serde::{Deserialize, Serialize};
pub use t3_contracts::base_schemas::{CommandId, EnvironmentId, ProjectId, ThreadId};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ThreadKey {
    pub environment: EnvironmentId,
    pub thread: ThreadId,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation(pub u64);
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncStatus {
    #[default]
    Empty,
    Cached,
    Synchronizing,
    Live,
    Deleted,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    None,
    Applied,
    Duplicate,
    StaleSession,
    ReloadSnapshot,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryState<T> {
    pub value: Option<T>,
    pub loading: bool,
    pub error: Option<String>,
    pub revision: u64,
}
