#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use t3_claude_cli::{
    PROTOCOL_BASELINE,
    control::{HookMatcher, Initialize, PermissionMode},
    launch::*,
};

#[test]
fn sdk_argv_order_settings_format_and_initialize_fields_match_pinned_source() {
    let options = LaunchOptions {
        cli_path: Some("C:/Claude/cli.mjs".into()),
        javascript_runtime: "bun".into(),
        executable_args: vec!["--inspect=0".into()],
        cwd: Some(PathBuf::from("C:/project")),
        thinking: Some(Thinking::Adaptive {
            display: Some("summarized".into()),
        }),
        effort: Some("high".into()),
        model: Some("sonnet".into()),
        can_use_tool: true,
        resume: Some("session".into()),
        allowed_tools: vec!["Read".into(), "mcp__t3-code__*".into()],
        disallowed_tools: vec!["Write".into()],
        tools: Some(Tools::Default),
        setting_sources: Some(vec!["user".into(), "project".into(), "local".into()]),
        strict_mcp_config: true,
        permission_mode: Some(PermissionMode::BypassPermissions),
        allow_dangerously_skip_permissions: true,
        include_partial_messages: true,
        additional_directories: vec![
            "C:/project".into(),
            "C:/attachments".into(),
            "C:/project".into(),
        ],
        fork_session: true,
        resume_session_at: Some("head".into()),
        persist_session: false,
        settings: Some(Settings::Json(
            json!({"showThinkingSummaries":true,"env":{"ENABLE_TOOL_SEARCH":"true"}}),
        )),
        extra_args: vec![
            ("thinking-display".into(), Some("summarized".into())),
            ("settings".into(), Some("overwritten".into())),
            ("flag".into(), None),
            ("dash-value".into(), Some("-x".into())),
        ],
        system_prompt: SystemPrompt::ClaudeCode {
            append: Some("T3 instructions\n\nuser instructions".into()),
        },
        initialize: Initialize {
            title: Some("thread".into()),
            prompt_suggestions: Some(false),
            supported_dialog_kinds: Some(vec!["resume_return".into()]),
            hooks: Some(BTreeMap::from([(
                "SubagentStart".into(),
                vec![HookMatcher {
                    hook_callback_ids: vec!["hook_0".into()],
                    matcher: None,
                    timeout: None,
                    on_failure: None,
                }],
            )])),
            ..Default::default()
        },
        ..Default::default()
    };
    let spawn = describe_spawn(&options).unwrap();
    assert_eq!(spawn.command, "bun");
    assert_eq!(spawn.cwd, options.cwd);
    assert_eq!(
        spawn.args,
        vec![
            "--inspect=0",
            "C:/Claude/cli.mjs",
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--thinking=adaptive",
            "--thinking-display=summarized",
            "--effort=high",
            "--model=sonnet",
            "--permission-prompt-tool=stdio",
            "--resume=session",
            "--allowedTools=Read,mcp__t3-code__*",
            "--disallowedTools=Write",
            "--tools=default",
            "--setting-sources=user,project,local",
            "--strict-mcp-config",
            "--permission-mode=bypassPermissions",
            "--allow-dangerously-skip-permissions",
            "--include-partial-messages",
            "--add-dir=C:/project",
            "--add-dir=C:/attachments",
            "--add-dir=C:/project",
            "--fork-session",
            "--resume-session-at=head",
            "--no-session-persistence",
            "--thinking-display",
            "summarized",
            "--settings",
            "{\"showThinkingSummaries\":true,\"env\":{\"ENABLE_TOOL_SEARCH\":\"true\"}}",
            "--flag",
            "--dash-value=-x"
        ]
    );
    assert_eq!(
        serde_json::to_value(spawn.initialize).unwrap(),
        json!({"appendSystemPrompt":"T3 instructions\n\nuser instructions","title":"thread","promptSuggestions":false,"supportedDialogKinds":["resume_return"],"hooks":{"SubagentStart":[{"hookCallbackIds":["hook_0"]}]}})
    );
}

