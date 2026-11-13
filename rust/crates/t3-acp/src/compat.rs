use serde_json::{Value, json};

use crate::{ClientRequest, Generation, Negotiated, RequestId, RpcError, SessionUpdate, v1, v2};

pub(crate) fn initialize_request(mut request: Value) -> Value {
    let mut capabilities = request
        .get("clientCapabilities")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    if capabilities.get("fs").is_none() {
        capabilities["fs"] = json!({"readTextFile":false,"writeTextFile":false});
    }
    if capabilities.get("terminal").is_none() {
        capabilities["terminal"] = json!(false);
    }
    if !capabilities["session"].is_object() {
        capabilities["session"] = json!({});
    }
    // v1 agents suppress session notices and compaction unless explicitly advertised.
    capabilities["session"]["notices"] = json!({});
    capabilities["session"]["compaction"] = json!({});
    let mut v2_caps = json!({});
    if capabilities.pointer("/auth/terminal") == Some(&json!(true)) {
        v2_caps["auth"] = json!({"terminal":{}});
    }
    for key in ["elicitation", "_meta"] {
        if let Some(value) = capabilities.get(key) {
            v2_caps[key] = value.clone();
        }
    }
    // Shipped agents negotiate v1 and draft v2 using different capability field names.
    request["protocolVersion"] = json!(2);
    request["info"] = request
        .get("clientInfo")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or(json!({"name":"t3-code","version":"unknown"}));
    request["capabilities"] = v2_caps;
    request["clientCapabilities"] = capabilities;
    request
}

pub(crate) fn negotiate(raw: Value) -> Negotiated {
    // Antigravity reports protocolVersion 2 with a v1 response shape.
    let generation = if raw.get("info").is_some() {
        Generation::V2
    } else {
        Generation::V1
    };
    if generation == Generation::V1 {
        return Negotiated {
            generation,
            response: raw.clone(),
            raw,
        };
    }
    let mut response = raw.clone();
    response["agentInfo"] = raw["info"].clone();
    let session = raw
        .pointer("/capabilities/session")
        .filter(|v| !v.is_null());
    let mut caps =
        json!({"loadSession":session.is_some(), "promptCapabilities":{}, "mcpCapabilities":{}});
    for key in ["image", "audio", "embeddedContext"] {
        caps["promptCapabilities"][key] = json!(
            session
                .and_then(|s| s.get("prompt"))
                .and_then(|p| p.get(key))
                .is_some_and(|v| !v.is_null())
        );
    }
    for key in ["stdio", "http", "acp"] {
        caps["mcpCapabilities"][key] = json!(
            session
                .and_then(|s| s.get("mcp"))
                .and_then(|p| p.get(key))
                .is_some_and(|v| !v.is_null())
        );
    }
    if let Some(session) = session {
        caps["sessionCapabilities"] = json!({"list":{},"resume":{},"close":{}});
        for key in ["delete", "fork", "additionalDirectories"] {
            if session.get(key).is_some_and(|v| !v.is_null()) {
                caps["sessionCapabilities"][key] = json!({});
            }
        }
    }
    if raw
        .pointer("/capabilities/providers")
        .is_some_and(|v| !v.is_null())
    {
        caps["providers"] = json!({});
    }
    if let Some(methods) = raw["authMethods"].as_array() {
        if !methods.is_empty() {
            caps["auth"] = json!({"logout":{}});
        }
        response["authMethods"] = Value::Array(
            methods
                .iter()
                .filter_map(|method| {
                    // Registry env-var login must not become an interactive agent authentication method.
                    let kind = method["type"].as_str()?;
                    if !matches!(kind, "agent" | "terminal" | "env_var") {
                        return None;
                    }
                    if kind == "env_var"
                        && !method["vars"]
                            .as_array()
                            .is_some_and(|vars| vars.iter().all(|v| v["name"].is_string()))
                    {
                        return None;
                    }
                    let mut method = method.clone();
                    method["id"] = method["methodId"].clone();
                    if kind == "terminal"
                        && let Some(env) = method["env"].as_array()
                    {
                        method["env"] = Value::Object(
                            env.iter()
                                .filter_map(|v| {
                                    Some((v["name"].as_str()?.to_owned(), v["value"].clone()))
                                })
                                .collect(),
                        );
                    }
                    Some(method)
                })
                .collect(),
        );
    }
    response["agentCapabilities"] = caps;
    Negotiated {
        generation,
        response,
        raw,
    }
}

pub(crate) fn setup_request(mut request: Value) -> Value {
    if let Some(servers) = request["mcpServers"].as_array_mut() {
        for server in servers {
            if server.get("type").is_none() {
                server["type"] = json!("stdio");
            }
        }
    }
    request
}

