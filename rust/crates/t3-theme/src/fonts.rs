//! Fonts: default stacks, how the user's font settings resolve into CSS variables, and the
//! availability and monospace checks the font pickers use. A port of
//! `apps/web/src/appearanceFonts.ts` with the browser probes behind [`FontProbe`].

use serde::{Deserialize, Serialize};

/// `DEFAULT_SANS_FONT_STACK`; `index.css` declares the same list for `--font-sans`.
pub const DEFAULT_SANS_FONT_STACK: &str =
    "-apple-system, BlinkMacSystemFont, \"Inter Variable\", \"Segoe UI\", system-ui, sans-serif";

/// `DEFAULT_CODE_FONT_STACK`. Some engines map `ui-monospace` to a proportional font, so
/// the runtime stack prefers concrete names (`index.css` still lists `ui-monospace` first).
pub const DEFAULT_CODE_FONT_STACK: &str =
    "\"SF Mono\", \"SFMono-Regular\", Menlo, Consolas, \"Liberation Mono\", monospace";

pub const TYPOGRAPHY_ADVANCED_STORAGE_KEY: &str = "t3code:typography-advanced";

/// Limits and defaults from `packages/contracts/src/settings.ts`.
pub const MIN_INTERFACE_FONT_SIZE: u32 = 12;
pub const MAX_INTERFACE_FONT_SIZE: u32 = 20;
pub const DEFAULT_INTERFACE_FONT_SIZE: u32 = 16;
pub const MIN_PROMPT_FONT_SIZE: u32 = 12;
pub const MAX_PROMPT_FONT_SIZE: u32 = 20;
pub const DEFAULT_PROMPT_FONT_SIZE: u32 = 14;
pub const MIN_CODE_FONT_SIZE: u32 = 10;
pub const MAX_CODE_FONT_SIZE: u32 = 18;
pub const DEFAULT_CODE_FONT_SIZE: u32 = 13;
pub const MIN_TERMINAL_FONT_SIZE: u32 = 8;
pub const MAX_TERMINAL_FONT_SIZE: u32 = 20;
pub const DEFAULT_TERMINAL_FONT_SIZE: u32 = 12;

/// `resolveTerminalFontPreference`.
pub fn resolve_terminal_font_preference<'a>(
    advanced: bool,
    code: &'a str,
    terminal: &'a str,
) -> &'a str {
    if advanced { terminal } else { code }
}

/// `resolveTerminalFontSizePreference`.
pub fn resolve_terminal_font_size_preference(advanced: bool, code: u32, terminal: u32) -> u32 {
    if advanced { terminal } else { code }
}

fn is_quoted(name: &str) -> bool {
    let mut chars = name.chars();
    match (chars.next(), name.chars().last()) {
        (Some(open @ ('"' | '\'')), Some(close)) => name.len() >= 2 && open == close,
        _ => false,
    }
}

fn quote_font_family_name(name: &str) -> String {
    let bare = name.trim();
    if bare.is_empty() {
        return String::new();
    }
    if is_quoted(bare) {
        return bare.to_owned();
    }
    let mut chars = bare.chars();
    let identifier = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-');
    if identifier {
        return bare.to_owned();
    }
    format!("\"{}\"", bare.replace('"', ""))
}

/// `cssFontFamilies`: the comma list as CSS, quoting names that need it; `None` when empty.
pub fn css_font_families(input: &str) -> Option<String> {
    let families: Vec<String> = input
        .split(',')
        .map(quote_font_family_name)
        .filter(|n| !n.is_empty())
        .collect();
    (!families.is_empty()).then(|| families.join(", "))
}

