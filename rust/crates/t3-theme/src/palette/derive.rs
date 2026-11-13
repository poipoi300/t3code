//! Deriving a full role set from a few inputs: the guided generator, single-role
//! edits that keep dependent roles readable, and the per-theme context chip solve.

use super::definition::{Appearance, ThemeColors, context_chip_accents, default_theme_colors};
use super::math::{
    BLACK, Direction, LIGHT_FOREGROUND, Oklch, Rgb, WHITE, contrast_ratio, format_oklch, mix,
    oklch_to_rgb, oklch_to_theme_color, parse_rgb, parse_theme_color, readable_foreground,
    readable_text, relative_luminance, rgb_to_hex, rgb_to_oklch, rgb_to_theme_color,
    solve_oklch_lightness, standard_muted_text,
};
use super::roles::ThemeColorRole as R;

/// Black and white text have equal contrast at this relative luminance.
const DARK_LUMINANCE_THRESHOLD: f64 = 0.179;

fn is_dark(color: Rgb) -> bool {
    relative_luminance(color) < DARK_LUMINANCE_THRESHOLD
}

const CONTEXT_CHIP_CONTRAST: f64 = 5.0;
pub const CONTEXT_CHIP_MESSAGE_VARIABLE: &str = "--context-chip-message";
pub const CONTEXT_CHIP_INVALID_VARIABLE: &str = "--context-chip-invalid";

/// `contextChipVariables`: for every chip kind, the lightness-solved colour (as
/// `#rrggbb`) that reads at 5:1 on every surface a chip can sit on.
pub fn context_chip_variables(colors: &ThemeColors) -> Vec<(String, String)> {
    let canvas =
        parse_theme_color(colors.get(R::Canvas)).map_or(WHITE, |p| oklch_to_rgb(p.color, false));
    let on_canvas = |value: &str| match parse_theme_color(value) {
        Some(parsed) => mix(canvas, oklch_to_rgb(parsed.color, false), parsed.alpha),
        None => canvas,
    };
    let message_matches_canvas =
        is_dark(on_canvas(colors.get(R::MessageSurface))) == is_dark(canvas);
    let mut surfaces = vec![
        canvas,
        on_canvas(colors.get(R::Surface)),
        on_canvas(colors.get(R::SurfaceRaised)),
    ];
    if message_matches_canvas {
        surfaces.push(on_canvas(colors.get(R::MessageSurface)));
    }
    let contrast_on_surfaces = |color: Rgb| {
        surfaces
            .iter()
            .map(|surface| contrast_ratio(color, *surface))
            .fold(f64::INFINITY, f64::min)
    };
    let black_contrast = contrast_on_surfaces(BLACK);
    let white_contrast = contrast_on_surfaces(WHITE);
    let direction = if black_contrast >= white_contrast {
        Direction::Darker
    } else {
        Direction::Lighter
    };
    let min_contrast = CONTEXT_CHIP_CONTRAST.min(black_contrast.max(white_contrast));
    let mut accents: Vec<(String, String)> = context_chip_accents()
        .map(|(_, accent, variable)| (variable.to_owned(), accent.to_owned()))
        .collect();
    accents.push((
        CONTEXT_CHIP_INVALID_VARIABLE.to_owned(),
        colors.get(R::ErrorForeground).to_owned(),
    ));
    let mut variables: Vec<(String, String)> = accents
        .into_iter()
        .filter_map(|(variable, accent)| {
            let parsed = parse_theme_color(&accent)?;
            let solved = solve_oklch_lightness(parsed.color, &surfaces, min_contrast, direction);
            Some((variable, rgb_to_hex(oklch_to_rgb(solved, true))))
        })
        .collect();
    if !message_matches_canvas {
        variables.push((
            CONTEXT_CHIP_MESSAGE_VARIABLE.to_owned(),
            colors.get(R::MessageForeground).to_owned(),
        ));
    }
    variables
}

