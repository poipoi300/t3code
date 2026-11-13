//! Typed shadows and the glass and backdrop utilities (`surface-glass`, `dialog-glass`, ...).

use std::collections::BTreeMap;

use serde::Serialize;

use super::expr::{MapEnv, Value, eval};
use super::table::TokenTable;
use crate::color::Color;

/// One resolved shadow layer, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ShadowLayer {
    pub inset: bool,
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    pub spread: f64,
    pub color: Option<Color>,
}

/// Resolves a shadow token (`--shadow-md`, `--shadow-composer`, ...) against computed values.
pub fn resolve_shadow(
    table: &TokenTable,
    name: &str,
    values: &BTreeMap<String, Value>,
    dark: bool,
    root_px: f64,
) -> Option<Vec<ShadowLayer>> {
    let env = MapEnv {
        values,
        dark,
        root_px,
    };
    let px = |expr: &super::expr::Expr| eval(expr, &env).px(root_px);
    table
        .shadows
        .get(name)?
        .iter()
        .map(|layer| {
            Some(ShadowLayer {
                inset: layer.inset,
                x: px(&layer.x)?,
                y: px(&layer.y)?,
                blur: px(&layer.blur)?,
                spread: px(&layer.spread)?,
                color: layer.color.as_ref().and_then(|c| eval(c, &env).color()),
            })
        })
        .collect()
}

/// One declaration of a utility with its value computed.
#[derive(Clone, Debug, PartialEq)]
pub struct UtilityDeclaration {
    pub prop: String,
    /// `dark`/`light` for appearance variants, or a nested selector such as `&[data-variant="error"]`.
    pub variant: Option<String>,
    pub value: Value,
}

/// Resolves a utility's declarations for the appearance: appearance variants that do not apply
/// are left out, selector variants are kept with their selector in `variant`.
pub fn resolve_utility(
    table: &TokenTable,
    name: &str,
    values: &BTreeMap<String, Value>,
    dark: bool,
    root_px: f64,
) -> Option<Vec<UtilityDeclaration>> {
    let utility = table.utilities.iter().find(|u| u.name == name)?;
    let env = MapEnv {
        values,
        dark,
        root_px,
    };
    Some(
        utility
            .declarations
            .iter()
            .filter(|d| match d.variant.as_deref() {
                Some("dark") => dark,
                Some("light") => !dark,
                _ => true,
            })
            .map(|d| UtilityDeclaration {
                prop: d.prop.clone(),
                variant: d.variant.clone(),
                value: eval(&d.value, &env),
            })
            .collect(),
    )
}
