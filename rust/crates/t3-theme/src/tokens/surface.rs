//! Code surfaces: the `--diffs-*` custom properties the diff and file views set on their
//! containers (`DIFF_SURFACE_THEME_UNSAFE_CSS`), resolved against a computed theme.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::expr::{Env, Expr, Lookup, Value, eval};
use super::table::TokenTable;
use crate::palette::Appearance;
use crate::syntax::syntax_theme_for;

struct SurfaceElement<'a> {
    own: BTreeMap<&'a str, &'a Expr>,
    parent: &'a BTreeMap<String, Value>,
    inputs: &'a BTreeMap<String, Value>,
    dark: bool,
    root_px: f64,
    memo: RefCell<BTreeMap<String, Option<Value>>>,
    active: RefCell<BTreeSet<String>>,
}

impl SurfaceElement<'_> {
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
        let inherited = || {
            self.inputs
                .get(name)
                .or_else(|| self.parent.get(name))
                .cloned()
        };
        let Some(expr) = self.own.get(name) else {
            return inherited();
        };
        if !self.active.borrow_mut().insert(name.to_owned()) {
            return None;
        }
        let value = eval(expr, self);
        self.active.borrow_mut().remove(name);
        if value.is_invalid() {
            inherited()
        } else {
            Some(value)
        }
    }
}

impl Env for SurfaceElement<'_> {
    fn lookup(&self, name: &str) -> Lookup {
        if self.active.borrow().contains(name) {
            return Lookup::Invalid;
        }
        self.computed(name).map_or(Lookup::Missing, Lookup::Value)
    }

    fn dark(&self) -> bool {
        self.dark
    }

    fn root_px(&self) -> f64 {
        self.root_px
    }
}

/// The values a diff or file container resolves to.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeSurface {
    values: BTreeMap<String, Value>,
}

impl CodeSurface {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn color(&self, name: &str) -> Option<crate::color::Color> {
        self.get(name).and_then(Value::color)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// Resolves the `--diffs-*` properties of the diff container in `appearance`.
///
/// `root` is the computed style of `<html>` (see [`super::compute`]). The highlighter's
/// theme supplies the base colours `--diffs-addition-base`, `--diffs-deletion-base` and
/// `--diffs-modified-base` unless the diff colour scheme overrides them with
/// `--diffs-addition-color-override` and friends, which `index.css` sets for blue-orange.
pub fn code_surface(
    table: &TokenTable,
    appearance: Appearance,
    root: &BTreeMap<String, Value>,
    root_px: f64,
) -> CodeSurface {
    let mut own: BTreeMap<&str, &Expr> = BTreeMap::new();
    if let Some(surface) = table.surfaces.first() {
        for declaration in &surface.declarations {
            own.insert(declaration.prop.as_str(), &declaration.value);
        }
    }
    let theme = syntax_theme_for(appearance);
    let color = |text: &str| crate::color::Color::parse(text).map(|color| Value::Color { color });
    let mut inputs = BTreeMap::new();
    let base = |override_name: &str, fallback: &str| {
        root.get(override_name).cloned().or_else(|| color(fallback))
    };
    if let Some(value) = base("--diffs-addition-color-override", &theme.style.created) {
        inputs.insert("--diffs-addition-base".to_owned(), value);
    }
    if let Some(value) = base("--diffs-deletion-color-override", &theme.style.deleted) {
        inputs.insert("--diffs-deletion-base".to_owned(), value);
    }
    if let Some(value) = base("--diffs-modified-color-override", &theme.style.modified) {
        inputs.insert("--diffs-modified-base".to_owned(), value);
    }
    let element = SurfaceElement {
        own,
        parent: root,
        inputs: &inputs,
        dark: appearance == Appearance::Dark,
        root_px,
        memo: RefCell::default(),
        active: RefCell::default(),
    };
    let mut values = BTreeMap::new();
    let names: Vec<String> = element.own.keys().map(|n| (*n).to_owned()).collect();
    for name in names.iter().chain(inputs.keys()) {
        if name.starts_with("--")
            && let Some(value) = element.computed(name)
        {
            values.insert(name.clone(), value);
        }
    }
    CodeSurface { values }
}
