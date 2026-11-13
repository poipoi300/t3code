//! The token cascade against Chrome running the real `index.css`
//! (fixtures/chrome-tokens.json, written by tools/chrome-cascade.mjs).

use std::collections::BTreeMap;

use serde::Deserialize;
use t3_theme::color::Color;
use t3_theme::palette::ThemeColorRole;
use t3_theme::tokens::{StyleContext, TokenTable, Value, compute};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    name: String,
    dark: bool,
    theme_id: Option<String>,
    colors: Option<BTreeMap<String, String>>,
    contrast: Option<f64>,
    diff_scheme: Option<String>,
    #[serde(default)]
    wco: bool,
    chat_width: Option<String>,
    viewport_rem: Option<f64>,
}

#[derive(Deserialize)]
struct Scope {
    colors: BTreeMap<String, [u8; 6]>,
    lengths: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Entry {
    context: Context,
    root: Scope,
    sidebar: Scope,
    surface: Option<BTreeMap<String, [u8; 6]>>,
}

#[derive(Deserialize)]
struct Oracle {
    names: Vec<String>,
    contexts: Vec<Entry>,
}

fn oracle() -> Oracle {
    serde_json::from_str(include_str!("../fixtures/chrome-tokens.json")).expect("fixture parses")
}

fn inline_values(context: &Context) -> BTreeMap<String, Value> {
    let mut inline = BTreeMap::new();
    for role in ThemeColorRole::ALL {
        if let Some(text) = context.colors.as_ref().and_then(|c| c.get(role.name())) {
            let color = Color::parse(text).expect("theme colour");
            inline.insert(role.css_variable().to_owned(), Value::Color { color });
        }
    }
    let contrast = context.contrast.unwrap_or(100.0);
    inline.insert(
        "--appearance-contrast-base".into(),
        Value::Percent {
            v: contrast.min(100.0),
        },
    );
    inline.insert(
        "--appearance-contrast-boost".into(),
        Value::Percent {
            v: (contrast - 100.0).max(0.0),
        },
    );
    inline.insert(
        "--appearance-contrast-border-boost".into(),
        Value::Percent {
            v: (contrast - 100.0).max(0.0) / 4.0,
        },
    );
    inline
}

fn style_context(context: &Context) -> StyleContext {
    let mut ctx = StyleContext::new(context.dark, context.theme_id.as_deref());
    ctx.diff_color_scheme.clone_from(&context.diff_scheme);
    ctx.chat_width.clone_from(&context.chat_width);
    ctx.wco = context.wco;
    ctx.viewport_rem = context.viewport_rem;
    ctx
}

/// Painted channels over black then over white, like the browser readback in the oracle.
fn paint(color: &Color) -> [i32; 6] {
    let rgb = color.to_srgb_unclamped();
    let alpha = color.alpha.clamp(0.0, 1.0);
    let channel = |backdrop: f64, i: usize| {
        let v = (rgb[i].max(0.0) * alpha + backdrop * (1.0 - alpha)).clamp(0.0, 1.0);
        (v * 255.0 + 0.5).floor() as i32
    };
    [
        channel(0.0, 0),
        channel(0.0, 1),
        channel(0.0, 2),
        channel(1.0, 0),
        channel(1.0, 1),
        channel(1.0, 2),
    ]
}

#[test]
fn computed_tokens_match_chrome() {
    let table = TokenTable::builtin();
    let oracle = oracle();
    assert!(oracle.contexts.len() >= 19);
    let mut problems: Vec<String> = Vec::new();
    let mut compared = 0usize;
    for entry in &oracle.contexts {
        let computed = compute(
            table,
            &style_context(&entry.context),
            &inline_values(&entry.context),
        );
        for (sidebar, scope) in [(false, &entry.root), (true, &entry.sidebar)] {
            let scope_name = if sidebar { "sidebar" } else { "root" };
            for name in &oracle.names {
                let rust = computed.get(sidebar, name);
                match (scope.colors.get(name), rust.and_then(Value::color)) {
                    (Some(chrome), Some(color)) => {
                        compared += 1;
                        let ours = paint(&color);
                        let worst = ours
                            .iter()
                            .zip(chrome)
                            .map(|(a, b)| (a - i32::from(*b)).abs())
                            .max()
                            .unwrap_or(0);
                        // Chrome's raster path rounds to within a level; translucent mixes within two.
                        if worst > 2 {
                            problems.push(format!(
                                "{} {scope_name} {name}: rust {ours:?} chrome {chrome:?}",
                                entry.context.name
                            ));
                        }
                    }
                    (Some(chrome), None) => problems.push(format!(
                        "{} {scope_name} {name}: chrome {chrome:?} rust {rust:?}",
                        entry.context.name
                    )),
                    (None, Some(color)) => problems.push(format!(
                        "{} {scope_name} {name}: rust colour {} but chrome has none",
                        entry.context.name,
                        color.to_css()
                    )),
                    (None, None) => {}
                }
                if let Some(chrome_px) = scope.lengths.get(name) {
                    compared += 1;
                    // text-indent accepts percentages and em, which are not lengths in the rem/px model.
                    let comparable = match rust {
                        Some(Value::Percent { v }) => Some(*v),
                        Some(Value::Dimension { v, unit }) if unit == "em" => Some(v * 16.0),
                        other => other.and_then(|v| v.px(16.0)),
                    };
                    match comparable {
                        Some(px) if (px - chrome_px).abs() < 1e-3 => {}
                        other => problems.push(format!(
                            "{} {scope_name} {name}: length rust {other:?} chrome {chrome_px}",
                            entry.context.name
                        )),
                    }
                } else if let Some(Value::Length { .. }) = rust
                    && table.category(name).is_some_and(|c| !c.is_color())
                {
                    problems.push(format!(
                        "{} {scope_name} {name}: rust length {rust:?}, chrome none",
                        entry.context.name
                    ));
                }
            }
        }
    }
    assert!(compared > 20_000, "{compared}");
    assert!(
        problems.is_empty(),
        "{} of {compared} comparisons differ; first:\n{}",
        problems.len(),
        problems
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn diff_surfaces_match_chrome() {
    use t3_theme::palette::Appearance;
    use t3_theme::tokens::surface::code_surface;
    let table = TokenTable::builtin();
    let mut compared = 0;
    let mut problems = Vec::new();
    for entry in oracle().contexts.iter().filter(|e| e.surface.is_some()) {
        let computed = compute(
            table,
            &style_context(&entry.context),
            &inline_values(&entry.context),
        );
        let appearance = if entry.context.dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let surface = code_surface(table, appearance, &computed.root, 16.0);
        for (name, chrome) in entry.surface.iter().flatten() {
            compared += 1;
            let Some(color) = surface.color(name) else {
                problems.push(format!(
                    "{} {name}: chrome {chrome:?}, rust {:?}",
                    entry.context.name,
                    surface.get(name)
                ));
                continue;
            };
            let ours = paint(&color);
            let worst = ours
                .iter()
                .zip(chrome)
                .map(|(a, b)| (a - i32::from(*b)).abs())
                .max()
                .unwrap_or(0);
            if worst > 2 {
                problems.push(format!(
                    "{} {name}: rust {ours:?} chrome {chrome:?}",
                    entry.context.name
                ));
            }
        }
    }
    assert!(compared >= 100, "{compared}");
    assert!(
        problems.is_empty(),
        "{} of {compared}:
{}",
        problems.len(),
        problems.join(
            "
"
        )
    );
}
