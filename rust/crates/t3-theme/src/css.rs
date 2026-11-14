//! Fetch guards shared by theme publication and parsed client stylesheets.

fn fetches_by_string(value: &str) -> bool {
    value.contains("image-set(")
        || value.match_indices("src(").any(|(at, _)| {
            at == 0 || !matches!(value.as_bytes()[at - 1], b'a'..=b'z' | b'0'..=b'9' | b'_')
        })
}

fn url_not_local(value: &str) -> bool {
    value.match_indices("url(").any(|(at, _)| {
        let address = value[at + 4..].trim_start_matches([' ', '\t', '\n', '\u{c}', '\r']);
        let address = address.strip_prefix(['\'', '"']).unwrap_or(address);
        !(address.starts_with("data:") || address.starts_with('#'))
    })
}

/// Apply to CSSOM-normalized declaration values; remaining escapes are refused.
pub fn theme_css_value_fetches(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains('\\') || fetches_by_string(&lower) || url_not_local(&lower)
}

/// Early publication feedback. The client's parsed declaration guard is authoritative.
pub fn theme_css_loads_remote_files(css: &str) -> bool {
    let mut code = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        let Some(end) = rest[start + 2..].find("*/") else {
            break;
        };
        code.push_str(&rest[..start]);
        rest = &rest[start + 2 + end + 2..];
    }
    code.push_str(rest);
    let code = code.to_ascii_lowercase();
    code.contains("@import") || fetches_by_string(&code) || url_not_local(&code)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssDeclaration {
    pub property: String,
    pub value: String,
    pub important: bool,
}

/// Rule shapes after a host CSS parser has normalized escapes and validated syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CssRule {
    Style {
        declarations: Vec<CssDeclaration>,
        children: Vec<CssRule>,
    },
    Group(Vec<CssRule>),
    Keyframes(Vec<Vec<CssDeclaration>>),
    Property {
        initial_value: String,
    },
    LayerStatement,
    Namespace,
    Unsupported,
}

fn strip_declarations(declarations: &mut Vec<CssDeclaration>) {
    declarations.retain(|declaration| !theme_css_value_fetches(&declaration.value));
}

/// The recursive CSSOM filtering policy; unsupported leaf rules fail closed.
pub fn strip_fetches(rules: &mut Vec<CssRule>) {
    rules.retain_mut(|rule| {
        match rule {
            CssRule::Style {
                declarations,
                children,
            } => {
                strip_declarations(declarations);
                strip_fetches(children);
            }
            CssRule::Group(children) => strip_fetches(children),
            CssRule::Keyframes(frames) => frames.iter_mut().for_each(strip_declarations),
            CssRule::Property { initial_value } => return !theme_css_value_fetches(initial_value),
            CssRule::Unsupported => return false,
            CssRule::LayerStatement | CssRule::Namespace => {}
        }
        true
    });
}

pub const THEME_GUIDE: &str = include_str!("../fixtures/theme-guide.txt");
pub const THEME_CSS_HOOKS: &[(&str, &str)] = &include!("../fixtures/theme-css-hooks.rs");
