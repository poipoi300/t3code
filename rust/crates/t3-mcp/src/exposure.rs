use crate::{
    InvocationScope,
    catalog::{advertised, catalog},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Mutex,
};

#[derive(Default)]
pub struct ToolExposure {
    sessions: Mutex<VecDeque<(String, BTreeSet<String>)>>,
}

fn direct(name: &str) -> bool {
    matches!(
        name,
        "orchestrator_capabilities" | "delegate_task" | "task_status" | "tool_search"
    )
}

pub fn enabled(scope: &InvocationScope, name: &str) -> bool {
    !(scope.thread.as_ref().is_some_and(|t| t.subagent)
        && matches!(name, "watch_pull_request" | "unwatch_pull_request")
        || scope.linked_environments == Some(false)
            && matches!(
                name,
                "t3_environment_links" | "t3_environment_link" | "t3_environment_unlink"
            ))
}

impl ToolExposure {
    pub fn list(&self, scope: &InvocationScope, deferred: bool) -> Value {
        let sessions = self.sessions.lock().expect("tool exposure lock poisoned");
        let session = scope
            .thread
            .as_ref()
            .map(|thread| thread.provider_session_id.as_str());
        let tools = catalog()
            .tools
            .iter()
            .filter(|tool| enabled(scope, &tool.name))
            .filter(|tool| {
                if deferred && let Some(session) = session {
                    direct(&tool.name)
                        || sessions
                            .iter()
                            .find(|(id, _)| id == session)
                            .is_some_and(|(_, listed)| listed.contains(&tool.name))
                } else {
                    tool.name != "tool_search"
                }
            })
            .map(|tool| advertised(&tool.name).expect("catalog contains advertised tool"))
            .collect::<Vec<_>>();
        json!({"tools": tools})
    }
    pub fn search(&self, session: &str, query: &str) -> Value {
        let query = query.to_ascii_lowercase();
        let terms = query
            .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let mut matches = catalog()
            .tools
            .iter()
            .enumerate()
            .filter(|(_, tool)| tool.name != "tool_search")
            .filter_map(|(index, tool)| {
                let name = tool.name.to_ascii_lowercase();
                let description = tool.description.to_ascii_lowercase();
                let score: usize = terms
                    .iter()
                    .map(|term| {
                        if name == *term {
                            100
                        } else if name.contains(term) {
                            3
                        } else if description.contains(term) {
                            1
                        } else {
                            0
                        }
                    })
                    .sum();
                (score > 0).then_some((index, score, tool))
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(index, score, _)| (std::cmp::Reverse(*score), *index));
        matches.truncate(10);
        let names = matches
            .iter()
            .map(|(_, _, tool)| tool.name.clone())
            .collect::<Vec<_>>();
        if !names.is_empty() {
            let mut sessions = self.sessions.lock().expect("tool exposure lock poisoned");
            let mut entry = sessions
                .iter()
                .position(|(id, _)| id == session)
                .and_then(|index| sessions.remove(index))
                .unwrap_or_else(|| (session.into(), BTreeSet::new()));
            entry.1.extend(names);
            sessions.push_back(entry);
            while sessions.len() > 256 {
                sessions.pop_front();
            }
        }
        json!({"tools": matches.iter().map(|(_, _, tool)| json!({"name": tool.name, "description": tool.description})).collect::<Vec<_>>()})
    }
}
