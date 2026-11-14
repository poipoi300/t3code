#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use std::{path::Path, process::Command, rc::Rc};
use t3_client::view_logic::{
    auto_settle::AutoSettleDaysField,
    session::{
        image_gallery::*, markdown::*, markdown_references::*, terminal::*, thread_links::*,
        workspace_lookup::*,
    },
};
use t3_contracts::base_schemas::EnvironmentId;

fn node(value: &Value) -> MarkdownNode {
    let point = |value: &Value| MarkdownPoint {
        line: value["line"].as_u64().unwrap_or(1) as usize,
        column: value["column"].as_u64().unwrap_or(1) as usize,
        offset: value["offset"].as_u64().map(|v| v as usize),
    };
    MarkdownNode {
        kind: value["type"].as_str().unwrap().into(),
        position: value.get("position").map(|p| MarkdownPosition {
            start: point(&p["start"]),
            end: point(&p["end"]),
        }),
        children: value["children"]
            .as_array()
            .map_or_else(Vec::new, |children| children.iter().map(node).collect()),
        data: Default::default(),
    }
}

#[test]
fn wp10_helpers_match_current_typescript() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let output = Command::new("node")
        .current_dir(root)
        .arg("rust/crates/t3-client/src/view_logic/session/wp10_oracle.mjs")
        .output()
        .expect("Node is required for TypeScript differential checks");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Value = serde_json::from_slice(&output.stdout).expect("TypeScript oracle JSON");
    for case in cases["definitions"].as_array().unwrap() {
        assert_eq!(
            json!(markdown_reference_definitions(
                case["flattened"].as_str().unwrap(),
                &node(&case["root"])
            )),
            case["expected"],
            "{case}"
        );
    }
    for case in cases["fragments"].as_array().unwrap() {
        let definitions: Vec<String> = serde_json::from_value(case["definitions"].clone()).unwrap();
        assert_eq!(
            with_reference_definitions(case["fragment"].as_str().unwrap(), &definitions, |_| {
                markdown_reference_definitions(
                    case["flattened"].as_str().unwrap(),
                    &node(&case["root"]),
                )
            }),
            case["expected"].as_str().unwrap(),
            "{case}"
        );
    }
    for case in cases["directoryCases"].as_array().unwrap() {
        let items: Vec<t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem> =
            serde_json::from_value(case["items"].clone()).unwrap();
        assert_eq!(
            json!(agent_working_directories(
                &items,
                case["reply"]["sourceThreadId"].as_str().unwrap(),
                case["reply"]["sourceItemId"].as_str().unwrap()
            )),
            case["expected"]
        );
    }
    for case in cases["stateCases"].as_array().unwrap() {
        let mut field = AutoSettleDaysField::new(14);
        for (step, expected) in case["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["states"].as_array().unwrap())
        {
            let submit = match step[0].as_str().unwrap() {
                "edit" => {
                    field.edit(step[1].as_str().unwrap());
                    None
                }
                "commit" => field.commit(),
                "sync" => {
                    field.sync(step[1].as_u64().unwrap() as u32);
                    None
                }
                "fail" => {
                    field.fail(step[1].as_u64().unwrap());
                    None
                }
                "cancel" => {
                    field.cancel();
                    None
                }
                operation => panic!("unknown operation {operation}"),
            };
            assert_eq!(json!({"field":field,"submit":submit}), *expected, "{step}");
        }
    }
    for case in cases["galleries"].as_array().unwrap() {
        let viewed = case["viewed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["path"].as_str().unwrap().to_string(),
                    (!v["item"].is_null()).then(|| v["item"].clone()),
                )
            })
            .collect::<Vec<_>>();
        let preview = Rc::new(ImagePreview {
            images: case["preview"]["images"].as_array().unwrap().clone(),
            index: case["preview"]["index"].as_u64().unwrap() as usize,
        });
        let result = work_group_image_gallery(&viewed, case["clicked"].as_str(), preview);
        assert_eq!(
            json!({"images":result.images,"index":result.index}),
            case["expected"]
        );
    }
    for case in cases["terminal"].as_array().unwrap() {
        assert_eq!(
            json!(extract_terminal_links(case["input"].as_str().unwrap())),
            case["expected"],
            "{}",
            case["input"]
        );
    }
    for case in cases["themeSummaries"].as_array().unwrap() {
        let calls = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| t3_client::work_log::T3ToolSummaryCall {
                input: call["input"].clone(),
                output: call["output"].clone(),
                outcome: call["outcome"].as_str().unwrap().into(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            json!(t3_client::work_log::summarize_t3_tool_calls(
                case["action"].as_str().unwrap(),
                &calls
            )),
            case["expected"]
        );
    }
    for case in cases["threadLinks"].as_array().unwrap() {
        let markdown = case["markdown"].as_str().unwrap();
        assert_eq!(
            replace_thread_links(markdown, |_, _| Some("REPLACED".into())),
            case["expected"].as_str().unwrap(),
            "{markdown}"
        );
        let refs = thread_link_refs(markdown, &EnvironmentId("local".into()))
            .iter()
            .map(|key| json!({"environmentId":key.environment,"threadId":key.thread}))
            .collect::<Vec<_>>();
        assert_eq!(json!(refs), case["refs"], "{markdown}");
    }
}
