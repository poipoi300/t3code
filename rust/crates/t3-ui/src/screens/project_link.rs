//! CONN-5: Project link entry. Not built yet; see ui-plan.md and ui-package.md.

use nsh_core::prelude::*;

use crate::ui::placeholder::not_built;

pub fn screen(key: &str) -> AnyElement {
    let _ = key;
    not_built("Project link entry")
}
