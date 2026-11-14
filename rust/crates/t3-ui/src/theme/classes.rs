//! The web app's Tailwind class strings, copied verbatim from the TSX, made ready for NSH's own
//! Tailwind engine (`.app_class(..)`), which parses about 90% of the web's class uses unchanged.
//!
//! [`tw`] rewrites only the families the web defines itself or resolves from attributes:
//! - `text-2xs`..`text-5xs`: `index.css`'s sizes, as literal size and line height;
//! - `any-hover:` and `group-any-hover/<name>:`: the web's custom variant, NSH's `hover`;
//! - `light:` (and `not-dark:`) on a colour: a token that is that colour in light and clear in
//!   dark; a non-colour `light:` utility applies in both only beside such a colour (invisible in
//!   dark), and is otherwise left for the parse check to reject;
//! - `[var(--x)]` values: Tailwind's `(--x)` form, which NSH resolves against the tokens;
//! - `data-[k=v]:`, `data-k:`, `aria-[k=v]:`, `aria-k:` and their `group-`/`in-` forms: kept when
//!   the attributes passed in say so (the element's, or the ancestor's the variant names), as the
//!   TSX sets them;
//! - inside the sidebar ([`Scope::Sidebar`]), colours the sidebar redeclares use its values.
//!
//! Everything else passes through. What NSH cannot express (pseudo-elements, selector variants,
//! container queries, `has-*`) is ported by hand per the table in `ui-package.md`; the
//! `every_class_literal_in_the_crate_parses` test fails on any class left unparsed.

use nsh_core::prelude::SharedString;

/// Where a class string sits: the sidebar subtree redeclares some colours (`[data-sidebar]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Root,
    Sidebar,
}

/// The prefix of a colour's sidebar-scoped value among the tokens.
pub const SIDEBAR_COLOR_PREFIX: &str = "sb-";
/// The prefix of a colour's light-only value (clear in dark), for `light:`.
pub const LIGHT_COLOR_PREFIX: &str = "light-";

/// `classes` for an element outside the sidebar with no attributes a variant tests.
pub fn tw(classes: &str) -> SharedString {
    tw_with(Scope::Root, classes, &[])
}

/// `classes` for an element in `scope` carrying `attrs` (`("data-active", "true")`,
/// `("aria-current", "page")`), as the TSX sets them.
pub fn tw_with(scope: Scope, classes: &str, attrs: &[(&str, &str)]) -> SharedString {
    let tokens = super::current_tokens();
    rewrite(
        classes,
        scope,
        attrs,
        &|name| tokens.color(name).is_some(),
        false,
    )
    .into()
}

/// The TSX's `cn(..)`: the parts that are present, space separated.
pub fn cn<'a>(parts: impl IntoIterator<Item = Option<&'a str>>) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rewrite. `is_color` says whether a name is a colour token. With `keep_conditional`, a
/// class behind an attribute variant is kept whatever the attributes (for the parse check).
pub(crate) fn rewrite(
    classes: &str,
    scope: Scope,
    attrs: &[(&str, &str)],
    is_color: &dyn Fn(&str) -> bool,
    keep_conditional: bool,
) -> String {
    // A non-colour `light:` utility is safe in dark only beside a light-only colour.
    let light_color = classes.split_whitespace().any(|class| {
        let parts = split_variants(class.trim_start_matches('!'));
        parts.len() > 1
            && parts[..parts.len() - 1]
                .iter()
                .any(|v| *v == "light" || *v == "not-dark")
            && color_utility(parts[parts.len() - 1], is_color).is_some()
    });
    let mut out: Vec<String> = Vec::new();
    for class in classes.split_whitespace() {
        let (important, class) = match class.strip_prefix('!') {
            Some(rest) => ("!", rest),
            None => ("", class),
        };
        let mut parts = split_variants(class);
        let Some(utility) = parts.pop() else {
            continue;
        };
        let mut variants = Vec::with_capacity(parts.len());
        let mut light = false;
        let mut applies = true;
        for variant in parts {
            match resolve_variant(variant, attrs) {
                Variant::Keep(v) => variants.push(v),
                Variant::Light => light = true,
                Variant::Condition(holds) => applies &= holds || keep_conditional,
            }
        }
        if !applies {
            continue;
        }
        let mut utility = rewrite_utility(utility);
        if utility.is_empty() {
            continue;
        }
        let color = color_utility(&utility, is_color);
        if light && color.is_none() && !light_color {
            variants.insert(0, "light".to_owned());
            light = false;
        }
        if let Some((prefix, color, alpha)) = color {
            let mut color = color.to_owned();
            if scope == Scope::Sidebar && is_color(&format!("{SIDEBAR_COLOR_PREFIX}{color}")) {
                color = format!("{SIDEBAR_COLOR_PREFIX}{color}");
            }
            if light {
                color = format!("{LIGHT_COLOR_PREFIX}{color}");
            }
            utility = format!("{prefix}{color}{alpha}");
        }
        let chain: String = variants.iter().map(|v| format!("{v}:")).collect();
        out.push(format!("{important}{chain}{utility}"));
    }
    out.join(" ")
}

