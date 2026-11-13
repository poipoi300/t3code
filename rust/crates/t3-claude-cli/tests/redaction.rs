#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_claude_cli::{DecodeKind, Error, control::*, launch::*, message::Content};

const SECRET: &str = "MARKER_PRIVATE_prompt_env_argv_authorization";
fn redacted(rendered: String) {
    assert!(
        !rendered.contains(SECRET),
        "secret in diagnostic: {rendered}"
    );
}

#[test]
fn every_public_error_redacts_payloads_in_display_and_debug() {
    for error in [
        Error::Closed,
        Error::Io(SECRET.into()),
        Error::Protocol(SECRET.into()),
        Error::Cli {
            request_id: SECRET.into(),
            message: SECRET.into(),
        },
        Error::Unsupported(SECRET.into()),
        Error::Cancelled,
        Error::Options(SECRET.into()),
    ] {
        redacted(format!("{error}"));
        redacted(format!("{error:?}"));
    }
    for kind in [
        DecodeKind::Io,
        DecodeKind::Syntax,
        DecodeKind::Data,
        DecodeKind::Eof,
    ] {
        let error = Error::Decode {
            kind,
            line: 1,
            column: 2,
        };
        redacted(format!("{error} {error:?}"));
    }
    let io = Error::from(std::io::Error::other(SECRET));
    redacted(format!("{io} {io:?}"));
    let malformed = json!({"subtype":SECRET});
    let error = serde_json::from_value::<OutboundRequest>(malformed).unwrap_err();
    redacted(format!("{}", Error::from(error)));
    let error = serde_json::from_value::<PermissionMode>(json!(SECRET)).unwrap_err();
    redacted(format!("{:?}", Error::from(error)));
}

#[test]
fn launch_and_requests_redact_all_caller_values_in_debug() {
    let mut options = LaunchOptions {
        cli_path: Some(SECRET.into()),
        executable_args: vec![SECRET.into()],
        cwd: Some(SECRET.into()),
        env: Environment::from([(SECRET.into(), SECRET.into())]),
        system_prompt: SystemPrompt::Custom(vec![SECRET.into()]),
        settings: Some(Settings::Json(json!({"env":{"KEY":SECRET}}))),
        ..Default::default()
    };
    options.initialize.title = Some(SECRET.into());
    redacted(format!("{options:?}"));
    redacted(format!("{:?}", describe_spawn(&options).unwrap()));
    let requests = [
        json!({"subtype":"initialize","systemPrompt":[SECRET],"title":SECRET}),
        json!({"subtype":"mcp_set_servers","servers":{"s":{"headers":{"Authorization":SECRET}}}}),
        json!({"subtype":"set_model","model":SECRET}),
        json!({"subtype":"apply_flag_settings","settings":{"effortLevel":SECRET}}),
        json!({"subtype":"set_permission_mode","mode":"default"}),
        json!({"subtype":"stop_task","task_id":SECRET}),
        json!({"subtype":"background_tasks","tool_use_id":SECRET}),
        json!({"subtype":"mcp_status"}),
        json!({"subtype":"mcp_read_resource","serverName":SECRET,"uri":SECRET}),
        json!({"subtype":"mcp_call","tool":SECRET,"arguments":{"prompt":SECRET},"result_not_for_model":true}),
        json!({"subtype":"list_permission_rules"}),
        json!({"subtype":"interrupt"}),
        json!({"subtype":"get_usage","skip_behaviors":true}),
        json!({"subtype":"get_settings"}),
        json!({"subtype":"mcp_message","server_name":SECRET,"message":{"headers":{"Authorization":SECRET}}}),
    ];
    for raw in requests {
        redacted(format!(
            "{:?}",
            serde_json::from_value::<OutboundRequest>(raw).unwrap()
        ));
    }
    redacted(format!(
        "{:?}",
        UserMessage::new(Content::Text(SECRET.into()))
    ));
    redacted(format!(
        "{:?}",
        serde_json::from_value::<ToolPermission>(
            json!({"tool_name":SECRET,"tool_use_id":SECRET,"input":{"prompt":SECRET}})
        )
        .unwrap()
    ));
    redacted(format!(
        "{:?}",
        serde_json::from_value::<HookCallback>(
            json!({"callback_id":SECRET,"input":{"prompt":SECRET},"tool_use_id":SECRET})
        )
        .unwrap()
    ));
    redacted(format!(
        "{:?}",
        serde_json::from_value::<Elicitation>(
            json!({"mcp_server_name":SECRET,"message":SECRET,"url":SECRET})
        )
        .unwrap()
    ));
    redacted(format!(
        "{:?}",
        serde_json::from_value::<UserDialog>(
            json!({"dialog_kind":SECRET,"payload":{"Authorization":SECRET}})
        )
        .unwrap()
    ));
    redacted(format!(
        "{:?}",
        serde_json::from_value::<McpMessage>(
            json!({"server_name":SECRET,"message":{"Authorization":SECRET}})
        )
        .unwrap()
    ));
    let reply = t3_claude_cli::ControlReply {
        raw: Value::String(SECRET.into()),
        payload: Some(json!(SECRET)),
    };
    redacted(format!("{reply:?}"));
}

#[test]
fn optional_input_fields_do_not_grow_nulls_and_usage_false_is_omitted() {
    assert_eq!(
        serde_json::to_value(OutboundRequest::GetUsage {
            skip_behaviors: false
        })
        .unwrap(),
        json!({"subtype":"get_usage"})
    );
    let permission: ToolPermission =
        serde_json::from_value(json!({"tool_name":"Read","tool_use_id":"t","input":{}})).unwrap();
    assert_eq!(
        serde_json::to_value(permission).unwrap(),
        json!({"tool_name":"Read","tool_use_id":"t","input":{}})
    );
    let message: UserMessage = serde_json::from_value(
        json!({"type":"user","message":{"role":"user","content":"hi"},"parent_tool_use_id":null}),
    )
    .unwrap();
    assert!(serde_json::to_value(message).unwrap().get("uuid").is_none());
}
