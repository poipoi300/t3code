use crate::{
    state::shell::ShellState,
    types::{EnvironmentId, ProjectId, QueryState, ThreadId, ThreadKey},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{orchestration_v2::*, wire::WireDateTime};
pub const OLDER_SETTLED_PAGE_SIZE: usize = 25;
pub const COUNT_REREAD_DELAY_MS: u64 = 500;
#[derive(Debug, Clone, PartialEq)]
pub struct OlderSettled {
    pub before: WireDateTime,
    pub before_ms: i64,
    pub counts: BTreeMap<ProjectId, u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OlderSettledScope {
    pub environment: EnvironmentId,
    pub project_ids: Option<Vec<ProjectId>>,
}
pub fn sort_older_settled_scopes(scopes: &[OlderSettledScope]) -> Vec<OlderSettledScope> {
    let mut scopes = scopes.to_vec();
    for scope in &mut scopes {
        if let Some(ids) = &mut scope.project_ids {
            ids.sort();
        }
    }
    scopes.sort_by(|a, b| a.environment.cmp(&b.environment));
    scopes
}
fn listed(thread: &OrchestrationV2ThreadShell) -> bool {
    thread.lineage.relationship_to_parent != Some(AppThreadLineageRelationshipToParent::Subagent)
        && thread.lineage.side_chat != Some(Some(true))
}
pub fn is_older_settled_thread(
    thread: &OrchestrationV2ThreadShell,
    before: i64,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> bool {
    listed(thread)
        && thread.settled_override == Some(AppThreadSettledOverride::Settled)
        && thread
            .settled_at
            .as_ref()
            .and_then(parse)
            .is_some_and(|at| at < before)
}
pub fn older_settled_count_key(
    threads: &[OrchestrationV2ThreadShell],
    before: i64,
    removed: &BTreeMap<ThreadId, u64>,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> String {
    let mut rows = vec![];
    for t in threads {
        if !listed(t) {
            continue;
        }
        let older = is_older_settled_thread(t, before, parse);
        if older || parse(&t.created_at).is_some_and(|at| at < before) {
            rows.push(format!("{}:{}", t.id.0, u8::from(older)));
        }
    }
    rows.sort();
    format!(
        "{}|{}",
        removed.values().max().unwrap_or(&0),
        rows.join(",")
    )
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PageStatus {
    #[default]
    Ready,
    Pending,
    Failed,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OlderSettledPages {
    pub reads: Vec<Rc<OrchestrationV2ThreadFamily>>,
    pub frontier: Option<OrchestrationV2OlderSettledCursor>,
    pub status: PageStatus,
}
#[derive(Debug, Clone, PartialEq)]
pub struct OlderSettledShelfEnvironment {
    pub older: Option<OlderSettled>,
    pub pages: OlderSettledPages,
}
fn higher(
    a: &OrchestrationV2OlderSettledCursor,
    b: &OrchestrationV2OlderSettledCursor,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> bool {
    let (a_time, b_time) = (parse(&a.settled_at), parse(&b.settled_at));
    a_time > b_time
        || (a_time == b_time
            && a.thread_id
                .as_ref()
                .is_some_and(|id| b.thread_id.as_ref().is_none_or(|other| id < other)))
}
pub fn settled_shelf_frontier<'a>(
    environments: impl IntoIterator<Item = &'a OlderSettledShelfEnvironment>,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> Option<&'a OrchestrationV2OlderSettledCursor> {
    environments
        .into_iter()
        .filter(|e| e.older.is_some() && e.pages.status != PageStatus::Failed)
        .filter_map(|e| e.pages.frontier.as_ref())
        .reduce(|highest, next| {
            if higher(next, highest, parse) {
                next
            } else {
                highest
            }
        })
}
pub fn is_listed_settled_thread(
    thread: &OrchestrationV2ThreadShell,
    frontier: Option<&OrchestrationV2OlderSettledCursor>,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> bool {
    frontier.is_none_or(|cursor| {
        thread.settled_at.as_ref().is_none_or(|at| {
            parse(at) > parse(&cursor.settled_at)
                || (parse(at) == parse(&cursor.settled_at)
                    && cursor.thread_id.as_ref().is_none_or(|id| &thread.id <= id))
        })
    })
}
pub fn unloaded_older_settled_count(
    scopes: &[OlderSettledScope],
    older: &BTreeMap<EnvironmentId, OlderSettled>,
    threads: &[(&ThreadKey, &OrchestrationV2ThreadShell)],
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> u64 {
    let mut unloaded = 0i128;
    let mut seen = BTreeSet::new();
    for scope in scopes {
        let Some(older) = older.get(&scope.environment) else {
            continue;
        };
        let projects: BTreeSet<_> = scope
            .project_ids
            .clone()
            .unwrap_or_else(|| older.counts.keys().cloned().collect())
            .into_iter()
            .collect();
        for project in projects {
            if !seen.insert((scope.environment.clone(), project.clone())) {
                continue;
            }
            unloaded += i128::from(older.counts.get(&project).copied().unwrap_or(0));
            let mut loaded = BTreeSet::new();
            for (key, thread) in threads {
                if key.environment == scope.environment
                    && thread.project_id == project
                    && is_older_settled_thread(thread, older.before_ms, parse)
                    && loaded.insert(&key.thread)
                {
                    unloaded -= 1;
                }
            }
        }
    }
    unloaded.max(0) as u64
}
pub fn wanted_older_settled_pages(
    requested: &BTreeMap<EnvironmentId, usize>,
    environments: &BTreeMap<EnvironmentId, OlderSettledShelfEnvironment>,
    listed_count: usize,
    visible_count: usize,
    parse: &impl Fn(&WireDateTime) -> Option<i64>,
) -> BTreeMap<EnvironmentId, usize> {
    let mut next = requested.clone();
    if listed_count >= visible_count {
        return next;
    }
    let Some(frontier) = settled_shelf_frontier(environments.values(), parse) else {
        return next;
    };
    for (id, e) in environments {
        if e.older.is_some()
            && e.pages.status == PageStatus::Ready
            && e.pages.frontier.as_ref().is_some_and(|cursor| {
                parse(&cursor.settled_at) == parse(&frontier.settled_at)
                    && cursor.thread_id == frontier.thread_id
            })
        {
            *next.entry(id.clone()).or_default() += 1;
        }
    }
    next
}
pub fn read_older_settled_pages(
    state: &ShellState,
    count: usize,
    mut page: impl FnMut(
        &OrchestrationV2OlderSettledCursor,
    ) -> QueryState<Rc<OrchestrationV2OlderSettledThreadsPage>>,
) -> OlderSettledPages {
    let Some(cut) = state
        .snapshot
        .as_ref()
        .and_then(|s| s.older_settled.as_ref())
        .filter(|_| state.generation > 0)
    else {
        return OlderSettledPages::default();
    };
    let mut result = OlderSettledPages {
        frontier: Some(OrchestrationV2OlderSettledCursor {
            settled_at: cut.before.clone(),
            thread_id: None,
        }),
        ..Default::default()
    };
    for _ in 0..count {
        let Some(frontier) = &result.frontier else {
            break;
        };
        let query = page(frontier);
        let Some(value) = query.value else {
            result.status = if query.loading || query.error.is_none() {
                PageStatus::Pending
            } else {
                PageStatus::Failed
            };
            break;
        };
        result.reads.push(Rc::new(OrchestrationV2ThreadFamily {
            snapshot_sequence: value.snapshot_sequence,
            threads: value.threads.clone(),
        }));
        result.frontier = if value.threads.len() < OLDER_SETTLED_PAGE_SIZE {
            None
        } else {
            value.threads.last().and_then(|t| {
                t.settled_at
                    .as_ref()
                    .map(|at| OrchestrationV2OlderSettledCursor {
                        settled_at: at.clone(),
                        thread_id: Some(t.id.clone()),
                    })
            })
        };
    }
    result
}

#[derive(Default)]
pub struct OlderSettledCounts {
    generation: Option<u64>,
    snapshot_key: Option<String>,
    from_snapshot: Option<OlderSettled>,
    last_read: Option<OlderSettled>,
}
impl OlderSettledCounts {
    pub fn observe(
        &mut self,
        state: &ShellState,
        parse: &impl Fn(&WireDateTime) -> Option<i64>,
        mut read: impl FnMut(u64, &WireDateTime, &str) -> Option<OrchestrationV2OlderSettledThreadsPage>,
    ) -> Option<OlderSettled> {
        let snapshot = state.snapshot.as_ref()?;
        let cut = snapshot.older_settled.as_ref()?;
        let before_ms = parse(&cut.before)?;
        if self.generation != Some(state.generation)
            || self
                .from_snapshot
                .as_ref()
                .is_none_or(|s| s.before != cut.before)
        {
            self.from_snapshot = Some(OlderSettled {
                before: cut.before.clone(),
                before_ms,
                counts: cut
                    .counts
                    .iter()
                    .map(|c| (c.project_id.clone(), c.count))
                    .collect(),
            });
        }
        let key = older_settled_count_key(&snapshot.threads, before_ms, &state.removed_at, parse);
        if self.generation != Some(state.generation) {
            self.generation = Some(state.generation);
            self.snapshot_key =
                (snapshot.snapshot_sequence == cut.as_of_sequence).then(|| key.clone());
            self.last_read = None;
        }
        if state.generation == 0 || self.snapshot_key.as_deref() == Some(&key) {
            return self.from_snapshot.clone();
        }
        if let Some(page) = read(state.generation, &cut.before, &key) {
            self.last_read = Some(OlderSettled {
                before: cut.before.clone(),
                before_ms,
                counts: page
                    .counts
                    .into_iter()
                    .map(|c| (c.project_id, c.count))
                    .collect(),
            });
        }
        self.last_read
            .clone()
            .or_else(|| self.from_snapshot.clone())
    }
}
