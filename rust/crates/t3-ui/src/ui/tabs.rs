//! Port of apps/web/src/components/ui/panel-tab-close-button.tsx PanelTabCloseButton.
//!
//! `panel_tab_close_button(children, label, on_click, tooltip)` keeps the surface-icon slot and
//! its close-glyph swap on `group/tab` hover or close-button keyboard focus. Labels and tooltip
//! text are already translated. `tabs` / `tab` supply NSH's tab-list focus and controlled
//! selection behavior; there is no web ui/tabs.tsx or cva variant/size to invent. Their content
//! slots carry the feature's layout, and `tab` establishes the web's `group/tab` for closing.

use nsh_core::prelude::*;

use crate::theme::{palette, tw};
use crate::ui::icon::Icon;
use crate::ui::tooltip::{tooltip as hint, tooltip_create_handle};

/// A semantic tab list. The feature supplies its tab content, ordering and layout.
pub fn tabs(label: String, children: impl IntoElement) -> impl IntoElement {
    nsh_base::tabs()
        .accessibility_label(Some(SharedString::from(label)))
        .child(children)
}

/// A controlled tab; arrows select through NSH's tab-list keyboard behavior.
pub fn tab(
    label: String,
    selected: impl Fn() -> bool + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let disabled = std::rc::Rc::new(disabled);
    let disabled_prop = disabled.clone();
    nsh_base::tab()
        .accessibility_label(Some(SharedString::from(label)))
        .selected(selected)
        .disabled(move || disabled_prop())
        .on_press(move || {
            if !disabled() {
                on_press();
            }
        })
        .app_class(tw("group/tab"))
        .child(children)
}

/// The children slot is the surface icon (and optional pending badge), exactly as in the TSX.
pub fn panel_tab_close_button(
    children: impl IntoElement,
    label: String,
    on_click: impl Fn() + 'static,
    tooltip: Option<String>,
) -> impl IntoElement {
    let button = nsh_base::button()
        .accessibility_label(Some(SharedString::from(label)))
        .on_press(on_click)
        .app_class(tw("cursor-pointer group/close relative flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-muted"))
        .child(view()
            .app_class(tw("relative flex size-3 items-center justify-center group-hover/tab:hidden group-focus-visible/close:hidden"))
            .child(children))
        .child(image(Icon::X.asset())
            .app_class(tw("hidden size-3 group-hover/tab:block group-focus-visible/close:block"))
            .tint_color(move || palette().foreground)
            .accessible(false));
    match tooltip.filter(|text| !text.is_empty()) {
        Some(tooltip) => button
            // NSH component builders attach the hint as a child; only host views expose tooltip.
            .child(hint(
                tooltip_create_handle(),
                Default::default(),
                text(SharedString::from(tooltip)),
            ))
            .into_element(),
        None => button.into_element(),
    }
}