enum Variant {
    Keep(String),
    Light,
    Condition(bool),
}

fn resolve_variant(variant: &str, attrs: &[(&str, &str)]) -> Variant {
    if variant == "light" || variant == "not-dark" {
        return Variant::Light;
    }
    if variant == "any-hover" {
        return Variant::Keep("hover".into());
    }
    if let Some(group) = variant.strip_prefix("group-any-hover") {
        return Variant::Keep(format!("group-hover{group}"));
    }
    let scoped = ["group-", "in-"]
        .iter()
        .find_map(|prefix| variant.strip_prefix(prefix))
        .unwrap_or(variant);
    // A named group's attribute variant (`group-data-[x]/row`) tests the same attributes.
    let scoped = scoped.split_once('/').map_or(scoped, |(base, _)| base);
    for kind in ["data-", "aria-"] {
        let Some(rest) = scoped.strip_prefix(kind) else {
            continue;
        };
        if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let (name, expected) = match inner.split_once('=') {
                Some((name, value)) => (name, Some(value.trim_matches(['"', '\'']))),
                None => (inner, None),
            };
            return Variant::Condition(attribute_holds(attrs, &format!("{kind}{name}"), expected));
        }
        // NSH reads `aria-pressed` and friends from the accessibility state itself.
        if kind == "aria-" && NATIVE_ARIA.contains(&rest) && scoped.len() == variant.len() {
            return Variant::Keep(variant.into());
        }
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            let expected = (kind == "aria-").then_some("true");
            return Variant::Condition(attribute_holds(attrs, &format!("{kind}{rest}"), expected));
        }
    }
    Variant::Keep(variant.into())
}

const NATIVE_ARIA: [&str; 5] = ["pressed", "busy", "checked", "invalid", "selected"];

/// `expected` `None` is Tailwind's presence test (`data-active`); `Some` compares the value.
fn attribute_holds(attrs: &[(&str, &str)], name: &str, expected: Option<&str>) -> bool {
    attrs
        .iter()
        .find(|(key, _)| *key == name)
        .is_some_and(|(_, value)| expected.is_none_or(|expected| *value == expected))
}

