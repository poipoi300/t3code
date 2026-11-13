//! Importing a VS Code colour theme: workbench colours become T3 roles.
//! A port of `apps/web/src/vscodeThemeImport.ts`.

use serde_json::{Map, Value, json};

use super::jsonc::parse_jsonc;
use crate::palette::math::{Rgb, contrast_ratio, relative_luminance, rgb_to_hex};
use crate::palette::{
    Appearance, THEME_FILE_VERSION, ThemeColorRole as R, ThemeDefinition, ThemeError,
    create_vivid_theme_colors, is_reserved_theme_id, parse_theme_file, readable_theme_color_on,
    standard_theme_colors, theme_color_to_hex, theme_id_from_name,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

const NOT_JSON: &str = "That theme file is not valid JSON.";

fn decode_gamma(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encode_gamma(value: f64) -> f64 {
    let clamped = value.clamp(0.0, 1.0);
    if clamped <= 0.0031308 {
        clamped * 12.92
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    }
}

/// `Number.parseFloat`: the longest numeric prefix, or `None`.
fn js_parse_float(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        let c = bytes[end];
        match c {
            b'+' | b'-' if end == 0 => {}
            b'+' | b'-' if seen_exp && matches!(bytes[end - 1], b'e' | b'E') => {}
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot && !seen_exp => seen_dot = true,
            b'e' | b'E' if seen_digit && !seen_exp => seen_exp = true,
            _ => break,
        }
        end += 1;
    }
    let mut candidate = &text[..end];
    while !candidate.is_empty() {
        if let Ok(value) = candidate.parse::<f64>() {
            return Some(value);
        }
        candidate = &candidate[..candidate.len() - 1];
    }
    None
}

fn parse_percent_or_number(part: &str) -> Option<f64> {
    let value = if part.ends_with('%') {
        js_parse_float(part)? / 100.0
    } else {
        js_parse_float(part)?
    };
    value.is_finite().then_some(value)
}

fn parse_color_function(value: &str) -> Option<Rgba> {
    let lower = value.to_ascii_lowercase();
    let inner = lower.strip_prefix("color(")?.strip_suffix(')')?;
    let inner = inner.trim_start();
    let (space, body) = if let Some(rest) = inner.strip_prefix("display-p3") {
        ("display-p3", rest)
    } else {
        ("srgb", inner.strip_prefix("srgb")?)
    };
    if !body.starts_with(char::is_whitespace) || body.contains(')') {
        return None;
    }
    let (channel_part, alpha_part) = match body.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha)),
        None => (body, None),
    };
    let channels: Vec<f64> = channel_part
        .split_whitespace()
        .map(parse_percent_or_number)
        .collect::<Option<_>>()?;
    let [red, green, blue] = channels[..] else {
        return None;
    };
    let alpha = match alpha_part {
        None => 1.0,
        Some(raw) => parse_percent_or_number(raw.trim())?,
    };
    let alpha = alpha.clamp(0.0, 1.0);
    if space == "srgb" {
        return Some(Rgba {
            r: red * 255.0,
            g: green * 255.0,
            b: blue * 255.0,
            a: alpha,
        });
    }
    let [lr, lg, lb] = [red, green, blue].map(decode_gamma);
    let srgb = [
        1.2249401762805 * lr - 0.2249401762805 * lg,
        -0.042056961239 * lr + 1.042056961239 * lg,
        -0.0196375547643 * lr - 0.0786360655012 * lg + 1.0982736202656 * lb,
    ]
    .map(|channel| encode_gamma(channel) * 255.0);
    Some(Rgba {
        r: srgb[0],
        g: srgb[1],
        b: srgb[2],
        a: alpha,
    })
}

fn parse_vscode_color(value: Option<&Value>) -> Option<Rgba> {
    let text = value?.as_str()?.trim();
    if text.starts_with("color(") {
        return parse_color_function(text);
    }
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| f64::from(u8::from_str_radix(&hex[i..=i], 16).unwrap_or(0) * 17);
    let byte = |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0));
    Some(if hex.len() <= 4 {
        Rgba {
            r: digit(0),
            g: digit(1),
            b: digit(2),
            a: if hex.len() == 4 {
                digit(3) / 255.0
            } else {
                1.0
            },
        }
    } else {
        Rgba {
            r: byte(0),
            g: byte(2),
            b: byte(4),
            a: if hex.len() == 8 { byte(6) / 255.0 } else { 1.0 },
        }
    })
}

