//! MISC-NOT-FOUND: Not found. Not built yet; see ui-plan.md and ui-package.md.

use nsh_core::prelude::*;

use crate::ui::placeholder::not_built;

pub fn screen(path: &str) -> AnyElement {
    let _ = path;
    not_built("Not found")
}
