#![allow(clippy::unwrap_used)]
use serde_json::{Value,json};
use t3_mcp::{access::tool_rule,catalog::catalog};

#[test]
fn every_access_mapping_matches_the_real_typescript_declarations() {
    let bindings:Value=serde_json::from_str(include_str!("../fixtures/bindings.json")).unwrap();
    assert_eq!(bindings.as_object().unwrap().len(),catalog().tools.len());
    for tool in &catalog().tools {
        let binding=&bindings[&tool.name];
        assert!(binding["accessDeclaration"]["kind"].is_string(),"{} has no declaration",tool.name);
        for case in binding["accessCases"].as_array().unwrap() {
            let rule=tool_rule(&tool.name,&case["input"]).unwrap_or_else(|error|panic!("{}: {error}",tool.name));
            assert_eq!(serde_json::to_value(rule).unwrap(),case["rule"],"{} {}",tool.name,case["input"]);
        }
    }
    for name in ["new_upstream_mutation","preview_new_mutation",""] {assert!(tool_rule(name,&json!({})).is_err());}
}

#[test]
fn cargo_detects_stale_source_or_fixture_snapshots() {
    use sha2::{Digest,Sha256};
    let manifest:Value=serde_json::from_str(include_str!("../fixtures/source-hashes.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (path,expected) in manifest.as_object().unwrap() {
        let source=std::fs::read_to_string(root.join(path)).unwrap_or_else(|error|panic!("{path}: {error}; regenerate t3-mcp fixtures"));
        assert_eq!(format!("{:x}",Sha256::digest(source.replace("\r\n","\n").as_bytes())),expected.as_str().unwrap(),"{path} changed; run t3-mcp/export.ts before Cargo tests");
    }
}
