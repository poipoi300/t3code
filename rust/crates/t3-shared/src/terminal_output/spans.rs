use super::{Attributes, terminal_output_plain_text};
use serde::{Deserialize, Serialize};

pub const DARK_PALETTE: [&str; 16] = [
    "#3b4048", "#f47067", "#57ab5a", "#c69026", "#539bf5", "#b083f0", "#39c5cf", "#adbac7",
    "#768390", "#ff938a", "#6bc46d", "#daaa3f", "#6cb6ff", "#dcbdfb", "#56d4dd", "#cdd9e5",
];
pub const LIGHT_PALETTE: [&str; 16] = [
    "#24292f", "#cf222e", "#116329", "#7d4e00", "#0550ae", "#8250df", "#1b7c83", "#6e7781",
    "#57606a", "#a40e26", "#1a7f37", "#633c01", "#0969da", "#6639ba", "#3192aa", "#8c959f",
];
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSpanStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_decoration_line: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalSpan {
    pub text: String,
    pub style: Option<TerminalSpanStyle>,
}
fn rgb(r: u32, g: u32, b: u32) -> String {
    format!("rgb({r},{g},{b})")
}
fn color(code: &str, base: u32, palette: &[&str; 16]) -> Option<String> {
    let parts: Vec<u32> = code.split(';').filter_map(|p| p.parse().ok()).collect();
    let head = *parts.first()?;
    if head == base + 8 {
        if parts.get(1) == Some(&5) {
            let index = *parts.get(2)?;
            if index < 16 {
                return Some(palette[index as usize].into());
            }
            if index >= 232 {
                let level = 8 + (index - 232) * 10;
                return Some(rgb(level, level, level));
            }
            let cube = index - 16;
            let channel = |v| if v == 0 { 0 } else { 55 + v * 40 };
            return Some(rgb(
                channel(cube / 36),
                channel(cube / 6 % 6),
                channel(cube % 6),
            ));
        }
        return Some(rgb(*parts.get(2)?, *parts.get(3)?, *parts.get(4)?));
    }
    if (base..=base + 7).contains(&head) {
        return Some(palette[(head - base) as usize].into());
    }
    if (base + 60..=base + 67).contains(&head) {
        return Some(palette[(head - base - 52) as usize].into());
    }
    None
}
fn parse_color(color: &str) -> Option<[f64; 3]> {
    if color.starts_with('#') && color.len() == 7 {
        return Some([
            u32::from_str_radix(&color[1..3], 16).ok()? as f64,
            u32::from_str_radix(&color[3..5], 16).ok()? as f64,
            u32::from_str_radix(&color[5..7], 16).ok()? as f64,
        ]);
    }
    let mut parts = color.strip_prefix("rgb(")?.strip_suffix(')')?.split(',');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}
fn luminance(rgb: [f64; 3]) -> f64 {
    let channel = |v: f64| {
        let s = v / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}
fn style(pen: &str, palette: &[&str; 16]) -> Option<TerminalSpanStyle> {
    if pen.is_empty() {
        return None;
    }
    let Attributes {
        bold,
        dim,
        italic,
        underline,
        strike,
        inverse,
        fg,
        bg,
    } = Attributes::from_pen(pen);
    let mut color = color(&fg, 30, palette);
    let mut background = self::color(&bg, 40, palette);
    if inverse {
        (color, background) = (background, Some(color.unwrap_or_else(|| palette[7].into())));
    }
    if let Some(bg) = background.as_deref().and_then(parse_color) {
        let fg = color.as_deref().and_then(parse_color);
        if fg.is_none_or(|fg| {
            let (a, b) = (luminance(fg), luminance(bg));
            (a.max(b) + 0.05) / (a.min(b) + 0.05) < 3.0
        }) {
            color = Some(
                if luminance(bg) > 0.35 {
                    "#1f2328"
                } else {
                    "#ffffff"
                }
                .into(),
            );
        }
    }
    Some(TerminalSpanStyle {
        color,
        background_color: background,
        font_weight: bold.then(|| "600".into()),
        opacity: dim.then_some(0.65),
        font_style: italic.then(|| "italic".into()),
        text_decoration_line: match (underline, strike) {
            (true, true) => Some("underline line-through".into()),
            (true, false) => Some("underline".into()),
            (false, true) => Some("line-through".into()),
            _ => None,
        },
    })
}
pub fn terminal_output_spans(
    text: &str,
    palette: &[&str; 16],
    max_spans: usize,
) -> Vec<TerminalSpan> {
    let mut spans = vec![];
    let mut cursor = 0;
    let mut pen = "";
    while cursor < text.len() && spans.len() < max_spans {
        let Some(escape) = text[cursor..].find("\x1b[").map(|i| i + cursor) else {
            break;
        };
        if escape > cursor {
            spans.push(TerminalSpan {
                text: text[cursor..escape].into(),
                style: style(pen, palette),
            });
        }
        let Some(end) = text[escape..].find('m').map(|i| i + escape) else {
            break;
        };
        let params = &text[escape + 2..end];
        pen = if params == "0" {
            ""
        } else {
            params.strip_prefix("0;").unwrap_or(params)
        };
        cursor = end + 1;
    }
    if cursor < text.len() {
        spans.push(TerminalSpan {
            text: if spans.len() >= max_spans {
                terminal_output_plain_text(&text[cursor..])
            } else {
                text[cursor..].into()
            },
            style: if spans.len() >= max_spans {
                None
            } else {
                style(pen, palette)
            },
        });
    }
    spans
}
