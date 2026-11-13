//! MISC-NEW: New thread and drafts. Not built yet; see ui-plan.md and ui-package.md.

use nsh_core::prelude::*;

use crate::ui::placeholder::not_built;

pub fn screen(draft: Option<&str>) -> AnyElement {
    let _ = draft;
    not_built("New thread and drafts")
}
