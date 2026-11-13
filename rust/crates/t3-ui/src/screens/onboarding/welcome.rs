//! ONB-1: Welcome and first run. Not built yet; see ui-plan.md and ui-package.md.

use nsh_core::prelude::*;

use crate::ui::placeholder::not_built;

pub fn screen() -> AnyElement {
    not_built("Welcome and first run")
}
