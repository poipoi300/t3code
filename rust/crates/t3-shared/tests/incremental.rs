#![allow(clippy::unwrap_used)]
use serde_json::Value;
use t3_shared::terminal_output::*;

#[test]
fn incremental_feeds_match_every_package09_case() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/transport_terminal_output.json")).unwrap();
    for case in cases {
        let max = case["max"].as_u64().unwrap() as usize;
        let mut output = TerminalOutput::new(max);
        let mut eager = TerminalOutputState::default();
        for chunk in case["chunks"].as_array().unwrap() {
            let chunk = chunk.as_str().unwrap();
            output.push(chunk);
            eager = append_terminal_output(&eager, chunk, max);
            let snapshot = output.snapshot();
            assert_eq!(snapshot.text, eager.text, "{}", case["name"]);
            assert_eq!(snapshot.pending, eager.pending, "{}", case["name"]);
            assert_eq!(snapshot.truncated, eager.truncated, "{}", case["name"]);
            assert_eq!(
                terminal_output_resume_text(&snapshot),
                terminal_output_resume_text(&eager),
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn utf8_decoding_survives_every_byte_split_and_one_byte_feeds() {
    let input = "名😀 café\r\x1b[2K🚀 résumé\n\x1b[38:2::12:34:56m世界\x1b[0m";
    let expected = normalize_terminal_output(input, 65536);
    for cut in 0..=input.len() {
        let mut output = TerminalOutput::default();
        output.push_bytes(&input.as_bytes()[..cut]);
        output.push_bytes(&input.as_bytes()[cut..]);
        output.finish_bytes();
        assert_eq!(output.snapshot().text, expected.text, "cut={cut}");
    }
    let mut output = TerminalOutput::default();
    for byte in input.as_bytes() {
        output.push_bytes(&[*byte]);
    }
    assert_eq!(output.snapshot().text, expected.text);
}

#[test]
fn incomplete_utf8_is_flushed_only_at_end_of_stream() {
    let mut output = TerminalOutput::default();
    output.push_bytes(&[b'a', 0xf0, 0x9f]);
    assert_eq!(output.snapshot().text, "a");
    output.finish_bytes();
    assert_eq!(output.snapshot().text, "a�");
    output.push_bytes(&[0xff, b'b']);
    assert_eq!(output.snapshot().text, "a��b");
}

#[test]
fn utf16_surrogate_chunks_can_rejoin_before_materialization() {
    let mut output = TerminalOutput::default();
    output.push_units(&[0xd83d]);
    output.push_units(&[0xde00]);
    assert_eq!(output.snapshot().text, "😀");
}

#[test]
fn truncation_from_style_overhead_survives_a_later_erase() {
    let mut output = TerminalOutput::new(10);
    let mut eager = TerminalOutputState::default();
    for chunk in ["\x1b[31mred", "\r\x1b[2K\x1b[0mx"] {
        output.push(chunk);
        eager = append_terminal_output(&eager, chunk, 10);
        assert_eq!(output.snapshot().text, eager.text);
        assert_eq!(output.snapshot().truncated, eager.truncated);
    }
}
