//! Pure Codex text-generation launch policy; the host owns probing and execution.

fn replaces_mcp_table(argument: &str) -> bool {
    argument.match_indices("mcp_servers").any(|(at, _)| {
        if at > 0
            && matches!(argument.as_bytes()[at - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        {
            return false;
        }
        argument[at + "mcp_servers".len()..]
            .trim_start_matches(|ch: char| ch.is_whitespace() && ch != '\u{85}' || ch == '\u{feff}')
            .starts_with('=')
    })
}

/// Takes the filtered `codexExecLaunchArgs` argv and enabled MCP names in discovery order.
/// Append `--ephemeral` and the remaining operation/model/output flags after this prefix.
pub fn codex_exec_mcp_prefix(
    exec_launch_args: &[String],
    enabled_servers: &[String],
) -> Vec<String> {
    let off = (!enabled_servers.is_empty()).then(|| {
        let entries = enabled_servers
            .iter()
            .map(|name| {
                format!(
                    "{}={{enabled=false}}",
                    serde_json::Value::String(name.clone())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("mcp_servers={{{entries}}}")
    });
    let mut args = vec!["exec".into()];
    if let Some(off) = &off {
        args.extend(["--config".into(), off.clone()]);
    }
    args.extend_from_slice(exec_launch_args);
    args.extend(
        [
            "--config",
            "features.plugins=false",
            "--config",
            "features.apps=false",
        ]
        .map(str::to_owned),
    );
    if let Some(off) = &off
        && exec_launch_args.iter().any(|arg| replaces_mcp_table(arg))
    {
        args.extend(["--config".into(), off.clone()]);
    }
    for name in enabled_servers
        .iter()
        .filter(|name| !name.contains(['.', '=']))
    {
        args.extend([
            "--config".into(),
            format!("mcp_servers.{name}.enabled=false"),
        ]);
    }
    args
}
