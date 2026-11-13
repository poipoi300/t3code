#![allow(clippy::unwrap_used)]
use serde_json::Value;
use t3_contracts::rpc::WS_METHODS;
#[test]
fn every_fixture_method_has_exactly_one_disposition_and_kind() {
    let registry = t3_server::handlers::registry().unwrap();
    registry.validate().unwrap();
    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("../../../fixtures/rpc.json")).unwrap();
    let mut tags = std::collections::BTreeSet::new();
    for row in fixture.iter().filter(|r| r["group"] == "WsRpcGroup") {
        let tag = row["tag"].as_str().unwrap();
        assert!(tags.insert(tag));
        assert!(registry.disposition(tag).is_some(), "{tag}");
        assert_eq!(
            WS_METHODS.iter().find(|m| m.tag == tag).unwrap().streams,
            row["streams"].as_bool().unwrap()
        );
    }
    assert_eq!(tags.len(), 220);
    assert_eq!(tags.len(), WS_METHODS.len());
}
