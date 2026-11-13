use super::items;
use crate::{ClientError, types::ThreadKey};
use futures::future::LocalBoxFuture;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryMeta {
    pub cursor: Option<String>,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub expanded: bool,
    pub latest_local_ordinal: Option<u64>,
}
impl HistoryMeta {
    pub fn partial(&self) -> bool {
        self.has_more || self.cursor.is_some()
    }
    pub fn clear_loading(&mut self, cursor: &str) {
        if self.cursor.as_deref() == Some(cursor) {
            self.loading = false;
        }
    }
    pub fn show_load_earlier(&self) -> bool {
        self.has_more || self.error.is_some()
    }
    pub fn apply_page(&mut self, page: &OrchestrationV2ThreadHistoryPage) {
        self.cursor = page.next_cursor.as_ref().map(|c| c.0.clone());
        self.has_more = page.has_more_history;
        self.loading = false;
        self.error = None;
        self.expanded = true;
    }
}
#[derive(Debug, Clone)]
pub struct WorkPage {
    pub anchor: IdOrIndex<t3_contracts::base_schemas::TurnItemId>,
    pub next_cursor: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
}
fn same_key(a: &OrchestrationV2ProjectedTurnItem, b: &OrchestrationV2ProjectedTurnItem) -> bool {
    a.source_thread_id == b.source_thread_id && a.source_item_id == b.source_item_id
}
pub fn merge_history(
    p: &mut OrchestrationV2ThreadProjection,
    older: Vec<OrchestrationV2ProjectedTurnItem>,
    work: Option<WorkPage>,
) {
    if older.is_empty() && work.is_none() {
        return;
    }
    let anchor = work
        .as_ref()
        .and_then(|w| {
            p.visible_turn_items
                .iter()
                .position(|r| r.source_item_id == w.anchor)
        })
        .and_then(|i| {
            items::deferred(&p.visible_turn_items[i].item)
                .clone()
                .map(|d| (i, p.visible_turn_items[i].clone(), d))
        });
    if work.is_some() && anchor.is_none() {
        return;
    }
    let mut page: Vec<OrchestrationV2ProjectedTurnItem> = Vec::new();
    for incoming in older {
        if let Some((_, a, d)) = &anchor
            && (incoming.source_thread_id != a.source_thread_id
                || items::ordinal(&incoming.item) < items::ordinal(&a.item)
                || items::ordinal(&incoming.item) > d.last_ordinal)
        {
            continue;
        }
        if page.iter().any(|r| same_key(r, &incoming)) {
            continue;
        }
        let mut row = p
            .visible_turn_items
            .iter()
            .find(|r| same_key(r, &incoming))
            .cloned()
            .unwrap_or(incoming);
        let local = row.visibility == ProjectedTurnItemVisibility::Local
            || row.source_thread_id == p.thread.id;
        if !p.visible_turn_items.iter().any(|r| same_key(r, &row))
            && local
            && let Some(current) = p
                .turn_items
                .iter()
                .find(|i| items::id(i) == &row.source_item_id)
        {
            if !matches!(
                current,
                OrchestrationV2TurnItem::RunInterruptRequest(_)
                    | OrchestrationV2TurnItem::ApprovalRequest(_)
                    | OrchestrationV2TurnItem::UserInputRequest(_)
            ) || !items::visible(p, current)
            {
                continue;
            }
            row.item = current.clone();
        }
        if local
            && !p
                .turn_items
                .iter()
                .any(|i| items::id(i) == &row.source_item_id)
        {
            p.turn_items.push(row.item.clone());
        }
        page.push(row);
    }
    if page.is_empty() && work.is_none() {
        return;
    }
    if let (Some(w), Some((start, mut a, mut d))) = (work, anchor) {
        let end = p
            .visible_turn_items
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, r)| {
                r.source_thread_id != a.source_thread_id
                    || r.visibility == ProjectedTurnItemVisibility::Synthetic
                    || items::ordinal(&r.item) > d.last_ordinal
            })
            .map_or(p.visible_turn_items.len(), |(i, _)| i);
        let deferred = if let Some(cursor) = w.next_cursor {
            d.cursor = cursor;
            Some(d)
        } else {
            None
        };
        items::set_deferred(&mut a.item, deferred);
        page.extend(
            p.visible_turn_items[start..end]
                .iter()
                .filter(|r| !page.iter().any(|v| same_key(v, r)))
                .cloned()
                .collect::<Vec<_>>(),
        );
        for row in &mut page {
            if same_key(row, &a) {
                *row = a.clone();
            }
        }
        page.sort_by_key(|r| items::ordinal(&r.item));
        p.visible_turn_items.splice(start..end, page);
        if a.visibility == ProjectedTurnItemVisibility::Local {
            super::shell::upsert(&mut p.turn_items, a.item, items::id);
        }
    } else if !page.is_empty() {
        page.extend(
            p.visible_turn_items
                .iter()
                .filter(|r| !page.iter().any(|v| same_key(v, r)))
                .cloned()
                .collect::<Vec<_>>(),
        );
        p.visible_turn_items = page;
    }
    items::renumber(&mut p.visible_turn_items);
}

pub trait HistoryHandler {
    fn load_earlier(
        &self,
        through_entry: Option<String>,
        work_anchor: Option<String>,
    ) -> LocalBoxFuture<'_, Result<super::sync::HistoryLoadResult, ClientError>>;
    fn reconcile_pending_work(&self) -> LocalBoxFuture<'_, Result<(), ClientError>>;
}

type Handlers = BTreeMap<ThreadKey, Rc<dyn HistoryHandler>>;

#[derive(Default, Clone)]
pub struct HistoryController {
    handlers: Rc<RefCell<Handlers>>,
}

pub struct HistoryRegistration {
    key: ThreadKey,
    handler: Rc<dyn HistoryHandler>,
    handlers: Weak<RefCell<Handlers>>,
}

impl Drop for HistoryRegistration {
    fn drop(&mut self) {
        if let Some(handlers) = self.handlers.upgrade() {
            let mut handlers = handlers.borrow_mut();
            if handlers
                .get(&self.key)
                .is_some_and(|current| Rc::ptr_eq(current, &self.handler))
            {
                handlers.remove(&self.key);
            }
        }
    }
}

impl HistoryController {
    pub fn register(&self, key: ThreadKey, handler: Rc<dyn HistoryHandler>) -> HistoryRegistration {
        self.handlers
            .borrow_mut()
            .insert(key.clone(), handler.clone());
        HistoryRegistration {
            key,
            handler,
            handlers: Rc::downgrade(&self.handlers),
        }
    }

    pub async fn load_earlier(
        &self,
        key: &ThreadKey,
        through_entry: Option<String>,
        work_anchor: Option<String>,
    ) -> Result<super::sync::HistoryLoadResult, ClientError> {
        let handler = self.handlers.borrow().get(key).cloned();
        match handler {
            Some(handler) => handler.load_earlier(through_entry, work_anchor).await,
            None => Ok(super::sync::HistoryLoadResult::Noop),
        }
    }

    pub async fn reconcile_pending_work(&self, key: &ThreadKey) -> Result<(), ClientError> {
        let handler = self.handlers.borrow().get(key).cloned();
        match handler {
            Some(handler) => handler.reconcile_pending_work().await,
            None => Ok(()),
        }
    }
}
