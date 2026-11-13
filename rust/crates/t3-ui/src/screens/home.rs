//! `/`: the landing state beside the sidebar on wide layouts.

use nsh_core::prelude::*;

use crate::i18n::t;
use crate::theme::palette;
use crate::ui::surface::empty_state;

pub fn screen() -> AnyElement {
    view()
        .flex_1()
        .bg(move || palette().background)
        .child(empty_state(
            t("Select a thread"),
            t("Pick a thread in the sidebar, or start a new one."),
        ))
        .into_element()
}
