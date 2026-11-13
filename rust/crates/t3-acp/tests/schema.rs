#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_acp::{RequestId, RpcError, v1, v2};

#[test]
fn sdk_content_covers_all_five_typescript_content_types_and_v2_future_types() {
    for content in [
        json!({"type":"text","text":"hello"}),
        json!({"type":"image","data":"AA==","mimeType":"image/png"}),
        json!({"type":"audio","data":"AA==","mimeType":"audio/wav"}),
        json!({"type":"resource_link","uri":"file:///file","name":"file"}),
        json!({"type":"resource","resource":{"uri":"file:///file","text":"embedded"}}),
    ] {
        let old: v1::schema::ContentBlock = serde_json::from_value(content.clone()).unwrap();
        let draft: v2::schema::ContentBlock = serde_json::from_value(content.clone()).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap()["type"], content["type"]);
        assert_eq!(
            serde_json::to_value(draft).unwrap()["type"],
            content["type"]
        );
    }
    let future = json!({"type":"chart","points":[1,2]});
    assert!(serde_json::from_value::<v1::schema::ContentBlock>(future.clone()).is_err());
    let draft: v2::schema::ContentBlock = serde_json::from_value(future.clone()).unwrap();
    assert_eq!(serde_json::to_value(draft).unwrap(), future);
}

#[test]
fn sdk_drops_unknown_fields_on_known_structs_but_retains_meta_and_unknown_v2_variants() {
    let wire = json!({"type":"text","text":"hello","vendorField":{"x":1},"_meta":{"vendor":true}});
    let typed: v2::schema::ContentBlock = serde_json::from_value(wire.clone()).unwrap();
    let encoded = serde_json::to_value(typed).unwrap();
    assert!(encoded.get("vendorField").is_none());
    assert_eq!(encoded["_meta"], wire["_meta"]);
    let future = json!({"sessionUpdate":"timeline_update","entries":[1,2]});
    let typed: v2::schema::SessionUpdate = serde_json::from_value(future.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), future);
}

#[test]
fn sdk_draft_mcp_changed_from_typescript_connection_id_to_server_and_request_ids() {
    let old = json!({"connectionId":"mcp-1","method":"tools/list"});
    assert!(serde_json::from_value::<v2::schema::MessageMcpRequest>(old).is_err());
    let current = json!({"serverId":"mcp-1","requestId":"inner-1","method":"tools/list"});
    let typed: v2::schema::MessageMcpRequest = serde_json::from_value(current.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), current);
}

#[test]
fn numeric_string_and_reserved_prefix_identities_are_disjoint() {
    let ids = [
        json!(0),
        json!("0"),
        json!("$t3:jsonrpc:number:0"),
        json!("$t3:jsonrpc:string:0"),
    ];
    let parsed: Vec<RequestId> = ids
        .iter()
        .map(|id| serde_json::from_value(id.clone()).unwrap())
        .collect();
    let mut identities: Vec<_> = parsed.iter().map(RequestId::identity).collect();
    identities.sort();
    identities.dedup();
    assert_eq!(identities.len(), ids.len());
    for (id, wire) in parsed.into_iter().zip(ids) {
        assert_eq!(serde_json::to_value(id).unwrap(), wire);
    }
}

#[test]
fn rpc_errors_distinguish_absent_data_from_explicit_null() {
    for value in [
        json!({"code":-32602,"message":"bad"}),
        json!({"code":-32602,"message":"bad","data":null}),
        json!({"code":-32602,"message":"bad","data":{"field":"x"}}),
    ] {
        let typed: RpcError = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), value);
    }
}

#[test]
fn known_malformed_content_cannot_fall_through_to_future_variants() {
    for value in [
        json!({"type":"text"}),
        json!({"type":"image","data":"AA=="}),
    ] {
        assert!(serde_json::from_value::<v2::schema::ContentBlock>(value).is_err());
    }
}
