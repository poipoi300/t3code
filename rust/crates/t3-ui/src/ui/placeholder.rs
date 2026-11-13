//! What a route shows until its package lands: an honest "not built yet", never a fake screen.

use nsh_core::prelude::*;

use super::surface::empty_state;
use crate::i18n::t;

pub fn not_built(name: &'static str) -> AnyElement {
    empty_state(
        t(name),
        t("This screen is not built yet in the Rust client."),
    )
    .into_element()
}
