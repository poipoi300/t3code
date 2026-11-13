//! Panel visibility and sizes for the workspace shell.

use nsh_core::prelude::*;

use super::layout::SIDEBAR_DEFAULT_WIDTH;

#[derive(Clone, Copy)]
pub struct Panels {
    /// Medium and Wide: whether the sidebar shows. Compact ignores it (the list is a page).
    pub sidebar_open: Signal<bool>,
    pub sidebar_width: Signal<f32>,
}

impl Panels {
    pub fn new() -> Self {
        Self {
            sidebar_open: signal(true),
            sidebar_width: signal(SIDEBAR_DEFAULT_WIDTH),
        }
    }
}

impl Default for Panels {
    fn default() -> Self {
        Self::new()
    }
}

pub fn panels() -> Panels {
    use_context::<Panels>().expect("App provides the panels")
}