pub(crate) fn config_options(value: &mut Value) {
    if let Some(options) = value.get_mut("configOptions").and_then(Value::as_array_mut) {
        options.retain(|option| matches!(option["type"].as_str(), Some("select" | "boolean")));
        for option in options {
            if let Some(id) = option.get("configId").cloned() {
                option["id"] = id;
            }
        }
    }
}

pub(crate) fn session_update(raw: Value, generation: Generation) -> Option<SessionUpdate> {
    let session_id = raw["sessionId"].as_str()?.to_owned();
    let mut update = raw.get("update")?.as_object()?.clone();
    let kind = update.get("sessionUpdate")?.as_str()?.to_owned();
    if kind == "available_commands_update"
        && !update
            .get("availableCommands")
            .and_then(Value::as_array)
            .is_some_and(|commands| {
                commands.iter().all(|command| {
                    command.is_object()
                        && command["name"].is_string()
                        && command["description"].is_string()
                })
            })
    {
        return None;
    }
    if kind == "current_mode_update" && !update.get("currentModeId").is_some_and(Value::is_string) {
        return None;
    }
    if matches!(
        kind.as_str(),
        "agent_message_chunk" | "user_message_chunk" | "agent_thought_chunk"
    ) && serde_json::from_value::<v2::schema::ContentBlock>(update.get("content")?.clone())
        .is_err()
    {
        return None;
    }
    if matches!(kind.as_str(), "tool_call" | "tool_call_update") {
        if !update.get("toolCallId")?.is_string() {
            return None;
        }
        // v1 agents use null tool names to mean unchanged; v2 uses null to clear.
        if generation == Generation::V1 && update.get("name") == Some(&Value::Null) {
            update.remove("name");
        }
    }
    if kind == "usage_update"
        && (!update.get("used").is_some_and(Value::is_number)
            || !update.get("size").is_some_and(Value::is_number))
    {
        return None;
    }
    if kind == "notice"
        && (!update
            .get("title")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
            || !update.get("severity").is_some_and(Value::is_string))
    {
        // Agents send untitled notices that the T3 notice renderer cannot display.
        update = serde_json::from_value(json!({"sessionUpdate":"_t3_unknown", "originalSessionUpdate":"notice", "raw":raw["update"]})).ok()?;
    }
    let mut update = Value::Object(update);
    if generation == Generation::V2 {
        config_options(&mut update);
    }
    if kind == "available_commands_update"
        && let Some(commands) = update["availableCommands"].as_array_mut()
    {
        for command in commands {
            if generation == Generation::V1 {
                if command.get("input").is_some_and(Value::is_object) {
                    command["input"]["type"] = json!("text");
                }
            } else if command.pointer("/input/type").and_then(Value::as_str) != Some("text") {
                command["input"] = Value::Null;
            }
        }
    }
    Some(SessionUpdate {
        session_id,
        update,
        raw,
    })
}

