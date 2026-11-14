//! The `/usage-limits` command decoration from `packages/shared/src/usageLimits.ts`
//! (`USAGE_LIMITS_COMMAND`, `providersWithLimits`, `withUsageLimitsCommands`).
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    provider_usage_limits::UsageLimitSourceSnapshot,
    server::{ServerProvider, ServerProviderAvailability, ServerProviderSlashCommand},
};

pub const USAGE_LIMITS_COMMAND_NAME: &str = "usage-limits";
pub const USAGE_LIMITS_COMMAND_DESCRIPTION: &str = "Show this provider's usage limits";

/// TypeScript `USAGE_LIMITS_COMMAND`.
pub fn usage_limits_command() -> ServerProviderSlashCommand {
    ServerProviderSlashCommand {
        name: TrimmedNonEmptyString(USAGE_LIMITS_COMMAND_NAME.into()),
        description: Some(Some(TrimmedNonEmptyString(
            USAGE_LIMITS_COMMAND_DESCRIPTION.into(),
        ))),
        input: None,
    }
}

/// TypeScript `isProviderAvailable` (`packages/contracts/src/server.ts`).
fn is_provider_available(provider: &ServerProvider) -> bool {
    !matches!(
        provider.availability,
        Some(Some(ServerProviderAvailability::Unavailable))
    )
}

/// TypeScript `providersWithLimits`: `usageLimits !== undefined`, so a present null counts.
fn has_limits(provider: &ServerProvider) -> bool {
    provider.enabled
        && provider.installed
        && is_provider_available(provider)
        && provider.usage_limits.is_some()
}

fn unreadable(source: &UsageLimitSourceSnapshot) -> bool {
    source.error.is_some() && source.accounts.is_empty()
}

fn with_command(items: &[ServerProviderSlashCommand]) -> Vec<ServerProviderSlashCommand> {
    items
        .iter()
        .filter(|command| command.name.0 != USAGE_LIMITS_COMMAND_NAME)
        .cloned()
        .chain(std::iter::once(usage_limits_command()))
        .collect()
}

