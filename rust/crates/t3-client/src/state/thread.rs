use super::{
    history::{HistoryMeta, merge_history},
    items, projection,
};
use crate::{
    ClientError,
    types::{Generation, SyncAction, SyncStatus},
};
use std::rc::Rc;
use t3_contracts::{base_schemas::ThreadId, orchestration_v2::*};
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadState {
    pub id: ThreadId,
    pub projection: Option<Rc<OrchestrationV2ThreadProjection>>,
    pub status: SyncStatus,
    pub sequence: u64,
    pub session: Generation,
    pub generation: u64,
    pub history: HistoryMeta,
    pub error: Option<String>,
    pub resync_required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRequest {
    pub session: Generation,
    pub generation: u64,
    pub cursor: String,
}
impl ThreadState {
    pub fn cache_snapshot(
        &self,
        accepts_bounded: bool,
    ) -> Option<OrchestrationV2ThreadDetailSnapshot> {
        let projection = self.projection.as_ref()?;
        if self.history.expanded
            || projection.runs.iter().any(|run| {
                matches!(
                    run.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                )
            })
        {
            return None;
        }
        let bounded = accepts_bounded || self.history.partial();
        Some(OrchestrationV2ThreadDetailSnapshot {
            snapshot_sequence: self.sequence,
            projection: projection.as_ref().clone(),
            history_cursor: bounded.then(|| {
                self.history
                    .cursor
                    .clone()
                    .map(t3_contracts::base_schemas::TrimmedNonEmptyString)
            }),
            has_more_history: bounded.then_some(self.history.has_more),
            latest_local_turn_ordinal: bounded.then_some(self.history.latest_local_ordinal),
        })
    }
    pub fn missing_pending_question_card(&self) -> bool {
        self.projection.as_ref().is_some_and(|p| p.runtime_requests.iter().any(|request| {
            matches!(&request.kind, RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput) && request.status == RuntimeRequestStatus::Pending
                && !p.turn_items.iter().any(|item| matches!(item, OrchestrationV2TurnItem::UserInputRequest(item) if item.request_id == request.id))
        }))
    }
    pub fn new(id: ThreadId) -> Self {
        Self {
            id,
            projection: None,
            status: SyncStatus::Empty,
            sequence: 0,
            session: Generation(0),
            generation: 0,
            history: HistoryMeta::default(),
            error: None,
            resync_required: false,
        }
    }
    pub fn begin_session(&mut self, session: Generation) {
        self.session = session;
        if self.status != SyncStatus::Deleted {
            self.status = SyncStatus::Synchronizing;
        }
        self.error = None;
    }
    pub fn disconnect(&mut self) {
        if self.status != SyncStatus::Deleted {
            self.status = if self.projection.is_some() {
                SyncStatus::Cached
            } else {
                SyncStatus::Empty
            };
        }
    }
    pub fn delivery_gap(&mut self) -> SyncAction {
        self.resync_required = true;
        self.status = SyncStatus::Synchronizing;
        SyncAction::ReloadSnapshot
    }
    pub fn resume_sequence(&self) -> Option<u64> {
        if self.resync_required {
            None
        } else {
            self.projection.as_ref().map(|_| self.sequence)
        }
    }
    pub fn install(
        &mut self,
        mut snapshot: OrchestrationV2ThreadDetailSnapshot,
    ) -> Result<(), ClientError> {
        if snapshot.projection.thread.id
            != t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(self.id.clone())
        {
            return Err(ClientError::InvalidPayload(
                "thread snapshot identity".into(),
            ));
        }
        items::restore_snapshot(&mut snapshot.projection);
        self.history = HistoryMeta {
            cursor: snapshot.history_cursor.flatten().map(|s| s.0),
            has_more: snapshot.has_more_history.unwrap_or(false),
            latest_local_ordinal: snapshot.latest_local_turn_ordinal.flatten(),
            ..HistoryMeta::default()
        };
        self.sequence = snapshot.snapshot_sequence;
        self.projection = Some(Rc::new(snapshot.projection));
        self.status = SyncStatus::Synchronizing;
        self.generation += 1;
        self.error = None;
        self.resync_required = false;
        Ok(())
    }
    pub fn history_request(&mut self) -> Option<HistoryRequest> {
        if self.history.loading || self.status == SyncStatus::Deleted || !self.history.has_more {
            return None;
        }
        let cursor = self.history.cursor.clone()?;
        self.history.loading = true;
        self.history.error = None;
        Some(HistoryRequest {
            session: self.session,
            generation: self.generation,
            cursor,
        })
    }
    pub fn apply_history(
        &mut self,
        request: &HistoryRequest,
        page: OrchestrationV2ThreadHistoryPage,
    ) -> SyncAction {
        if request.session != self.session
            || request.generation != self.generation
            || self.history.cursor.as_deref() != Some(request.cursor.as_str())
        {
            return SyncAction::StaleSession;
        }
        let Some(projection) = &mut self.projection else {
            return SyncAction::None;
        };
        self.history.apply_page(&page);
        merge_history(Rc::make_mut(projection), page.items, None);
        SyncAction::Applied
    }
    pub fn apply(
        &mut self,
        session: Generation,
        item: OrchestrationV2ThreadStreamItem,
    ) -> Result<SyncAction, ClientError> {
        if session != self.session {
            return Ok(SyncAction::StaleSession);
        }
        use OrchestrationV2ThreadStreamItem::*;
        match item {
            Snapshot(mut v) => {
                if v.turn_items_omit_local_visible.is_some() {
                    super::decoding::restore_local(&mut v.projection);
                }
                self.install(OrchestrationV2ThreadDetailSnapshot {
                    snapshot_sequence: v.snapshot_sequence,
                    projection: v.projection,
                    history_cursor: v.history_cursor,
                    has_more_history: v.has_more_history,
                    latest_local_turn_ordinal: v.latest_local_turn_ordinal,
                })?;
                Ok(SyncAction::Applied)
            }
            Synchronized(_) => {
                if self.status == SyncStatus::Deleted {
                    return Ok(SyncAction::None);
                }
                if self.projection.is_some() && !self.resync_required {
                    self.status = SyncStatus::Live;
                    Ok(SyncAction::Applied)
                } else {
                    Ok(self.delivery_gap())
                }
            }
            event => {
                if self.resync_required {
                    return Ok(SyncAction::ReloadSnapshot);
                }
                let sequence = match &event {
                    Event(e) => e.sequence,
                    UnknownEvent { sequence, .. } => *sequence,
                    _ => return Ok(SyncAction::None),
                };
                if sequence <= self.sequence {
                    return Ok(SyncAction::Duplicate);
                }
                if self.status == SyncStatus::Deleted {
                    return Ok(SyncAction::None);
                }
                match event {
                    UnknownEvent { .. } => {
                        self.sequence = sequence;
                        Ok(SyncAction::Applied)
                    }
                    Event(e) => {
                        if projection::event_thread(&e.event) != &self.id {
                            return Err(ClientError::InvalidPayload(
                                "event thread identity".into(),
                            ));
                        }
                        if matches!(e.event, OrchestrationV2DomainEvent::ThreadDeleted(_)) {
                            self.sequence = sequence;
                            self.projection = None;
                            self.status = SyncStatus::Deleted;
                            self.history = HistoryMeta::default();
                            self.generation += 1;
                            return Ok(SyncAction::Applied);
                        }
                        let Some(current) = &mut self.projection else {
                            return Ok(self.delivery_gap());
                        };
                        let ordinal =
                            if let OrchestrationV2DomainEvent::TurnItemUpdated(v) = &e.event {
                                Some(items::ordinal(&v.payload))
                            } else {
                                None
                            };
                        if projection::ignores_partial_item(current, &e.event, &self.history) {
                            self.sequence = sequence;
                            return Ok(SyncAction::Applied);
                        }
                        match projection::apply_event(Rc::make_mut(current), e.event, &self.history)
                        {
                            Ok(changed) => {
                                self.sequence = sequence;
                                if changed
                                    && self.history.partial()
                                    && let Some(n) = ordinal
                                {
                                    self.history.latest_local_ordinal = Some(
                                        self.history
                                            .latest_local_ordinal
                                            .map_or(n, |old| old.max(n)),
                                    );
                                }
                                self.error = None;
                                Ok(SyncAction::Applied)
                            }
                            Err(error) => {
                                self.error = Some(error.to_string());
                                self.delivery_gap();
                                Err(error)
                            }
                        }
                    }
                    _ => Ok(SyncAction::None),
                }
            }
        }
    }
}
