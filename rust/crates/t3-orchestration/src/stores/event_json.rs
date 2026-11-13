//! Persisted JSON is the TypeScript server's: numbers are doubles, dates are Effect's
//! `DateTimeUtcFromString` encoding, and object keys follow schema declaration order.
use crate::error::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};
use std::{collections::HashMap, sync::OnceLock};

/// Typed decoding of persisted JSON. Numbers are first given their JavaScript value, so decoding
/// does not depend on serde_json's `arbitrary_precision` feature: with it, 128-bit integers and
/// non-shortest float spellings cannot pass through serde's internally tagged enum buffering.
pub(crate) fn decode<T: DeserializeOwned>(mut value: Value) -> Result<T> {
    normalize_numbers(&mut value);
    Ok(serde_json::from_value(value)?)
}

pub(crate) fn parse(text: &str) -> Result<Value> {
    Ok(serde_json::from_str(text)?)
}

/// Rewrites every number as the double `JSON.parse` yields, kept as an integer when exact.
pub(crate) fn normalize_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(normalized) = javascript_value(number) {
                *number = normalized;
            }
        }
        Value::Array(values) => values.iter_mut().for_each(normalize_numbers),
        Value::Object(values) => values.values_mut().for_each(normalize_numbers),
        _ => {}
    }
}

fn javascript_value(number: &Number) -> Option<Number> {
    let double = number.as_f64().filter(|double| double.is_finite())?;
    // 2^64 is exactly representable and is the first double outside u64.
    if double.fract() == 0.0 && double >= -(2f64.powi(63)) && double < 2f64.powi(64) {
        return Some(if double < 0.0 {
            Number::from(double as i64)
        } else {
            Number::from(double as u64)
        });
    }
    Number::from_f64(double)
}

/// `JSON.stringify` of an already-encoded value.
pub(crate) fn encode_json(value: &Value) -> Result<String> {
    fn write(value: &Value, output: &mut String) -> Result<()> {
        match value {
            Value::Number(number) => output.push_str(&javascript_number(number)?),
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                let mut fields: Vec<_> = values.iter().collect();
                fields.sort_by_key(|(key, _)| {
                    key.parse::<u32>()
                        .ok()
                        .filter(|index| *index != u32::MAX && index.to_string() == **key)
                        .map_or((1, 0), |index| (0, index))
                });
                output.push('{');
                for (index, (key, value)) in fields.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key)?);
                    output.push(':');
                    write(value, output)?;
                }
                output.push('}');
            }
            _ => output.push_str(&serde_json::to_string(value)?),
        }
        Ok(())
    }
    let mut output = String::new();
    write(value, &mut output)?;
    Ok(output)
}

fn javascript_number(number: &Number) -> Result<String> {
    let number = number
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| Error::Decode {
            entity: "event number",
            detail: "number is outside the JavaScript number domain".into(),
        })?;
    if number == 0.0 {
        return Ok("0".into());
    }
    let raw = serde_json::to_string(&number.abs())?;
    let (mantissa, exponent) = raw.split_once('e').unwrap_or((&raw, "0"));
    let exponent: i32 = exponent.parse().map_err(|_| Error::Decode {
        entity: "event number",
        detail: "invalid decimal exponent".into(),
    })?;
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let mut digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading);
    while digits.ends_with('0') {
        digits.pop();
    }
    let position = decimal + exponent - leading as i32;
    let mut result = if number.is_sign_negative() {
        "-".to_owned()
    } else {
        String::new()
    };
    if position > 0 && position <= 21 {
        let at = position as usize;
        if at >= digits.len() {
            result.push_str(&digits);
            result.extend(std::iter::repeat_n('0', at - digits.len()));
        } else {
            result.push_str(&digits[..at]);
            result.push('.');
            result.push_str(&digits[at..]);
        }
    } else if position > -6 && position <= 0 {
        result.push_str("0.");
        result.extend(std::iter::repeat_n('0', (-position) as usize));
        result.push_str(&digits);
    } else {
        result.push_str(&digits[..1]);
        if digits.len() > 1 {
            result.push('.');
            result.push_str(&digits[1..]);
        }
        let exponent = position - 1;
        result.push('e');
        if exponent >= 0 {
            result.push('+');
        }
        result.push_str(&exponent.to_string());
    }
    Ok(result)
}

const MAX_TIME_MS: i64 = 8_640_000_000_000_000;

/// Effect's `DateTimeUtcFromString` decode then encode: a zone-less input gets `Z` appended
/// (`effect/internal/dateTime.js` `hasZone`), V8's ISO grammar parses it (day overflow rolls into
/// the next month, `24:00` is the next midnight), and `toISOString` formats it. Strings that only
/// V8's legacy fallback parser accepts, such as RFC 2822 dates, are rejected.
pub fn canonical_date(input: &str) -> Option<String> {
    let zoned = if has_zone(input) {
        input.to_owned()
    } else {
        format!("{input}Z")
    };
    let millis = parse_iso(zoned.as_bytes())?;
    Some(format_iso(millis))
}

