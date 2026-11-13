pub use crate::thread_views::visits::{THREAD_OPEN_DEDUPE_MS, ThreadOpens};
use crate::{
    types::{EnvironmentId, ProjectId, QueryState, ThreadKey},
    view_logic::shell::search::fold_query,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{orchestration_v2::*, thread_search::OrchestrationThreadSearchMatch};
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSearchScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<ProjectId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_labels: Option<BTreeMap<EnvironmentId, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub titles_only: Option<bool>,
}
pub fn make_thread_search_key(
    environments: &[EnvironmentId],
    query: &str,
    scope: &ThreadSearchScope,
) -> Result<String, serde_json::Error> {
    let mut ids = environments.to_vec();
    ids.sort();
    if *scope == ThreadSearchScope::default() {
        serde_json::to_string(&(ids, query))
    } else {
        serde_json::to_string(&(ids, query, scope))
    }
}
pub fn parse_thread_search_key(
    key: &str,
) -> Option<(Vec<EnvironmentId>, String, ThreadSearchScope)> {
    let value: serde_json::Value = serde_json::from_str(key).ok()?;
    let array = value.as_array()?;
    if !(2..=3).contains(&array.len()) {
        return None;
    }
    let ids = serde_json::from_value(array[0].clone()).ok()?;
    let query: String = serde_json::from_value(array[1].clone()).ok()?;
    let query = t3_contracts::wire::trim_js(&query).to_owned();
    if !(2..=200).contains(&query.encode_utf16().count()) {
        return None;
    }
    let scope = if let Some(scope) = array.get(2) {
        serde_json::from_value(scope.clone()).ok()?
    } else {
        ThreadSearchScope::default()
    };
    Some((ids, query, scope))
}
pub fn thread_search_match_key(key: &ThreadKey) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(&key.environment, &key.thread))
}
pub fn thread_search_input(
    query: &str,
    scope: &ThreadSearchScope,
    environment: &EnvironmentId,
    before: Option<t3_contracts::base_schemas::IsoDateTime>,
) -> Option<OrchestrationV2SearchThreadsInput> {
    let search_messages = scope.titles_only != Some(true) && message_search_has_long_token(query);
    if !search_messages && before.is_none() {
        return None;
    }
    let older = before.is_some();
    Some(OrchestrationV2SearchThreadsInput {
        query: t3_contracts::base_schemas::TrimmedString(t3_contracts::wire::trim_js(query).into()),
        limit: None,
        older_settled_before: before,
        project_ids: older.then(|| scope.project_ids.clone()).flatten(),
        environment_label: older
            .then(|| scope.environment_labels.as_ref()?.get(environment).cloned())
            .flatten(),
        titles_only: (older && !search_messages).then_some(true),
    })
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentThreadSearchMatch {
    pub environment: EnvironmentId,
    pub entry: OrchestrationThreadSearchMatch,
}
#[derive(Default)]
pub struct ThreadSearchResults {
    pub matches: Vec<EnvironmentThreadSearchMatch>,
    pub is_loading: bool,
    pub rows: Vec<(EnvironmentId, OrchestrationV2ThreadFamily)>,
}
pub fn merge_thread_search_results<'a>(
    queries: impl IntoIterator<
        Item = (
            &'a EnvironmentId,
            &'a QueryState<Rc<OrchestrationV2SearchThreadsResult>>,
        ),
    >,
) -> ThreadSearchResults {
    let mut result = ThreadSearchResults::default();
    for (id, query) in queries {
        result.is_loading |= query.loading;
        if let Some(value) = &query.value {
            result.matches.extend(
                value
                    .matches
                    .iter()
                    .map(|entry| EnvironmentThreadSearchMatch {
                        environment: id.clone(),
                        entry: entry.clone(),
                    }),
            );
            if let Some(threads) = &value.threads
                && !threads.is_empty()
            {
                result.rows.push((
                    id.clone(),
                    OrchestrationV2ThreadFamily {
                        snapshot_sequence: value.snapshot_sequence.unwrap_or(0),
                        threads: threads.clone(),
                    },
                ));
            }
        }
    }
    result
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownOrder {
    pub query: String,
    pub keys: Vec<String>,
    pub shown: usize,
}
pub fn next_shown_order(
    previous: Option<Rc<ShownOrder>>,
    query: &str,
    ranked: &[String],
    limit: usize,
) -> Rc<ShownOrder> {
    let Some(previous) = previous.filter(|p| p.query == query) else {
        return Rc::new(ShownOrder {
            query: query.into(),
            keys: ranked.to_vec(),
            shown: limit.min(ranked.len()),
        });
    };
    let listed: BTreeSet<_> = ranked.iter().collect();
    let kept: Vec<_> = previous
        .keys
        .iter()
        .take(previous.shown)
        .filter(|key| listed.contains(key))
        .cloned()
        .collect();
    let held: BTreeSet<_> = kept.iter().collect();
    let seen: BTreeSet<_> = previous.keys.iter().collect();
    let rest: Vec<_> = ranked
        .iter()
        .filter(|key| !held.contains(key))
        .cloned()
        .collect();
    let arrived = rest.iter().filter(|key| !seen.contains(key)).count();
    let shown = (kept.len() + rest.len())
        .min(limit.saturating_add(3))
        .min(limit.max(kept.len() + arrived.min(3)));
    let mut keys = kept;
    keys.extend(rest);
    if previous.shown == shown && previous.keys == keys {
        previous
    } else {
        Rc::new(ShownOrder {
            query: query.into(),
            keys,
            shown,
        })
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct SettledThreadSearch {
    pub query: String,
    pub matches: Vec<EnvironmentThreadSearchMatch>,
}
pub fn visible_thread_search(
    query: &str,
    ready: Option<Rc<SettledThreadSearch>>,
    previous: Option<Rc<SettledThreadSearch>>,
) -> Rc<SettledThreadSearch> {
    if let Some(ready) = ready {
        return ready;
    }
    let current = fold_query(query);
    let before = previous
        .as_ref()
        .map_or_else(String::new, |p| fold_query(&p.query));
    if let Some(previous) = previous
        && !current.is_empty()
        && !before.is_empty()
        && (current.starts_with(&before) || before.starts_with(&current))
    {
        previous
    } else {
        Rc::new(SettledThreadSearch {
            query: String::new(),
            matches: vec![],
        })
    }
}

fn message_search_has_long_token(query: &str) -> bool {
    query.trim_matches(|c| matches!(c, '\"' | '\'' | '`' | '“' | '”' | '‘' | '’')).split(crate::view_logic::is_js_whitespace).any(|t| {let count=t.chars().count();count>=3 || (count==2 && t.chars().any(|c| matches!(u32::from(c),0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)))})
}

pub fn thread_search_subject(
    key: &ThreadKey,
    thread: &OrchestrationV2ThreadShell,
    project_title: Option<&str>,
    environment_label: Option<&str>,
    opens: &ThreadOpens,
    parse: &impl Fn(&str) -> Option<i64>,
) -> crate::view_logic::shell::search::ThreadSubject {
    use super::sort::{SortOrder, ThreadSortInput, get_thread_sort_timestamp};
    use crate::view_logic::shell::search::{SearchPullRequest, ThreadSubject};
    let entry = |number, repository: &str, url: &str, title| SearchPullRequest {
        number,
        title,
        references: vec![format!("{repository}#{number}"), url.into()],
    };
    let mut pull_requests = vec![];
    if let Some(link) = thread.branch_pull_request.as_ref().and_then(Option::as_ref) {
        pull_requests.push(entry(link.number, &link.repository.0, &link.url.0, None));
    }
    if let Some(links) = thread
        .pull_requests
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|links| !links.is_empty())
    {
        let mut links: Vec<_>=links.iter().filter(|link| link.source!=t3_contracts::thread_pull_request::ThreadPullRequestLinkSource::StackDismissed).collect();
        let updated = |link: &t3_contracts::thread_pull_request::ThreadPullRequestLink| {
            link.snapshot
                .as_ref()
                .and_then(|s| s.updated_at.as_ref())
                .map_or_else(|| parse(&link.linked_at.0), |at| parse(&at.0))
                .unwrap_or(0)
        };
        links.sort_by_key(|link| std::cmp::Reverse(updated(link)));
        pull_requests.extend(links.iter().enumerate().map(|(index, link)| {
            entry(
                link.number,
                &link.repository.0,
                &link.url.0,
                (index < 3)
                    .then(|| link.snapshot.as_ref().map(|s| s.title.0.clone()))
                    .flatten(),
            )
        }));
    } else if let Some(link) = thread.linked_pull_request.as_ref().and_then(Option::as_ref) {
        pull_requests.push(entry(link.number, &link.repository.0, &link.url.0, None));
    }
    let activity_at = get_thread_sort_timestamp(
        &ThreadSortInput {
            created_at: &thread.created_at.0,
            updated_at: &thread.updated_at.0,
            latest_user_message_at: thread
                .latest_user_message_at
                .as_ref()
                .map(|at| at.0.as_str()),
            latest_user_authored_message_at: thread
                .latest_user_authored_message_at
                .as_ref()
                .map(|at| at.as_ref().map(|at| at.0.as_str())),
            messages: &[],
        },
        SortOrder::LatestUserMessage,
        parse,
    )
    .map(|v| v as f64);
    let mut reported = thread.opens.clone().flatten();
    if let Some(&local) = opens.local.get(key)
        && local as f64
            > reported
                .as_ref()
                .and_then(|o| o.opened_at.as_f64())
                .unwrap_or(0.0)
    {
        match &mut reported {
            Some(reported) => reported.opened_at = local.into(),
            None => {
                reported = Some(OrchestrationV2ThreadOpens {
                    opened_at: local.into(),
                    score: 1u64.into(),
                    picks: vec![],
                })
            }
        }
    }
    ThreadSubject {
        id: Some(key.thread.0.clone()),
        title: thread.title.clone(),
        branch: thread.branch.as_ref().map(|b| b.0.clone()),
        project_title: project_title.map(str::to_owned),
        environment_label: environment_label.map(str::to_owned),
        pull_requests,
        activity_at,
        opens: reported,
    }
}

struct SearchSubjectEntry {
    source: std::rc::Weak<OrchestrationV2ThreadShell>,
    project_title: Option<String>,
    environment_label: Option<String>,
    opens_revision: u64,
    subject: Rc<crate::view_logic::shell::search::ThreadSubject>,
}
#[derive(Default)]
pub struct ThreadSearchSubjects {
    entries: BTreeMap<ThreadKey, SearchSubjectEntry>,
}
impl ThreadSearchSubjects {
    pub fn subject(
        &mut self,
        key: &ThreadKey,
        thread: &Rc<OrchestrationV2ThreadShell>,
        labels: (Option<&str>, Option<&str>),
        opens: &ThreadOpens,
        parse: &impl Fn(&str) -> Option<i64>,
    ) -> Rc<crate::view_logic::shell::search::ThreadSubject> {
        if let Some(entry) = self.entries.get(key)
            && entry
                .source
                .upgrade()
                .is_some_and(|old| Rc::ptr_eq(&old, thread))
            && entry.project_title.as_deref() == labels.0
            && entry.environment_label.as_deref() == labels.1
            && entry.opens_revision == opens.revision
        {
            return entry.subject.clone();
        }
        let subject = Rc::new(thread_search_subject(
            key, thread, labels.0, labels.1, opens, parse,
        ));
        self.entries.insert(
            key.clone(),
            SearchSubjectEntry {
                source: Rc::downgrade(thread),
                project_title: labels.0.map(str::to_owned),
                environment_label: labels.1.map(str::to_owned),
                opens_revision: opens.revision,
                subject: subject.clone(),
            },
        );
        subject
    }
    pub fn collect(&mut self) {
        self.entries
            .retain(|_, entry| entry.source.strong_count() > 0);
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.entries
            .retain(|key, _| &key.environment != environment);
    }
}