#[test]
fn sdk_environment_defaults_preserve_explicit_values_and_remove_node_debug_inheritance() {
    let env = Environment::from([
        ("CLAUDE_CODE_ENTRYPOINT".into(), "t3-code".into()),
        ("CLAUDE_AGENT_SDK_VERSION".into(), "explicit".into()),
        ("claude_code_sdk_reads_session_state".into(), "0".into()),
        ("NODE_OPTIONS".into(), "--inspect".into()),
        ("DEBUG".into(), "inherited".into()),
    ]);
    let mut options = LaunchOptions {
        cli_path: Some("claude.exe".into()),
        env,
        ..Default::default()
    };
    let spawn = describe_spawn(&options).unwrap();
    assert_eq!(spawn.command, "claude.exe");
    assert_eq!(spawn.env["CLAUDE_CODE_ENTRYPOINT"], "t3-code");
    assert_eq!(spawn.env["CLAUDE_AGENT_SDK_VERSION"], "explicit");
    assert!(
        !spawn
            .env
            .contains_key("CLAUDE_CODE_SDK_READS_SESSION_STATE")
    );
    assert!(!spawn.env.contains_key("NODE_OPTIONS"));
    assert!(!spawn.env.contains_key("DEBUG"));
    options
        .env
        .insert("DEBUG_CLAUDE_AGENT_SDK".into(), " YeS ".into());
    assert_eq!(describe_spawn(&options).unwrap().env["DEBUG"], "1");
    options.env = Environment::new();
    let spawn = describe_spawn(&options).unwrap();
    assert_eq!(spawn.env["CLAUDE_AGENT_SDK_VERSION"], PROTOCOL_BASELINE.sdk);
    assert_eq!(spawn.env["CLAUDE_CODE_SDK_READS_SESSION_STATE"], "1");
    assert_eq!(spawn.initialize.system_prompt, Some(vec![String::new()]));
}

#[test]
fn explicit_empty_settings_sources_and_tools_are_distinct_from_omission_and_mcp_is_stdin_only_when_postponed()
 {
    let mut options = LaunchOptions {
        cli_path: Some("claude".into()),
        ..Default::default()
    };
    let initial = describe_spawn(&options).unwrap();
    assert!(
        !initial
            .args
            .iter()
            .any(|v| v.starts_with("--setting-sources") || v.starts_with("--tools"))
    );
    options.setting_sources = Some(vec![]);
    options.tools = Some(Tools::List(vec![]));
    options.mcp_servers = BTreeMap::from([(
        "s".into(),
        json!({"type":"http","url":"https://server","headers":{"Authorization":"private"}}),
    )]);
    let spawn = describe_spawn(&options).unwrap();
    assert!(spawn.args.contains(&"--setting-sources=".into()));
    assert!(spawn.args.contains(&"--tools=".into()));
    let index = spawn.args.iter().position(|v| v == "--mcp-config").unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&spawn.args[index + 1]).unwrap(),
        json!({"mcpServers":options.mcp_servers})
    );
    options.mcp_servers.clear();
    let spawn = describe_spawn(&options).unwrap();
    assert!(
        !spawn
            .args
            .iter()
            .any(|v| v.contains("private") || v.contains("mcp-config"))
    );
}

#[test]
fn platform_binary_candidates_and_native_js_detection_match_sdk() {
    assert_eq!(
        binary_candidates("linux", "x64", true),
        vec![
            "@anthropic-ai/claude-agent-sdk-linux-x64-musl/claude",
            "@anthropic-ai/claude-agent-sdk-linux-x64/claude"
        ]
    );
    assert_eq!(
        binary_candidates("win32", "arm64", false),
        vec!["@anthropic-ai/claude-agent-sdk-win32-arm64/claude.exe"]
    );
    assert_eq!(
        binary_candidates("android", "arm64", false),
        vec!["@anthropic-ai/claude-agent-sdk-linux-arm64-android/claude"]
    );
    let mut options = LaunchOptions {
        platform: "linux".into(),
        arch: "x64".into(),
        resolved_packages: vec![(
            "@anthropic-ai/claude-agent-sdk-linux-x64-musl/claude".into(),
            "/sdk/musl".into(),
        )],
        resume: Some("s".into()),
        custom_spawn: false,
        ..Default::default()
    };
    let spawn = describe_spawn(&options).unwrap();
    assert_eq!(spawn.command, "/sdk/musl");
    assert!(spawn.args.contains(&"--await-initialize".into()));
    options.custom_spawn = true;
    assert!(
        !describe_spawn(&options)
            .unwrap()
            .args
            .contains(&"--await-initialize".into())
    );
    for path in ["file.js", "file.mjs", "file.tsx", "file.ts", "file.jsx"] {
        options.cli_path = Some(path.into());
        let spawn = describe_spawn(&options).unwrap();
        assert_eq!(spawn.command, "node");
        assert_eq!(spawn.args[0], path);
    }
    options.cli_path = Some("file.JS".into());
    assert_eq!(describe_spawn(&options).unwrap().command, "file.JS");
}

