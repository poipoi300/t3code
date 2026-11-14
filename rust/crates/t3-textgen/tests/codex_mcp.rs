use t3_textgen::codex::codex_exec_mcp_prefix;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}

#[test]
fn keeps_custom_pr_instructions_and_disables_listed_servers_before_launch_args() {
    let instructions = "developer_instructions=\"Use TEAM: as the PR title prefix\"";
    let args = codex_exec_mcp_prefix(
        &strings(&["--config", instructions]),
        &strings(&["augmenter", "my.server"]),
    );
    assert_eq!(
        args,
        strings(&[
            "exec",
            "--config",
            "mcp_servers={\"augmenter\"={enabled=false},\"my.server\"={enabled=false}}",
            "--config",
            instructions,
            "--config",
            "features.plugins=false",
            "--config",
            "features.apps=false",
            "--config",
            "mcp_servers.augmenter.enabled=false"
        ])
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("features.shell_tool=false"))
    );
}

#[test]
fn turns_off_a_launch_defined_server_after_its_transport_without_replacing_its_table() {
    let launch = strings(&[
        "-c",
        "mcp_servers.t3-code.url=\"http://127.0.0.1:1/mcp\"",
        "-c",
        "mcp_servers.t3-code.enabled=true",
    ]);
    let args = codex_exec_mcp_prefix(&launch, &strings(&["t3-code"]));
    assert_eq!(
        args,
        strings(&[
            "exec",
            "--config",
            "mcp_servers={\"t3-code\"={enabled=false}}",
            "-c",
            "mcp_servers.t3-code.url=\"http://127.0.0.1:1/mcp\"",
            "-c",
            "mcp_servers.t3-code.enabled=true",
            "--config",
            "features.plugins=false",
            "--config",
            "features.apps=false",
            "--config",
            "mcp_servers.t3-code.enabled=false"
        ])
    );
}

#[test]
fn repeats_the_off_table_after_a_launch_table_and_keeps_dotted_keys_out_of_flags() {
    let table = "mcp_servers={\"my.server\"={command=\"echo\",enabled=true}}";
    let off = "mcp_servers={\"my.server\"={enabled=false},\"my server\"={enabled=false},\"a=b\"={enabled=false}}";
    let names = strings(&["my.server", "my server", "a=b"]);
    assert_eq!(
        codex_exec_mcp_prefix(&strings(&["-c", table]), &names),
        strings(&[
            "exec",
            "--config",
            off,
            "-c",
            table,
            "--config",
            "features.plugins=false",
            "--config",
            "features.apps=false",
            "--config",
            off,
            "--config",
            "mcp_servers.my server.enabled=false"
        ])
    );
    for (value, replaces) in [
        ("prefix_mcp_servers={}", false),
        ("émcp_servers\u{feff}={}", true),
        ("mcp_servers\u{85}={}", false),
        ("mcp_servers.foo={}", false),
    ] {
        let args = codex_exec_mcp_prefix(&strings(&["-c", value]), &names);
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == off).count(),
            if replaces { 2 } else { 1 },
            "{value}"
        );
    }
    assert_eq!(
        codex_exec_mcp_prefix(&[], &[]),
        strings(&[
            "exec",
            "--config",
            "features.plugins=false",
            "--config",
            "features.apps=false"
        ])
    );
}
