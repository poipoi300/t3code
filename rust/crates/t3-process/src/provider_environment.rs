//! What a provider child (a coding agent, its probes, helpers and text-generation runners)
//! inherits from the server's own environment.
//!
//! An agent needs the ordinary process environment to run (PATH, home, locale, proxy and
//! certificate settings, and its own vendor's credentials when the user keeps them there), so the
//! base is the server environment minus what belongs to T3 itself: everything in the `T3CODE_`
//! namespace and the credentials T3 reads for its own services. Values a launch sets on purpose
//! (an instance's declared variables, a per-session token) are added after this, so they survive.
use std::collections::BTreeMap;

/// T3's own namespace (TypeScript reads `T3CODE_*` for its configuration and secrets).
pub const T3_NAMESPACE: &str = "T3CODE_";

/// Launch settings TypeScript reads from the provider environment itself (`resolveCodexLaunchArgs`,
/// `resolveProviderProbeTimeoutMs`), which managed policy can lock on an instance. They carry no
/// secret; the launch code reads them from the merged environment, so they stay in it.
pub fn is_launch_setting(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name == "T3CODE_CODEX_LAUNCH_ARGS"
        || (name.starts_with(T3_NAMESPACE) && name.ends_with("_PROBE_TIMEOUT_MS"))
}

/// Credentials outside the namespace that T3 reads for its own use: the T3 MCP credentials
/// (`t3-mcp`, `pi_launch`), the device cloud's tokens (`t3-device`), Connect's tunnel token
/// (`t3-cloud`) and the git.cafe forge token (`t3-forge`).
pub const T3_CREDENTIALS: &[&str] = &[
    "T3_ACP_MCP_ENDPOINT",
    "T3_ACP_MCP_AUTHORIZATION",
    "T3_MCP_BEARER_TOKEN",
    "EXPO_TOKEN",
    "AGENT_DEVICE_DAEMON_AUTH_TOKEN",
    "TUNNEL_TOKEN",
    "CAFE_TOKEN",
];

/// Credentials T3 code also names that are the agents' own (their vendors' keys and the GitHub
/// CLI's tokens an agent's `gh` uses). They are inherited as in TypeScript; whether a
/// third-party agent should see another vendor's key is a product decision, recorded upstream.
pub const INHERITED_BY_DESIGN: &[&str] = &[
    "OPENAI_API_KEY",
    "ACCESS_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CURSOR_API_KEY",
    "CURSOR_AUTH_TOKEN",
    "XAI_API_KEY",
    "KIRO_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "META_API_KEY",
    "OPENCODE_API_KEY",
    "OPENCODE_PASSWORD",
    "OPENCODE_SERVER_PASSWORD",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// Whether a server variable named `name` belongs to T3 and is withheld from provider children.
pub fn is_t3_owned(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    (upper.starts_with(T3_NAMESPACE) && !is_launch_setting(&upper))
        || T3_CREDENTIALS.contains(&upper.as_str())
}

/// The base environment of a provider child: `server` without T3's own variables.
pub fn provider_base_environment(server: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    server
        .iter()
        .filter(|(name, _)| !is_t3_owned(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}
