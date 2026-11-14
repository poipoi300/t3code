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

/// Whether NSH will show its file picker (`pick_files`). NSH answers a pick it may not show
/// with no files, exactly like a cancel, so where `shells/nsh.json` does not grant `picker` the
/// composer hides "Attach files" instead of offering a control that does nothing. Logged once.
pub fn file_picker_granted() -> bool {
    static GRANTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *GRANTED.get_or_init(|| {
        let granted = picker_granted_by(NSH_MANIFEST);
        if !granted {
            log::warn!(
                "nsh.json does not grant the `picker` capability, so \"Attach files\" is hidden; \
                 add `\"picker\": true` to its `capabilities`"
            );
        }
        granted
    })
}

/// The manifest NSH's generated root compiles its capabilities from.
const NSH_MANIFEST: &str = include_str!("../../../shells/nsh.json");

/// [`file_picker_granted`] for the `nsh.json` text `manifest`, read as NSH reads it
/// (`Root::capabilities_json`): no `capabilities` grants everything; a block grants the picker
/// only with `"picker": true`, and one NSH cannot read grants nothing.
fn picker_granted_by(manifest: &str) -> bool {
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return false;
    };
    manifest.get("capabilities").is_none_or(|declared| {
        nsh_core::access::Capabilities::from_json_lenient(declared)
            .is_ok_and(|(capabilities, _)| capabilities.picker)
    })
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

/// Where the code tokenizer's syntaxes come from, matching NSH's loader boundary: native binaries
/// carry two-face's dump (built on a blocking thread); the web page supplies `nsh-syntaxes.bin`
/// (nsh-base/src/syntax.rs::load_async). `None` when they cannot be read.
#[cfg(not(target_arch = "wasm32"))]
pub async fn load_syntax_set() -> Option<syntect::parsing::SyntaxSet> {
    blocking(two_face::syntax::extra_newlines).await.ok()
}

#[cfg(target_arch = "wasm32")]
pub async fn load_syntax_set() -> Option<syntect::parsing::SyntaxSet> {
    next_frame().await;
    let bytes = nsh_core::runtime_data::load("syntaxes").await?;
    syntect::dumps::from_uncompressed_data(&bytes)
        .inspect_err(|error| log::warn!("Code syntaxes could not be loaded: {error}"))
        .ok()
}

/// The code highlighter the root provides to NSH's TextView, if it replaces NSH's own. NSH's
/// built-in highlighter answers with a `dynamic` region inside the code's text node, which the
/// native platforms do not paint (ui-plan.md, NSH finding 10; reports/ui-mounts-app-root.md):
/// native code is plain until NSH paints it. The web keeps NSH's highlighter.
pub fn code_highlighter() -> Option<nsh_base::text_view::CodeHighlighter> {
    #[cfg(not(target_arch = "wasm32"))]
    return Some(nsh_base::text_view::CodeHighlighter::new(|_| Vec::new()));
    #[cfg(target_arch = "wasm32")]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_counts_as_granted_only_as_nsh_grants_it() {
        for (manifest, granted) in [
            (r#"{"name":"T3"}"#, true),
            (r#"{"capabilities":{"picker":true,"clipboard":true}}"#, true),
            (r#"{"capabilities":{"clipboard":true}}"#, false),
            (r#"{"capabilities":{"picker":false}}"#, false),
            (r#"{"capabilities":5}"#, false),
            ("not json", false),
        ] {
            assert_eq!(picker_granted_by(manifest), granted, "{manifest}");
        }
    }

    #[test]
    fn the_shipped_manifest_lets_the_composer_attach_files() {
        // A `capabilities` block without `"picker": true` would make every pick a silent cancel.
        assert!(picker_granted_by(NSH_MANIFEST), "add \"picker\": true to shells/nsh.json");
    }
}
