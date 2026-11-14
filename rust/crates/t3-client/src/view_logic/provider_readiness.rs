//! Agents on the welcome page: whether each is ready, what to run to install it or sign in, and how
//! the page groups them. Ports `apps/web/src/onboarding/providerReadiness.logic.ts` and
//! `welcomePage.logic.ts`, with `managedCapabilityDenial` and `isProviderAvailable` from
//! `packages/contracts`. Name ordering is the caller's (locale-aware), as `localeCompare` is.

use std::cmp::Ordering;

use t3_contracts::environment::ExecutionEnvironmentPlatformOs;
use t3_contracts::managed_policy::{ManagedPolicyClient, ManagedPolicySummary};
use t3_contracts::provider_instance::ProviderInstanceConfig;
use t3_contracts::server::{
    ServerProvider, ServerProviderAuthStatus, ServerProviderAvailability,
    ServerProviderCompatibilityStatus, ServerProviderState,
};
use t3_contracts::settings::ServerSettings;

use crate::view_logic::shell::models::project_instance_enabled;

/// `getOnboardingProviderState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardingProviderState {
    Checking,
    Disabled,
    Install,
    SignIn,
    Ready,
    Attention,
}

pub fn onboarding_provider_state(provider: Option<&ServerProvider>) -> OnboardingProviderState {
    use OnboardingProviderState::*;
    let Some(provider) = provider else {
        return Checking;
    };
    if !provider.enabled || provider.status == ServerProviderState::Disabled {
        return Disabled;
    }
    if !provider.installed
        && provider.status == ServerProviderState::Warning
        && provider.auth.status == ServerProviderAuthStatus::Unknown
    {
        return Checking;
    }
    if !provider.installed {
        return Install;
    }
    if provider.auth.status == ServerProviderAuthStatus::Unauthenticated {
        return SignIn;
    }
    if provider.status == ServerProviderState::Ready {
        return Ready;
    }
    Attention
}

/// `isProviderAvailable`.
pub fn is_provider_available(provider: &ServerProvider) -> bool {
    !matches!(
        provider.availability,
        Some(Some(ServerProviderAvailability::Unavailable))
    )
}

fn is_safe_shell_binary(path: &str) -> bool {
    !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '\\' | '-'))
}

/// `quoteProviderBinary`.
fn quote_provider_binary(
    path: &str,
    fallback: &str,
    platform: ExecutionEnvironmentPlatformOs,
) -> String {
    use ExecutionEnvironmentPlatformOs::*;
    if is_safe_shell_binary(path) && (platform == Windows || !path.contains('\\')) {
        return path.to_owned();
    }
    let posix = |text: &str| text.replace('\'', r#"'"'"'"#);
    match platform {
        Windows => format!("& '{}'", path.replace('\'', "''")),
        Darwin | Linux => match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
            Some(rest) => format!("~/'{}'", posix(rest)),
            None => format!("'{}'", posix(path)),
        },
        Unknown => fallback.to_owned(),
    }
}

/// `resolveOnboardingProviderInstallCommand`: native installers that need no Node or npm.
pub fn onboarding_install_command(
    driver: &str,
    platform: ExecutionEnvironmentPlatformOs,
) -> Option<&'static str> {
    let windows = platform == ExecutionEnvironmentPlatformOs::Windows;
    match driver {
        "claudeAgent" => Some(if windows {
            "irm https://claude.ai/install.ps1 | iex"
        } else {
            "curl -fsSL https://claude.ai/install.sh | bash"
        }),
        "codex" => Some(if windows {
            "irm https://chatgpt.com/codex/install.ps1 | iex"
        } else {
            "curl -fsSL https://chatgpt.com/codex/install.sh | sh"
        }),
        _ => None,
    }
}

