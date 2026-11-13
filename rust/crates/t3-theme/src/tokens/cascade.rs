//! Resolving the token declarations the way a browser does for `<html>` and for the
//! `[data-app-sidebar]` element beneath it: layers, specificity, source order,
//! inheritance of computed values, then `var()` substitution.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::expr::{Env, Lookup, Value, eval};
use super::table::{AttrOp, Declaration, Target, TokenTable};

/// The state of the page that selects which declarations apply.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleContext {
    /// The `dark` class on `<html>` (also what `light-dark()` follows).
    pub dark: bool,
    /// `data-theme-id` on `<html>`; `None` is the stock look (attribute absent).
    pub theme_id: Option<String>,
    pub diff_color_scheme: Option<String>,
    pub chat_width: Option<String>,
    /// Window Controls Overlay is active.
    pub wco: bool,
    pub electron_windows: bool,
    /// Viewport width in `rem` when responsive rules matter; `None` means narrower than every breakpoint.
    pub viewport_rem: Option<f64>,
    /// The root font size in pixels.
    pub root_px: f64,
}

impl StyleContext {
    pub fn new(dark: bool, theme_id: Option<&str>) -> Self {
        Self {
            dark,
            theme_id: theme_id.map(str::to_owned),
            diff_color_scheme: None,
            chat_width: None,
            wco: false,
            electron_windows: false,
            viewport_rem: None,
            root_px: 16.0,
        }
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        match name {
            "data-theme-id" => self.theme_id.as_deref(),
            "data-diff-color-scheme" => self.diff_color_scheme.as_deref(),
            "data-chat-width" => self.chat_width.as_deref(),
            _ => None,
        }
    }

    fn has_class(&self, class: &str) -> bool {
        match class {
            "dark" => self.dark,
            "wco" => self.wco,
            "electron-windows" => self.electron_windows,
            _ => false,
        }
    }

    fn matches(&self, declaration: &Declaration) -> bool {
        let when = &declaration.when;
        if when.dark.is_some_and(|dark| dark != self.dark) {
            return false;
        }
        if when.wco == Some(true) && !self.wco {
            return false;
        }
        if let Some(min) = when.min_width_rem
            && self.viewport_rem.is_none_or(|width| width < min)
        {
            return false;
        }
        let html = &declaration.html;
        html.classes.iter().all(|class| self.has_class(class))
            && html.attrs.iter().all(|test| {
                let actual = self.attribute(&test.name);
                match test.op {
                    AttrOp::Present => actual.is_some(),
                    AttrOp::Absent => actual.is_none(),
                    AttrOp::Equals => actual.is_some() && actual == test.value.as_deref(),
                    AttrOp::NotEquals => actual != test.value.as_deref(),
                }
            })
    }
}

/// Computed custom properties for both elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Computed {
    pub root: BTreeMap<String, Value>,
    pub sidebar: BTreeMap<String, Value>,
}

impl Computed {
    pub fn get(&self, sidebar: bool, name: &str) -> Option<&Value> {
        if sidebar {
            self.sidebar.get(name)
        } else {
            self.root.get(name)
        }
    }
}

type Winners<'a> = BTreeMap<&'a str, &'a Declaration>;

fn winners<'a>(table: &'a TokenTable, ctx: &StyleContext, target: Target) -> Winners<'a> {
    let mut best: Winners<'a> = BTreeMap::new();
    let key = |d: &Declaration| {
        (
            d.important,
            table.layer_rank(&d.layer),
            d.specificity,
            d.order,
        )
    };
    for declaration in table
        .declarations
        .iter()
        .filter(|d| d.target == target && ctx.matches(d))
    {
        // `!important` reverses layer precedence in browsers; the stylesheet only uses it on
        // diff surfaces, which are resolved separately, so ordinary precedence suffices here.
        match best.get(declaration.name.as_str()) {
            Some(current) if key(current) > key(declaration) => {}
            _ => {
                best.insert(declaration.name.as_str(), declaration);
            }
        }
    }
    best
}

struct Element<'a> {
    ctx: &'a StyleContext,
    own: Winners<'a>,
    inline: &'a BTreeMap<String, Value>,
    parent: Option<&'a Element<'a>>,
    memo: RefCell<BTreeMap<String, Option<Value>>>,
    active: RefCell<BTreeSet<String>>,
}

impl<'a> Element<'a> {
    fn computed(&self, name: &str) -> Option<Value> {
        if let Some(cached) = self.memo.borrow().get(name) {
            return cached.clone();
        }
        let result = self.compute(name);
        self.memo
            .borrow_mut()
            .insert(name.to_owned(), result.clone());
        result
    }

    fn compute(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.inline.get(name) {
            return Some(value.clone());
        }
        let inherited = || self.parent.and_then(|p| p.computed(name));
        let Some(declaration) = self.own.get(name) else {
            return inherited();
        };
        if !self.active.borrow_mut().insert(name.to_owned()) {
            // A reference cycle makes the property invalid at computed-value time.
            return None;
        }
        let value = eval(&declaration.value, self);
        self.active.borrow_mut().remove(name);
        if value.is_invalid() {
            inherited()
        } else {
            Some(value)
        }
    }
}

impl Env for Element<'_> {
    fn lookup(&self, name: &str) -> Lookup {
        if self.active.borrow().contains(name) {
            return Lookup::Invalid;
        }
        match self.computed(name) {
            Some(value) => Lookup::Value(value),
            None => Lookup::Missing,
        }
    }

    fn dark(&self) -> bool {
        self.ctx.dark
    }

    fn root_px(&self) -> f64 {
        self.ctx.root_px
    }
}

/// Resolves every token for `<html>` and for the sidebar element beneath it.
///
/// `inline` holds values set directly on `<html>` (`style.setProperty`), which outrank every
/// stylesheet rule: the `--app-theme-*` roles, `--appearance-contrast-*` and font overrides.
pub fn compute(
    table: &TokenTable,
    ctx: &StyleContext,
    inline: &BTreeMap<String, Value>,
) -> Computed {
    let empty = BTreeMap::new();
    let root = Element {
        ctx,
        own: winners(table, ctx, Target::Root),
        inline,
        parent: None,
        memo: RefCell::default(),
        active: RefCell::default(),
    };
    let sidebar = Element {
        ctx,
        own: winners(table, ctx, Target::Sidebar),
        inline: &empty,
        parent: Some(&root),
        memo: RefCell::default(),
        active: RefCell::default(),
    };
    let mut names: BTreeSet<&str> = table.declarations.iter().map(|d| d.name.as_str()).collect();
    names.extend(inline.keys().map(String::as_str));
    let mut out = Computed::default();
    for name in names {
        if let Some(value) = root.computed(name) {
            out.root.insert(name.to_owned(), value);
        }
        if let Some(value) = sidebar.computed(name) {
            out.sidebar.insert(name.to_owned(), value);
        }
    }
    out
}