/// TypeScript `withUsageLimitsCommands`: advertise `/usage-limits` on every provider whose driver
/// has limits (its own, or a hub account's), and on all of them while a hub cannot be read.
/// Workspace catalogs replace the global command list, so they are decorated too. A provider's
/// own `usage-limits` command is replaced.
pub fn with_usage_limits_commands(
    providers: &[ServerProvider],
    sources: &[UsageLimitSourceSnapshot],
) -> Vec<ServerProvider> {
    let supported: std::collections::BTreeSet<&str> = providers
        .iter()
        .filter(|provider| has_limits(provider))
        .map(|provider| provider.driver.0.as_str())
        .chain(
            sources
                .iter()
                .flat_map(|source| &source.accounts)
                .map(|account| account.driver.0.as_str()),
        )
        .collect();
    let unreadable_source = sources.iter().any(unreadable);
    providers
        .iter()
        .map(|provider| {
            if !unreadable_source && !supported.contains(provider.driver.0.as_str()) {
                return provider.clone();
            }
            let mut decorated = provider.clone();
            decorated.slash_commands = with_command(&provider.slash_commands);
            if let Some(snapshots) = &mut decorated.workspace_snapshots {
                for snapshot in snapshots {
                    snapshot.slash_commands = with_command(&snapshot.slash_commands);
                }
            }
            decorated
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn provider(overrides: Value) -> ServerProvider {
        let mut value = json!({
            "instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":null,
            "status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-09-03T11:00:00.000Z",
            "models":[],"slashCommands":[],"skills":[]
        });
        for (key, field) in overrides.as_object().unwrap() {
            value[key] = field.clone();
        }
        serde_json::from_value(value).unwrap()
    }
    fn limits() -> Value {
        json!({"checkedAt":"2026-09-03T11:00:00.000Z","windows":[{"id":"five_hour","kind":"session","label":"Session","usedPercent":40,"windowDurationMins":300,"resetsAt":"2026-09-03T14:00:00.000Z"}]})
    }
    fn source(accounts: Value, error: Option<&str>) -> UsageLimitSourceSnapshot {
        let mut value = json!({"id":"hub","kind":"cliproxy","label":"Accounts","checkedAt":"2026-09-03T11:00:00.000Z","accounts":accounts});
        if let Some(error) = error {
            value["error"] = json!(error);
        }
        serde_json::from_value(value).unwrap()
    }
    fn hub_accounts() -> Value {
        json!([
            {"id":"duplicate","driver":"codex","usageLimits":limits()},
            {"id":"oss","driver":"codex","usageLimits":limits()},
            {"id":"claude","driver":"claudeAgent","usageLimits":limits()}
        ])
    }
    fn names(commands: &[ServerProviderSlashCommand]) -> Vec<&str> {
        commands.iter().map(|command| command.name.0.as_str()).collect()
    }

    /// `usageLimits.test.ts`: "advertises global and workspace commands only for providers present
    /// in Limits".
    #[test]
    fn advertises_global_and_workspace_commands_only_for_providers_present_in_limits() {
        let sources = [source(hub_accounts(), None)];
        let with_workspace = provider(json!({"workspaceSnapshots":[
            {"cwd":"/tmp/project","checkedAt":"2026-09-03T11:00:00.000Z","slashCommands":[],"skills":[]}
        ]}));
        let supported = &with_usage_limits_commands(std::slice::from_ref(&with_workspace), &sources)[0];
        assert_eq!(names(&supported.slash_commands), ["usage-limits"]);
        assert_eq!(
            names(&supported.workspace_snapshots.as_ref().unwrap()[0].slash_commands),
            ["usage-limits"]
        );
        assert!(
            with_usage_limits_commands(std::slice::from_ref(&with_workspace), &[])[0]
                .slash_commands
                .is_empty()
        );
        let own = provider(json!({"slashCommands":[{"name":"usage-limits","description":"Provider's own"}]}));
        let kept = &with_usage_limits_commands(std::slice::from_ref(&own), &[])[0];
        assert_eq!(kept.slash_commands, own.slash_commands);
        let unreadable = source(json!([]), Some("token expired"));
        assert_eq!(
            names(&with_usage_limits_commands(std::slice::from_ref(&with_workspace), &[unreadable])[0].slash_commands),
            ["usage-limits"]
        );
        let selected = provider(json!({"usageLimits":limits()}));
        assert_eq!(
            names(&with_usage_limits_commands(std::slice::from_ref(&selected), &[])[0].slash_commands),
            ["usage-limits"]
        );
        let unrelated = provider(json!({"driver":"claudeAgent"}));
        let disabled = provider(json!({"driver":"claudeAgent","enabled":false,"usageLimits":limits()}));
        let mixed = with_usage_limits_commands(
            &[selected, with_workspace, unrelated.clone(), disabled.clone()],
            &[],
        );
        let advertised: Vec<Vec<&str>> = mixed.iter().map(|p| names(&p.slash_commands)).collect();
        assert_eq!(
            advertised,
            [vec!["usage-limits"], vec!["usage-limits"], vec![], vec![]]
        );
        assert_eq!(mixed[2], unrelated);
        assert_eq!(mixed[3], disabled);
    }

    /// A provider's own command of the same name is replaced by T3's, after its other commands.
    #[test]
    fn a_provider_command_of_the_same_name_is_replaced_and_appended_last() {
        let own = provider(json!({"usageLimits":limits(),"slashCommands":[
            {"name":"usage-limits","description":"Provider's own"},{"name":"review"}
        ]}));
        let decorated = &with_usage_limits_commands(&[own], &[])[0];
        assert_eq!(names(&decorated.slash_commands), ["review", "usage-limits"]);
        assert_eq!(decorated.slash_commands[1], usage_limits_command());
    }

    /// `providersWithLimits` skips providers that are uninstalled or reported unavailable.
    #[test]
    fn unavailable_or_uninstalled_providers_do_not_bring_their_driver() {
        for overrides in [
            json!({"usageLimits":limits(),"installed":false}),
            json!({"usageLimits":limits(),"availability":"unavailable"}),
        ] {
            let decorated = &with_usage_limits_commands(&[provider(overrides)], &[])[0];
            assert!(decorated.slash_commands.is_empty());
        }
        let available = provider(json!({"usageLimits":limits(),"availability":"available"}));
        assert_eq!(
            names(&with_usage_limits_commands(&[available], &[])[0].slash_commands),
            ["usage-limits"]
        );
    }
}