fn to_hex(color: Rgb) -> String {
    rgb_to_hex(color)
}

/// Composites a translucent overlay onto its surface; theme roles are opaque.
fn flatten_over(color: Rgba, base: Rgb) -> String {
    if color.a >= 1.0 {
        return to_hex(Rgb::new(color.r, color.g, color.b));
    }
    to_hex(Rgb::new(
        color.r * color.a + base.r * (1.0 - color.a),
        color.g * color.a + base.g * (1.0 - color.a),
        color.b * color.a + base.b * (1.0 - color.a),
    ))
}

fn hex_to_rgb(value: &str) -> Rgb {
    let hex = theme_color_to_hex(value);
    let parsed = parse_vscode_color(Some(&Value::String(
        hex.unwrap_or_else(|| value.to_owned()),
    )));
    parsed.map_or(Rgb::new(0.0, 0.0, 0.0), |c| Rgb::new(c.r, c.g, c.b))
}

/// `isVsCodeThemeFile`.
pub fn is_vscode_theme_file(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("version").and_then(Value::as_f64) == Some(f64::from(THEME_FILE_VERSION)) {
        return false;
    }
    let workbench_colors = object
        .get("colors")
        .and_then(Value::as_object)
        .is_some_and(|colors| colors.keys().any(|key| key.contains('.')));
    workbench_colors || object.get("tokenColors").is_some_and(Value::is_array)
}

/// `parseThemeImport`: a VS Code theme (JSONC) or a native theme file (strict JSON).
pub fn parse_theme_import(source: &str) -> Result<ThemeDefinition, ThemeError> {
    let value = parse_jsonc(source).map_err(|_| ThemeError::new(NOT_JSON))?;
    if is_vscode_theme_file(&value) {
        return parse_vscode_theme_file(&value);
    }
    let strict: Value = serde_json::from_str(source).map_err(|_| ThemeError::new(NOT_JSON))?;
    parse_theme_file(&strict)
}

fn resolve_appearance(value: &Map<String, Value>, canvas: Rgb) -> Appearance {
    match value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("light" | "hc-light") => Appearance::Light,
        Some("dark" | "hc-black") => Appearance::Dark,
        _ if relative_luminance(canvas) < 0.179 => Appearance::Dark,
        _ => Appearance::Light,
    }
}

fn utf16_prefix(text: &str, units: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        used += c.len_utf16();
        if used > units {
            break;
        }
        out.push(c);
    }
    out
}

