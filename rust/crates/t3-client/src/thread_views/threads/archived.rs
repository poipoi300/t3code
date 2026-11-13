use crate::{
    thread_views::tag_filter::{FilterEnvironment, FilterThread, hidden_keys},
    types::{EnvironmentId, QueryState, ThreadKey},
};
use std::{collections::BTreeSet, rc::Rc};
use t3_contracts::orchestration_v2::OrchestrationV2ArchivedShellSnapshot;
pub fn make_archived_threads_environment_key(ids: &[EnvironmentId]) -> String {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}
pub fn parse_archived_threads_environment_key(key: &str) -> Vec<EnvironmentId> {
    if key.is_empty() {
        vec![]
    } else {
        key.split('\u{1f}')
            .map(|id| EnvironmentId(id.into()))
            .collect()
    }
}
pub struct ArchivedEnvironment<'a> {
    pub id: &'a EnvironmentId,
    pub query: &'a QueryState<Rc<OrchestrationV2ArchivedShellSnapshot>>,
    pub hidden_tags: Option<&'a [String]>,
}
pub struct ArchivedThreadSnapshots {
    pub snapshots: Vec<(EnvironmentId, Rc<OrchestrationV2ArchivedShellSnapshot>)>,
    pub error: Option<&'static str>,
    pub is_loading: bool,
}
pub fn archived_thread_snapshots(
    environments: &[ArchivedEnvironment<'_>],
    already_hidden: &BTreeSet<ThreadKey>,
    live: &BTreeSet<ThreadKey>,
    hiding_anywhere: bool,
) -> ArchivedThreadSnapshots {
    let filters: Vec<_> = environments
        .iter()
        .map(|e| FilterEnvironment {
            environment: e.id,
            threads: e.query.value.as_ref().map_or_else(Vec::new, |s| {
                s.threads.iter().map(FilterThread::from).collect()
            }),
            hidden_tags: e.hidden_tags,
        })
        .collect();
    let hidden = hidden_keys(&filters, already_hidden, live, hiding_anywhere);
    ArchivedThreadSnapshots {
        is_loading: environments.iter().any(|e| e.query.loading),
        error: environments
            .iter()
            .any(|e| e.query.error.is_some())
            .then_some("Failed to load archived threads."),
        snapshots: environments
            .iter()
            .filter_map(|e| {
                let snapshot = e.query.value.as_ref()?;
                if !snapshot.threads.iter().any(|t| {
                    hidden.contains(&ThreadKey {
                        environment: e.id.clone(),
                        thread: t.id.clone(),
                    })
                }) {
                    return Some((e.id.clone(), snapshot.clone()));
                }
                let mut filtered = (**snapshot).clone();
                filtered.threads.retain(|t| {
                    !hidden.contains(&ThreadKey {
                        environment: e.id.clone(),
                        thread: t.id.clone(),
                    })
                });
                Some((e.id.clone(), Rc::new(filtered)))
            })
            .collect(),
    }
}
