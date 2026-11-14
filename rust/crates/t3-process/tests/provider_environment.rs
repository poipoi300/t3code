#![allow(clippy::unwrap_used)]
use std::{collections::BTreeMap, path::Path};
use t3_process::provider_environment::*;

#[test]
fn t3_owned_variables_are_withheld_and_the_rest_is_inherited() {
    let server: BTreeMap<String, String> = [
        ("PATH", "/usr/bin"),
        ("HOME", "/home/user"),
        ("LANG", "en_US.UTF-8"),
        ("HTTPS_PROXY", "http://proxy.invalid"),
        ("OPENAI_API_KEY", "vendor-key"),
        ("GH_TOKEN", "gh-token"),
        ("T3CODE_DEV_AUTH_TOKEN", "t3-secret"),
        ("t3code_bitbucket_access_token", "t3-secret"),
        ("T3CODE_HOME", "/home/user/.t3"),
        ("T3_ACP_MCP_AUTHORIZATION", "Bearer t3-secret"),
        ("EXPO_TOKEN", "t3-secret"),
        ("TUNNEL_TOKEN", "t3-secret"),
        ("CAFE_TOKEN", "t3-secret"),
        ("T3CODE_CODEX_LAUNCH_ARGS", "--model x"),
        ("T3CODE_CLAUDE_PROBE_TIMEOUT_MS", "90000"),
        ("T3_FAKE_PROVIDER", "fixture"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    let child = provider_base_environment(&server);
    let names: Vec<&str> = child.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "GH_TOKEN",
            "HOME",
            "HTTPS_PROXY",
            "LANG",
            "OPENAI_API_KEY",
            "PATH",
            "T3CODE_CLAUDE_PROBE_TIMEOUT_MS",
            "T3CODE_CODEX_LAUNCH_ARGS",
            "T3_FAKE_PROVIDER",
        ]
    );
    assert!(!child.values().any(|value| value.contains("t3-secret")));
}

/// The credential-like names (`*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*API_KEY*`, ...) quoted in
/// `source`, and every `T3CODE_*` name.
fn declared_names(source: &str) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "API_KEY",
        "AUTHORIZATION",
        "CREDENTIAL",
        "PRIVATE_KEY",
    ];
    source
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| {
            literal.len() > 3
                && literal
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
                && literal
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && (literal.starts_with(T3_NAMESPACE)
                    || MARKERS.iter().any(|marker| literal.contains(marker)))
        })
        .map(str::to_owned)
        .collect()
}

/// Names that neither belong to T3 (withheld) nor are inherited by design.
fn unclassified(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| {
            !is_t3_owned(name)
                && !is_launch_setting(name)
                && !INHERITED_BY_DESIGN.contains(&name.as_str())
        })
        .collect()
}

#[test]
fn a_new_credential_must_be_classified() {
    let source =
        r#"let token = environment.get("RELAY_SHARED_SECRET"); env.get("T3CODE_NEW_SETTING");"#;
    assert_eq!(
        unclassified(declared_names(source)),
        ["RELAY_SHARED_SECRET"]
    );
}

/// Every credential-like variable name the server-side crates quote is either T3's own (and
/// withheld from provider children) or listed as inherited by design. A new one fails here
/// until it is classified in `t3_process::provider_environment`.
#[test]
fn every_credential_t3_code_names_is_classified() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = BTreeMap::<String, Vec<String>>::new();
    let mut pending = vec![];
    for entry in std::fs::read_dir(&crates).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // Client and UI crates run in the client; they launch no provider child.
        if matches!(
            name.as_str(),
            "t3-client" | "t3-ui" | "t3-i18n" | "t3-theme"
        ) {
            continue;
        }
        if path.join("src").is_dir() {
            pending.push(path.join("src"));
        }
    }
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for name in declared_names(&source) {
                    found
                        .entry(name)
                        .or_default()
                        .push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        found.contains_key("T3CODE_DEV_AUTH_TOKEN"),
        "the scan found nothing"
    );
    let missing = unclassified(found.keys().cloned());
    assert!(
        missing.is_empty(),
        "classify these in t3-process provider_environment: {:?}",
        missing
            .iter()
            .map(|name| (name, &found[name]))
            .collect::<Vec<_>>()
    );
}
