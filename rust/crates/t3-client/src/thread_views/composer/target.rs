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
    pub fn thread_key(&self, document: &serde_json::Value) -> Option<ThreadKey> {
        match self {
            Self::Thread(key) => Some(key.clone()),
            Self::Draft {
                draft_id,
                environment_id,
            } => {
                let session = &document["draftThreadsByThreadKey"][draft_id];
                let environment = environment_id.clone().or_else(|| {
                    session["environmentId"]
                        .as_str()
                        .map(|id| EnvironmentId(id.into()))
                })?;
                let thread = crate::types::ThreadId(session["threadId"].as_str()?.into());
                Some(ThreadKey {
                    environment,
                    thread,
                })
            }
        }
    }
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