fn binary_path(settings: &ServerSettings, provider: &ServerProvider, default: &str) -> String {
    settings
        .provider_instances
        .get(&provider.instance_id)
        .and_then(|instance| instance.config.as_ref())
        .and_then(|config| config.get("binaryPath"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| default.to_owned())
}

/// `resolveOnboardingProviderLoginCommand`.
pub fn onboarding_login_command(
    provider: &ServerProvider,
    settings: &ServerSettings,
    platform: ExecutionEnvironmentPlatformOs,
) -> String {
    match provider.driver.0.as_str() {
        "claudeAgent" => format!(
            "{} auth login",
            quote_provider_binary(
                &binary_path(settings, provider, "claude"),
                "claude",
                platform
            )
        ),
        "codex" => format!(
            "{} login",
            quote_provider_binary(&binary_path(settings, provider, "codex"), "codex", platform)
        ),
        other => other.to_owned(),
    }
}

/// `welcomeAgentState`: an agent's state for new threads on this computer.
pub fn welcome_agent_state(
    provider: &ServerProvider,
    settings: Option<&ServerSettings>,
) -> OnboardingProviderState {
    use OnboardingProviderState::*;
    if !is_provider_available(provider) {
        return Attention;
    }
    if let Some(settings) = settings
        && !project_instance_enabled(settings, None, &provider.instance_id)
    {
        return Disabled;
    }
    let state = onboarding_provider_state(Some(provider));
    if state == Ready && is_broken(provider) {
        return Attention;
    }
    if state == Checking && provider.driver.0 == "acpRegistry" {
        return Attention;
    }
    state
}

fn is_broken(provider: &ServerProvider) -> bool {
    provider
        .compatibility_advisory
        .as_ref()
        .is_some_and(|advisory| advisory.status == ServerProviderCompatibilityStatus::Broken)
}

/// `isBrokenVersion`: a CLI the server knows is too old or otherwise broken.
pub fn is_broken_version(provider: &ServerProvider) -> bool {
    provider.installed && is_broken(provider)
}

fn found_rank(state: OnboardingProviderState) -> Option<u8> {
    match state {
        OnboardingProviderState::Ready => Some(0),
        OnboardingProviderState::SignIn => Some(1),
        OnboardingProviderState::Attention => Some(2),
        _ => None,
    }
}

/// `groupWelcomeAgents`'s result.
#[derive(Clone, Debug, Default)]
pub struct WelcomeAgents<'a> {
    pub checking: bool,
    pub ready_count: usize,
    pub found: Vec<&'a ServerProvider>,
    pub available: Vec<&'a ServerProvider>,
}

/// `groupWelcomeAgents`: agents already on the computer (ready, then needing sign-in, then a look,
/// each by name) and agents to add (by name). Agents still being checked are listed with the ones
/// to add, so a slow check never drops one. `providers: None` means the list has not arrived.
pub fn group_welcome_agents<'a>(
    providers: Option<&'a [ServerProvider]>,
    settings: Option<&ServerSettings>,
    name_of: impl Fn(&ServerProvider) -> String,
    is_allowed: impl Fn(&ServerProvider) -> bool,
    compare_names: impl Fn(&str, &str) -> Ordering,
) -> WelcomeAgents<'a> {
    let mut groups = WelcomeAgents {
        checking: providers.is_none(),
        ..WelcomeAgents::default()
    };
    let mut found = Vec::new();
    for provider in providers.unwrap_or(&[]) {
        if !is_allowed(provider) {
            continue;
        }
        let state = welcome_agent_state(provider, settings);
        if state == OnboardingProviderState::Ready {
            groups.ready_count += 1;
        }
        if state == OnboardingProviderState::Checking {
            groups.checking = true;
        }
        match found_rank(state) {
            Some(rank) => found.push((rank, provider)),
            None => groups.available.push(provider),
        }
    }
    let by_name = |a: &ServerProvider, b: &ServerProvider| compare_names(&name_of(a), &name_of(b));
    found.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then_with(|| by_name(a, b)));
    groups.available.sort_by(|a, b| by_name(a, b));
    groups.found = found.into_iter().map(|(_, provider)| provider).collect();
    groups
}