#[test]
fn ports_windows_executable_resolution_native_before_js_and_original_on_missing_entry() {
    let shim = r"C:\npm\claude.cmd";
    let native = r"C:\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe";
    let js = r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js";
    assert_eq!(
        resolve_executable("claude", "win32", Some(shim), |p| [native, js].contains(&p)),
        native
    );
    assert_eq!(
        resolve_executable("claude", "win32", Some(shim), |p| p == js),
        js
    );
    assert_eq!(
        resolve_executable("claude", "win32", Some(shim), |_| false),
        "claude"
    );
    assert_eq!(
        resolve_executable("claude", "linux", Some(shim), |p| p == native),
        "claude"
    );
    assert_eq!(
        resolve_executable("claude", "win32", Some("C:/native/claude.exe"), |_| false),
        "C:/native/claude.exe"
    );
}

#[test]
fn ports_t3_home_environment_isolation_and_resume_mcp_connection_behavior() {
    let inherited = Environment::from([
        ("HOME".into(), "/real/home".into()),
        ("CLAUDE_CONFIG_DIR".into(), "~/literal".into()),
        ("CLAUDECODE".into(), "1".into()),
        ("CLAUDE_CODE_MESSAGING_TOKEN".into(), "parent-token".into()),
        ("CLAUDE_EFFORT".into(), "low".into()),
        ("ELECTRON_RUN_AS_NODE".into(), "1".into()),
        ("CLAUDE_CODE_ENTRYPOINT".into(), "parent".into()),
    ]);
    let env = t3_environment(&inherited, &Environment::new(), None, true);
    assert_eq!(env["HOME"], "/real/home");
    assert_eq!(env["CLAUDE_CONFIG_DIR"], "~/literal");
    assert_eq!(env["MCP_CONNECTION_NONBLOCKING"], "0");
    assert_eq!(env["CLAUDE_CODE_ENTRYPOINT"], "t3-code");
    for key in [
        "CLAUDECODE",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CLAUDE_EFFORT",
        "ELECTRON_RUN_AS_NODE",
    ] {
        assert!(!env.contains_key(key));
    }
    let env = t3_environment(
        &inherited,
        &Environment::from([
            ("CLAUDE_EFFORT".into(), "high".into()),
            ("MCP_CONNECTION_NONBLOCKING".into(), "1".into()),
            ("CLAUDE_CODE_ENTRYPOINT".into(), "explicit".into()),
        ]),
        Some("/isolated/home"),
        true,
    );
    assert_eq!(env["CLAUDE_CONFIG_DIR"], "/isolated/home");
    assert_eq!(env["CLAUDE_EFFORT"], "high");
    assert_eq!(env["MCP_CONNECTION_NONBLOCKING"], "1");
    assert_eq!(env["CLAUDE_CODE_ENTRYPOINT"], "explicit");
}

#[test]
fn rejects_sdk_option_conflicts_before_any_transport_is_created() {
    let mut options = LaunchOptions {
        cli_path: Some("claude".into()),
        can_use_tool: true,
        permission_prompt_tool: Some("other".into()),
        ..Default::default()
    };
    assert!(describe_spawn(&options).is_err());
    options.permission_prompt_tool = None;
    options.model = Some("same".into());
    options.fallback_model = options.model.clone();
    assert!(describe_spawn(&options).is_err());
}

#[test]
fn zero_task_budget_and_bare_shims_match_typescript() {
    let spawn = describe_spawn(&LaunchOptions {
        cli_path: Some("claude".into()),
        task_budget: Some(0),
        ..Default::default()
    })
    .unwrap();
    assert!(spawn.args.contains(&"--task-budget=0".into()));
    assert_eq!(
        resolve_executable("claude.cmd", "win32", None, |p| p
            == r"node_modules\@anthropic-ai\claude-code\cli.js"),
        r"node_modules\@anthropic-ai\claude-code\cli.js"
    );
}

#[test]
fn windows_configured_environment_replaces_inherited_case_variants() {
    if !cfg!(windows) {
        return;
    }
    let env = t3_environment(
        &Environment::from([
            ("Path".into(), "old".into()),
            ("pAtH".into(), "old2".into()),
        ]),
        &Environment::from([("path".into(), "configured".into())]),
        None,
        false,
    );
    assert_eq!(env.get("PATH").map(String::as_str), Some("configured"));
    assert!(!env.contains_key("Path"));
    assert!(!env.contains_key("pAtH"));
}
