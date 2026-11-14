//! Port of apps/web/src/components/chat/MessagesTimeline.tsx ContextCompactionTimelineRow.
//!
//! Departures: NSH has no separator role, so the divider is static text carrying the web's
//! `aria-label`; the active label's ActivityShimmerOverlay (a continuous animation) is not drawn.

use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};

/// The client row's data: `{ kind: "context-compaction", label, tokenCounts, active, title?,
/// failed?, failure? }`. Token counts arrive formatted by the client ("899K").
#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionRow {
    pub label: String,
    pub token_counts: Option<(String, String)>,
    pub active: bool,
    pub title: Option<String>,
    pub failed: bool,
    pub failure: Option<String>,
}

const COMPACTED: &str = "Context compacted";

impl ContextCompactionRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::ContextCompaction {
            label,
            token_counts,
            active,
            title,
            failed,
            failure,
        } = &row.content
        else {
            return None;
        };
        let count = |key: &str| match &token_counts[key] {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        };
        Some(Self {
            label: label.clone(),
            token_counts: count("before").zip(count("after")),
            active: *active,
            title: title.clone(),
            failed: failed.unwrap_or(false),
            failure: failure.clone(),
        })
    }

    /// The web's `label`: a title replaces the "Context compacted" prefix of the entry's label.
    /// Labels and titles are English sentences the client returns for display (catalogue keys).
    pub fn display_label(&self) -> String {
        match &self.title {
            Some(title) if self.label.starts_with(COMPACTED) => format!(
                "{}{}",
                Text::client_key(title.clone()).resolve(),
                &self.label[COMPACTED.len()..]
            ),
            _ => Text::client_key(self.label.clone()).resolve().to_string(),
        }
    }

    /// The divider's `aria-label`.
    pub fn accessibility_label(&self) -> String {
        match &self.failure {
            Some(failure) => format!("{}: {failure}", self.display_label()),
            None => self.display_label(),
        }
    }
}

/// Port of MessagesTimeline.tsx ContextCompactionTimelineRow.
pub fn context_compaction_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
) -> impl IntoElement {
    let compaction = computed(move || {
        row.with(|row| {
            row.as_ref()
                .and_then(|row| ContextCompactionRow::from_row(&row.0))
        })
    });
    dynamic(move || {
        let row = compaction.get()?;
        Some(untracked(move || context_compaction(&row).into_element()))
    })
}

fn context_compaction(row: &ContextCompactionRow) -> impl IntoElement {
    let line = || {
        view().app_class(tw(&cn([
            Some("h-px flex-1"),
            Some(if row.failed {
                "bg-destructive/30"
            } else {
                "bg-border/70"
            }),
        ])))
    };
    let mut content = view()
        .app_class(tw("flex min-w-0 items-center gap-1.5"))
        .child(if row.failed {
            icon_with_class(Icon::CircleAlert, "size-3 shrink-0").into_element()
        } else {
            icon_with_class(Icon::Minimize2, "size-3").into_element()
        });
    content = match &row.token_counts {
        Some((before, after)) => {
            let title = row
                .title
                .as_ref()
                .map_or_else(
                    || t("Context compacted"),
                    |title| Text::client_key(title.clone()).resolve().to_string(),
                );
            content
                .child(text(format!("{title} {before}")))
                .child(icon_with_class(Icon::ArrowRight, "size-3"))
                .child(text(tr(
                    "{count} tokens",
                    &[("count", Value::Text(after.clone()))],
                )))
        }
        None => content.child(text(row.display_label())),
    };
    if let Some(failure) = &row.failure {
        content = content.child(
            text(format!("\u{b7} {failure}")).app_class(tw("min-w-0 truncate opacity-80")),
        );
    }
    view()
        .role(AccessibilityRole::Text)
        .label(row.accessibility_label())
        .test_id("context-compaction")
        .app_class(tw(&cn([
            Some("mx-auto flex w-full max-w-(--chat-content-max-width) items-center gap-3 py-1 text-xs"),
            Some(if row.failed {
                "text-destructive"
            } else {
                "text-muted-foreground"
            }),
        ])))
        .child(line())
        .child(
            view()
                .app_class(tw("relative min-w-0 overflow-hidden"))
                .child(content),
        )
        .child(line())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(value: serde_json::Value) -> ContextCompactionRow {
        let mut row = json!({"id":"c","createdAt":null,"kind":"context-compaction"});
        row.as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
        ContextCompactionRow::from_row(&serde_json::from_value(row).unwrap()).unwrap()
    }

    /// MessagesTimeline.browser.test.tsx: "renders automatic compaction as one live divider
    /// that finishes with its reason and token delta" (the divider's names).
    #[test]
    fn the_title_replaces_the_compacted_prefix_and_counts_stay_formatted() {
        let live = row(json!({"label":"Compacting before the cache expires…","tokenCounts":null,"active":true,
            "title":"Context automatically compacted before cache expiry"}));
        assert_eq!(live.accessibility_label(), "Compacting before the cache expires…");
        assert_eq!(live.token_counts, None);
        let done = row(json!({"label":"Context compacted 899K → 19K tokens","tokenCounts":{"before":"899K","after":"19K"},
            "active":false,"title":"Context automatically compacted before cache expiry"}));
        assert_eq!(
            done.accessibility_label(),
            "Context automatically compacted before cache expiry 899K → 19K tokens"
        );
        assert_eq!(done.token_counts, Some(("899K".into(), "19K".into())));
    }

    #[test]
    fn a_failure_is_named_after_the_label() {
        let failed = row(json!({"label":"Context compaction","tokenCounts":null,"active":false,
            "failed":true,"failure":"Provider refused"}));
        assert!(failed.failed);
        assert_eq!(failed.accessibility_label(), "Context compaction: Provider refused");
        let untitled = row(json!({"label":"Context compacted","tokenCounts":null,"active":false}));
        assert!(!untitled.failed);
        assert_eq!(untitled.accessibility_label(), "Context compacted");
    }
}
