use crate::types::{EnvironmentId, ProjectId, ThreadId, ThreadKey};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{environment::ScopedProjectRef, orchestration_v2::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PendingChildRequest {
    UserInput,
    UserOnlyInput,
    Approval,
}
fn child_run_live(t: &OrchestrationV2ThreadShell) -> bool {
    match &t.activity_run_status {
        Some(status) => status.is_some(),
        None => matches!(
            t.status,
            OrchestrationV2ShellThreadStatus::Preparing
                | OrchestrationV2ShellThreadStatus::Starting
                | OrchestrationV2ShellThreadStatus::Running
                | OrchestrationV2ShellThreadStatus::Waiting
        ),
    }
}
fn native_child(t: &OrchestrationV2ThreadShell) -> bool {
    t.creation_source == OrchestrationV2CreationSource::Provider
        && t.lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
}
fn subagent_request(t: &OrchestrationV2ThreadShell) -> Option<PendingChildRequest> {
    if (t.lineage.relationship_to_parent != Some(AppThreadLineageRelationshipToParent::Subagent)
        && t.delegated_from.is_none())
        || native_child(t)
        || !child_run_live(t)
    {
        return None;
    }
    request_kind(t)
}
fn request_kind(t: &OrchestrationV2ThreadShell) -> Option<PendingChildRequest> {
    let request = t.pending_runtime_request.as_ref()?;
    match &request.kind {
        RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::AuthRefresh => {
            None
        }
        RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput => {
            Some(PendingChildRequest::UserInput)
        }
        _ => Some(PendingChildRequest::Approval),
    }
}
fn rolled_up(t: &OrchestrationV2ThreadShell) -> Option<PendingChildRequest> {
    subagent_request(t).map(|request| {
        if request == PendingChildRequest::UserInput
            && t.pending_runtime_request
                .as_ref()
                .is_some_and(|r| r.user_only == Some(Some(true)))
        {
            PendingChildRequest::UserOnlyInput
        } else {
            request
        }
    })
}
fn side_chat(t: &OrchestrationV2ThreadShell) -> bool {
    t.lineage.side_chat == Some(Some(true))
}
fn local_flags(
    threads: &[Rc<OrchestrationV2ThreadShell>],
    delegated: &[(ThreadId, PendingChildRequest)],
) -> BTreeMap<ThreadId, PendingChildRequest> {
    let by_id: BTreeMap<_, _> = threads.iter().map(|t| (&t.id, t.as_ref())).collect();
    let mut requests = BTreeMap::new();
    let mut flag = |parent: Option<&ThreadId>, request, mut seen: BTreeSet<ThreadId>| {
        if request == PendingChildRequest::UserInput {
            return;
        }
        let mut parent = parent;
        while let Some(id) = parent {
            if !seen.insert(id.clone()) {
                break;
            }
            if requests.get(id) != Some(&PendingChildRequest::Approval) {
                requests.insert(id.clone(), request);
            }
            parent = by_id
                .get(id)
                .filter(|p| {
                    p.lineage.relationship_to_parent
                        == Some(AppThreadLineageRelationshipToParent::Subagent)
                        || side_chat(p)
                })
                .and_then(|p| p.lineage.parent_thread_id.as_ref());
        }
    };
    // Side chats ask their user directly; process these after the chain walk below.
    for child in threads {
        if side_chat(child) {
            continue;
        }
        if child.lineage.relationship_to_parent
            == Some(AppThreadLineageRelationshipToParent::Subagent)
            && let Some(request) = rolled_up(child)
        {
            flag(
                child.lineage.parent_thread_id.as_ref(),
                request,
                BTreeSet::from([child.id.clone()]),
            );
        }
    }
    for (parent, request) in delegated {
        flag(Some(parent), *request, BTreeSet::new());
    }
    for child in threads.iter().filter(|t| side_chat(t)) {
        if let (Some(parent), Some(request)) =
            (child.lineage.parent_thread_id.as_ref(), request_kind(child))
            && requests.get(parent) != Some(&PendingChildRequest::Approval)
        {
            requests.insert(parent.clone(), request);
        }
    }
    requests
}

pub struct EnvironmentShells<'a> {
    pub environment: &'a EnvironmentId,
    pub enabled: bool,
    pub threads: &'a [Rc<OrchestrationV2ThreadShell>],
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadShellView {
    pub key: ThreadKey,
    pub source: Rc<OrchestrationV2ThreadShell>,
    pub pending_child_request: Option<PendingChildRequest>,
}
impl ThreadShellView {
    pub fn has_pending_approvals(&self) -> bool {
        matches!(
            request_kind(&self.source),
            Some(PendingChildRequest::Approval)
        )
    }
    pub fn has_pending_user_input(&self) -> bool {
        matches!(
            request_kind(&self.source),
            Some(PendingChildRequest::UserInput)
        )
    }
}
#[derive(Default)]
pub struct ThreadShellViews {
    pub ordered: Vec<ThreadKey>,
    pub rows: BTreeMap<ThreadKey, Rc<ThreadShellView>>,
    pub quiet_rows: BTreeMap<ThreadKey, Rc<ThreadShellView>>,
    pub pending_child_requests: BTreeMap<ThreadKey, PendingChildRequest>,
    pub delegated_under_parent: BTreeSet<ThreadKey>,
    pub hidden: BTreeSet<ThreadKey>,
}
fn streaming_equal(a: &ThreadShellView, b: &ThreadShellView) -> bool {
    if a.key != b.key || a.pending_child_request != b.pending_child_request {
        return false;
    }
    let active = b
        .source
        .activity_run_status
        .as_ref()
        .is_some_and(Option::is_some)
        || matches!(
            b.source.status,
            OrchestrationV2ShellThreadStatus::Preparing
                | OrchestrationV2ShellThreadStatus::Queued
                | OrchestrationV2ShellThreadStatus::Starting
                | OrchestrationV2ShellThreadStatus::Running
                | OrchestrationV2ShellThreadStatus::Waiting
        );
    if !active
        || (a.source.item_count > 0) != (b.source.item_count > 0)
        || (a.source.visible_item_count > 0) != (b.source.visible_item_count > 0)
    {
        return false;
    }
    let mut before = (*a.source).clone();
    before.updated_at = b.source.updated_at.clone();
    before.item_count = b.source.item_count;
    before.visible_item_count = b.source.visible_item_count;
    before.latest_visible_message = b.source.latest_visible_message.clone();
    before == *b.source
}
impl ThreadShellViews {
    pub fn update(&mut self, environments: &[EnvironmentShells<'_>], hidden: BTreeSet<ThreadKey>) {
        let by_environment: BTreeMap<_, _> = environments
            .iter()
            .map(|e| (e.environment, e.threads))
            .collect();
        let mut flags: BTreeMap<_, _> = environments
            .iter()
            .map(|e| (e.environment.clone(), local_flags(e.threads, &[])))
            .collect();
        loop {
            let mut next = flags.clone();
            for environment in environments {
                let mut delegated = vec![];
                for remote in environments.iter().filter(|e| e.enabled) {
                    for task in remote.threads {
                        let Some(from) = task.delegated_from.as_ref().and_then(Option::as_ref)
                        else {
                            continue;
                        };
                        if &from.environment_id != environment.environment
                            || &from.environment_id == remote.environment
                        {
                            continue;
                        }
                        let own = rolled_up(task);
                        let nested = flags
                            .get(remote.environment)
                            .and_then(|f| f.get(&task.id))
                            .copied();
                        let request = if own == Some(PendingChildRequest::Approval) {
                            own
                        } else {
                            nested.or(own)
                        };
                        if let Some(request) = request {
                            delegated.push((from.thread_id.clone(), request));
                        }
                    }
                }
                if !delegated.is_empty() {
                    next.insert(
                        environment.environment.clone(),
                        local_flags(environment.threads, &delegated),
                    );
                }
            }
            if next == flags {
                break;
            }
            flags = next;
        }
        let mut next = Self {
            hidden,
            ..Default::default()
        };
        for environment in environments {
            for source in environment.threads {
                let key = ThreadKey {
                    environment: environment.environment.clone(),
                    thread: source.id.clone(),
                };
                let request = flags
                    .get(environment.environment)
                    .and_then(|f| f.get(&source.id))
                    .copied()
                    .map(|r| {
                        if r == PendingChildRequest::UserOnlyInput {
                            PendingChildRequest::UserInput
                        } else {
                            r
                        }
                    });
                if let Some(request) = request {
                    next.pending_child_requests.insert(key.clone(), request);
                }
                let view = ThreadShellView {
                    key: key.clone(),
                    source: source.clone(),
                    pending_child_request: request,
                };
                let row = self
                    .rows
                    .get(&key)
                    .filter(|r| ***r == view)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(view));
                let quiet = self
                    .quiet_rows
                    .get(&key)
                    .filter(|r| streaming_equal(r, &row) || ***r == *row)
                    .cloned()
                    .unwrap_or_else(|| row.clone());
                next.rows.insert(key.clone(), row);
                next.quiet_rows.insert(key.clone(), quiet);
                if environment.enabled {
                    next.ordered.push(key.clone());
                }
                if let Some(parent) = source.delegated_from.as_ref().and_then(Option::as_ref)
                    && environments
                        .iter()
                        .any(|e| e.enabled && e.environment == &parent.environment_id)
                    && by_environment
                        .get(&parent.environment_id)
                        .is_some_and(|rows| {
                            rows.iter()
                                .any(|t| t.id == parent.thread_id && t.archived_at.is_none())
                        })
                {
                    next.delegated_under_parent.insert(key);
                }
            }
        }
        *self = next;
    }
    pub fn visible(&self) -> Vec<&Rc<ThreadShellView>> {
        self.ordered
            .iter()
            .filter(|key| !self.hidden.contains(*key))
            .filter_map(|key| self.rows.get(key))
            .collect()
    }
    pub fn navigation(&self) -> Vec<&Rc<ThreadShellView>> {
        self.visible()
            .into_iter()
            .filter(|row| {
                row.source.archived_at.is_none()
                    && !is_shown_under_parent(
                        &row.key,
                        &row.source.lineage,
                        &self.delegated_under_parent,
                    )
            })
            .collect()
    }
    pub fn for_projects(&self, projects: &[ScopedProjectRef]) -> Vec<&Rc<ThreadShellView>> {
        let mut seen = BTreeSet::new();
        let mut rows = vec![];
        for project in projects {
            for key in &self.ordered {
                if key.environment == project.environment_id
                    && let Some(row) = self.rows.get(key)
                    && row.source.project_id == project.project_id
                    && seen.insert(key.clone())
                {
                    rows.push(row);
                }
            }
        }
        rows
    }
    pub fn refs_by_project(
        &self,
        environment: &EnvironmentId,
    ) -> BTreeMap<ProjectId, Vec<&ThreadKey>> {
        let mut grouped = BTreeMap::<_, Vec<_>>::new();
        for key in &self.ordered {
            if &key.environment == environment
                && let Some(row) = self.rows.get(key)
            {
                grouped
                    .entry(row.source.project_id.clone())
                    .or_default()
                    .push(key);
            }
        }
        grouped
    }
    pub fn side_chats(&self, parent: &ThreadKey) -> Vec<&Rc<ThreadShellView>> {
        self.ordered
            .iter()
            .filter_map(|key| self.rows.get(key))
            .filter(|row| {
                row.key.environment == parent.environment
                    && row.source.lineage.parent_thread_id.as_ref() == Some(&parent.thread)
                    && side_chat(&row.source)
            })
            .collect()
    }
    pub fn child_inputs(&self, parent: &ThreadKey) -> Vec<&Rc<ThreadShellView>> {
        let mut children = BTreeMap::<ThreadKey, Vec<&Rc<ThreadShellView>>>::new();
        for key in &self.ordered {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            let local = row
                .source
                .lineage
                .parent_thread_id
                .as_ref()
                .filter(|_| {
                    row.source.lineage.relationship_to_parent
                        == Some(AppThreadLineageRelationshipToParent::Subagent)
                        || side_chat(&row.source)
                })
                .map(|id| ThreadKey {
                    environment: key.environment.clone(),
                    thread: id.clone(),
                });
            let owner = local.or_else(|| {
                row.source
                    .delegated_from
                    .as_ref()?
                    .as_ref()
                    .map(|p| ThreadKey {
                        environment: p.environment_id.clone(),
                        thread: p.thread_id.clone(),
                    })
            });
            if let Some(owner) = owner {
                children.entry(owner).or_default().push(row);
            }
        }
        let mut seen = BTreeSet::from([parent.clone()]);
        let mut pending = vec![parent.clone()];
        let mut result = vec![];
        let mut index = 0;
        while index < pending.len() {
            if let Some(rows) = children.get(&pending[index]) {
                for row in rows {
                    if seen.insert(row.key.clone()) {
                        pending.push(row.key.clone());
                        if subagent_request(&row.source).is_some()
                            || (side_chat(&row.source) && request_kind(&row.source).is_some())
                        {
                            result.push(*row);
                        }
                    }
                }
            }
            index += 1;
        }
        result
    }
}
pub fn is_shown_under_parent(
    key: &ThreadKey,
    lineage: &OrchestrationV2AppThreadLineage,
    delegated: &BTreeSet<ThreadKey>,
) -> bool {
    lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent)
        || lineage.side_chat == Some(Some(true))
        || delegated.contains(key)
}
pub fn child_thread_inputs_title(children: &[&Rc<ThreadShellView>]) -> String {
    let approvals = children
        .iter()
        .filter(|c| c.has_pending_approvals())
        .count();
    let need = if approvals == 0 {
        "input"
    } else if approvals == children.len() {
        "approval"
    } else {
        "you"
    };
    let side_chats = children.iter().filter(|c| side_chat(&c.source)).count();
    let noun = if side_chats == children.len() {
        "side chat"
    } else if side_chats == 0 {
        "subagent"
    } else {
        "thread"
    };
    if children.len() == 1 {
        format!(
            "{} needs {need}",
            if noun == "side chat" {
                "Side chat"
            } else {
                "Subagent"
            }
        )
    } else {
        format!("{} {noun}s need {need}", children.len())
    }
}
