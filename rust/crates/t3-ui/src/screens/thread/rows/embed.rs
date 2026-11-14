//! Port of apps/web/src/components/chat/HtmlRenderFrame.tsx HtmlRenderFrame and McpAppFrame.tsx McpAppFrame.

use nsh_core::prelude::*;
use t3_client::thread_views::embedded_content::{HtmlRenderReference, McpAppReference};
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::{ThreadId, ThreadKey};
use t3_contracts::assets::AssetResource;
use t3_contracts::base_schemas::TurnItemId;

use crate::runtime::Shared;

pub type TimelineRow = Memo<Option<Shared<MessagesTimelineRow>>>;

#[derive(Clone, Debug, PartialEq)]
pub struct HtmlRenderRow {
    pub id: String,
    pub thread: ThreadKey,
    pub reference: HtmlRenderReference,
    pub resource: AssetResource,
}

impl HtmlRenderRow {
    pub fn from_row(row: &MessagesTimelineRow, thread: &ThreadKey) -> Option<Self> {
        let MessagesTimelineRowContent::HtmlRender { html_render } = &row.content else {
            return None;
        };
        let reference = HtmlRenderReference::from_projection(html_render)?;
        let resource = reference.asset_resource();
        Some(Self {
            id: row.id.clone(),
            thread: thread.clone(),
            reference,
            resource,
        })
    }
}

/// Port of McpAppFrame.tsx props: inherited apps execute against their source item.
#[derive(Clone, Debug, PartialEq)]
pub struct McpAppRow {
    pub id: String,
    pub source: ThreadKey,
    pub conversation: ThreadKey,
    pub item_id: TurnItemId,
    pub revision: String,
    pub app: McpAppReference,
    pub resource: AssetResource,
}

impl McpAppRow {
    pub fn from_row(row: &MessagesTimelineRow, conversation: &ThreadKey) -> Option<Self> {
        let MessagesTimelineRowContent::McpApp {
            source_thread_id,
            item_id,
            revision,
            mcp_app,
        } = &row.content
        else {
            return None;
        };
        let app = McpAppReference::from_projection(mcp_app)?;
        let resource = app.asset_resource();
        Some(Self {
            id: row.id.clone(),
            source: ThreadKey {
                environment: conversation.environment.clone(),
                thread: ThreadId(source_thread_id.clone()),
            },
            conversation: conversation.clone(),
            item_id: TurnItemId(item_id.clone()),
            revision: revision.clone(),
            app,
            resource,
        })
    }
}

pub fn html_render_frame(row: TimelineRow, thread: ThreadKey) -> impl IntoElement {
    // Backend must supply guarded long-lived assets before a guest document can be mounted.
    dynamic(move || {
        let _ = row.with(|row| {
            row.as_ref()
                .and_then(|row| HtmlRenderRow::from_row(row, &thread))
        });
        None::<AnyElement>
    })
}

pub fn mcp_app_frame(row: TimelineRow, conversation: ThreadKey) -> impl IntoElement {
    // The guarded MCP commands and authenticated frame bridge are frozen integrator seams.
    dynamic(move || {
        let _ = row.with(|row| {
            row.as_ref()
                .and_then(|row| McpAppRow::from_row(row, &conversation))
        });
        None::<AnyElement>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::rc::Rc;
    use t3_client::thread_views::messages_timeline::{
        MessagesTimelineRowsInput, derive_messages_timeline_rows,
    };
    use t3_client::types::EnvironmentId;
    use t3_client::view_logic::session::{
        TimelineEntriesInput, derive_timeline_entries_from_visible_turn_items_with_state,
    };

    #[test]
    fn inherited_apps_retain_source_scope_revision_and_the_guarded_asset_intent() {
        let projected_item = serde_json::from_value(json!({
            "position":0,"visibility":"inherited","sourceThreadId":"original","sourceItemId":"call",
            "item":{"id":"call","threadId":"original","runId":null,"nodeId":null,"providerThreadId":null,
            "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":0,"status":"completed",
            "title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-11-14T12:00:00Z",
            "type":"dynamic_tool","toolName":"weather.lookup","input":{},"output":{"t3McpApp":{
                "attachmentId":"app","server":"weather","tool":"lookup","resourceUri":"ui://weather/card"
            },"result":{"content":[]}}}
        })).expect("stored projection");
        let html_item = serde_json::from_value(json!({
            "position":1,"visibility":"local","sourceThreadId":"fork","sourceItemId":"html",
            "item":{"id":"html","threadId":"fork","runId":null,"nodeId":null,"providerThreadId":null,
            "providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"completed",
            "title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-11-14T12:01:00Z",
            "type":"dynamic_tool","toolName":"html_render","input":{},"output":{"htmlRender":{
                "attachmentId":"page","title":"Settings","height":420
            }}}
        })).expect("stored HTML projection");
        let input = TimelineEntriesInput {
            visible_turn_items: vec![Rc::new(projected_item), Rc::new(html_item)],
            ..Default::default()
        };
        let entries = derive_timeline_entries_from_visible_turn_items_with_state(input, None);
        let rows = derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: entries.entries.as_ref().clone(),
            ..Default::default()
        });
        let conversation = ThreadKey {
            environment: EnvironmentId("remote".into()),
            thread: ThreadId("fork".into()),
        };
        let row = rows
            .iter()
            .find_map(|row| McpAppRow::from_row(row, &conversation))
            .expect("actual client MCP row");
        assert_eq!(row.source.thread.0, "original");
        assert_eq!(row.source.environment.0, "remote");
        assert_eq!(row.conversation, conversation);
        assert_eq!(row.item_id.0, "call");
        assert_eq!(row.revision, "2026-11-14T12:00:00Z");
        assert_eq!(
            serde_json::to_value(row.resource).expect("asset")["renderIntent"],
            "mcp-app"
        );
        let html = rows
            .iter()
            .find_map(|row| HtmlRenderRow::from_row(row, &conversation))
            .expect("actual client HTML row");
        assert_eq!(html.thread, conversation);
        assert_eq!(html.reference.title, "Settings");
        let resource = serde_json::to_value(html.resource).expect("HTML asset");
        assert_eq!(resource["attachmentId"], "page");
        assert!(resource.get("renderIntent").is_none());
        let wrong = Rc::new(MessagesTimelineRow {
            id: "sent".into(),
            created_at: None,
            continues_work_log: None,
            content: MessagesTimelineRowContent::SentMessage { text: "x".into() },
        });
        assert!(McpAppRow::from_row(&wrong, &conversation).is_none());
        assert!(HtmlRenderRow::from_row(&wrong, &conversation).is_none());
    }
}