fn upper_first(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `humanizeThemeName`: `github-dark` becomes `Github Dark`.
pub fn humanize_theme_name(raw: &str) -> String {
    let trimmed = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.chars().any(char::is_whitespace) || !trimmed.contains(['-', '_', '.']) {
        return trimmed.to_owned();
    }
    trimmed
        .split(['-', '_', '.'])
        .filter(|word| !word.is_empty())
        .map(upper_first)
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_name(value: &Map<String, Value>) -> String {
    for key in ["displayName", "name"] {
        let Some(candidate) = value.get(key).and_then(Value::as_str) else {
            continue;
        };
        let humanized = humanize_theme_name(candidate);
        if !humanized.is_empty() {
            return utf16_prefix(&humanized, 48);
        }
    }
    "VS Code theme".to_owned()
}

/// `parseVsCodeThemeFile`: derive a T3 theme from a VS Code colour theme.
pub fn parse_vscode_theme_file(value: &Value) -> Result<ThemeDefinition, ThemeError> {
    let Some(object) = value.as_object() else {
        return Err(ThemeError::new("Theme files must contain a JSON object."));
    };
    let empty = Map::new();
    let colors = object
        .get("colors")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let color_of = |key: &str| parse_vscode_color(colors.get(key));

    let pick = |keys: &[&str]| keys.iter().find_map(|key| color_of(key));
    let solid_over = |base: Rgb, keys: &[&str]| pick(keys).map(|c| flatten_over(c, base));

    let Some(canvas_color) = pick(&["editor.background", "editorPane.background"]) else {
        return Err(ThemeError::new(
            "That VS Code theme has no \"editor.background\" color, so there is nothing to build a palette from.",
        ));
    };
    let canvas = Rgb::new(canvas_color.r, canvas_color.g, canvas_color.b);
    let appearance = resolve_appearance(object, canvas);

    let canvas_hex = to_hex(canvas);
    let raised_candidate = solid_over(canvas, &["editorWidget.background", "dropdown.background"]);
    let stands_apart =
        |first: &str, second: &str| contrast_ratio(hex_to_rgb(first), hex_to_rgb(second)) >= 1.1;

    let mut accent: Option<(Rgba, String)> = None;
    for key in [
        "focusBorder",
        "button.background",
        "textLink.foreground",
        "activityBarBadge.background",
        "progressBar.background",
        "badge.background",
    ] {
        let Some(candidate) = color_of(key) else {
            continue;
        };
        let candidate_hex = flatten_over(candidate, canvas);
        if !stands_apart(&candidate_hex, &canvas_hex)
            || raised_candidate
                .as_deref()
                .is_some_and(|raised| !stands_apart(&candidate_hex, raised))
        {
            continue;
        }
        accent = Some((candidate, candidate_hex));
        break;
    }
    let (accent_color, accent_hex) = match accent {
        Some(found) => found,
        None => {
            let standard = theme_color_to_hex(standard_theme_colors(appearance).get(R::Accent))
                .unwrap_or_default();
            let hex = [standard.clone(), "#ffffff".to_owned(), "#000000".to_owned()]
                .into_iter()
                .find(|candidate| {
                    stands_apart(candidate, &canvas_hex)
                        && raised_candidate
                            .as_deref()
                            .is_none_or(|raised| stands_apart(candidate, raised))
                })
                .unwrap_or(standard);
            let color = parse_vscode_color(Some(&Value::String(hex.clone()))).unwrap_or(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            });
            (color, hex)
        }
    };

    let muted_accent_hex = flatten_over(
        Rgba {
            a: 0.2,
            ..accent_color
        },
        canvas,
    );
    let derived = create_vivid_theme_colors(appearance, &canvas_hex, &muted_accent_hex);
    let sidebar_hex = solid_over(canvas, &["sideBar.background", "activityBar.background"])
        .unwrap_or_else(|| derived.get(R::Sidebar).to_owned());
    let sidebar = hex_to_rgb(&sidebar_hex);
    let terminal_hex = solid_over(canvas, &["terminal.background", "panel.background"])
        .unwrap_or_else(|| derived.get(R::TerminalBackground).to_owned());
    let terminal = hex_to_rgb(&terminal_hex);

    let readable_on = |surface: &str, fallback: &str, keys: &[&str]| {
        let surface_rgb = hex_to_rgb(surface);
        let is_readable =
            |candidate: &str| contrast_ratio(hex_to_rgb(candidate), surface_rgb) >= 4.5;
        if let Some(specified) = solid_over(surface_rgb, keys)
            && is_readable(&specified)
        {
            return specified;
        }
        if is_readable(fallback) {
            return fallback.to_owned();
        }
        if relative_luminance(surface_rgb) < 0.179 {
            "#ffffff".to_owned()
        } else {
            "#000000".to_owned()
        }
    };
    // Status colours also fill controls that carry white text, so a dim one is lightened
    // along its hue; one hidden in the canvas has no hue to keep and the derived colour stands in.
    let readable_status = |fallback: &str, keys: &[&str]| {
        let specified = solid_over(canvas, keys);
        let base = match &specified {
            Some(s) if stands_apart(s, &canvas_hex) => s.as_str(),
            _ => fallback,
        };
        readable_theme_color_on(base, &canvas_hex, 4.5)
    };

    let surface_raised_hex =
        raised_candidate.unwrap_or_else(|| derived.get(R::SurfaceRaised).to_owned());
    // Checked tracks use messageAction; resolve it before the unchecked input role.
    let button_hex = solid_over(canvas, &["button.background"]);
    let action_hex = match &button_hex {
        Some(button)
            if stands_apart(button, &canvas_hex) && stands_apart(button, &surface_raised_hex) =>
        {
            button.clone()
        }
        _ => accent_hex.clone(),
    };
    let standard_input = standard_theme_colors(appearance).get(R::Input).to_owned();
    let input_candidates = [
        derived.get(R::Input).to_owned(),
        derived.get(R::SurfaceRaised).to_owned(),
        standard_input,
        "#000000".to_owned(),
        "#ffffff".to_owned(),
        "#808080".to_owned(),
    ];
    let mut input_hex = input_candidates
        .iter()
        .find(|candidate| {
            stands_apart(candidate, &canvas_hex) && stands_apart(candidate, &action_hex)
        })
        .cloned()
        .unwrap_or_else(|| "#808080".to_owned());
    for key in ["input.background", "input.border"] {
        if let Some(candidate) = solid_over(canvas, &[key])
            && stands_apart(&candidate, &canvas_hex)
            && stands_apart(&candidate, &action_hex)
        {
            input_hex = candidate;
            break;
        }
    }
    let search_match_background = solid_over(canvas, &["editor.findMatchHighlightBackground"])
        .unwrap_or_else(|| derived.get(R::SearchMatchBackground).to_owned());
    let search_match_active_background = solid_over(canvas, &["editor.findMatchBackground"])
        .unwrap_or_else(|| derived.get(R::SearchMatchActiveBackground).to_owned());

    let mut overrides: Vec<(R, String)> = vec![
        (R::Canvas, canvas_hex.clone()),
        (
            R::Text,
            readable_on(
                &canvas_hex,
                derived.get(R::Text),
                &["editor.foreground", "foreground"],
            ),
        ),
        (
            R::TextMuted,
            readable_on(
                &canvas_hex,
                derived.get(R::TextMuted),
                &["descriptionForeground", "disabledForeground"],
            ),
        ),
        (
            R::Surface,
            solid_over(canvas, &["editorWidget.background"])
                .unwrap_or_else(|| derived.get(R::Surface).to_owned()),
        ),
        (R::SurfaceRaised, surface_raised_hex.clone()),
        (
            R::SurfaceOverlay,
            solid_over(
                canvas,
                &[
                    "menu.background",
                    "quickInput.background",
                    "dropdown.background",
                ],
            )
            .unwrap_or_else(|| derived.get(R::SurfaceOverlay).to_owned()),
        ),
        (
            R::Border,
            solid_over(
                canvas,
                &["panel.border", "editorGroup.border", "contrastBorder"],
            )
            .unwrap_or_else(|| derived.get(R::Border).to_owned()),
        ),
        (R::Input, input_hex),
        (
            R::Placeholder,
            readable_on(
                &surface_raised_hex,
                derived.get(R::Placeholder),
                &["input.placeholderForeground"],
            ),
        ),
        (
            R::Error,
            readable_status(
                derived.get(R::Error),
                &["editorError.foreground", "errorForeground"],
            ),
        ),
        (
            R::Warning,
            readable_status(derived.get(R::Warning), &["editorWarning.foreground"]),
        ),
        (
            R::AccentSurface,
            solid_over(
                canvas,
                &["list.activeSelectionBackground", "list.hoverBackground"],
            )
            .unwrap_or_else(|| derived.get(R::AccentSurface).to_owned()),
        ),
        (
            R::CodeBackground,
            solid_over(canvas, &["textCodeBlock.background"])
                .unwrap_or_else(|| derived.get(R::CodeBackground).to_owned()),
        ),
        (R::SearchMatchBackground, search_match_background.clone()),
        (
            R::SearchMatchForeground,
            readable_on(
                &search_match_background,
                derived.get(R::SearchMatchForeground),
                &[
                    "editor.findMatchHighlightForeground",
                    "editor.foreground",
                    "foreground",
                ],
            ),
        ),
        (
            R::SearchMatchActiveBackground,
            search_match_active_background.clone(),
        ),
        (
            R::SearchMatchActiveForeground,
            readable_on(
                &search_match_active_background,
                derived.get(R::SearchMatchActiveForeground),
                &[
                    "editor.findMatchForeground",
                    "editor.foreground",
                    "foreground",
                ],
            ),
        ),
        (R::Sidebar, sidebar_hex.clone()),
        (
            R::SidebarForeground,
            readable_on(
                &sidebar_hex,
                derived.get(R::SidebarForeground),
                &["sideBar.foreground"],
            ),
        ),
        (
            R::SidebarBorder,
            solid_over(sidebar, &["sideBar.border"])
                .unwrap_or_else(|| derived.get(R::SidebarBorder).to_owned()),
        ),
        (
            R::SidebarRowHover,
            solid_over(sidebar, &["list.hoverBackground"])
                .unwrap_or_else(|| derived.get(R::SidebarRowHover).to_owned()),
        ),
        (
            R::SidebarRowActive,
            solid_over(
                sidebar,
                &["list.inactiveSelectionBackground", "list.hoverBackground"],
            )
            .unwrap_or_else(|| derived.get(R::SidebarRowActive).to_owned()),
        ),
        (
            R::SidebarRowSelected,
            solid_over(sidebar, &["list.activeSelectionBackground"])
                .unwrap_or_else(|| derived.get(R::SidebarRowSelected).to_owned()),
        ),
        (R::TerminalBackground, terminal_hex.clone()),
        (
            R::TerminalForeground,
            readable_on(
                &terminal_hex,
                derived.get(R::TerminalForeground),
                &["terminal.foreground"],
            ),
        ),
        (
            R::TerminalCursor,
            solid_over(
                terminal,
                &["terminalCursor.foreground", "editorCursor.foreground"],
            )
            .unwrap_or_else(|| derived.get(R::TerminalCursor).to_owned()),
        ),
        (
            R::TerminalSelection,
            solid_over(
                terminal,
                &["terminal.selectionBackground", "editor.selectionBackground"],
            )
            .unwrap_or_else(|| derived.get(R::TerminalSelection).to_owned()),
        ),
        (
            R::TerminalScrollbar,
            solid_over(terminal, &["scrollbarSlider.background"])
                .unwrap_or_else(|| derived.get(R::TerminalScrollbar).to_owned()),
        ),
    ];
    overrides.push((R::Accent, accent_hex.clone()));
    overrides.push((R::Focus, accent_hex.clone()));
    overrides.push((R::MessageAction, action_hex.clone()));
    overrides.push((
        R::MessageActionForeground,
        readable_on(
            &action_hex,
            derived.get(R::MessageActionForeground),
            &["button.foreground"],
        ),
    ));
    overrides.push((
        R::AccentForeground,
        readable_on(
            &accent_hex,
            derived.get(R::AccentForeground),
            &["button.foreground"],
        ),
    ));

    let name = resolve_name(object);
    let generated_id = theme_id_from_name(&name);
    let mut merged = derived;
    for (role, color) in overrides {
        merged.set(role, color);
    }
    let mut file = Map::new();
    file.insert("version".into(), json!(THEME_FILE_VERSION));
    if is_reserved_theme_id(&generated_id) {
        file.insert("id".into(), json!(format!("{generated_id}-vscode")));
    }
    file.insert("name".into(), json!(name));
    file.insert("appearance".into(), json!(appearance.as_str()));
    file.insert(
        "colors".into(),
        serde_json::to_value(&merged).map_err(|e| ThemeError::new(e.to_string()))?,
    );
    parse_theme_file(&Value::Object(file))
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Replaces `light` and `dark` (whole words, any case) with spaces, collapses
/// whitespace and trims.
fn strip_appearance(label: &str) -> String {
    let chars: Vec<char> = label.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..chars.len().min(i + 5)]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();
        let word_len = if rest.starts_with("light") {
            5
        } else if rest.starts_with("dark") {
            4
        } else {
            0
        };
        let before_ok = i == 0 || !is_word(chars[i - 1]);
        let after_ok = i + word_len >= chars.len() || !is_word(chars[i + word_len]);
        if word_len > 0 && before_ok && after_ok {
            out.push(' ');
            i += word_len;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn theme_file_value(
    theme: &ThemeDefinition,
    name: &str,
    appearance: Appearance,
) -> Result<Map<String, Value>, ThemeError> {
    let mut file = Map::new();
    file.insert("version".into(), json!(THEME_FILE_VERSION));
    file.insert("name".into(), json!(name));
    file.insert("appearance".into(), json!(appearance.as_str()));
    file.insert(
        "colors".into(),
        serde_json::to_value(&theme.colors).map_err(|e| ThemeError::new(e.to_string()))?,
    );
    Ok(file)
}

/// Chooses the id of a paired theme from its light and dark halves.
pub type PairedId<'a> = dyn Fn(&ThemeDefinition, &ThemeDefinition) -> String + 'a;

/// `pairVsCodeThemes`: joins `x-light` and `x-dark` files into one two-appearance theme.
pub fn pair_vscode_themes(
    themes: &[ThemeDefinition],
    paired_id: Option<&PairedId>,
) -> Vec<ThemeDefinition> {
    struct Group<'a> {
        key: String,
        light: Vec<&'a ThemeDefinition>,
        dark: Vec<&'a ThemeDefinition>,
        order: usize,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut passthrough: Vec<(usize, ThemeDefinition)> = Vec::new();
    for (order, theme) in themes.iter().enumerate() {
        let key = strip_appearance(&theme.label);
        if theme.modes().len() != 1
            || key == theme.label
            || !key.chars().any(|c| c.is_ascii_alphanumeric())
        {
            passthrough.push((order, theme.clone()));
            continue;
        }
        let index = match groups.iter().position(|g| g.key == key) {
            Some(index) => index,
            None => {
                groups.push(Group {
                    key,
                    light: Vec::new(),
                    dark: Vec::new(),
                    order,
                });
                groups.len() - 1
            }
        };
        match theme.appearance {
            Appearance::Light => groups[index].light.push(theme),
            Appearance::Dark => groups[index].dark.push(theme),
        }
    }
    let mut paired: Vec<(usize, ThemeDefinition)> = Vec::new();
    for group in groups {
        if let ([light], [dark]) = (group.light.as_slice(), group.dark.as_slice()) {
            let attempt = (|| {
                let mut file = theme_file_value(light, &group.key, Appearance::Light)?;
                if let Some(make_id) = paired_id {
                    file.insert("id".into(), json!(make_id(light, dark)));
                }
                file.insert(
                    "variants".into(),
                    json!({ "dark": serde_json::to_value(&dark.colors).map_err(|e| ThemeError::new(e.to_string()))? }),
                );
                // `id` goes before `name` in the TypeScript literal; key order does not matter to parsing.
                parse_theme_file(&Value::Object(file))
            })();
            if let Ok(theme) = attempt {
                paired.push((group.order, theme));
                continue;
            }
        }
        for theme in group.light.into_iter().chain(group.dark) {
            paired.push((group.order, theme.clone()));
        }
    }
    let mut all: Vec<(usize, ThemeDefinition)> = passthrough.into_iter().chain(paired).collect();
    all.sort_by_key(|(order, _)| *order);
    all.into_iter().map(|(_, theme)| theme).collect()
}

fn rename(theme: &ThemeDefinition, name: &str) -> Option<ThemeDefinition> {
    let mut file = theme_file_value(theme, &utf16_prefix(name, 48), theme.appearance).ok()?;
    if let Some(variants) = &theme.variants {
        file.insert("variants".into(), serde_json::to_value(variants).ok()?);
    }
    if theme.is_managed() {
        file.insert("managed".into(), json!(true));
    }
    parse_theme_file(&Value::Object(file)).ok()
}

/// `resolveThemeLabelCollisions`: gives same-named themes distinct labels and ids.
pub fn resolve_theme_label_collisions(
    entries: &[(ThemeDefinition, Option<&str>)],
) -> Vec<ThemeDefinition> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (theme, _) in entries {
        match counts.iter_mut().find(|(id, _)| *id == theme.id) {
            Some(entry) => entry.1 += 1,
            None => counts.push((&theme.id, 1)),
        }
    }
    let count_of = |id: &str| counts.iter().find(|(c, _)| *c == id).map_or(0, |(_, n)| *n);
    let relabelled: Vec<ThemeDefinition> = entries
        .iter()
        .map(|(theme, source_name)| {
            if count_of(&theme.id) < 2 {
                return theme.clone();
            }
            let stem = source_name.map(strip_extension);
            let from_file = stem.map(humanize_theme_name);
            let renamed = match from_file {
                Some(name)
                    if !name.is_empty() && name.to_lowercase() != theme.label.to_lowercase() =>
                {
                    rename(theme, &name)
                }
                _ => None,
            };
            renamed.unwrap_or_else(|| theme.clone())
        })
        .collect();
    let mut seen: Vec<String> = Vec::new();
    relabelled
        .into_iter()
        .map(|theme| {
            if !seen.contains(&theme.id) {
                seen.push(theme.id.clone());
                return theme;
            }
            for suffix in 2..100 {
                if let Some(candidate) = rename(&theme, &format!("{} {suffix}", theme.label))
                    && !seen.contains(&candidate.id)
                {
                    seen.push(candidate.id.clone());
                    return candidate;
                }
            }
            theme
        })
        .collect()
}

/// `name.replace(/\.[^.]+$/, "")`.
fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot + 1 < name.len() => &name[..dot],
        _ => name,
    }
}
