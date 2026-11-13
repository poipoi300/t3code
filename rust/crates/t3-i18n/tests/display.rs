//! `format_bytes`, `format_tokens`, `format_count`, `format_usd` and `format_percent` against the
//! TypeScript helpers run in Node (`tests/fixtures/display.json`).

use serde::Deserialize;
use t3_i18n::display;

#[derive(Deserialize)]
struct File {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    r#fn: String,
    value: f64,
    digits: Option<usize>,
    expected: String,
}

#[test]
fn matches_the_typescript_helpers() {
    let file: File = serde_json::from_str(include_str!("fixtures/display.json")).expect("fixture");
    let mut mismatches = Vec::new();
    for case in &file.cases {
        let actual = match case.r#fn.as_str() {
            "bytes" => display::format_bytes(case.value),
            "tokens" => display::format_tokens(case.value),
            "count" => display::format_count(case.value),
            "usd" => display::format_usd(case.value),
            "percent" => display::format_percent(case.value, case.digits.expect("digits")),
            other => panic!("unknown helper {other}"),
        };
        if actual != case.expected {
            mismatches.push(format!(
                "{}({}, {:?}): expected {:?}, got {actual:?}",
                case.r#fn, case.value, case.digits, case.expected
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} differ, first: {:#?}",
        mismatches.len(),
        file.cases.len(),
        &mismatches[..mismatches.len().min(8)]
    );
}
