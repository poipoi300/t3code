use crate::types::{EnvironmentId, ThreadId, ThreadKey};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::orchestration_v2::{
    AppThreadLineageRelationshipToParent, OrchestrationV2AppThreadLineage,
    OrchestrationV2DelegatedFrom, OrchestrationV2ThreadShell,
};
pub struct FilterThread<'a> {
    pub id: &'a ThreadId,
    pub lineage: &'a OrchestrationV2AppThreadLineage,
    pub tags: Option<&'a [String]>,
    pub delegated_from: Option<&'a OrchestrationV2DelegatedFrom>,
}
impl<'a> From<&'a OrchestrationV2ThreadShell> for FilterThread<'a> {
    fn from(thread: &'a OrchestrationV2ThreadShell) -> Self {
        Self {
            id: &thread.id,
            lineage: &thread.lineage,
            tags: thread
                .tags
                .as_ref()
                .and_then(Option::as_ref)
                .map(|tags| tags.0.as_slice()),
            delegated_from: thread.delegated_from.as_ref().and_then(Option::as_ref),
        }
    }
}
pub struct FilterEnvironment<'a> {
    pub environment: &'a EnvironmentId,
    pub threads: Vec<FilterThread<'a>>,
    pub hidden_tags: Option<&'a [String]>,
}
pub fn visibility_parent(
    environment: &EnvironmentId,
    thread: &FilterThread<'_>,
) -> Option<ThreadKey> {
    let listed = thread.lineage.relationship_to_parent
        != Some(AppThreadLineageRelationshipToParent::Subagent)
        && thread.lineage.side_chat != Some(Some(true));
    if !listed && let Some(parent) = &thread.lineage.parent_thread_id {
        return Some(ThreadKey {
            environment: environment.clone(),
            thread: parent.clone(),
        });
    }
    thread.delegated_from.map(|d| ThreadKey {
        environment: d.environment_id.clone(),
        thread: d.thread_id.clone(),
    })
}
pub fn hidden_keys(
    environments: &[FilterEnvironment<'_>],
    already_hidden: &BTreeSet<ThreadKey>,
    loaded_elsewhere: &BTreeSet<ThreadKey>,
    hiding_anywhere: bool,
) -> BTreeSet<ThreadKey> {
    if already_hidden.is_empty() && !hiding_anywhere {
        return BTreeSet::new();
    }
    let mut by_key = BTreeMap::new();
    for environment in environments {
        for thread in &environment.threads {
            by_key.insert(
                ThreadKey {
                    environment: environment.environment.clone(),
                    thread: thread.id.clone(),
                },
                (thread, environment),
            );
        }
    }
    let mut memo = BTreeMap::new();
    let mut hidden = BTreeSet::new();
    for key in by_key.keys() {
        let mut at = key.clone();
        let mut path = vec![];
        let result = loop {
            if already_hidden.contains(&at) {
                break true;
            }
            if let Some(hidden) = memo.get(&at) {
                break *hidden;
            }
            let Some((thread, environment)) = by_key.get(&at) else {
                break false;
            };
            if let Some(parent) = visibility_parent(environment.environment, thread) {
                if by_key.contains_key(&parent) || already_hidden.contains(&parent) {
                    memo.insert(at.clone(), false);
                    path.push(at);
                    at = parent;
                    continue;
                }
                if loaded_elsewhere.contains(&parent) {
                    memo.insert(at, false);
                    break false;
                }
                if hiding_anywhere {
                    memo.insert(at, true);
                    break true;
                }
            }
            let hidden = environment.hidden_tags.is_some_and(|tags| {
                tags.iter()
                    .any(|tag| thread.tags.is_some_and(|own| own.contains(tag)))
            });
            memo.insert(at, hidden);
            break hidden;
        };
        for key in path {
            memo.insert(key, result);
        }
        if result {
            hidden.insert(key.clone());
        }
    }
    hidden
}
pub fn visible_linked_threads<'a, T>(
    rows: &'a [T],
    environment: &EnvironmentId,
    id: impl Fn(&T) -> &ThreadId,
    loaded: &BTreeSet<ThreadKey>,
    hidden: &BTreeSet<ThreadKey>,
    hiding_anywhere: bool,
) -> Vec<&'a T> {
    rows.iter()
        .filter(|row| {
            let key = ThreadKey {
                environment: environment.clone(),
                thread: id(row).clone(),
            };
            if loaded.contains(&key) {
                !hidden.contains(&key)
            } else {
                !hiding_anywhere
            }
        })
        .collect()
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn key(environment: &str, id: &str) -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId(environment.into()),
            thread: ThreadId(id.into()),
        }
    }
    fn lineage(parent: Option<&str>, subagent: bool) -> OrchestrationV2AppThreadLineage {
        OrchestrationV2AppThreadLineage {
            parent_thread_id: parent.map(|p| ThreadId(p.into())),
            root_thread_id: ThreadId("root".into()),
            relationship_to_parent: parent.map(|_| {
                if subagent {
                    AppThreadLineageRelationshipToParent::Subagent
                } else {
                    AppThreadLineageRelationshipToParent::Fork
                }
            }),
            side_chat: None,
        }
    }
    #[test]
    fn hidden_parent_subagents_forks_and_cross_environment_tasks() {
        let local = EnvironmentId("local".into());
        let remote = EnvironmentId("remote".into());
        let ids = ["private", "child", "fork", "plain", "tagged-child"].map(|v| ThreadId(v.into()));
        let lineages = [
            lineage(None, false),
            lineage(Some("private"), true),
            lineage(Some("private"), false),
            lineage(None, false),
            lineage(Some("plain"), true),
        ];
        let private = vec!["private".into()];
        let linked = OrchestrationV2DelegatedFrom {
            environment_id: local.clone(),
            thread_id: ids[0].clone(),
            title: String::new(),
        };
        let delegated = ThreadId("delegated".into());
        let root = lineage(None, false);
        let environments = [
            FilterEnvironment {
                environment: &local,
                threads: ids
                    .iter()
                    .zip(&lineages)
                    .enumerate()
                    .map(|(i, (id, lineage))| FilterThread {
                        id,
                        lineage,
                        tags: matches!(i, 0 | 4).then_some(private.as_slice()),
                        delegated_from: None,
                    })
                    .collect(),
                hidden_tags: Some(&private),
            },
            FilterEnvironment {
                environment: &remote,
                threads: vec![FilterThread {
                    id: &delegated,
                    lineage: &root,
                    tags: None,
                    delegated_from: Some(&linked),
                }],
                hidden_tags: None,
            },
        ];
        assert_eq!(
            hidden_keys(&environments, &BTreeSet::new(), &BTreeSet::new(), true),
            BTreeSet::from([
                key("local", "private"),
                key("local", "child"),
                key("remote", "delegated")
            ])
        );
        assert!(hidden_keys(&environments, &BTreeSet::new(), &BTreeSet::new(), false).is_empty());
    }
    #[test]
    fn orphan_live_parent_and_cycles() {
        let env = EnvironmentId("e".into());
        let id = ThreadId("child".into());
        let parent = lineage(Some("missing"), true);
        let environments = [FilterEnvironment {
            environment: &env,
            threads: vec![FilterThread {
                id: &id,
                lineage: &parent,
                tags: None,
                delegated_from: None,
            }],
            hidden_tags: None,
        }];
        let missing = key("e", "missing");
        assert!(
            hidden_keys(&environments, &BTreeSet::new(), &BTreeSet::new(), true)
                .contains(&key("e", "child"))
        );
        assert!(
            hidden_keys(
                &environments,
                &BTreeSet::new(),
                &BTreeSet::from([missing.clone()]),
                true
            )
            .is_empty()
        );
        assert!(
            hidden_keys(
                &environments,
                &BTreeSet::from([missing.clone()]),
                &BTreeSet::from([missing]),
                true
            )
            .contains(&key("e", "child"))
        );
        let cycle = lineage(Some("child"), true);
        let environments = [FilterEnvironment {
            environment: &env,
            threads: vec![FilterThread {
                id: &id,
                lineage: &cycle,
                tags: None,
                delegated_from: None,
            }],
            hidden_tags: None,
        }];
        assert!(hidden_keys(&environments, &BTreeSet::new(), &BTreeSet::new(), true).is_empty());
    }
    #[test]
    fn pr_rows_cannot_borrow_loaded_identity_from_another_environment() {
        let rows = [ThreadId("same".into())];
        let loaded = BTreeSet::from([key("other", "same")]);
        assert!(
            visible_linked_threads(
                &rows,
                &EnvironmentId("e".into()),
                |id| id,
                &loaded,
                &BTreeSet::new(),
                true
            )
            .is_empty()
        );
        assert_eq!(
            visible_linked_threads(
                &rows,
                &EnvironmentId("other".into()),
                |id| id,
                &loaded,
                &BTreeSet::new(),
                true
            )
            .len(),
            1
        );
    }
}
