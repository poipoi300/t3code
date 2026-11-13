//! Graph selectors operate within one environment; callers never combine unscoped shells.
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::ThreadId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    wire::WireDateTime,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationshipKind {
    Parent,
    Fork,
    Subagent,
    Transfer,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub source: ThreadId,
    pub target: ThreadId,
    pub kind: RelationshipKind,
    pub status: Option<&'static str>,
}
pub struct Graph<'a> {
    pub nodes: BTreeMap<ThreadId, Option<&'a OrchestrationV2ThreadShell>>,
    pub edges: Vec<Edge>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRow {
    pub thread: ThreadId,
    pub from: ThreadId,
    pub depth: usize,
    pub edge: Edge,
}
fn status(shell: &OrchestrationV2ThreadShell) -> &'static str {
    if let Some(Some(active)) = &shell.activity_run_status {
        return match active {
            ThreadShellActivityRunStatus::Preparing => "preparing",
            ThreadShellActivityRunStatus::Starting => "starting",
            ThreadShellActivityRunStatus::Running => "running",
            ThreadShellActivityRunStatus::Waiting => "waiting",
        };
    }
    match shell.status {
        OrchestrationV2ShellThreadStatus::Idle => "idle",
        OrchestrationV2ShellThreadStatus::Preparing => "preparing",
        OrchestrationV2ShellThreadStatus::Queued => "queued",
        OrchestrationV2ShellThreadStatus::Starting => "starting",
        OrchestrationV2ShellThreadStatus::Running => "running",
        OrchestrationV2ShellThreadStatus::Waiting => "waiting",
        OrchestrationV2ShellThreadStatus::Completed => "completed",
        OrchestrationV2ShellThreadStatus::Interrupted => "interrupted",
        OrchestrationV2ShellThreadStatus::Failed => "failed",
        OrchestrationV2ShellThreadStatus::Cancelled => "cancelled",
        OrchestrationV2ShellThreadStatus::RolledBack => "rolled_back",
    }
}
fn item_status(status: &OrchestrationV2TurnItemStatus) -> &'static str {
    match status {
        OrchestrationV2TurnItemStatus::Idle => "idle",
        OrchestrationV2TurnItemStatus::Pending => "pending",
        OrchestrationV2TurnItemStatus::Running => "running",
        OrchestrationV2TurnItemStatus::Waiting => "waiting",
        OrchestrationV2TurnItemStatus::Completed => "completed",
        OrchestrationV2TurnItemStatus::Failed => "failed",
        OrchestrationV2TurnItemStatus::Cancelled => "cancelled",
        OrchestrationV2TurnItemStatus::Interrupted => "interrupted",
    }
}
fn parent<'a>(
    lineage: &'a OrchestrationV2AppThreadLineage,
    fork: Option<&'a AppThreadForkedFrom>,
) -> Option<&'a ThreadId> {
    match fork {
        Some(AppThreadForkedFrom::Run(run)) => Some(&run.thread_id),
        _ => lineage.parent_thread_id.as_ref(),
    }
}
pub fn merge_back_target(projection: &OrchestrationV2ThreadProjection) -> Option<&ThreadId> {
    if projection.thread.lineage.relationship_to_parent
        != Some(AppThreadLineageRelationshipToParent::Fork)
    {
        return None;
    }
    parent(
        &projection.thread.lineage,
        projection.thread.forked_from.as_ref(),
    )
}
fn add_edge(graph: &mut Graph<'_>, mut edge: Edge) {
    graph.nodes.entry(edge.source.clone()).or_insert(None);
    graph.nodes.entry(edge.target.clone()).or_insert(None);
    if edge.kind == RelationshipKind::Subagent && edge.status == Some("rolled_back") {
        edge.status = Some("interrupted");
    }
    if let Some(existing) = graph
        .edges
        .iter_mut()
        .find(|e| e.source == edge.source && e.target == edge.target && e.kind == edge.kind)
    {
        *existing = edge;
    } else {
        graph.edges.push(edge);
    }
}
pub fn derive_graph<'a>(
    threads: &'a [OrchestrationV2ThreadShell],
    projection: Option<&OrchestrationV2ThreadProjection>,
) -> Result<Graph<'a>, crate::ClientError> {
    let mut graph = Graph {
        nodes: BTreeMap::new(),
        edges: vec![],
    };
    let mut unique = vec![];
    for thread in threads {
        if !graph.nodes.contains_key(&thread.id) {
            graph.nodes.insert(thread.id.clone(), Some(thread));
            unique.push(thread);
        }
    }
    for thread in unique {
        if let Some(parent) = parent(
            &thread.lineage,
            thread.forked_from.as_ref().and_then(Option::as_ref),
        ) {
            add_edge(
                &mut graph,
                Edge {
                    source: parent.clone(),
                    target: thread.id.clone(),
                    kind: if thread.lineage.relationship_to_parent
                        == Some(AppThreadLineageRelationshipToParent::Subagent)
                    {
                        RelationshipKind::Subagent
                    } else {
                        RelationshipKind::Fork
                    },
                    status: Some(status(thread)),
                },
            );
        }
    }
    if let Some(p) = projection {
        let IdOrIndex::Id(owner) = &p.thread.id else {
            return Err(crate::ClientError::InvalidPayload(
                "relationship graph requires expanded thread IDs".into(),
            ));
        };
        if !graph.nodes.contains_key(owner)
            && let Some(parent) = parent(&p.thread.lineage, p.thread.forked_from.as_ref())
        {
            add_edge(
                &mut graph,
                Edge {
                    source: parent.clone(),
                    target: owner.clone(),
                    kind: if p.thread.lineage.relationship_to_parent
                        == Some(AppThreadLineageRelationshipToParent::Subagent)
                    {
                        RelationshipKind::Subagent
                    } else {
                        RelationshipKind::Fork
                    },
                    status: None,
                },
            );
        }
        for subagent in &p.subagents {
            let Some(child) = &subagent.child_thread_id else {
                continue;
            };
            let IdOrIndex::Id(child_id) = child else {
                return Err(crate::ClientError::InvalidPayload(
                    "relationship graph requires expanded child IDs".into(),
                ));
            };
            let child = graph.nodes.get(child_id).and_then(|s| *s);
            let active = matches!(
                subagent.status,
                OrchestrationV2TurnItemStatus::Pending
                    | OrchestrationV2TurnItemStatus::Running
                    | OrchestrationV2TurnItemStatus::Waiting
            );
            let current = child.map_or_else(
                || item_status(&subagent.status),
                |c| {
                    if c.activity_run_status.as_ref().is_some_and(Option::is_some)
                        || (c.latest_run_id.is_some() && !active)
                    {
                        status(c)
                    } else {
                        item_status(&subagent.status)
                    }
                },
            );
            add_edge(
                &mut graph,
                Edge {
                    source: owner.clone(),
                    target: child_id.clone(),
                    kind: RelationshipKind::Subagent,
                    status: Some(current),
                },
            );
        }
        for transfer in &p.context_transfers {
            if transfer.source_thread_id == transfer.target_thread_id {
                continue;
            }
            let (IdOrIndex::Id(source), IdOrIndex::Id(target)) =
                (&transfer.source_thread_id, &transfer.target_thread_id)
            else {
                return Err(crate::ClientError::InvalidPayload(
                    "relationship graph requires expanded transfer IDs".into(),
                ));
            };
            let status = match transfer.status {
                ContextTransferStatus::Pending => "pending",
                ContextTransferStatus::ResolvedNative => "resolved_native",
                ContextTransferStatus::ResolvedPortable => "resolved_portable",
                ContextTransferStatus::Failed => "failed",
                ContextTransferStatus::Consumed => "consumed",
                ContextTransferStatus::Superseded => "superseded",
            };
            add_edge(
                &mut graph,
                Edge {
                    source: source.clone(),
                    target: target.clone(),
                    kind: RelationshipKind::Transfer,
                    status: Some(status),
                },
            );
        }
    }
    Ok(graph)
}
pub fn immediate(graph: &Graph<'_>, thread: &ThreadId) -> Vec<WalkRow> {
    let mut seen = BTreeSet::new();
    let mut rows = vec![];
    for edge in &graph.edges {
        let related = if &edge.source == thread {
            Some(&edge.target)
        } else if &edge.target == thread {
            Some(&edge.source)
        } else {
            None
        };
        if let Some(related) = related
            && seen.insert(related.clone())
        {
            rows.push(WalkRow {
                thread: related.clone(),
                from: thread.clone(),
                depth: 1,
                edge: edge.clone(),
            });
        }
    }
    rows
}
pub fn is_parent(edge: &Edge, current: &ThreadId) -> bool {
    edge.kind != RelationshipKind::Transfer && &edge.target == current
}
pub fn row_status(graph: &Graph<'_>, row: &WalkRow) -> Option<&'static str> {
    if row.edge.kind == RelationshipKind::Transfer || row.thread == row.edge.target {
        return row.edge.status;
    }
    graph.nodes.get(&row.thread).and_then(|s| *s).map(status)
}
pub fn order_web_rows(
    graph: &Graph<'_>,
    rows: &mut [WalkRow],
    current: &ThreadId,
    merge: Option<&ThreadId>,
    epoch_millis: impl Fn(&WireDateTime) -> Option<i64>,
) {
    let rank = |r: &WalkRow| {
        if is_parent(&r.edge, current) {
            0
        } else if merge == Some(&r.thread) {
            1
        } else {
            2
        }
    };
    let created = |r: &WalkRow| {
        graph
            .nodes
            .get(&r.thread)
            .and_then(|s| *s)
            .and_then(|s| epoch_millis(&s.created_at))
    };
    rows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| match (created(a), created(b)) {
                (Some(a), Some(b)) => b.cmp(&a),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.thread.cmp(&b.thread))
    });
}
pub fn family_rows_missing<'a>(
    shell: &[OrchestrationV2ThreadShell],
    families: &'a [OrchestrationV2ThreadFamily],
    removed: &BTreeMap<ThreadId, u64>,
) -> Vec<&'a OrchestrationV2ThreadShell> {
    let mut present: BTreeSet<_> = shell.iter().map(|s| s.id.clone()).collect();
    let mut missing = vec![];
    for family in families {
        for thread in &family.threads {
            if present.contains(&thread.id)
                || removed
                    .get(&thread.id)
                    .is_some_and(|sequence| *sequence > family.snapshot_sequence)
            {
                continue;
            }
            present.insert(thread.id.clone());
            missing.push(thread);
        }
    }
    missing
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn shell_json(
        name: &str,
        parent: Option<&str>,
        relationship: Option<&str>,
        status: &str,
    ) -> Value {
        json!({"id":name,"projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":parent.unwrap_or(name),"parentThreadId":parent,"relationshipToParent":relationship},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":status,"pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0})
    }
    fn projection(thread: Value) -> OrchestrationV2ThreadProjection {
        serde_json::from_value(json!({"thread":thread,"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":"2026-06-20T00:00:00.000Z"})).unwrap()
    }
    #[test]
    fn graph_uses_first_shell_and_direction_specific_status() {
        for (parent_status, child_status) in [("running", "completed"), ("completed", "running")] {
            let mut p = shell_json("parent", None, None, "cancelled");
            p["activityRunStatus"] = json!(if parent_status == "running" {
                "running"
            } else {
                "waiting"
            });
            let mut c = shell_json("child", Some("parent"), Some("fork"), child_status);
            c["activityRunStatus"] = json!("running");
            let stale = shell_json("child", None, None, "cancelled");
            let threads = vec![
                serde_json::from_value(p).unwrap(),
                serde_json::from_value(c).unwrap(),
                serde_json::from_value(stale).unwrap(),
            ];
            let g = derive_graph(&threads, None).unwrap();
            assert_eq!(g.edges.len(), 1);
            assert_eq!(
                row_status(&g, &immediate(&g, &id("child"))[0]),
                Some(if parent_status == "running" {
                    "running"
                } else {
                    "waiting"
                })
            );
            assert_eq!(
                row_status(&g, &immediate(&g, &id("parent"))[0]),
                Some("running")
            );
        }
    }
    #[test]
    fn omitted_selected_child_keeps_parent_edge_and_merge_target() {
        for relationship in ["fork", "subagent"] {
            let p = projection(shell_json(
                "child",
                Some("parent"),
                Some(relationship),
                "idle",
            ));
            let g = derive_graph(&[], Some(&p)).unwrap();
            assert_eq!(immediate(&g, &id("child"))[0].thread, id("parent"));
            assert_eq!(
                merge_back_target(&p).cloned(),
                if relationship == "fork" {
                    Some(id("parent"))
                } else {
                    None
                }
            );
        }
        let mut p = projection(shell_json("child", Some("parent"), Some("fork"), "idle"));
        p.thread.forked_from = Some(AppThreadForkedFrom::Run(Box::new(AppThreadForkedFromRun {
            thread_id: id("merge"),
            run_id: t3_contracts::base_schemas::RunId("r".into()),
        })));
        assert_eq!(merge_back_target(&p), Some(&id("merge")));
        p.thread.id = IdOrIndex::Index(0);
        assert!(derive_graph(&[], Some(&p)).is_err());
    }
    #[test]
    fn family_missing_rows_respect_removals_and_shell_authority() {
        let a: OrchestrationV2ThreadShell =
            serde_json::from_value(shell_json("a", None, None, "idle")).unwrap();
        let b: OrchestrationV2ThreadShell =
            serde_json::from_value(shell_json("b", None, None, "idle")).unwrap();
        let families = [
            OrchestrationV2ThreadFamily {
                snapshot_sequence: 3,
                threads: vec![a.clone(), b.clone()],
            },
            OrchestrationV2ThreadFamily {
                snapshot_sequence: 4,
                threads: vec![b.clone()],
            },
        ];
        assert_eq!(
            family_rows_missing(&[a], &families, &BTreeMap::from([(id("b"), 4)]))
                .iter()
                .map(|t| &t.id)
                .collect::<Vec<_>>(),
            vec![&id("b")]
        );
    }
    fn id(value: &str) -> ThreadId {
        ThreadId(value.into())
    }
    fn edge(source: &str, target: &str, kind: RelationshipKind, status: &'static str) -> Edge {
        Edge {
            source: id(source),
            target: id(target),
            kind,
            status: Some(status),
        }
    }
    #[test]
    fn cycles_missing_and_dedup() {
        let mut g = Graph {
            nodes: BTreeMap::new(),
            edges: vec![],
        };
        add_edge(
            &mut g,
            edge("missing", "child", RelationshipKind::Subagent, "running"),
        );
        add_edge(
            &mut g,
            edge("child", "missing", RelationshipKind::Transfer, "completed"),
        );
        assert!(g.nodes[&id("missing")].is_none());
        assert_eq!(immediate(&g, &id("child")).len(), 1);
        assert_eq!(row_status(&g, &immediate(&g, &id("child"))[0]), None);
        assert_eq!(
            row_status(&g, &immediate(&g, &id("missing"))[0]),
            Some("running")
        );
    }
    #[test]
    fn edge_status_replacement_keeps_order() {
        let mut g = Graph {
            nodes: BTreeMap::new(),
            edges: vec![],
        };
        add_edge(
            &mut g,
            edge("a", "b", RelationshipKind::Subagent, "running"),
        );
        add_edge(&mut g, edge("a", "c", RelationshipKind::Fork, "completed"));
        add_edge(
            &mut g,
            edge("a", "b", RelationshipKind::Subagent, "rolled_back"),
        );
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].status, Some("interrupted"));
        assert_eq!(g.edges[1].target, id("c"));
    }
}