/// `turnedOnInstance`: older settings can also keep `enabled: false` inside `config`.
pub fn turned_on_instance(instance: &ProviderInstanceConfig) -> ProviderInstanceConfig {
    let mut next = instance.clone();
    next.enabled = Some(true);
    if let Some(serde_json::Value::Object(config)) = next.config.as_mut()
        && config.get("enabled") == Some(&serde_json::Value::Bool(false))
    {
        config.insert("enabled".into(), serde_json::Value::Bool(true));
    }
    next
}

/// `cliTerminalStep`: what "Use existing CLI" does next for Codex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliTerminalStep {
    Wait,
    Open,
    Done,
}

pub fn cli_terminal_step(
    provider: &ServerProvider,
    settings: Option<&ServerSettings>,
) -> CliTerminalStep {
    use OnboardingProviderState::*;
    if provider
        .setup
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|setup| setup.can_authenticate)
    {
        return CliTerminalStep::Wait;
    }
    let state = welcome_agent_state(provider, settings);
    if state == Checking {
        return CliTerminalStep::Wait;
    }
    if is_broken_version(provider) {
        return CliTerminalStep::Done;
    }
    if provider.installed && provider.auth.status == ServerProviderAuthStatus::Authenticated {
        return CliTerminalStep::Done;
    }
    let fixable = state == Install
        || state == SignIn
        || (state == Attention && is_provider_available(provider));
    if fixable {
        CliTerminalStep::Open
    } else {
        CliTerminalStep::Done
    }
}

/// `codexNeedsSetup`: whether Codex's row opens its setup card.
pub fn codex_needs_setup(provider: &ServerProvider, settings: Option<&ServerSettings>) -> bool {
    use OnboardingProviderState::*;
    if provider.driver.0 != "codex"
        || provider.auth.status == ServerProviderAuthStatus::Authenticated
        || is_broken_version(provider)
    {
        return false;
    }
    let state = welcome_agent_state(provider, settings);
    state == Install || state == SignIn || (state == Attention && is_provider_available(provider))
}

/// `managedCapabilityDenial` for the two capabilities the welcome page asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedCapability {
    RemoteClients,
    Connect,
}

pub fn managed_capability_denial(
    summary: Option<&ManagedPolicySummary>,
    capability: ManagedCapability,
) -> Option<String> {
    let summary = summary?;
    let limits = summary.limits.as_ref()?;
    let (denied, label) = match capability {
        ManagedCapability::RemoteClients => (
            limits
                .clients
                .as_ref()
                .is_some_and(|clients| !clients.contains(&ManagedPolicyClient::Remote)),
            "connecting from other devices",
        ),
        ManagedCapability::Connect => (limits.connect == Some(false), "T3 Connect"),
    };
    denied.then(|| {
        format!(
            "{}'s policy does not allow {label}.",
            summary.organization.0
        )
    })
}

/// `welcomeOffersPhone`: the phone joins through T3 Connect, which shares the primary computer.
pub fn welcome_offers_phone(
    is_primary: bool,
    cloud: bool,
    can_share: bool,
    policy: Option<&ManagedPolicySummary>,
) -> bool {
    is_primary
        && cloud
        && can_share
        && managed_capability_denial(policy, ManagedCapability::RemoteClients).is_none()
        && managed_capability_denial(policy, ManagedCapability::Connect).is_none()
}

/// `codexCardProvider`: the Codex instance the setup card shows, or `None` when it is closed. A
/// rebuild briefly drops an instance, so the card keeps its last status meanwhile.
pub fn codex_card_provider<'a>(
    providers: Option<&'a [ServerProvider]>,
    instance_id: Option<&str>,
    last: Option<&'a ServerProvider>,
    settings: Option<&ServerSettings>,
) -> Option<&'a ServerProvider> {
    let instance_id = instance_id?;
    let provider = providers
        .and_then(|providers| providers.iter().find(|p| p.instance_id.0 == instance_id))
        .or_else(|| last.filter(|last| last.instance_id.0 == instance_id))?;
    let open = codex_needs_setup(provider, settings)
        || welcome_agent_state(provider, settings) == OnboardingProviderState::Checking;
    open.then_some(provider)
}

#[cfg(test)]
#[path = "provider_readiness_tests.rs"]
mod tests;
