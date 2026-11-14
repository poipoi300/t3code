use crate::types::{EnvironmentId, ThreadKey};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComposerTarget {
    Thread(ThreadKey),
    Draft {
        draft_id: String,
        environment_id: Option<EnvironmentId>,
    },
}
impl ComposerTarget {
    pub fn environment(&self) -> Option<&EnvironmentId> {
        match self {
            Self::Thread(key) => Some(&key.environment),
            Self::Draft { environment_id, .. } => environment_id.as_ref(),
        }
    }
    pub fn draft_key(&self, document: &serde_json::Value) -> Option<String> {
        match self {
            Self::Thread(key) => super::web_sessions::resolve(
                document,
                super::web_sessions::Target::Thread(&t3_contracts::environment::ScopedThreadRef {
                    environment_id: key.environment.clone(),
                    thread_id: key.thread.clone(),
                }),
            ),
            Self::Draft { draft_id, .. } => {
                super::web_sessions::resolve(document, super::web_sessions::Target::Draft(draft_id))
            }
        }
    }
}
