//! Port of apps/web/src/components/ui/table.tsx Table and its row/cell slots.
//!
//! `table(label, column_min_widths, |layout| ...)` supplies one shared column layout. Compose
//! `table_header` and `table_body`, each holding `table_row`; pass the same layout and column
//! index to `table_head`/`table_cell`. NSH has flex rows, no HTML intrinsic table algorithm:
//! minima are caller layout data in points; surplus width is shared equally by all columns.
//! `RowOptions` carries source selected/descendant-expanded/last-body-row conditions. These
//! are appearance/semantics only: a table never invents selection, sorting or row actions.

use crate::i18n::Text;
use crate::theme::{Scope, cn, tw, tw_with};
use nsh_core::{AccessibilityState, prelude::*};
use std::rc::Rc;

#[derive(Clone)]
pub struct TableLayout {
    widths: Rc<Vec<f32>>,
}

impl TableLayout {
    pub fn column_count(&self) -> usize {
        self.widths.len()
    }
    pub fn minimum_width(&self) -> f32 {
        self.widths.iter().sum()
    }
    fn width(&self, column: usize) -> f32 {
        self.widths.get(column).copied().unwrap_or(0.0)
    }
}

pub fn table<E: IntoElement>(
    label: impl Into<Text>,
    column_min_widths: Vec<f32>,
    children: impl FnOnce(TableLayout) -> E,
) -> impl IntoElement {
    let label = label.into();
    let layout = TableLayout {
        widths: Rc::new(
            column_min_widths
                .into_iter()
                .map(|width| width.max(0.0))
                .collect(),
        ),
    };
    let width = layout.minimum_width();
    // overflow-x-auto becomes a native horizontal scroller; caption-bottom has no slot here.
    view().app_class(tw("relative w-full")).child(
        scroll().horizontal().app_class(tw("w-full")).child(
            nsh_base::table()
                .accessibility_label(move || Some(label.resolve()))
                .app_class(tw("w-full text-xs"))
                .min_w(px(width))
                .child(children(layout)),
        ),
    )
}

pub fn table_header<M>(children: impl nsh_core::element::IntoChild<M>) -> impl IntoElement {
    // The source's descendant border-b is already on table_row.
    nsh_base::table_header().child(children)
}

pub fn table_body<M>(children: impl nsh_core::element::IntoChild<M>) -> impl IntoElement {
    // last-child:border-0 is resolved by RowOptions::last_body_row, across dynamic children.
    nsh_base::table_body().child(children)
}

#[derive(Clone, Copy)]
pub struct RowOptions {
    pub selected: Prop<bool>,
    pub expanded: Prop<bool>,
    pub last_body_row: Prop<bool>,
}

impl Default for RowOptions {
    fn default() -> Self {
        Self {
            selected: false.into(),
            expanded: false.into(),
            last_body_row: false.into(),
        }
    }
}

pub fn table_row<M>(
    options: RowOptions,
    children: impl nsh_core::element::IntoChild<M>,
) -> impl IntoElement {
    // Explicit state replaces has-aria-expanded; the row itself is not an invented button.
    view()
        .role(AccessibilityRole::Row)
        .flex_row()
        .accessibility_state(move || AccessibilityState {
            selected: options.selected.get(),
            ..Default::default()
        })
        .app_class(move || {
            tw_with(
                Scope::Root,
                &cn([
                    Some("border-b transition-colors hover:bg-muted/50"),
                    options.expanded.get().then_some("bg-muted/50"),
                    options.last_body_row.get().then_some("border-0"),
                    Some("data-[state=selected]:bg-muted"),
                ]),
                if options.selected.get() {
                    &[("data-state", "selected")]
                } else {
                    &[]
                },
            )
        })
        .child(children)
}

pub fn table_head(
    layout: &TableLayout,
    column: usize,
    has_checkbox: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    // align-middle and nowrap map to flex centering and native no-wrap text inheritance.
    view()
        .role(AccessibilityRole::ColumnHeader)
        .w(px(layout.width(column)))
        .flex_grow(1.0)
        .flex_shrink(0.0)
        .justify_center()
        .app_class(tw(&cn([
            Some("h-10 px-2 text-left font-medium whitespace-nowrap text-foreground"),
            has_checkbox.then_some("pr-0"),
        ])))
        .child(children)
}

pub fn table_cell(
    layout: &TableLayout,
    column: usize,
    has_checkbox: bool,
    children: impl IntoElement,
) -> impl IntoElement {
    view()
        .role(AccessibilityRole::Cell)
        .w(px(layout.width(column)))
        .flex_grow(1.0)
        .flex_shrink(0.0)
        .justify_center()
        .app_class(tw(&cn([
            Some("p-2 whitespace-nowrap"),
            has_checkbox.then_some("pr-0"),
        ])))
        .child(children)
}
