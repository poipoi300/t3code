//! CSS value expressions and their evaluation (`var()`, `calc()`, `color-mix()`,
//! `light-dark()`, `--alpha()` and relative colours), as `index.css` uses them.

use serde::{Deserialize, Serialize};

use crate::color::{Color, MixSpace, Space, color_mix};

/// A parsed CSS value (see `tools/css-value.mjs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "camelCase")]
pub enum Expr {
    Empty,
    Color {
        c: Color,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Var {
        name: String,
        fallback: Option<Box<Expr>>,
    },
    Mix {
        space: MixSpace,
        a: Box<Expr>,
        pa: Option<Box<Expr>>,
        b: Box<Expr>,
        pb: Option<Box<Expr>>,
    },
    /// `--alpha(color / pct)`: the colour at `pct` opacity.
    Alpha {
        color: Box<Expr>,
        pct: Box<Expr>,
    },
    /// `rgb(from color r g b / alpha)`: the same colour with another alpha.
    Relative {
        from: Box<Expr>,
        alpha: Option<Box<Expr>>,
    },
    LightDark {
        light: Box<Expr>,
        dark: Box<Expr>,
    },
    Length {
        px: f64,
        rem: f64,
        #[serde(default)]
        vw: f64,
    },
    Number {
        v: f64,
    },
    Percent {
        v: f64,
    },
    Duration {
        ms: f64,
    },
    Angle {
        deg: f64,
    },
    Dimension {
        v: f64,
        unit: String,
    },
    Calc {
        op: String,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    List {
        sep: String,
        items: Vec<Expr>,
    },
    Keyword {
        v: String,
    },
    String {
        v: String,
    },
    CurrentColor,
    Initial {
        v: String,
    },
    CubicBezier {
        p: [f64; 4],
    },
    Steps {
        n: f64,
        jump: String,
    },
    Env {
        name: String,
        fallback: Option<Box<Expr>>,
    },
    Fn {
        name: String,
        args: Vec<Expr>,
    },
    /// A filter function such as `blur(12px)` or `saturate(1.14)`.
    Filter {
        name: String,
        arg: Box<Expr>,
    },
    Raw {
        text: String,
    },
}

/// A computed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Value {
    Color {
        color: Color,
    },
    /// `px + rem * root font size + vw * viewport width / 100`.
    Length {
        px: f64,
        rem: f64,
        #[serde(default)]
        vw: f64,
    },
    Number {
        v: f64,
    },
    Percent {
        v: f64,
    },
    Duration {
        ms: f64,
    },
    Angle {
        deg: f64,
    },
    Dimension {
        v: f64,
        unit: String,
    },
    List {
        sep: String,
        items: Vec<Value>,
    },
    Keyword {
        v: String,
    },
    Text {
        v: String,
    },
    CubicBezier {
        p: [f64; 4],
    },
    Steps {
        n: f64,
        jump: String,
    },
    CurrentColor,
    /// A filter function with its evaluated argument.
    Filter {
        name: String,
        arg: Box<Value>,
    },
    /// Text kept as written (gradients, URLs, anything with unresolved references).
    Raw {
        text: String,
    },
    /// The guaranteed-invalid value.
    Invalid,
}

impl Value {
    pub fn color(&self) -> Option<Color> {
        match self {
            Self::Color { color } => Some(*color),
            _ => None,
        }
    }

    pub fn px(&self, root_px: f64) -> Option<f64> {
        match self {
            Self::Length { px, rem, vw } if *vw == 0.0 => Some(px + rem * root_px),
            Self::Number { v } if *v == 0.0 => Some(0.0),
            _ => None,
        }
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Number { v } => Some(*v),
            _ => None,
        }
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }
}

/// What a `var()` reference finds on the element being computed.
pub enum Lookup {
    Value(Value),
    /// Declared, but invalid at computed-value time.
    Invalid,
    Missing,
}

pub trait Env {
    fn lookup(&self, name: &str) -> Lookup;
    /// Whether `light-dark()` takes its second argument.
    fn dark(&self) -> bool;
    /// The root font size in pixels (for comparing `rem` with `px`).
    fn root_px(&self) -> f64;
}

fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number { v } | Value::Percent { v } => Some(*v),
        _ => None,
    }
}

fn percent_of(value: &Value) -> Option<f64> {
    match value {
        Value::Percent { v } => Some(*v),
        _ => None,
    }
}

fn calc(op: &str, l: Value, r: Value) -> Value {
    use Value::{Duration, Length, Number, Percent};
    match (op, l, r) {
        (
            "+",
            Length {
                px: a,
                rem: b,
                vw: e,
            },
            Length {
                px: c,
                rem: d,
                vw: f,
            },
        ) => Length {
            px: a + c,
            rem: b + d,
            vw: e + f,
        },
        (
            "-",
            Length {
                px: a,
                rem: b,
                vw: e,
            },
            Length {
                px: c,
                rem: d,
                vw: f,
            },
        ) => Length {
            px: a - c,
            rem: b - d,
            vw: e - f,
        },
        ("+", Number { v: a }, Number { v: b }) => Number { v: a + b },
        ("-", Number { v: a }, Number { v: b }) => Number { v: a - b },
        ("+", Percent { v: a }, Percent { v: b }) => Percent { v: a + b },
        ("-", Percent { v: a }, Percent { v: b }) => Percent { v: a - b },
        ("+", Duration { ms: a }, Duration { ms: b }) => Duration { ms: a + b },
        ("-", Duration { ms: a }, Duration { ms: b }) => Duration { ms: a - b },
        ("*", Length { px, rem, vw }, Number { v })
        | ("*", Number { v }, Length { px, rem, vw }) => Length {
            px: px * v,
            rem: rem * v,
            vw: vw * v,
        },
        ("/", Length { px, rem, vw }, Number { v }) if v != 0.0 => Length {
            px: px / v,
            rem: rem / v,
            vw: vw / v,
        },
        ("*", Number { v: a }, Number { v: b }) => Number { v: a * b },
        ("/", Number { v: a }, Number { v: b }) if b != 0.0 => Number { v: a / b },
        ("*", Percent { v: a }, Number { v: b }) | ("*", Number { v: b }, Percent { v: a }) => {
            Percent { v: a * b }
        }
        ("/", Percent { v: a }, Number { v: b }) if b != 0.0 => Percent { v: a / b },
        _ => Value::Invalid,
    }
}

fn alpha_fraction(value: &Value) -> Option<f64> {
    match value {
        Value::Percent { v } => Some(v / 100.0),
        Value::Number { v } => Some(*v),
        _ => None,
    }
}

