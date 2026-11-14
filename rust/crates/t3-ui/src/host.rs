//! What the window host decides per platform. Feature code never reads the OS: it asks here for
//! the capability (ui-package-addendum.md, "Platform facts"). This module and `runtime/client.rs`
//! are the only places in the crate that know which platform they run on.

use nsh_core::desktop::{TitleBarOptions, TitleBarStyle};
use nsh_core::geometry::point;
use nsh_core::keymap::Keystroke;
use nsh_core::platform::Os;
use nsh_core::prelude::*;

/// The title bar this window asks NSH for: `desktop` is whether it is a desktop window. None
/// elsewhere (web and mobile draw no native bar).
pub fn desktop_title_bar(desktop: bool) -> Option<TitleBarOptions> {
    desktop.then(|| desktop_title_bar_for(os()))
}

/// Port of apps/desktop/src/window/DesktopWindow.ts getWindowTitleBarOptions: darwin's
/// hiddenInset bar with its traffic-light position (NSH names it Hidden), a 40px bar with
/// caption buttons elsewhere.
pub fn desktop_title_bar_for(platform: Os) -> TitleBarOptions {
    let mut options = TitleBarOptions::default();
    options.style = TitleBarStyle::Hidden;
    if platform == Os::MacOs {
        options.height = Some(px(52.0));
        options.controls_offset = Some(point(px(16.0), px(20.0)));
    } else {
        options.height = Some(px(40.0));
    }
    options
}

/// Whether the platform keeps `stroke` for itself (quitting, hiding, minimizing, full screen),
/// so it cannot be bound.
pub fn reserved_stroke(stroke: &Keystroke) -> bool {
    stroke.reserved(os()).is_some()
}

/// `isMacPlatform(navigator.platform)`: settings search lists Mac-only rows here.
pub fn is_mac() -> bool {
    os() == Os::MacOs
}

/// `isWindowsPlatform(navigator.platform)`: settings search lists Windows-only rows here.
pub fn is_windows() -> bool {
    os() == Os::Windows
}

/// HistorySwipeNavigation's gate (`isElectron` and `isMacPlatform`): only the macOS desktop app
/// turns two-finger trackpad swipes into wheel events a mouse's tilt wheel cannot be mistaken
/// for. The web app, Windows, Linux and mobile answer false.
pub fn trackpad_history_swipe() -> bool {
    os() == Os::MacOs
}
