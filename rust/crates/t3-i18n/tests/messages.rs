//! The ICU engine against the real `@generaltranslation/icu` formatter run in Node
//! (`tests/fixtures/messages.json`, see `tools/generate-fixtures.mjs`): same output, or an error
//! on both sides.

use serde::Deserialize;
use serde_json::Value as Json;
use t3_i18n::{Formats, Message, MessageError, Value};

/// Features the engine parses but does not format (the product's catalogues use none).
const UNSUPPORTED: [&str; 2] = ["{n, number, percent}", "{d, date, short}"];

#[derive(Deserialize)]
struct File {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    locale: String,
    message: String,
    args: serde_json::Map<String, Json>,
    expected: Option<String>,
    error: Option<String>,
}

fn value(json: &Json) -> Value {
    match json {
        Json::String(text) => Value::Text(text.clone()),
        Json::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Float(n.as_f64().unwrap_or(f64::NAN)), Value::Int),
        other => panic!("unsupported argument {other}"),
    }
}

#[test]
fn matches_the_generaltranslation_formatter() {
    let file: File = serde_json::from_str(include_str!("fixtures/messages.json")).expect("fixture");
    let mut mismatches = Vec::new();
    let mut errors = 0;
    let mut unsupported = 0;
    for case in &file.cases {
        let args: Vec<(&str, Value)> = case
            .args
            .iter()
            .map(|(name, json)| (name.as_str(), value(json)))
            .collect();
        let actual = Message::parse(&case.message)
            .and_then(|m| m.format(&Formats::new(&case.locale), &args));
        match (&case.expected, &case.error, actual) {
            (Some(expected), None, Ok(actual)) if *expected == actual => {}
            (None, Some(_), Err(_)) => errors += 1,
            (_, _, Err(MessageError::Unsupported(_)))
                if UNSUPPORTED.contains(&case.message.as_str()) =>
            {
                unsupported += 1
            }
            (_, _, actual) => mismatches.push(format!(
                "[{}] {:?} {:?}: expected {:?}/{:?}, got {actual:?}",
                case.locale, case.message, case.args, case.expected, case.error
            )),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} differ, first: {:#?}",
        mismatches.len(),
        file.cases.len(),
        &mismatches[..mismatches.len().min(5)]
    );
    assert_eq!(unsupported, 22, "two unsupported messages in 11 locales");
    assert_eq!(errors, 77, "error paths agree with GT: both sides fail");
}
