//! Port of apps/web/src/components/chat/CommandOutputPanel.tsx CommandOutputPanel.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_contracts::orchestration_v2::{OrchestrationV2ProjectedTurnItem, OrchestrationV2TurnItem};

use crate::runtime::Shared;

#[derive(Clone, Debug)]
pub struct CommandOutputRow {
    pub projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
}

impl CommandOutputRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let entry = match &row.content {
            MessagesTimelineRowContent::Work {
                grouped_entries, ..
            } => grouped_entries.first()?,
            MessagesTimelineRowContent::WorkLive { entry, .. } => entry,
            _ => return None,
        };
        let projected_item = entry.projected_item.as_ref()?;
        matches!(
            projected_item.item,
            OrchestrationV2TurnItem::CommandExecution(_)
        )
        .then(|| Self {
            projected_item: projected_item.clone(),
        })
    }
}

pub fn command_output_panel(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> impl IntoElement {
    // C6/Backend must expose the subscribed, incrementally folded output before this can mount it.
    // Raw frames are not display text; folding them here would duplicate t3-shared's client fold.
    dynamic(move || {
        row.with(|row| {
            row.as_ref()
                .and_then(|row| CommandOutputRow::from_row(row))
                .map(|_| view().into_element())
        })
    })
}
