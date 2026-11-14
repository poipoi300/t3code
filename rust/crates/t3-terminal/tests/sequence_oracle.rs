use serde::Deserialize;
use t3_terminal::{BoundedTerminalHistory, retained_terminal_history, sequence::*};

#[derive(Deserialize)]
struct StateCase {
    state: u32,
    units: Vec<u16>,
    cut: usize,
    first: u32,
    after: u32,
    end: usize,
}
#[derive(Deserialize)]
struct RetainedCase {
    text: String,
    bytes: usize,
    kept: String,
}
#[derive(Deserialize)]
struct Step {
    clear: bool,
    chunk: String,
    kept: String,
}
#[derive(Deserialize)]
struct BoundedCase {
    lines: usize,
    bytes: usize,
    steps: Vec<Step>,
}
#[derive(Deserialize)]
struct SanitizedCase {
    chunks: Vec<String>,
    kept: String,
}
#[derive(Deserialize)]
struct Oracle {
    states: Vec<StateCase>,
    retained: Vec<RetainedCase>,
    bounded: Vec<BoundedCase>,
    sanitized: Vec<SanitizedCase>,
}

#[test]
fn parser_state_and_retained_history_match_typescript_including_split_surrogates() {
    let oracle: Oracle =
        serde_json::from_str(include_str!("../fixtures/sequences.json")).expect("oracle");
    for case in oracle.states {
        let first = terminal_sequence_state_after_utf16(case.state, &case.units[..case.cut]);
        assert_eq!(first, case.first, "{:?} cut={}", case.units, case.cut);
        assert_eq!(
            terminal_sequence_state_after_utf16(first, &case.units[case.cut..]),
            case.after
        );
        assert_eq!(
            terminal_sequence_end_utf16(case.state, &case.units),
            case.end
        );
    }
    for case in oracle.retained {
        assert_eq!(retained_terminal_history(&case.text, case.bytes), case.kept);
    }
    for case in oracle.bounded {
        let mut history = BoundedTerminalHistory::new(case.lines, case.bytes);
        for (step, update) in case.steps.into_iter().enumerate() {
            if update.clear {
                history.clear();
            }
            history.append(&update.chunk);
            assert_eq!(
                history.value(),
                update.kept,
                "lines={} bytes={} step={step}",
                case.lines,
                case.bytes
            );
        }
    }
    for case in oracle.sanitized {
        let mut sanitizer = t3_terminal::HistorySanitizer::default();
        let actual: String = case
            .chunks
            .iter()
            .map(|chunk| sanitizer.push(chunk))
            .collect();
        assert_eq!(actual, case.kept, "{:?}", case.chunks);
    }
}