/// Evaluates an expression on an element.
pub fn eval(expr: &Expr, env: &dyn Env) -> Value {
    match expr {
        Expr::Empty => Value::Invalid,
        Expr::Color { c, .. } => Value::Color { color: *c },
        Expr::Var { name, fallback } => match env.lookup(name) {
            Lookup::Value(value) => value,
            Lookup::Invalid | Lookup::Missing => {
                fallback.as_deref().map_or(Value::Invalid, |f| eval(f, env))
            }
        },
        Expr::Mix {
            space,
            a,
            pa,
            b,
            pb,
        } => {
            let (Some(ca), Some(cb)) = (eval(a, env).color(), eval(b, env).color()) else {
                return Value::Invalid;
            };
            let pct = |p: &Option<Box<Expr>>| -> Option<Option<f64>> {
                match p {
                    None => Some(None),
                    Some(e) => percent_of(&eval(e, env)).map(Some),
                }
            };
            let (Some(pa), Some(pb)) = (pct(pa), pct(pb)) else {
                return Value::Invalid;
            };
            Value::Color {
                color: color_mix(*space, ca, pa, cb, pb),
            }
        }
        Expr::Alpha { color, pct } => {
            let (Some(c), Some(p)) = (eval(color, env).color(), percent_of(&eval(pct, env))) else {
                return Value::Invalid;
            };
            Value::Color {
                color: color_mix(MixSpace::Oklab, c, Some(p), Color::TRANSPARENT, None),
            }
        }
        Expr::Relative { from, alpha } => {
            let Some(c) = eval(from, env).color() else {
                return Value::Invalid;
            };
            let srgb = c.to_space(Space::Srgb);
            let alpha = match alpha {
                None => Some(srgb.alpha),
                Some(e) => alpha_fraction(&eval(e, env)),
            };
            match alpha {
                Some(a) => Value::Color {
                    color: Color::new(Space::Srgb, srgb.c, a.clamp(0.0, 1.0)),
                },
                None => Value::Invalid,
            }
        }
        Expr::LightDark { light, dark } => eval(if env.dark() { dark } else { light }, env),
        Expr::Length { px, rem, vw } => Value::Length {
            px: *px,
            rem: *rem,
            vw: *vw,
        },
        Expr::Number { v } => Value::Number { v: *v },
        Expr::Percent { v } => Value::Percent { v: *v },
        Expr::Duration { ms } => Value::Duration { ms: *ms },
        Expr::Angle { deg } => Value::Angle { deg: *deg },
        Expr::Dimension { v, unit } => Value::Dimension {
            v: *v,
            unit: unit.clone(),
        },
        Expr::Calc { op, l, r } => {
            let (l, r) = (eval(l, env), eval(r, env));
            if l.is_invalid() || r.is_invalid() {
                Value::Invalid
            } else {
                calc(op, l, r)
            }
        }
        Expr::List { sep, items } => Value::List {
            sep: sep.clone(),
            items: items.iter().map(|i| eval(i, env)).collect(),
        },
        Expr::Keyword { v } => Value::Keyword { v: v.clone() },
        Expr::String { v } => Value::Text { v: v.clone() },
        Expr::CurrentColor => Value::CurrentColor,
        Expr::Initial { .. } => Value::Invalid,
        Expr::CubicBezier { p } => Value::CubicBezier { p: *p },
        Expr::Steps { n, jump } => Value::Steps {
            n: *n,
            jump: jump.clone(),
        },
        Expr::Env { fallback, .. } => match fallback {
            Some(f) => eval(f, env),
            // Safe-area and titlebar insets are zero outside notched or overlay-controls windows.
            None => Value::Length {
                px: 0.0,
                rem: 0.0,
                vw: 0.0,
            },
        },
        Expr::Fn { name, args } => {
            let values: Vec<Value> = args.iter().map(|a| eval(a, env)).collect();
            let root = env.root_px();
            let pxs: Option<Vec<f64>> = values.iter().map(|v| v.px(root)).collect();
            let Some(pxs) = pxs else {
                return Value::Invalid;
            };
            let pick = |best: fn(f64, f64) -> bool| {
                let mut chosen = 0;
                for i in 1..pxs.len() {
                    if best(pxs[i], pxs[chosen]) {
                        chosen = i;
                    }
                }
                values.get(chosen).cloned().unwrap_or(Value::Invalid)
            };
            match name.as_str() {
                "max" if !values.is_empty() => pick(|a, b| a > b),
                "min" if !values.is_empty() => pick(|a, b| a < b),
                "clamp" if values.len() == 3 => {
                    let clamped = pxs[1].max(pxs[0]).min(pxs[2]);
                    let index = pxs
                        .iter()
                        .position(|p| (*p - clamped).abs() < 1e-9)
                        .unwrap_or(1);
                    values[index].clone()
                }
                _ => Value::Invalid,
            }
        }
        Expr::Raw { text } => Value::Raw { text: text.clone() },
        Expr::Filter { name, arg } => Value::Filter {
            name: name.clone(),
            arg: Box::new(eval(arg, env)),
        },
    }
}

/// An environment backed by computed values, for evaluating expressions outside the cascade.
pub struct MapEnv<'a> {
    pub values: &'a std::collections::BTreeMap<String, Value>,
    pub dark: bool,
    pub root_px: f64,
}

impl Env for MapEnv<'_> {
    fn lookup(&self, name: &str) -> Lookup {
        self.values
            .get(name)
            .cloned()
            .map_or(Lookup::Missing, Lookup::Value)
    }

    fn dark(&self) -> bool {
        self.dark
    }

    fn root_px(&self) -> f64 {
        self.root_px
    }
}

/// Convenience for numeric tokens.
pub fn as_number(value: &Value) -> Option<f64> {
    number_of(value)
}