struct StatusColors {
    error: String,
    error_foreground: String,
    error_surface: String,
    warning: String,
    warning_foreground: String,
    warning_surface: String,
}

struct StandardStatus {
    error: &'static str,
    error_foreground: &'static str,
    warning: &'static str,
    warning_foreground: &'static str,
}

const STANDARD_LIGHT_STATUS: StandardStatus = StandardStatus {
    error: "#fb2c36",
    error_foreground: "#c10007",
    warning: "#fe9a00",
    warning_foreground: "#bb4d00",
};
const STANDARD_DARK_STATUS: StandardStatus = StandardStatus {
    error: "#fb414a",
    error_foreground: "#ff6467",
    warning: "#fe9a00",
    warning_foreground: "#ffb900",
};

fn canonical_literal(value: &str) -> String {
    super::math::to_canonical_theme_color(value).unwrap_or_else(|| value.to_owned())
}

fn standard_status_colors(canvas: Rgb) -> StatusColors {
    let appearance = if is_dark(canvas) {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    let standard = match appearance {
        Appearance::Dark => &STANDARD_DARK_STATUS,
        Appearance::Light => &STANDARD_LIGHT_STATUS,
    };
    let surface_mix = if appearance == Appearance::Dark {
        0.16
    } else {
        0.08
    };
    let surface_of = |value: &str| mix(canvas, parse_rgb(value, canvas), surface_mix);
    let readable_on = |foreground: &str, surface: Rgb| {
        let color = rgb_to_oklch(parse_rgb(foreground, canvas));
        let direction = if appearance == Appearance::Dark {
            Direction::Lighter
        } else {
            Direction::Darker
        };
        oklch_to_theme_color(solve_oklch_lightness(color, &[surface], 4.6, direction))
    };
    let error_surface = surface_of(standard.error);
    let warning_surface = surface_of(standard.warning);
    StatusColors {
        error: canonical_literal(standard.error),
        error_foreground: readable_on(standard.error_foreground, error_surface),
        error_surface: rgb_to_theme_color(error_surface),
        warning: canonical_literal(standard.warning),
        warning_foreground: readable_on(standard.warning_foreground, warning_surface),
        warning_surface: rgb_to_theme_color(warning_surface),
    }
}

fn fallback_canvas(appearance: Appearance) -> Rgb {
    match appearance {
        Appearance::Dark => Rgb::new(24.0, 15.0, 27.0),
        Appearance::Light => Rgb::new(250.0, 245.0, 250.0),
    }
}

const FALLBACK_ACCENT: Rgb = Rgb::new(168.0, 67.0, 112.0);

/// `createVividThemeColors`: a whole palette from a canvas colour and an accent.
pub fn create_vivid_theme_colors(
    appearance: Appearance,
    background: &str,
    accent_value: &str,
) -> ThemeColors {
    let defaults = default_theme_colors(appearance).clone();
    let canvas_rgb = parse_rgb(background, fallback_canvas(appearance));
    let accent_rgb = parse_rgb(accent_value, FALLBACK_ACCENT);
    let canvas = rgb_to_oklch(canvas_rgb);
    let accent = rgb_to_oklch(accent_rgb);
    let dark = relative_luminance(canvas_rgb) < DARK_LUMINANCE_THRESHOLD;
    let hue = if accent.c < 0.02 { canvas.h } else { accent.h };
    let tint_c = 0.045_f64.min(0.008_f64.max(accent.c * 0.22));
    let step = if dark { 1.0 } else { -1.0 };
    let pick = |d: f64, l: f64| if dark { d } else { l };

    let surface_at = |delta_l: f64, chroma: f64| Oklch {
        l: 0.98_f64.min(0.05_f64.max(canvas.l + step * delta_l)),
        c: chroma,
        h: hue,
    };
    let color_of = oklch_to_theme_color;

    let text_base = Oklch {
        l: pick(0.95, 0.2),
        c: 0.035_f64.min(accent.c * 0.25),
        h: hue,
    };
    let text_direction = if dark {
        Direction::Lighter
    } else {
        Direction::Darker
    };
    let text = solve_oklch_lightness(text_base, &[canvas_rgb], 7.0, text_direction);
    let text_rgb = oklch_to_rgb(text, true);
    let text_muted_rgb = standard_muted_text(canvas_rgb, text_rgb);

    let action = Oklch {
        l: 0.85_f64.min(0.35_f64.max(accent.l + pick(0.06, -0.02))),
        c: (accent.c * 0.9).max(0.06),
        h: (hue + 50.0) % 360.0,
    };
    let action_rgb = oklch_to_rgb(action, true);
    let action_foreground = readable_foreground(action_rgb);
    let accent_foreground = readable_foreground(accent_rgb);

    let sidebar = surface_at(0.045, tint_c * 1.4);
    let sidebar_rgb = oklch_to_rgb(sidebar, true);
    let surface = surface_at(0.015, tint_c);
    let surface_raised = surface_at(0.05, tint_c);
    let surface_raised_rgb = oklch_to_rgb(surface_raised, true);
    let surface_overlay = surface_at(0.075, tint_c);
    let border = surface_at(pick(0.16, 0.12), 0.07_f64.min(accent.c * 0.35));
    let input = surface_at(pick(0.21, 0.16), 0.08_f64.min(accent.c * 0.4));
    let secondary = surface_at(pick(0.1, 0.06), 0.09_f64.min(accent.c * 0.5));
    let secondary_rgb = oklch_to_rgb(secondary, true);
    let muted = surface_at(pick(0.06, 0.04), 0.06_f64.min(accent.c * 0.35));
    let muted_rgb = oklch_to_rgb(muted, true);
    let accent_surface = surface_at(pick(0.13, 0.08), 0.11_f64.min(accent.c * 0.55));
    let accent_surface_rgb = oklch_to_rgb(accent_surface, true);
    let message_surface = surface_at(pick(0.16, 0.1), 0.13_f64.min(accent.c * 0.6));
    let message_surface_rgb = oklch_to_rgb(message_surface, true);
    let code_background = surface_at(0.035, tint_c * 0.8);
    let update_surface = surface_at(pick(0.14, 0.09), 0.12_f64.min(accent.c * 0.55));

    let foreground_on = |surface_rgb: Rgb| {
        oklch_to_theme_color(solve_oklch_lightness(
            text_base,
            &[surface_rgb],
            4.6,
            text_direction,
        ))
    };
    let muted_foreground = rgb_to_theme_color(readable_text(muted_rgb, text_rgb, 1.0, 4.6));
    let placeholder = rgb_to_theme_color(readable_text(surface_raised_rgb, text_rgb, 1.0, 4.6));

    let search_match = oklch_to_rgb(surface_at(0.22, 0.12_f64.min(accent.c)), true);
    let search_match_active = oklch_to_rgb(surface_at(0.4, 0.18_f64.min(accent.c)), true);
    let action_hover = Oklch {
        l: action.l + pick(0.06, -0.06),
        ..action
    };
    let status = standard_status_colors(canvas_rgb);

    let mut colors = defaults;
    let mut set = |role: R, value: String| colors.set(role, value);
    set(R::Error, status.error);
    set(R::ErrorForeground, status.error_foreground);
    set(R::ErrorSurface, status.error_surface);
    set(R::Warning, status.warning);
    set(R::WarningForeground, status.warning_foreground);
    set(R::WarningSurface, status.warning_surface);
    set(R::Canvas, rgb_to_theme_color(canvas_rgb));
    set(R::Chrome, rgb_to_theme_color(canvas_rgb));
    set(R::Toolbar, rgb_to_theme_color(canvas_rgb));
    set(R::ToolbarForeground, rgb_to_theme_color(text_rgb));
    set(
        R::ToolbarBorder,
        color_of(surface_at(pick(0.14, 0.1), 0.08_f64.min(accent.c * 0.4))),
    );
    set(
        R::ToolbarControl,
        color_of(surface_at(pick(0.09, 0.05), tint_c * 1.3)),
    );
    set(R::ToolbarControlForeground, rgb_to_theme_color(text_rgb));
    set(
        R::ToolbarControlHover,
        color_of(surface_at(pick(0.14, 0.09), tint_c * 1.6)),
    );
    set(R::Surface, color_of(surface));
    set(R::SurfaceRaised, color_of(surface_raised));
    set(R::SurfaceOverlay, color_of(surface_overlay));
    set(R::Text, rgb_to_theme_color(text_rgb));
    set(R::TextMuted, rgb_to_theme_color(text_muted_rgb));
    set(R::Border, color_of(border));
    set(R::Input, color_of(input));
    set(R::Focus, rgb_to_theme_color(accent_rgb));
    set(R::Accent, rgb_to_theme_color(accent_rgb));
    set(R::AccentForeground, rgb_to_theme_color(accent_foreground));
    set(R::Secondary, color_of(secondary));
    set(R::SecondaryForeground, foreground_on(secondary_rgb));
    set(R::Muted, color_of(muted));
    set(R::MutedForeground, muted_foreground);
    set(R::Placeholder, placeholder);
    set(R::SecondaryLabel, rgb_to_theme_color(text_muted_rgb));
    set(R::IconMuted, rgb_to_theme_color(text_muted_rgb));
    set(R::Update, rgb_to_theme_color(accent_rgb));
    set(
        R::UpdateForeground,
        foreground_on(oklch_to_rgb(update_surface, true)),
    );
    set(R::UpdateSurface, color_of(update_surface));
    set(R::AccentSurface, color_of(accent_surface));
    set(
        R::AccentSurfaceForeground,
        foreground_on(accent_surface_rgb),
    );
    set(R::MessageSurface, color_of(message_surface));
    set(R::MessageForeground, foreground_on(message_surface_rgb));
    set(R::MessageAction, rgb_to_theme_color(action_rgb));
    set(
        R::MessageActionForeground,
        rgb_to_theme_color(action_foreground),
    );
    set(R::MessageActionHover, color_of(action_hover));
    set(R::CodeBackground, color_of(code_background));
    set(R::CodeForeground, rgb_to_theme_color(text_rgb));
    set(R::SearchMatchBackground, rgb_to_theme_color(search_match));
    set(
        R::SearchMatchForeground,
        rgb_to_theme_color(readable_foreground(search_match)),
    );
    set(
        R::SearchMatchActiveBackground,
        rgb_to_theme_color(search_match_active),
    );
    set(
        R::SearchMatchActiveForeground,
        rgb_to_theme_color(readable_foreground(search_match_active)),
    );
    set(R::Sidebar, color_of(sidebar));
    set(R::SidebarForeground, foreground_on(sidebar_rgb));
    set(
        R::SidebarMutedForeground,
        rgb_to_theme_color(standard_muted_text(sidebar_rgb, text_rgb)),
    );
    set(
        R::SidebarControlSurface,
        color_of(surface_at(pick(0.1, 0.07), tint_c * 1.5)),
    );
    set(
        R::SidebarRowHover,
        color_of(surface_at(pick(0.08, 0.06), 0.08_f64.min(accent.c * 0.45))),
    );
    set(
        R::SidebarRowActive,
        color_of(surface_at(pick(0.12, 0.09), 0.1_f64.min(accent.c * 0.55))),
    );
    set(
        R::SidebarRowSelected,
        color_of(surface_at(pick(0.14, 0.1), 0.11_f64.min(accent.c * 0.6))),
    );
    set(
        R::SidebarBorder,
        color_of(surface_at(pick(0.17, 0.12), 0.08_f64.min(accent.c * 0.4))),
    );
    set(R::TerminalBackground, rgb_to_theme_color(canvas_rgb));
    set(R::TerminalForeground, rgb_to_theme_color(text_rgb));
    set(R::TerminalCursor, rgb_to_theme_color(accent_rgb));
    set(
        R::TerminalSelection,
        color_of(surface_at(pick(0.18, 0.12), 0.12_f64.min(accent.c * 0.55))),
    );
    set(
        R::TerminalScrollbar,
        color_of(surface_at(pick(0.22, 0.16), tint_c)),
    );
    set(
        R::TerminalScrollbarHover,
        color_of(surface_at(pick(0.3, 0.22), tint_c)),
    );
    colors
}

/// `updateThemeColorFamily`: sets `role` and re-derives the roles that depend on it.
/// A value that is not a literal colour is stored as written, like the editor does.
pub fn update_theme_color_family(
    appearance: Appearance,
    colors: &ThemeColors,
    role: R,
    value: &str,
) -> ThemeColors {
    let Some(parsed_selected) = parse_theme_color(value) else {
        return colors.clone().with(role, value);
    };
    let normalized = format_oklch(parsed_selected.color, parsed_selected.alpha);
    let mut out = colors.clone();
    let canvas = parse_rgb(colors.get(R::Canvas), fallback_canvas(appearance));
    let selected = oklch_to_rgb(parsed_selected.color, true);
    let selected_on = |background: Rgb| mix(background, selected, parsed_selected.alpha);
    let selected_on_canvas = selected_on(canvas);
    let accent = parse_rgb(colors.get(R::Accent), FALLBACK_ACCENT);
    let canvas_is_dark = is_dark(canvas);
    let terminal_is_dark = is_dark(selected_on_canvas);
    let color_of = rgb_to_theme_color;
    let foreground_on = |background: Rgb| color_of(readable_foreground(background));
    let selected_tone_on = |background: Rgb| {
        let direction = if is_dark(background) {
            Direction::Lighter
        } else {
            Direction::Darker
        };
        oklch_to_theme_color(solve_oklch_lightness(
            parsed_selected.color,
            &[background],
            4.6,
            direction,
        ))
    };
    let status_colors = || {
        let surface = mix(
            canvas,
            selected_on_canvas,
            if canvas_is_dark { 0.16 } else { 0.08 },
        );
        (selected_tone_on(surface), color_of(surface))
    };

    match role {
        R::Canvas => {
            out.set(R::Canvas, normalized.clone());
            out.set(R::Chrome, normalized.clone());
            out.set(R::Toolbar, normalized);
        }
        R::Surface | R::SurfaceRaised | R::SurfaceOverlay | R::Input | R::SidebarControlSurface => {
            out.set(role, normalized);
        }
        R::Text => {
            out.set(R::Text, normalized.clone());
            out.set(R::ToolbarForeground, normalized.clone());
            out.set(R::ToolbarControlForeground, normalized);
        }
        R::MutedForeground => {
            for dependent in [
                R::TextMuted,
                R::MutedForeground,
                R::Placeholder,
                R::SecondaryLabel,
                R::IconMuted,
                R::SidebarMutedForeground,
            ] {
                out.set(dependent, normalized.clone());
            }
        }
        R::Border => {
            out.set(R::Border, normalized.clone());
            out.set(R::ToolbarBorder, normalized.clone());
            out.set(R::SidebarBorder, normalized);
        }
        R::Secondary => {
            out.set(R::Secondary, normalized.clone());
            out.set(R::SecondaryForeground, foreground_on(selected_on_canvas));
            out.set(R::Muted, normalized.clone());
            out.set(R::ToolbarControl, normalized);
        }
        R::AccentSurface => {
            out.set(R::AccentSurface, normalized.clone());
            out.set(
                R::AccentSurfaceForeground,
                foreground_on(selected_on_canvas),
            );
            out.set(R::ToolbarControlHover, normalized);
        }
        R::Accent => {
            let update_surface = mix(
                canvas,
                selected_on_canvas,
                if canvas_is_dark { 0.32 } else { 0.16 },
            );
            out.set(R::Accent, normalized.clone());
            out.set(R::AccentForeground, foreground_on(selected_on_canvas));
            out.set(R::Focus, normalized.clone());
            out.set(R::Update, normalized.clone());
            out.set(R::UpdateForeground, selected_tone_on(update_surface));
            out.set(R::UpdateSurface, color_of(update_surface));
            out.set(R::TerminalCursor, normalized);
        }
        R::MessageAction => {
            let action_foreground = readable_foreground(selected_on_canvas);
            let toward_opposite =
                if action_foreground == LIGHT_FOREGROUND || action_foreground == WHITE {
                    BLACK
                } else {
                    WHITE
                };
            let action_hover = mix(selected, toward_opposite, 0.12);
            out.set(R::MessageAction, normalized);
            out.set(R::MessageActionForeground, color_of(action_foreground));
            out.set(
                R::MessageActionHover,
                format_oklch(rgb_to_oklch(action_hover), parsed_selected.alpha),
            );
        }
        R::MessageSurface => {
            out.set(R::MessageSurface, normalized);
            out.set(R::MessageForeground, foreground_on(selected_on_canvas));
        }
        R::SearchMatchBackground => {
            out.set(R::SearchMatchBackground, color_of(selected_on_canvas));
            out.set(R::SearchMatchForeground, foreground_on(selected_on_canvas));
        }
        R::SearchMatchActiveBackground => {
            out.set(R::SearchMatchActiveBackground, color_of(selected_on_canvas));
            out.set(
                R::SearchMatchActiveForeground,
                foreground_on(selected_on_canvas),
            );
        }
        R::CodeBackground => {
            out.set(R::CodeBackground, normalized);
            out.set(R::CodeForeground, foreground_on(selected_on_canvas));
        }
        R::Sidebar => {
            out.set(R::Sidebar, normalized);
            out.set(R::SidebarForeground, foreground_on(selected_on_canvas));
        }
        R::SidebarRowSelected => {
            let sidebar = parse_rgb(colors.get(R::Sidebar), canvas);
            let selected_on_sidebar = selected_on(sidebar);
            out.set(
                R::SidebarRowHover,
                color_of(mix(sidebar, selected_on_sidebar, 0.5)),
            );
            out.set(
                R::SidebarRowActive,
                color_of(mix(sidebar, selected_on_sidebar, 0.8)),
            );
            out.set(R::SidebarRowSelected, normalized);
        }
        R::TerminalBackground => {
            let terminal_foreground = readable_foreground(selected_on_canvas);
            let pick = |d: f64, l: f64| if terminal_is_dark { d } else { l };
            out.set(R::TerminalBackground, normalized);
            out.set(R::TerminalForeground, color_of(terminal_foreground));
            out.set(
                R::TerminalSelection,
                color_of(mix(selected_on_canvas, accent, pick(0.35, 0.18))),
            );
            out.set(
                R::TerminalScrollbar,
                color_of(mix(
                    selected_on_canvas,
                    terminal_foreground,
                    pick(0.42, 0.22),
                )),
            );
            out.set(
                R::TerminalScrollbarHover,
                color_of(mix(
                    selected_on_canvas,
                    terminal_foreground,
                    pick(0.55, 0.32),
                )),
            );
        }
        R::Error => {
            let (foreground, surface) = status_colors();
            out.set(R::Error, normalized);
            out.set(R::ErrorForeground, foreground);
            out.set(R::ErrorSurface, surface);
        }
        R::Warning => {
            let (foreground, surface) = status_colors();
            out.set(R::Warning, normalized);
            out.set(R::WarningForeground, foreground);
            out.set(R::WarningSurface, surface);
        }
        _ => out.set(role, normalized),
    }
    out
}
