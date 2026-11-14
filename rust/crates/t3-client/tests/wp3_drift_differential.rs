#![cfg(not(target_arch = "wasm32"))]
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, process::Command};
use t3_client::{
    authorization::connect_approval::connect_device_verification_code,
    permissions::connect_device::*, persistence::external_links::is_web_url,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prompt {
    pairing_link_id: String,
    host: String,
}
impl ApprovalPrompt for Prompt {
    fn pairing_link_id(&self) -> &str {
        &self.pairing_link_id
    }
}

#[test]
fn drift_helpers_match_current_typescript() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let output = Command::new("node")
        .current_dir(root)
        .arg("rust/crates/t3-client/tests/client_drift_typescript.mjs")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Value = serde_json::from_slice(&output.stdout).unwrap();
    for case in cases["clients"].as_array().unwrap() {
        let client: Option<t3_contracts::auth::AuthClientMetadata> =
            serde_json::from_value(case["client"].clone()).unwrap();
        assert_eq!(connect_device_name(client.as_ref()), case["short"]);
        assert_eq!(
            describe_connect_device_client(client.as_ref()),
            case["detailed"]
        );
        assert_eq!(describe_connect_device(client.as_ref()), case["mobile"]);
    }
    for case in cases["codes"].as_array().unwrap() {
        assert_eq!(
            connect_device_verification_code(case["input"].as_str().unwrap()),
            case["output"]
        );
    }
    for case in cases["urls"].as_array().unwrap() {
        assert_eq!(is_web_url(case["input"].as_str().unwrap()), case["output"]);
    }
    let links: Vec<t3_contracts::auth::AuthPairingLink> = serde_json::from_value(json!([
        {"id":"newer","scopes":[],"subject":"cloud-connect","createdAt":"1970-01-01T00:00:02.000Z","expiresAt":"1970-01-01T00:10:02.000Z","connectDeviceRequest":{"verificationCode":"WXYZ-2345"}},
        {"id":"plain","scopes":[],"subject":"cloud-connect","createdAt":"1970-01-01T00:00:00.500Z","expiresAt":"1970-01-01T00:10:00.500Z"},
        {"id":"older","scopes":[],"subject":"cloud-connect","createdAt":"1970-01-01T00:00:01.000Z","expiresAt":"1970-01-01T00:10:01.000Z","connectDeviceRequest":{"verificationCode":"ABCD-EFGH","client":{"deviceType":"desktop","browser":"Chrome","os":"macOS"}}}
    ])).unwrap();
    let prompts =
        connect_device_approval_prompts("Mac mini", &links, &BTreeSet::from(["newer".into()]));
    let actual: Vec<Value> = prompts.into_iter().map(|prompt| json!({"pairingLinkId":prompt.pairing_link_id,"title":prompt.title,"message":prompt.message})).collect();
    assert_eq!(json!(actual), cases["prompts"]);
    for case in cases["queues"].as_array().unwrap() {
        let mut queue = ApprovalPromptQueue::<Prompt>::default();
        let mut shown = Vec::new();
        let mut previous = None;
        let mut outputs = Vec::new();
        for command in case["commands"].as_array().unwrap() {
            match command[0].as_str().unwrap() {
                "offer" => {
                    queue.offer(serde_json::from_value::<Vec<Prompt>>(command[1].clone()).unwrap())
                }
                "drop" => queue.drop_matching(|prompt| prompt.host == command[1].as_str().unwrap()),
                "answer" => {
                    let outcome = match command[1].as_str().unwrap() {
                        "done" => ApprovalOutcome::Done,
                        "retry" => ApprovalOutcome::Retry,
                        "later" => ApprovalOutcome::Later,
                        _ => unreachable!(),
                    };
                    queue.answer(queue.showing().unwrap().0, outcome);
                }
                _ => unreachable!(),
            }
            if let Some((id, prompt)) = queue.showing()
                && previous != Some(id)
            {
                shown.push(prompt.pairing_link_id.clone());
                previous = Some(id);
            }
            outputs.push(json!({"shown":shown,"known":queue.known()}));
        }
        assert_eq!(json!(outputs), case["outputs"]);
    }
}