/// `hover:data-[a=b]:bg-x` into `hover`, `data-[a=b]`, `bg-x`: colons inside brackets or
/// parentheses belong to the part.
fn split_variants(class: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (index, c) in class.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ':' if depth == 0 => {
                parts.push(&class[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&class[start..]);
    parts
}

fn rewrite_utility(utility: &str) -> String {
    if let Some(size) = TEXT_SIZES
        .iter()
        .find_map(|(name, size)| (utility == *name).then_some(size))
    {
        return (*size).to_owned();
    }
    // NSH has one overflow for both axes (`overflow-x-hidden` already clips both): clipping one
    // axis clips the box, and making one axis visible is NSH's default.
    match utility {
        "overflow-x-clip" | "overflow-y-clip" => return "overflow-hidden".to_owned(),
        "overflow-x-visible" | "overflow-y-visible" => return String::new(),
        _ => {}
    }
    // `transition-[a,b]`: NSH animates the families colours, opacity, shadow and transform, all of
    // which plain `transition` covers.
    if let Some(list) = utility
        .strip_prefix("transition-[")
        .and_then(|rest| rest.strip_suffix(']'))
        && list.split(',').all(|property| TRANSITION_FAMILIES.contains(&property.trim()))
    {
        return "transition".to_owned();
    }
    // `h-[var(--x)]` is Tailwind v3's spelling of `h-(--x)`.
    if let Some((head, rest)) = utility.split_once("[var(")
        && let Some(name) = rest.strip_suffix(")]")
        && !name.contains(',')
    {
        return format!("{head}({name})");
    }
    utility.to_owned()
}

/// CSS properties NSH's `transition` animates (its colour, opacity, shadow and transform parts).
const TRANSITION_FAMILIES: [&str; 9] = [
    "color",
    "background-color",
    "border-color",
    "outline-color",
    "fill",
    "stroke",
    "opacity",
    "box-shadow",
    "transform",
];

/// `index.css`'s `--text-2xs`..`--text-5xs`, size and line height, at 16 px to the rem.
const TEXT_SIZES: [(&str, &str); 4] = [
    ("text-2xs", "text-[11px]/[16px]"),
    ("text-3xs", "text-[10px]/[14px]"),
    ("text-4xs", "text-[8px]/[8px]"),
    ("text-5xs", "text-[7px]/[7px]"),
];

const COLOR_UTILITIES: [&str; 15] = [
    "bg-",
    "text-",
    "border-",
    "ring-",
    "outline-",
    "from-",
    "via-",
    "to-",
    "fill-",
    "stroke-",
    "shadow-",
    "decoration-",
    "caret-",
    "accent-",
    "divide-",
];

/// A colour utility's prefix, colour token and `/alpha` suffix, when the name is a colour.
fn color_utility<'a>(
    utility: &'a str,
    is_color: &dyn Fn(&str) -> bool,
) -> Option<(&'a str, &'a str, &'a str)> {
    let prefix = COLOR_UTILITIES
        .iter()
        .find(|prefix| utility.starts_with(**prefix))?;
    let rest = &utility[prefix.len()..];
    let (color, alpha) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, ""),
    };
    is_color(color).then_some((&utility[..prefix.len()], color, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(name: &str) -> bool {
        matches!(
            name,
            "foreground"
                | "sidebar-border"
                | "sb-foreground"
                | "sidebar-row-active"
                | "secondary-label"
        )
    }

    fn run(classes: &str, scope: Scope, attrs: &[(&str, &str)]) -> String {
        rewrite(classes, scope, attrs, &colors, false)
    }

    #[test]
    fn plain_classes_pass_through_unchanged() {
        let classes = "flex h-5 min-w-0 items-center gap-1.5 hover:bg-foreground/10 sm:px-3";
        assert_eq!(run(classes, Scope::Root, &[]), classes);
    }

    #[test]
    fn the_webs_own_sizes_and_variants_become_what_nsh_parses() {
        assert_eq!(
            run(
                "text-2xs group-any-hover/sidebar-row:opacity-100 any-hover:underline",
                Scope::Root,
                &[]
            ),
            "text-[11px]/[16px] group-hover/sidebar-row:opacity-100 hover:underline"
        );
        assert_eq!(
            run("h-[var(--workspace-topbar-height)]", Scope::Root, &[]),
            "h-(--workspace-topbar-height)"
        );
    }

    #[test]
    fn light_colours_become_light_only_tokens_and_their_companions_stay() {
        assert_eq!(
            run(
                "light:ring-1 light:ring-inset light:ring-sidebar-border/70",
                Scope::Root,
                &[]
            ),
            "ring-1 ring-inset ring-light-sidebar-border/70"
        );
        // Alone, it would show in dark too: left as written, which NSH does not parse.
        assert_eq!(run("light:shadow-sm", Scope::Root, &[]), "light:shadow-sm");
    }

    #[test]
    fn attribute_variants_follow_the_attributes_the_tsx_sets() {
        let classes = "p-1 data-[active=true]:bg-sidebar-row-active data-pressed:opacity-50 \
                       aria-[current=page]:font-medium aria-disabled:opacity-60";
        assert_eq!(run(classes, Scope::Root, &[]), "p-1");
        assert_eq!(
            run(
                classes,
                Scope::Root,
                &[
                    ("data-active", "true"),
                    ("data-pressed", ""),
                    ("aria-current", "page"),
                    ("aria-disabled", "true"),
                ]
            ),
            "p-1 bg-sidebar-row-active opacity-50 font-medium opacity-60"
        );
        assert_eq!(
            run(classes, Scope::Root, &[("data-active", "false")]),
            "p-1"
        );
        // A state NSH reads from the accessibility tree stays a variant.
        assert_eq!(
            run("aria-selected:bg-foreground", Scope::Root, &[]),
            "aria-selected:bg-foreground"
        );
        // An ancestor's, named or not, as the porter passes it.
        assert_eq!(
            run(
                "group-data-[state=open]/menu:rotate-90 in-data-[side=left]:pl-2",
                Scope::Root,
                &[("data-state", "open"), ("data-side", "left")]
            ),
            "rotate-90 pl-2"
        );
    }

    #[test]
    fn the_sidebar_scope_takes_the_colours_the_sidebar_redeclares() {
        assert_eq!(
            run(
                "text-foreground hover:text-secondary-label",
                Scope::Sidebar,
                &[]
            ),
            "text-sb-foreground hover:text-secondary-label"
        );
        assert_eq!(run("text-foreground", Scope::Root, &[]), "text-foreground");
    }

    #[test]
    fn cn_joins_the_parts_that_are_present() {
        assert_eq!(
            cn([Some("a b"), None, Some(""), true.then_some("c")]),
            "a b c"
        );
    }

    /// Every string literal inside a `tw(..)`, `tw_with(..)`, `cn(..)` or `class(..)` call in `src`.
    fn class_literals() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root];
        let mut found = Vec::new();
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(
                    std::fs::read_dir(&path)
                        .expect("dir")
                        .map(|e| e.expect("entry").path()),
                );
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("classes.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            // `class(` is the local helper rows use for `tw_with` with fixed arguments.
            for call in ["tw(", "tw_with(", "cn(", "class("] {
                let mut from = 0;
                while let Some(at) = text[from..].find(call) {
                    let start = from + at + call.len();
                    from = start;
                    let boundary = text[..start - call.len()].chars().next_back();
                    if boundary.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let mut depth = 1;
                    let mut end = start;
                    let mut in_string = false;
                    for (index, c) in text[start..].char_indices() {
                        match c {
                            '"' => in_string = !in_string,
                            '(' if !in_string => depth += 1,
                            ')' if !in_string => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            end = start + index;
                            break;
                        }
                    }
                    // `tw_with`'s third argument is attributes, not classes.
                    let region = &text[start..end];
                    let region = match call {
                        "tw_with(" => region.split("&[").next().unwrap_or(region),
                        _ => region,
                    };
                    // A compared value or a match pattern is not a class list.
                    let pieces: Vec<&str> = region.split('"').collect();
                    for (index, piece) in pieces.iter().enumerate() {
                        let before = index.checked_sub(1).map_or("", |i| pieces[i].trim_end());
                        let after = pieces.get(index + 1).map_or("", |p| p.trim_start());
                        let not_classes = before.ends_with("==")
                            || before.ends_with("!=")
                            || after.starts_with("=>");
                        if index % 2 == 1 && !piece.trim().is_empty() && !not_classes {
                            found.push((path.display().to_string(), (*piece).to_owned()));
                        }
                    }
                }
            }
        }
        found
    }

    #[test]
    fn every_class_literal_in_the_crate_parses() {
        let tokens = super::super::tokens_for(&super::super::ThemeChoice::default());
        let mut theme = nsh_core::tailwind::Theme::default();
        theme.set_tokens(&tokens);
        let is_color = |name: &str| tokens.color(name).is_some();
        let literals = class_literals();
        assert!(!literals.is_empty(), "the scan found no class literals");
        let mut failures = Vec::new();
        for (file, literal) in &literals {
            for scope in [Scope::Root, Scope::Sidebar] {
                let classes = rewrite(literal, scope, &[], &is_color, true);
                let list = nsh_core::tailwind::parse(&theme, &classes);
                if !list.unknown().is_empty() {
                    failures.push(format!("{file}: {:?} in {literal:?}", list.unknown()));
                }
                // A `(--name)` parses whatever the name; it must also be a token to paint.
                for reference in classes.split("(--").skip(1) {
                    let name = reference.split(')').next().unwrap_or_default();
                    if tokens.color(name).is_none() && tokens.number(name).is_none() {
                        failures.push(format!("{file}: no token --{name} in {literal:?}"));
                    }
                }
            }
        }
        failures.dedup();
        assert!(
            failures.is_empty(),
            "classes NSH cannot parse (port them by hand):
{}",
            failures.join(
                "
"
            )
        );
    }

    #[test]
    fn the_rewrites_produce_classes_nsh_parses() {
        let tokens = super::super::tokens_for(&super::super::ThemeChoice::default());
        let mut theme = nsh_core::tailwind::Theme::default();
        theme.set_tokens(&tokens);
        let is_color = |name: &str| tokens.color(name).is_some();
        let classes = rewrite(
            "text-2xs text-3xs text-4xs text-5xs group-any-hover/sidebar-row:opacity-100              light:ring-1 light:ring-inset light:ring-sidebar-border/70 bg-sidebar-row-active              text-secondary-label px-(--sidebar-row-content-inset) h-[var(--workspace-topbar-height)]",
            Scope::Sidebar,
            &[],
            &is_color,
            false,
        );
        let list = nsh_core::tailwind::parse(&theme, &classes);
        assert!(
            list.unknown().is_empty(),
            "{:?} in {classes}",
            list.unknown()
        );
    }
}