fn has_zone(input: &str) -> bool {
    let bytes = input.as_bytes();
    let ends_with_offset = |colon: bool| {
        let digits = if colon { 5 } else { 4 };
        bytes.len() > digits && {
            let tail = &bytes[bytes.len() - digits - 1..];
            matches!(tail[0], b'+' | b'-')
                && tail[1].is_ascii_digit()
                && tail[2].is_ascii_digit()
                && if colon {
                    tail[3] == b':' && tail[4].is_ascii_digit() && tail[5].is_ascii_digit()
                } else {
                    tail[3].is_ascii_digit() && tail[4].is_ascii_digit()
                }
        }
    };
    let two_digit_offset = bytes.len() > 2 && {
        let tail = &bytes[bytes.len() - 3..];
        matches!(tail[0], b'+' | b'-') && tail[1].is_ascii_digit() && tail[2].is_ascii_digit()
    };
    let lower = input.to_ascii_lowercase();
    input.contains('Z')
        || input.contains("GMT")
        || two_digit_offset
        || ends_with_offset(true)
        || ends_with_offset(false)
        || input.ends_with(']')
        || lower.ends_with('z')
        || lower.ends_with("gmt")
}

fn parse_iso(bytes: &[u8]) -> Option<i64> {
    let mut at = 0;
    let digits = |at: &mut usize, count: usize| -> Option<i64> {
        let slice = bytes.get(*at..*at + count)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *at += count;
        Some(
            slice
                .iter()
                .fold(0, |sum, byte| sum * 10 + i64::from(byte - b'0')),
        )
    };
    let year = match bytes.first()? {
        sign @ (b'+' | b'-') => {
            at = 1;
            let year = digits(&mut at, 6)?;
            if *sign == b'-' {
                if year == 0 {
                    return None;
                }
                -year
            } else {
                year
            }
        }
        _ => digits(&mut at, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if bytes.get(at) == Some(&b'-') {
        at += 1;
        month = digits(&mut at, 2)?;
        if bytes.get(at) == Some(&b'-') {
            at += 1;
            day = digits(&mut at, 2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut time = 0;
    if matches!(bytes.get(at), Some(b'T' | b't' | b' ')) {
        at += 1;
        let hour = digits(&mut at, 2)?;
        if bytes.get(at) != Some(&b':') {
            return None;
        }
        at += 1;
        let minute = digits(&mut at, 2)?;
        let (mut second, mut millis) = (0, 0);
        if bytes.get(at) == Some(&b':') {
            at += 1;
            second = digits(&mut at, 2)?;
            if bytes.get(at) == Some(&b'.') {
                at += 1;
                let start = at;
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                if at == start {
                    return None;
                }
                let fraction = &bytes[start..at.min(start + 3)];
                millis = fraction
                    .iter()
                    .fold(0, |sum, byte| sum * 10 + i64::from(byte - b'0'))
                    * 10_i64.pow(3 - fraction.len() as u32);
            }
        }
        let midnight = hour == 24 && minute == 0 && second == 0 && millis == 0;
        if (hour > 23 && !midnight) || minute > 59 || second > 59 {
            return None;
        }
        time = ((hour * 60 + minute) * 60 + second) * 1000 + millis;
        let offset = match bytes.get(at) {
            Some(b'Z' | b'z') => {
                at += 1;
                0
            }
            Some(sign @ (b'+' | b'-')) => {
                let sign = if *sign == b'-' { -1 } else { 1 };
                at += 1;
                let hours = digits(&mut at, 2)?;
                if bytes.get(at) == Some(&b':') {
                    at += 1;
                }
                let minutes = digits(&mut at, 2)?;
                if hours > 23 || minutes > 59 {
                    return None;
                }
                sign * (hours * 60 + minutes) * 60_000
            }
            _ => return None,
        };
        time -= offset;
    } else if matches!(bytes.get(at), Some(b'Z' | b'z')) {
        at += 1;
    }
    if at != bytes.len() {
        return None;
    }
    let millis = days_from_civil(year, month, 1).checked_add(day - 1)? * 86_400_000 + time;
    (-MAX_TIME_MS..=MAX_TIME_MS)
        .contains(&millis)
        .then_some(millis)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn format_iso(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let time = millis.rem_euclid(86_400_000);
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000
    )
}

/// Generated from `OrchestrationV2DomainEventJson` by `tools/verify-event-encoding.ts`.
const PERSISTED_EVENT_SHAPE: &str = include_str!("persisted-event-shape.json");

enum Shape {
    Date,
    String,
    Number,
    Boolean,
    Null,
    Any,
    Object {
        properties: Vec<(String, usize)>,
        index: Option<usize>,
        required: Vec<String>,
        literals: HashMap<String, Vec<Value>>,
    },
    Array {
        items: Vec<usize>,
        rest: Option<usize>,
    },
    Union {
        tag: Option<String>,
        cases: HashMap<String, usize>,
        other: Vec<usize>,
    },
}

struct Shapes {
    root: usize,
    nodes: Vec<Shape>,
}

fn shapes() -> Result<&'static Shapes> {
    static SHAPES: OnceLock<std::result::Result<Shapes, String>> = OnceLock::new();
    SHAPES
        .get_or_init(|| load_shapes(PERSISTED_EVENT_SHAPE))
        .as_ref()
        .map_err(|detail| Error::Decode {
            entity: "persisted event shape",
            detail: detail.clone(),
        })
}

fn load_shapes(text: &str) -> std::result::Result<Shapes, String> {
    let file: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let nodes = file["nodes"].as_array().ok_or("missing nodes")?;
    let id = |value: &Value| -> std::result::Result<usize, String> {
        // -1 marks a schema member that only admits `undefined`, which JSON never carries.
        match value.as_i64() {
            Some(-1) => Ok(5),
            Some(id) if (0..nodes.len() as i64).contains(&id) => Ok(id as usize),
            _ => Err(format!("invalid shape reference {value}")),
        }
    };
    let optional = |value: &Value| (!value.is_null()).then(|| id(value)).transpose();
    let ids = |value: &Value| -> std::result::Result<Vec<usize>, String> {
        value
            .as_array()
            .ok_or("expected a reference list")?
            .iter()
            .map(id)
            .collect()
    };
    let nodes = nodes
        .iter()
        .map(|node| {
            Ok(match node {
                Value::String(leaf) => match leaf.as_str() {
                    "date" => Shape::Date,
                    "string" => Shape::String,
                    "number" => Shape::Number,
                    "boolean" => Shape::Boolean,
                    "null" => Shape::Null,
                    "any" => Shape::Any,
                    other => return Err(format!("unknown leaf {other}")),
                },
                Value::Object(node) if node.contains_key("o") => Shape::Object {
                    properties: node["o"]
                        .as_array()
                        .ok_or("expected properties")?
                        .iter()
                        .map(|pair| {
                            Ok((
                                pair[0].as_str().ok_or("expected a key")?.to_owned(),
                                id(&pair[1])?,
                            ))
                        })
                        .collect::<std::result::Result<_, String>>()?,
                    index: optional(&node["i"])?,
                    required: node["q"]
                        .as_array()
                        .ok_or("expected required keys")?
                        .iter()
                        .map(|key| key.as_str().map(str::to_owned).ok_or("expected a key"))
                        .collect::<std::result::Result<_, _>>()?,
                    literals: node["l"]
                        .as_object()
                        .ok_or("expected literals")?
                        .iter()
                        .map(|(key, values)| {
                            Ok((
                                key.clone(),
                                values.as_array().ok_or("expected literal values")?.clone(),
                            ))
                        })
                        .collect::<std::result::Result<_, String>>()?,
                },
                Value::Object(node) if node.contains_key("a") => Shape::Array {
                    items: ids(&node["a"])?,
                    rest: optional(&node["r"])?,
                },
                Value::Object(node) if node.contains_key("x") => Shape::Union {
                    tag: node["u"].as_str().map(str::to_owned),
                    cases: node["c"]
                        .as_object()
                        .ok_or("expected cases")?
                        .iter()
                        .map(|(literal, case)| Ok((literal.clone(), id(case)?)))
                        .collect::<std::result::Result<_, String>>()?,
                    other: ids(&node["x"])?,
                },
                other => return Err(format!("unknown node {other}")),
            })
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;
    Ok(Shapes {
        root: id(&file["root"])?,
        nodes,
    })
}

/// Gives an encoded domain event the form the TypeScript store's `Schema.encode` writes: dates
/// canonical, keys in schema order, unknown keys dropped. A value the TypeScript decoder would
/// reject (explicit null for a non-nullable key, a number where an id string belongs, a
/// non-date string where a date belongs) is an error, so it is never persisted.
pub(crate) fn persisted_event(value: &mut Value) -> Result<()> {
    let shapes = shapes()?;
    apply(shapes, shapes.root, value, &mut String::from("$"))
}

fn apply(shapes: &Shapes, id: usize, value: &mut Value, path: &mut String) -> Result<()> {
    let reject = |path: &str, expected: &str| Error::Decode {
        entity: "event payload",
        detail: format!("{path}: expected {expected}"),
    };
    match &shapes.nodes[id] {
        Shape::Date => {
            let Value::String(text) = value else {
                return Err(reject(path, "a date string"));
            };
            *text = canonical_date(text).ok_or_else(|| reject(path, "a valid date"))?;
        }
        Shape::String if !value.is_string() => return Err(reject(path, "a string")),
        Shape::Number if !value.is_number() => return Err(reject(path, "a number")),
        Shape::Boolean if !value.is_boolean() => return Err(reject(path, "a boolean")),
        Shape::Null if !value.is_null() => return Err(reject(path, "null")),
        Shape::String | Shape::Number | Shape::Boolean | Shape::Null | Shape::Any => {}
        Shape::Object {
            properties,
            index,
            required,
            literals,
        } => {
            let Value::Object(object) = value else {
                return Err(reject(path, "an object"));
            };
            if let Some(key) = required.iter().find(|key| !object.contains_key(*key)) {
                return Err(reject(path, &format!("required key {key}")));
            }
            if let Some((key, _)) = literals.iter().find(|(key, values)| {
                object
                    .get(*key)
                    .is_some_and(|value| !values.iter().any(|literal| literal_eq(literal, value)))
            }) {
                return Err(reject(path, &format!("a schema literal at {key}")));
            }
            let mut input = std::mem::take(object);
            let mut output = Map::with_capacity(input.len());
            let length = path.len();
            for (key, child) in properties {
                if let Some(mut field) = input.shift_remove(key) {
                    path.push('.');
                    path.push_str(key);
                    apply(shapes, *child, &mut field, path)?;
                    path.truncate(length);
                    output.insert(key.clone(), field);
                }
            }
            if let Some(index) = index {
                for (key, mut field) in input {
                    path.push('.');
                    path.push_str(&key);
                    apply(shapes, *index, &mut field, path)?;
                    path.truncate(length);
                    output.insert(key, field);
                }
            }
            *object = output;
        }
        Shape::Array { items, rest } => {
            let Value::Array(values) = value else {
                return Err(reject(path, "an array"));
            };
            let length = path.len();
            for (position, field) in values.iter_mut().enumerate() {
                let Some(child) = items.get(position).copied().or(*rest) else {
                    return Err(reject(path, "a shorter array"));
                };
                path.push_str(&format!("[{position}]"));
                apply(shapes, child, field, path)?;
                path.truncate(length);
            }
        }
        Shape::Union { tag, cases, other } => {
            if let Some(case) = tag
                .as_ref()
                .and_then(|tag| value.get(tag))
                .and_then(Value::as_str)
                .and_then(|literal| cases.get(literal))
            {
                return apply(shapes, *case, value, path);
            }
            let candidates: Vec<usize> = other
                .iter()
                .copied()
                .filter(|member| admits_kind(shapes, *member, value))
                .collect();
            match candidates.as_slice() {
                [] => return Err(reject(path, "a member of the schema union")),
                [only] => return apply(shapes, *only, value, path),
                _ => {}
            }
            let mut last = None;
            for member in candidates {
                let mut attempt = value.clone();
                match apply(shapes, member, &mut attempt, path) {
                    Ok(()) => {
                        *value = attempt;
                        return Ok(());
                    }
                    Err(error) => last = Some(error),
                }
            }
            return Err(last.unwrap_or_else(|| reject(path, "a member of the schema union")));
        }
    }
    Ok(())
}

fn admits_kind(shapes: &Shapes, id: usize, value: &Value) -> bool {
    match &shapes.nodes[id] {
        Shape::Date | Shape::String => value.is_string(),
        Shape::Number => value.is_number(),
        Shape::Boolean => value.is_boolean(),
        Shape::Null => value.is_null(),
        Shape::Any => true,
        Shape::Object {
            required, literals, ..
        } => value.as_object().is_some_and(|object| {
            required.iter().all(|key| object.contains_key(key))
                && literals.iter().all(|(key, values)| {
                    object
                        .get(key)
                        .is_none_or(|value| values.iter().any(|literal| literal_eq(literal, value)))
                })
        }),
        Shape::Array { .. } => value.is_array(),
        Shape::Union { tag, cases, other } => {
            tag.as_ref()
                .and_then(|tag| value.get(tag))
                .and_then(Value::as_str)
                .is_some_and(|literal| cases.contains_key(literal))
                || other
                    .iter()
                    .any(|member| admits_kind(shapes, *member, value))
        }
    }
}

/// Schema literals compare by JavaScript value, so `1` and `1.0` are the same literal.
fn literal_eq(literal: &Value, value: &Value) -> bool {
    match (literal, value) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        _ => literal == value,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_shape_loads() {
        assert!(super::shapes().is_ok());
    }
}
