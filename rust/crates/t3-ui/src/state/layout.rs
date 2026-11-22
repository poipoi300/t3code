//! How the one tree arranges itself: window width and pointer, never the platform name.

use nsh_core::prelude::*;

/// The web app's `sm` breakpoint.
pub const SM_BREAKPOINT: f32 = 640.0;
/// Where the sidebar stops being resizable and the right panel becomes a pane.
pub const WIDE_BREAKPOINT: f32 = 1024.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayoutClass {
    /// One column: the sidebar is the root page and a thread is pushed over it.
    Compact,
    /// Sidebar and main side by side; the right panel is a sheet.
    Medium,
    /// Sidebar, main and right panel side by side.
    Wide,
}

impl LayoutClass {
    pub fn of(width: f32) -> Self {
        if width < SM_BREAKPOINT {
            Self::Compact
        } else if width < WIDE_BREAKPOINT {
            Self::Medium
        } else {
            Self::Wide
        }
    }

    pub fn is_compact(self) -> bool {
        self == Self::Compact
    }
}

/// The layout class of the current window; reading it follows a resize.
pub fn layout_class() -> LayoutClass {
    LayoutClass::of(window().width.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Fine,
    Coarse,
}

/// Coarse on phones and tablets, fine elsewhere (a desktop browser included).
pub fn pointer() -> Pointer {
    match idiom() {
        nsh_core::platform::Idiom::Phone | nsh_core::platform::Idiom::Pad => Pointer::Coarse,
        _ => Pointer::Fine,
    }
}

/// Sidebar width on Medium and Wide layouts: the web app's 16rem default. The bounds are the
/// client's (`view_logic::shell::sidebar_width`, threadSidebarWidth.ts): at least 13rem, and at
/// most what leaves the main content 40rem.
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 256.0;
pub const SIDEBAR_MIN_WIDTH: f32 =
    t3_client::view_logic::shell::sidebar_width::THREAD_SIDEBAR_MIN_WIDTH as f32;

pub fn clamp_sidebar_width(width: f32, window_width: f32) -> f32 {
    use t3_client::view_logic::shell::sidebar_width::{
        THREAD_SIDEBAR_MIN_WIDTH, clamp_thread_sidebar_width, resolve_thread_sidebar_maximum_width,
    };
    let max =
        resolve_thread_sidebar_maximum_width(f64::from(window_width), THREAD_SIDEBAR_MIN_WIDTH);
    clamp_thread_sidebar_width(f64::from(width), THREAD_SIDEBAR_MIN_WIDTH, max) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_switch_exactly_at_the_breakpoints() {
        assert_eq!(LayoutClass::of(320.0), LayoutClass::Compact);
        assert_eq!(LayoutClass::of(639.0), LayoutClass::Compact);
        assert_eq!(LayoutClass::of(640.0), LayoutClass::Medium);
        assert_eq!(LayoutClass::of(1023.0), LayoutClass::Medium);
        assert_eq!(LayoutClass::of(1024.0), LayoutClass::Wide);
    }

    #[test]
    fn sidebar_width_leaves_the_main_content_its_minimum() {
        assert_eq!(clamp_sidebar_width(400.0, 700.0), SIDEBAR_MIN_WIDTH);
        assert_eq!(clamp_sidebar_width(100.0, 1400.0), SIDEBAR_MIN_WIDTH);
        assert_eq!(clamp_sidebar_width(900.0, 1400.0), 760.0);
        assert_eq!(clamp_sidebar_width(300.0, 1024.0), 300.0);
        assert_eq!(clamp_sidebar_width(300.0, 300.0), SIDEBAR_MIN_WIDTH);
    }
}