pub(crate) fn client_request(
    method: &str,
    mut params: Value,
    generation: Generation,
    id: &RequestId,
) -> Result<ClientRequest, RpcError> {
    fn valid<T: serde::de::DeserializeOwned>(value: &Value) -> Result<(), RpcError> {
        serde_json::from_value::<T>(value.clone())
            .map(|_| ())
            .map_err(|_| RpcError::invalid_params())
    }
    Ok(match method {
        "session/request_permission" => {
            if params.get("toolCall").is_none() {
                valid::<v2::schema::RequestPermissionRequest>(&params)?;
                let subject = &params["subject"];
                // The SDK defaults invalid optional IDs, but TypeScript rejects known command fields.
                if subject["type"] == "command"
                    && ["toolCallId", "terminalId"].iter().any(|key| {
                        subject
                            .get(key)
                            .is_some_and(|v| !v.is_null() && !v.is_string())
                    })
                {
                    return Err(RpcError::invalid_params());
                }
                params["toolCall"] = if subject["type"] == "tool_call" {
                    subject["toolCall"].clone()
                } else {
                    json!({"toolCallId":subject.get("toolCallId").filter(|value| subject["type"] == "command" && value.is_string()).cloned().unwrap_or(json!(id.identity())), "title":params["title"], "kind":if subject["type"] == "command" {"execute"} else {"other"}})
                };
            } else {
                valid::<v1::schema::RequestPermissionRequest>(&params)?;
            }
            ClientRequest::Permission(params)
        }
        "fs/read_text_file" => {
            valid::<v1::schema::ReadTextFileRequest>(&params)?;
            ClientRequest::ReadTextFile(params)
        }
        "fs/write_text_file" => {
            valid::<v1::schema::WriteTextFileRequest>(&params)?;
            ClientRequest::WriteTextFile(params)
        }
        "terminal/create" => {
            valid::<v1::schema::CreateTerminalRequest>(&params)?;
            ClientRequest::TerminalCreate(params)
        }
        "terminal/output" => {
            valid::<v1::schema::TerminalOutputRequest>(&params)?;
            ClientRequest::TerminalOutput(params)
        }
        "terminal/wait_for_exit" => {
            valid::<v1::schema::WaitForTerminalExitRequest>(&params)?;
            ClientRequest::TerminalWaitForExit(params)
        }
        "terminal/kill" => {
            valid::<v1::schema::KillTerminalRequest>(&params)?;
            ClientRequest::TerminalKill(params)
        }
        "terminal/release" => {
            valid::<v1::schema::ReleaseTerminalRequest>(&params)?;
            ClientRequest::TerminalRelease(params)
        }
        "elicitation/create" | "session/elicitation" | "_session/elicitation" => {
            if generation == Generation::V2 {
                valid::<v2::schema::CreateElicitationRequest>(&params)?;
            } else {
                valid::<v1::schema::CreateElicitationRequest>(&params)?;
            }
            ClientRequest::Elicitation(params)
        }
        "mcp/connect" => {
            if !params["serverId"].is_string() {
                return Err(RpcError::invalid_params());
            }
            ClientRequest::McpConnect(params)
        }
        "mcp/message" => {
            // Older draft agents address MCP by connectionId; schema 1.17 uses serverId/requestId.
            if params["connectionId"].is_string() {
                if !params["method"].is_string() {
                    return Err(RpcError::invalid_params());
                }
            } else {
                valid::<v2::schema::MessageMcpRequest>(&params)?;
            }
            ClientRequest::McpMessage(params)
        }
        "mcp/disconnect" => {
            if !params["connectionId"].is_string() {
                return Err(RpcError::invalid_params());
            }
            ClientRequest::McpDisconnect(params)
        }
        _ => ClientRequest::Extension {
            method: method.to_owned(),
            params,
        },
    })
}

#[cfg(test)]
mod regression {
    use super::*;

    #[test]
    fn permission_id_only_comes_from_a_string_on_a_command_subject() {
        for subject in [
            json!({"type":"future","toolCallId":"wrong"}),
            json!({"type":"command","toolCallId":null,"command":"echo","cwd":"/work"}),
        ] {
            let request = client_request(
                "session/request_permission",
                json!({"sessionId":"s","title":"Run","subject":subject,"options":[]}),
                Generation::V2,
                &RequestId::Number(7),
            )
            .unwrap();
            let ClientRequest::Permission(params) = request else {
                panic!()
            };
            assert_eq!(params["toolCall"]["toolCallId"], "$t3:jsonrpc:number:7");
        }
    }

    #[test]
    fn malformed_known_permission_ids_never_reach_the_handler() {
        let result = client_request(
            "session/request_permission",
            json!({"sessionId":"s","title":"Run","subject":{"type":"command","command":"never","cwd":"/w","toolCallId":42},"options":[]}),
            Generation::V2,
            &RequestId::Number(7),
        );
        assert_eq!(result, Err(RpcError::invalid_params()));
    }

    #[test]
    fn unknown_draft_config_types_are_dropped() {
        let mut value = json!({"configOptions":[{"type":"future","configId":"secret"},{"type":"boolean","configId":"allowed","name":"Allowed","currentValue":true}]});
        config_options(&mut value);
        assert_eq!(
            value["configOptions"],
            json!([{"type":"boolean","configId":"allowed","id":"allowed","name":"Allowed","currentValue":true}])
        );
    }

    #[test]
    fn either_elicitation_shape_is_accepted_in_both_generations() {
        for generation in [Generation::V1, Generation::V2] {
            for params in [
                json!({"sessionId":"s","mode":"form","message":"Choose","requestedSchema":{"type":"object","properties":{}}}),
                json!({"sessionId":"s","mode":"url","message":"Login","url":"https://example.test","elicitationId":"e"}),
                json!({"requestId":"r","mode":"form","message":"Choose","requestedSchema":{"type":"object","properties":{}}}),
                json!({"requestId":"r","mode":"url","message":"Login","url":"https://example.test","elicitationId":"e"}),
                json!({"sessionId":"s","mode":"future","message":"Choose","future":true}),
                json!({"requestId":"r","mode":"future","message":"Choose","future":true}),
            ] {
                assert!(
                    client_request(
                        "elicitation/create",
                        params,
                        generation,
                        &RequestId::Number(1)
                    )
                    .is_ok()
                );
            }
        }
    }
}
