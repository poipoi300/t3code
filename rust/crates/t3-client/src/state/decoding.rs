use crate::ClientError;
use serde_json::Value;
use t3_contracts::{
    orchestration_v2::*,
    orchestration_v2_id_table::{RecordContainer, decode_record_id_table},
};
pub fn shell_stream(raw: Value) -> Result<Option<OrchestrationV2ShellStreamItem>, ClientError> {
    match raw.get("kind").and_then(Value::as_str) {
        Some(
            "snapshot" | "synchronized" | "snapshot-required" | "project.updated"
            | "project.removed" | "thread.updated" | "thread.removed",
        ) => Ok(Some(serde_json::from_value(raw)?)),
        Some(_) => Ok(None),
        None => Err(ClientError::InvalidPayload("shell stream item kind".into())),
    }
}
pub fn thread_stream(mut raw: Value) -> Result<OrchestrationV2ThreadStreamItem, ClientError> {
    let container = if raw.get("kind").and_then(Value::as_str) == Some("snapshot") {
        RecordContainer::Key("projection")
    } else {
        RecordContainer::NoRecords
    };
    decode_record_id_table(&mut raw, container)?;
    let mut item: OrchestrationV2ThreadStreamItem = serde_json::from_value(raw)?;
    if let OrchestrationV2ThreadStreamItem::Snapshot(v) = &mut item {
        if v.turn_items_omit_local_visible.is_some() {
            restore_local(&mut v.projection);
        }
        super::items::restore_snapshot(&mut v.projection);
        v.turn_items_omit_local_visible = None;
    }
    Ok(item)
}
pub fn bounded_snapshot(
    mut raw: Value,
) -> Result<OrchestrationV2ThreadDetailSnapshot, ClientError> {
    decode_record_id_table(&mut raw, RecordContainer::Key("projection"))?;
    let bounded: OrchestrationV2ThreadBoundedSnapshot = serde_json::from_value(raw)?;
    let mut p = bounded.projection;
    if bounded.turn_items_omit_local_visible.is_some() {
        restore_local(&mut p);
    }
    super::items::restore_snapshot(&mut p);
    Ok(OrchestrationV2ThreadDetailSnapshot {
        snapshot_sequence: bounded.snapshot_sequence,
        projection: p,
        history_cursor: Some(bounded.history_cursor),
        has_more_history: Some(bounded.has_more_history),
        latest_local_turn_ordinal: Some(bounded.latest_local_turn_ordinal),
    })
}
pub fn history_page(mut raw: Value) -> Result<OrchestrationV2ThreadHistoryPage, ClientError> {
    decode_record_id_table(&mut raw, RecordContainer::TopLevel)?;
    Ok(serde_json::from_value(raw)?)
}
pub fn restore_local(p: &mut OrchestrationV2ThreadProjection) {
    let mut items = p
        .visible_turn_items
        .iter()
        .filter(|r| {
            r.visibility == ProjectedTurnItemVisibility::Local || r.source_thread_id == p.thread.id
        })
        .map(|r| r.item.clone())
        .collect::<Vec<_>>();
    items.append(&mut p.turn_items);
    p.turn_items = items;
}

pub fn full_snapshot(mut raw: Value) -> Result<OrchestrationV2ThreadDetailSnapshot, ClientError> {
    decode_record_id_table(&mut raw, RecordContainer::Key("projection"))?;
    let mut snapshot: OrchestrationV2ThreadDetailSnapshot = serde_json::from_value(raw)?;
    super::items::restore_snapshot(&mut snapshot.projection);
    Ok(snapshot)
}

