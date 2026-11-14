#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Command,
    rc::Rc,
};
use t3_client::services::terminal::*;

fn cursor(value: &Value) -> TerminalOutputCursor {
    TerminalOutputCursor {
        generation: value["generation"].as_u64(),
        reset_version: value["resetVersion"].as_u64(),
        offset: value["offset"].as_u64().unwrap(),
        sequence_state: value["sequenceState"].as_u64().unwrap() as u32,
    }
}
fn position(cursor: TerminalOutputCursor) -> Value {
    json!({"generation":cursor.generation,"resetVersion":cursor.reset_version,"offset":cursor.offset,"sequenceState":cursor.sequence_state})
}
fn state(value: &Value) -> TerminalOutputState {
    TerminalOutputState {
        generation: value["generation"].as_u64().unwrap(),
        chunks: value["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chunk| TerminalOutputChunk {
                start_offset: chunk["startOffset"].as_u64().unwrap(),
                data: Rc::from(chunk["data"].as_str().unwrap()),
            })
            .collect::<VecDeque<_>>(),
        retained_bytes: value["retainedBytes"].as_u64().unwrap() as usize,
        reset_version: value["resetVersion"].as_u64().unwrap(),
        next_offset: value["nextOffset"].as_u64().unwrap(),
        sequence_state: value["sequenceState"].as_u64().unwrap() as u32,
        text_start_offset: value["textStartOffset"].as_u64().unwrap(),
    }
}
#[test]
fn current_typescript_terminal_session_suite_and_utf8_c1_replay_operations() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("node")
        .arg(manifest.join("tests/wp6_terminal_oracle.mjs"))
        .current_dir(manifest.ancestors().nth(3).unwrap())
        .output()
        .expect("Node oracle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let corpus: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut states = HashMap::<u64, TerminalOutputState>::new();
    let mut compared = 0;
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name == "seed" {
            states.insert(case["id"].as_u64().unwrap(), state(&case["expected"]));
            continue;
        }
        let mut output = states[&case["input"].as_u64().unwrap()].clone();
        let args = &case["args"];
        let actual = match name {
            "appendOutput" | "resetOutput" | "clearOutput" => {
                if name == "clearOutput" {
                    output.clear(args[0].as_u64().unwrap() as usize);
                } else {
                    let data = args[0].as_str().unwrap();
                    let budget = args[1].as_u64().unwrap() as usize;
                    if name == "appendOutput" {
                        output.append(data, budget);
                    } else {
                        output.reset(data, budget);
                    }
                }
                states.insert(case["id"].as_u64().unwrap(), output);
                continue;
            }
            "terminalOutputText" => json!(output.text()),
            "terminalOutputReplay" => json!(output.replay()),
            "readTerminalOutputUpdate" => match output.read(cursor(&args[0])) {
                TerminalOutputUpdate::None(cursor) => {
                    json!({"type":"none","cursor":position(cursor)})
                }
                TerminalOutputUpdate::Reset { data, cursor } => {
                    json!({"type":"reset","data":data,"cursor":position(cursor)})
                }
                TerminalOutputUpdate::Append { data, cursor } => {
                    json!({"type":"append","data":data,"cursor":position(cursor)})
                }
            },
            _ => panic!("unexpected {name}"),
        };
        assert_eq!(actual, case["expected"], "{}: {name}", case["test"]);
        compared += 1;
    }
    println!(
        "{} upstream tests, {} operations, {compared} output comparisons",
        corpus["tests"],
        corpus["cases"].as_array().unwrap().len()
    );
}
