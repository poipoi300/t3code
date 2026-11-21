//! NSH paints from resolved tokens. Project literal color declarations for the root and
//! sidebar onto those tokens. Unhandled stylesheet rules remain in the exported theme.
use std::collections::BTreeMap;
use t3_theme::{color::Color, palette::Appearance, tokens::expr::Value};

use super::ResolvedTheme;

pub(super) fn apply(theme: &mut ResolvedTheme, css: &str) {
    let mut uncommented = String::with_capacity(css.len());
    let mut rest = css;
    while let Some((before, comment)) = rest.split_once("/*") {
        uncommented.push_str(before);
        rest = comment.split_once("*/").map_or("", |(_, after)| after);
    }
    uncommented.push_str(rest);
    let mut source = uncommented.as_str();
    let mut root = BTreeMap::new();
    let mut local = BTreeMap::new();
    while let Some((selector, remainder)) = source.split_once('{') {
        let mut depth = 1;
        let mut nested = false;
        let Some(end) = remainder.char_indices().find_map(|(index, c)| {
            match c {
                '{' => {
                    depth += 1;
                    nested = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(index)
        }) else {
            break;
        };
        let body = &remainder[..end];
        source = &remainder[end + 1..];
        if nested {
            continue;
        }
        for selector in selector.trim().split(',').map(str::trim) {
            let sidebar = match selector {
                ":root" | "html" => false,
                ".dark" | "html.dark" | ":root.dark" if theme.appearance == Appearance::Dark => {
                    false
                }
                ".light" | "html.light" | ":root.light"
                    if theme.appearance == Appearance::Light =>
                {
                    false
                }
                "[data-app-sidebar]" => true,
                _ => continue,
            };
            for declaration in body.split(';') {
                let Some((property, value)) = declaration.split_once(':') else {
                    continue;
                };
                let property = property.trim();
                if !property.starts_with("--") {
                    continue;
                }
                let (value, important) = match value.rsplit_once('!') {
                    Some((value, priority))
                        if priority.trim().eq_ignore_ascii_case("important") =>
                    {
                        (value.trim(), true)
                    }
                    _ => (value.trim(), false),
                };
                let Some(color) = Color::parse(value) else {
                    continue;
                };
                let specificity = match selector {
                    "html" => (0, 1),
                    "html.dark" | "html.light" => (1, 1),
                    ":root.dark" | ":root.light" => (2, 0),
                    _ => (1, 0),
                };
                let declarations = if sidebar { &mut local } else { &mut root };
                let rank = (important, specificity);
                let entry = declarations.entry(property).or_insert((rank, color));
                if rank >= entry.0 {
                    *entry = (rank, color);
                }
            }
        }
    }
    let set = |map: &mut BTreeMap<String, Value>, property: &str, color: Color| {
        if map.contains_key(property) {
            let value = Value::Color { color };
            map.insert(property.into(), value.clone());
            if let Some(alias) = map.get_mut(&format!("--color-{}", &property[2..])) {
                *alias = value;
            }
        }
    };
    for (property, (_, color)) in root {
        set(&mut theme.tokens.root, property, color);
        if !local.contains_key(property) {
            set(&mut theme.tokens.sidebar, property, color);
        }
    }
    for (property, (_, color)) in local {
        set(&mut theme.tokens.sidebar, property, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeChoice, tokens_for};
    use std::rc::Rc;
    use t3_theme::palette::{THEME_PREVIEW_ID, built_in_theme};

    #[test]
    fn themes_sidebar_color_overrides_inherited_important_value() {
        use t3_theme::resolved::{ThemeInput, resolve_theme};
        let mut theme = resolve_theme(&ThemeInput::new(None, Appearance::Light));
        apply(
            &mut theme,
            "[data-app-sidebar] { --background: #223344; } :root { --background: #112233 !important; }",
        );
        for (tokens, hex) in [
            (&theme.tokens.root, "#112233"),
            (&theme.tokens.sidebar, "#223344"),
        ] {
            assert_eq!(
                tokens.get("--background"),
                Some(&Value::Color {
                    color: Color::parse(hex).unwrap()
                })
            );
        }
    }

    #[test]
    fn themes_custom_root_css_applies_to_saved_and_preview_tokens() {
        for id in ["imported", THEME_PREVIEW_ID] {
            let mut file = built_in_theme("ocean").expect("built-in").clone();
            file.id = id.into();
            file.css = Some("/* imported */ :root { --background: #123456 !important; --background: #000000; } .dark { --background: #654321 !IMPORTANT; }".into());
            let choice = ThemeChoice {
                theme: id.into(),
                custom: Rc::from([file]),
                ..Default::default()
            };
            let colors = tokens_for(&choice)
                .color("background")
                .expect("resolved background");
            let color = |hex| crate::theme::hsla_of(&Color::parse(hex).expect("hex"));
            assert_eq!(colors.light, color("#123456"));
            assert_eq!(colors.dark, color("#654321"));
        }
    }
}