pub fn deferred_shell<P: crate::platform::Platform + 'static>(
    mut raw: Value,
    platform: std::rc::Rc<P>,
) -> Result<super::sync::LoadedShell, ClientError> {
    use futures::FutureExt;
    let project_identities = raw
        .get("projectIdentities")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let raw_links = raw
        .get_mut("threads")
        .and_then(Value::as_array_mut)
        .map(|rows| {
            rows.iter_mut()
                .map(|row| {
                    row.as_object_mut()
                        .and_then(|row| row.remove("pullRequests"))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let snapshot: OrchestrationV2ShellSnapshot = serde_json::from_value(raw)?;
    let ids = snapshot
        .threads
        .iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    let links = async move {
        let mut decoded = std::collections::BTreeMap::new();
        let mut budget = 400;
        for (thread, raw) in ids.into_iter().zip(raw_links) {
            let Some(raw) = raw else {
                continue;
            };
            let size = raw.as_array().map_or(1, Vec::len);
            if budget + size > 400 {
                platform.yield_to_host().await;
                budget = 0;
            }
            budget += size;
            if let Ok(links) = serde_json::from_value(raw) {
                decoded.insert(thread, links);
            }
        }
        decoded
    }
    .boxed_local();
    Ok(super::sync::LoadedShell {
        snapshot,
        project_identities,
        links: Some(links),
    })
}

pub fn transcript(raw: Value) -> Result<OrchestrationV2ThreadTranscript, ClientError> {
    Ok(serde_json::from_value(raw)?)
}

/// Schema 6 caches carry counts even when local rows could not be omitted.
pub fn compact_projection(
    mut projection: OrchestrationV2ThreadProjection,
    thread_id: &t3_contracts::base_schemas::ThreadId,
    omit_local_visible: bool,
    turn_item_count: usize,
    visible_turn_item_count: usize,
) -> Result<OrchestrationV2ThreadProjection, ClientError> {
    use std::collections::BTreeSet;
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    let thread = IdOrIndex::Id(thread_id.clone());
    let mut identities = BTreeSet::new();
    let mut position = None;
    if projection.thread.id != thread
        || projection.visible_turn_items.len() != visible_turn_item_count
    {
        return Err(ClientError::InvalidPayload(
            "compact thread identity or count".into(),
        ));
    }
    for row in &projection.visible_turn_items {
        let source = super::items::thread_id(&row.item);
        let fork_source = match &row.item {
            OrchestrationV2TurnItem::Fork(item) => match &item.source {
                AppThreadForkedFrom::Run(source) => Some(IdOrIndex::Id(source.thread_id.clone())),
                _ => None,
            },
            _ => None,
        };
        let key = serde_json::to_string(&(&row.source_thread_id, &row.source_item_id))?;
        if row.source_item_id != *super::items::id(&row.item)
            || !matches!(row.source_item_id, IdOrIndex::Id(_))
            || !matches!(row.source_thread_id, IdOrIndex::Id(_))
            || (&row.source_thread_id != source
                && Some(&row.source_thread_id) != fork_source.as_ref())
            || (row.visibility == ProjectedTurnItemVisibility::Local
                && row.source_thread_id != thread)
            || (row.visibility == ProjectedTurnItemVisibility::Inherited
                && row.source_thread_id == thread)
            || position.is_some_and(|previous| row.position <= previous)
            || !identities.insert(key)
        {
            return Err(ClientError::InvalidPayload(
                "compact visible row identity or order".into(),
            ));
        }
        position = Some(row.position);
    }
    if omit_local_visible {
        restore_local(&mut projection);
    }
    let mut ids = BTreeSet::new();
    for item in &projection.turn_items {
        if super::items::thread_id(item) != &thread
            || !matches!(super::items::id(item), IdOrIndex::Id(_))
            || !ids.insert(serde_json::to_string(super::items::id(item))?)
        {
            return Err(ClientError::InvalidPayload(
                "compact local item identity".into(),
            ));
        }
    }
    if projection.turn_items.len() != turn_item_count
        || projection.visible_turn_items.iter().any(|row| {
            row.source_thread_id == thread
                && !projection
                    .turn_items
                    .iter()
                    .any(|item| super::items::id(item) == &row.source_item_id)
        })
    {
        return Err(ClientError::InvalidPayload(
            "compact restored item count or membership".into(),
        ));
    }
    super::items::restore_snapshot(&mut projection);
    Ok(projection)
}