/// `appearanceFontStack`: the person's families first, then the default stack.
pub fn appearance_font_stack(custom: &str, default_stack: &str) -> String {
    match css_font_families(custom) {
        Some(families) => format!("{families}, {default_stack}"),
        None => default_stack.to_owned(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppearanceFontPreferences {
    pub sans: String,
    pub code: String,
    pub composer: String,
    pub size_interface: f64,
    pub size_prompt: f64,
    pub size_code: f64,
    pub smoothing: bool,
    /// Whether the composer has its own font settings.
    pub separate: bool,
}

fn clamp_font_size(value: f64, minimum: u32, maximum: u32, fallback: u32) -> u32 {
    if !value.is_finite() {
        return fallback;
    }
    (value + 0.5)
        .floor()
        .clamp(f64::from(minimum), f64::from(maximum)) as u32
}

pub fn clamp_interface_font_size(value: f64) -> u32 {
    clamp_font_size(
        value,
        MIN_INTERFACE_FONT_SIZE,
        MAX_INTERFACE_FONT_SIZE,
        DEFAULT_INTERFACE_FONT_SIZE,
    )
}

pub fn clamp_prompt_font_size(value: f64) -> u32 {
    clamp_font_size(
        value,
        MIN_PROMPT_FONT_SIZE,
        MAX_PROMPT_FONT_SIZE,
        DEFAULT_PROMPT_FONT_SIZE,
    )
}

pub fn clamp_code_font_size(value: f64) -> u32 {
    clamp_font_size(
        value,
        MIN_CODE_FONT_SIZE,
        MAX_CODE_FONT_SIZE,
        DEFAULT_CODE_FONT_SIZE,
    )
}

/// The prompt font size variable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptFontSize {
    /// `var(--text-sm)`: the composer follows the interface size.
    FollowsInterface,
    Px(u32),
}

/// The inline style `applyAppearanceFontVariables` writes on `<html>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontVariables {
    /// `--font-sans`; `None` removes the override so the stylesheet default applies.
    pub font_sans: Option<String>,
    pub font_mono: Option<String>,
    pub font_composer: Option<String>,
    /// `--list-marker-sans` in `em`, from a measured `0. ` marker; `None` removes it.
    pub list_marker_sans_em: Option<String>,
    /// The root `font-size`, which scales every `rem`.
    pub root_font_size_px: u32,
    pub font_size_prompt: PromptFontSize,
    /// `--font-size-code` and `--diffs-font-size`.
    pub code_font_size_px: u32,
    /// `-webkit-font-smoothing: antialiased` when set.
    pub antialiased: bool,
}

/// Width of one em in the probe font, used to size the numbered-list gutter.
pub const LIST_MARKER_PROBE_TEXT: &str = "0. ";
pub const PROBE_FONT_SIZE: f64 = 16.0;

/// `applyAppearanceFontVariables`. `marker_width_px` is the measured width of
/// [`LIST_MARKER_PROBE_TEXT`] at [`PROBE_FONT_SIZE`] in the person's sans stack.
pub fn font_variables(
    preferences: &AppearanceFontPreferences,
    marker_width_px: Option<f64>,
) -> FontVariables {
    let family = |custom: &str, fallback: &str| {
        css_font_families(custom).map(|list| format!("{list}, {fallback}"))
    };
    FontVariables {
        font_sans: family(&preferences.sans, DEFAULT_SANS_FONT_STACK),
        font_mono: family(&preferences.code, DEFAULT_CODE_FONT_STACK),
        font_composer: family(
            if preferences.separate {
                &preferences.composer
            } else {
                ""
            },
            "var(--font-sans)",
        ),
        list_marker_sans_em: marker_width_px
            .map(|width| format!("{:.3}em", width / PROBE_FONT_SIZE)),
        root_font_size_px: clamp_interface_font_size(preferences.size_interface),
        font_size_prompt: if preferences.separate {
            PromptFontSize::Px(clamp_prompt_font_size(preferences.size_prompt))
        } else {
            PromptFontSize::FollowsInterface
        },
        code_font_size_px: clamp_code_font_size(preferences.size_code),
        antialiased: preferences.smoothing,
    }
}

/// The browser measurements the font checks need.
pub trait FontProbe {
    /// Width of `text` in a canvas 2D context whose `font` is `font` (for example `16px Menlo, monospace`).
    fn canvas_text_width(&self, font: &str, text: &str) -> Option<f64>;
    /// Width of a hidden 100px-tall span with `font-family: family`, measured in the DOM.
    fn dom_text_width(&self, family: &str) -> Option<f64>;
    /// `navigator.platform` looks like an Apple device.
    fn is_apple_platform(&self) -> bool;
}

const FONT_PROBE_TEXT: &str = "mmmmmmmmMMWli1O0@# fjord";

fn probe_width(probe: &dyn FontProbe, font_list: &str, text: &str) -> Option<f64> {
    probe.canvas_text_width(&format!("{PROBE_FONT_SIZE}px {font_list}"), text)
}

fn is_generic_family(families: &str) -> bool {
    matches!(
        families.to_ascii_lowercase().as_str(),
        "system-ui" | "sans-serif" | "serif" | "monospace" | "ui-monospace"
    )
}

/// `isFontFamilyAvailable`: a family is installed when naming it changes the measured width
/// against every generic fallback (`document.fonts.check()` accepts uninstalled families).
pub fn is_font_family_available(probe: &dyn FontProbe, family: &str) -> bool {
    let Some(families) = css_font_families(family) else {
        return false;
    };
    if is_generic_family(&families) {
        return true;
    }
    for generic in ["monospace", "serif", "sans-serif"] {
        let (Some(baseline), Some(candidate)) = (
            probe_width(probe, generic, FONT_PROBE_TEXT),
            probe_width(probe, &format!("{families}, {generic}"), FONT_PROBE_TEXT),
        ) else {
            return false;
        };
        if candidate != baseline {
            return true;
        }
    }
    false
}

const MONOSPACE_PROBE_VARIANTS: [&str; 4] =
    ["normal 400", "normal 700", "italic 400", "italic 700"];
const MONOSPACE_PROBE_GLYPHS: [&str; 8] = ["i", "M", "W", "0", "@", "#", ".", " "];
const MONOSPACE_ADVANCE_TOLERANCE: f64 = 0.01;

/// `areFontAdvancesMonospace`.
pub fn are_font_advances_monospace(advances: &[f64]) -> bool {
    let Some(reference) = advances.first().copied() else {
        return true;
    };
    if reference <= 0.0 || advances.iter().any(|a| !a.is_finite() || *a <= 0.0) {
        return true;
    }
    advances
        .iter()
        .all(|a| (a - reference).abs() < MONOSPACE_ADVANCE_TOLERANCE)
}

/// `isMonospaceFamily`: unmeasurable fonts count as monospace so they are never hidden.
pub fn is_monospace_family(probe: &dyn FontProbe, family: &str) -> bool {
    let Some(families) = css_font_families(family) else {
        return true;
    };
    for variant in MONOSPACE_PROBE_VARIANTS {
        let font = format!("{variant} 32px {families}, monospace");
        let advances: Option<Vec<f64>> = MONOSPACE_PROBE_GLYPHS
            .iter()
            .map(|glyph| probe.canvas_text_width(&font, glyph))
            .collect();
        let Some(advances) = advances else {
            return true;
        };
        if !are_font_advances_monospace(&advances) {
            return false;
        }
    }
    true
}

const SANS_GENERIC_CANDIDATES: [&str; 9] = [
    "Segoe UI",
    "Roboto",
    "Noto Sans",
    "Ubuntu",
    "Cantarell",
    "DejaVu Sans",
    "Liberation Sans",
    "Helvetica Neue",
    "Arial",
];
const MONO_GENERIC_CANDIDATES: [&str; 10] = [
    "Menlo",
    "Consolas",
    "Cascadia Mono",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Roboto Mono",
    "Monaco",
    "Courier New",
];

fn widths_match(left: f64, right: f64) -> bool {
    (left - right).abs() < 0.01
}

fn resolve_generic_family_label(probe: &dyn FontProbe, generic: &str) -> Option<String> {
    let lower = generic.to_ascii_lowercase();
    if lower == "serif" {
        return None;
    }
    let mono_like = lower == "ui-monospace" || lower == "monospace";
    let generic_width = probe.dom_text_width(generic)?;
    let candidates: &[&str] = if mono_like {
        &MONO_GENERIC_CANDIDATES
    } else {
        &SANS_GENERIC_CANDIDATES
    };
    for candidate in candidates {
        if !is_font_family_available(probe, candidate) {
            continue;
        }
        if probe
            .dom_text_width(&format!("\"{candidate}\""))
            .is_some_and(|w| widths_match(generic_width, w))
        {
            return Some((*candidate).to_owned());
        }
    }
    // Unnameable Apple faces need -apple-system metrics to tell the UI face from SF Mono.
    if probe.is_apple_platform() {
        if probe
            .dom_text_width("-apple-system")
            .is_some_and(|w| widths_match(generic_width, w))
        {
            return Some("SF Pro".to_owned());
        }
        return Some(if mono_like { "SF Mono" } else { "SF Pro" }.to_owned());
    }
    None
}

fn strip_quotes(family: &str) -> &str {
    if is_quoted(family) {
        &family[1..family.len() - 1]
    } else {
        family
    }
}

/// `resolveDefaultFamilyLabel`: the name to show for what a default stack falls back to.
pub fn resolve_default_family_label(probe: &dyn FontProbe, stack: &str) -> Option<String> {
    for raw in stack.split(',') {
        let family = strip_quotes(raw.trim());
        if family.is_empty() {
            continue;
        }
        let lower = family.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "system-ui"
                | "sans-serif"
                | "serif"
                | "monospace"
                | "ui-monospace"
                | "-apple-system"
                | "blinkmacsystemfont"
        ) {
            if let Some(resolved) = resolve_generic_family_label(probe, family) {
                return Some(resolved);
            }
            continue;
        }
        if is_font_family_available(probe, family) {
            return Some(family.to_owned());
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstalledFontsStatus {
    Granted,
    Denied,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFontFamilies {
    pub families: Vec<String>,
    pub status: InstalledFontsStatus,
}

/// What `queryInstalledFontFamilies` makes of the list the browser returns: unique families,
/// without the dot-prefixed macOS UI faces that refuse to render in web content, sorted
/// case-insensitively. An empty list means the permission was denied.
pub fn installed_font_families(reported: &[String]) -> InstalledFontFamilies {
    let mut families: Vec<String> = Vec::new();
    for name in reported {
        if !name.starts_with('.') && !families.contains(name) {
            families.push(name.clone());
        }
    }
    families.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    if families.is_empty() {
        InstalledFontFamilies {
            families,
            status: InstalledFontsStatus::Denied,
        }
    } else {
        InstalledFontFamilies {
            families,
            status: InstalledFontsStatus::Granted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_quoted_only_when_needed() {
        assert_eq!(
            css_font_families("Inter, Fira Code, 'Cascadia Mono', ").as_deref(),
            Some("Inter, \"Fira Code\", 'Cascadia Mono'")
        );
        assert_eq!(css_font_families(" , ").as_deref(), None);
        assert_eq!(
            css_font_families("My \"Font\"").as_deref(),
            Some("\"My Font\"")
        );
        assert_eq!(
            appearance_font_stack("", DEFAULT_SANS_FONT_STACK),
            DEFAULT_SANS_FONT_STACK
        );
        assert_eq!(
            appearance_font_stack("Inter", "monospace"),
            "Inter, monospace"
        );
    }

    #[test]
    fn composer_size_follows_the_interface_until_fonts_are_separate() {
        let preferences = AppearanceFontPreferences {
            sans: String::new(),
            code: String::new(),
            composer: "Fira Code".into(),
            size_interface: 20.0,
            size_prompt: 14.0,
            size_code: 13.0,
            smoothing: true,
            separate: false,
        };
        let variables = font_variables(&preferences, None);
        assert_eq!(variables.root_font_size_px, 20);
        assert_eq!(variables.font_size_prompt, PromptFontSize::FollowsInterface);
        assert_eq!(variables.font_composer, None);
        let separate = font_variables(
            &AppearanceFontPreferences {
                separate: true,
                ..preferences
            },
            Some(25.0),
        );
        assert_eq!(separate.font_size_prompt, PromptFontSize::Px(14));
        assert_eq!(
            separate.font_composer.as_deref(),
            Some("\"Fira Code\", var(--font-sans)")
        );
        assert_eq!(separate.list_marker_sans_em.as_deref(), Some("1.562em"));
    }

    #[test]
    fn font_sizes_are_rounded_and_clamped() {
        assert_eq!(clamp_interface_font_size(25.0), 20);
        assert_eq!(clamp_interface_font_size(f64::NAN), 16);
        assert_eq!(clamp_code_font_size(12.5), 13);
        assert_eq!(clamp_prompt_font_size(3.0), 12);
    }

    #[test]
    fn monospace_needs_equal_advances() {
        assert!(are_font_advances_monospace(&[8.0, 8.0, 8.004]));
        assert!(!are_font_advances_monospace(&[8.0, 12.0]));
        assert!(are_font_advances_monospace(&[]));
        assert!(are_font_advances_monospace(&[0.0, 5.0]));
    }

    struct Probe;
    impl FontProbe for Probe {
        fn canvas_text_width(&self, font: &str, text: &str) -> Option<f64> {
            // Installed: Menlo (monospace). Everything else falls back to the generic width.
            let per_char = if font.contains("Menlo") || font.contains("monospace") {
                9.6
            } else {
                7.0
            };
            Some(per_char * text.chars().count() as f64)
        }
        fn dom_text_width(&self, _family: &str) -> Option<f64> {
            None
        }
        fn is_apple_platform(&self) -> bool {
            false
        }
    }

    #[test]
    fn installed_families_are_unique_sorted_and_visible() {
        let result = installed_font_families(&[
            "zeta".into(),
            ".SF".into(),
            "Alpha".into(),
            "alpha".into(),
            "Alpha".into(),
        ]);
        assert_eq!(result.families, ["Alpha", "alpha", "zeta"]);
        assert_eq!(result.status, InstalledFontsStatus::Granted);
        assert_eq!(
            installed_font_families(&[]).status,
            InstalledFontsStatus::Denied
        );
        let _ = Probe.is_apple_platform();
    }
}
