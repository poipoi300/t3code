//! Ports `providerReadiness.logic.test.ts` and `welcomePage.logic.test.ts`.

use serde_json::{Value, json};
use t3_contracts::environment::ExecutionEnvironmentPlatformOs as Os;
use t3_contracts::managed_policy::ManagedPolicySummary;
use t3_contracts::provider_instance::ProviderInstanceConfig;
use t3_contracts::server::ServerProvider;
use t3_contracts::settings::ServerSettings;

use super::*;

fn merge(base: &mut Value, overrides: Value) {
    if let (Value::Object(base), Value::Object(overrides)) = (base, overrides) {
        for (key, value) in overrides {
            base.insert(key, value);
        }
    }
}

/// welcomePage.logic.test.ts `provider(name, overrides)`.
fn provider(name: &str, overrides: Value) -> ServerProvider {
    let id = name.to_lowercase();
    let mut value = json!({
        "instanceId": id, "driver": id, "displayName": name, "enabled": true, "installed": true,
        "version": "1.0.0", "status": "ready", "auth": {"status": "authenticated"},
        "checkedAt": "2026-08-23T00:00:00.000Z", "models": [], "slashCommands": [], "skills": []
    });
    merge(&mut value, overrides);
    serde_json::from_value(value).expect("a provider")
}

/// providerReadiness.logic.test.ts `readyCodex` with overrides.
fn codex(overrides: Value) -> ServerProvider {
    let mut value = json!({
        "instanceId": "codex", "driver": "codex", "enabled": true, "installed": true,
        "version": "1.0.0", "status": "ready", "auth": {"status": "unknown"},
        "checkedAt": "2026-08-23T00:00:00.000Z", "models": [], "slashCommands": [], "skills": []
    });
    merge(&mut value, overrides);
    serde_json::from_value(value).expect("a provider")
}

fn settings(overrides: Value) -> ServerSettings {
    let mut value = serde_json::to_value(ServerSettings::default()).expect("defaults");
    merge(&mut value, overrides);
    serde_json::from_value(value).expect("settings")
}

fn name_of(provider: &ServerProvider) -> String {
    provider
        .display_name
        .clone()
        .flatten()
        .map_or_else(|| provider.driver.0.clone(), |name| name.0)
}

fn names(list: &[&ServerProvider]) -> Vec<String> {
    list.iter().map(|provider| name_of(provider)).collect()
}

fn group<'a>(
    providers: Option<&'a [ServerProvider]>,
    settings: Option<&ServerSettings>,
) -> WelcomeAgents<'a> {
    group_welcome_agents(providers, settings, name_of, |_| true, str::cmp)
}

#[test]
fn authenticated_providers_are_ready_only_when_their_status_is_ready() {
    use OnboardingProviderState::*;
    let state = |overrides| onboarding_provider_state(Some(&codex(overrides)));
    assert_eq!(state(json!({"auth": {"status": "authenticated"}})), Ready);
    assert_eq!(
        state(json!({"auth": {"status": "authenticated"}, "status": "error"})),
        Attention
    );
    assert_eq!(
        state(json!({"auth": {"status": "authenticated"}, "status": "warning"})),
        Attention
    );
    assert_eq!(
        state(json!({"status": "error", "auth": {"status": "unauthenticated"}})),
        SignIn
    );
    assert_eq!(state(json!({"status": "error"})), Attention);
    assert_eq!(state(json!({"status": "warning"})), Attention);
    assert_eq!(
        state(json!({"enabled": false, "installed": false})),
        Disabled
    );
    assert_eq!(state(json!({"status": "disabled"})), Disabled);
    assert_eq!(
        state(json!({"installed": false, "status": "error"})),
        Install
    );
    assert_eq!(onboarding_provider_state(None), Checking);
}

#[test]
fn login_commands_use_the_instance_binary_and_quote_it_for_the_shell() {
    let work = codex(json!({"instanceId": "codex_work"}));
    let with = |id: &str, driver: &str, path: &str| {
        settings(
            json!({"providerInstances": {id: {"driver": driver, "config": {"binaryPath": path}}}}),
        )
    };
    assert_eq!(
        onboarding_login_command(
            &work,
            &with("codex_work", "codex", "/opt/codex-work/bin/codex"),
            Os::Linux
        ),
        "/opt/codex-work/bin/codex login"
    );
    assert_eq!(
        onboarding_login_command(
            &codex(json!({})),
            &with(
                "codex",
                "codex",
                r"C:\Program Files\Codex & Tools\codex.exe"
            ),
            Os::Windows
        ),
        r"& 'C:\Program Files\Codex & Tools\codex.exe' login"
    );
    let claude = codex(json!({"driver": "claudeAgent", "instanceId": "claude"}));
    assert_eq!(
        onboarding_login_command(
            &claude,
            &with("claude", "claudeAgent", "/opt/Claude Tools/$current/claude"),
            Os::Linux
        ),
        "'/opt/Claude Tools/$current/claude' auth login"
    );
    for (path, expected) in [
        ("~/my tools/codex", "~/'my tools/codex' login"),
        (r"~\my tools/codex", "~/'my tools/codex' login"),
        (
            "~/tools/codex's build",
            r#"~/'tools/codex'"'"'s build' login"#,
        ),
        (
            r"~\tools\codex's build",
            r#"~/'tools\codex'"'"'s build' login"#,
        ),
        (
            "~/tools/codex; echo unsafe",
            "~/'tools/codex; echo unsafe' login",
        ),
    ] {
        assert_eq!(
            onboarding_login_command(&codex(json!({})), &with("codex", "codex", path), Os::Linux),
            expected,
            "{path}"
        );
    }
    assert_eq!(
        onboarding_login_command(
            &codex(json!({})),
            &with("codex", "codex", "/opt/Codex Tools/codex"),
            Os::Unknown
        ),
        "codex login",
        "an old server's unknown shell gets the default command"
    );
    assert_eq!(
        onboarding_install_command("codex", Os::Windows),
        Some("irm https://chatgpt.com/codex/install.ps1 | iex")
    );
    assert_eq!(
        onboarding_install_command("claudeAgent", Os::Darwin),
        Some("curl -fsSL https://claude.ai/install.sh | bash")
    );
    assert_eq!(onboarding_install_command("cursor", Os::Linux), None);
}

#[test]
fn agents_on_the_computer_are_listed_by_state_then_name_and_the_rest_by_name() {
    let providers = [
        provider(
            "Zeta",
            json!({"auth": {"status": "unauthenticated"}, "status": "error"}),
        ),
        provider("Codex", json!({})),
        provider("Pi", json!({"enabled": false, "status": "disabled"})),
        provider(
            "Grok",
            json!({"installed": false, "status": "error", "auth": {"status": "unknown"}}),
        ),
        provider("Claude", json!({})),
        provider(
            "Cursor",
            json!({"installed": false, "status": "warning", "auth": {"status": "unknown"}}),
        ),
    ];
    let groups = group(Some(&providers), None);
    assert_eq!(names(&groups.found), ["Claude", "Codex", "Zeta"]);
    assert_eq!(
        names(&groups.available),
        ["Cursor", "Grok", "Pi"],
        "Cursor is still being checked; it stays listed"
    );
    assert!(groups.checking);
    assert_eq!(groups.ready_count, 2);
}

#[test]
fn this_computers_settings_apply_and_agents_that_failed_to_load_stay_with_the_found_ones() {
    let computer = settings(json!({
        "providerInstances": {"pi": {"driver": "pi"}, "registry": {"driver": "acpRegistry"}},
        "projectSettingsOverrides": {"project-a": {"providerInstances": {"pi": {"enabled": true}}}}
    }));
    let providers = [
        provider("Pi", json!({})),
        provider(
            "Broken",
            json!({"enabled": false, "installed": false, "status": "error",
            "availability": "unavailable", "unavailableReason": "Invalid config for instance 'broken'."}),
        ),
        provider(
            "Codex",
            json!({"compatibilityAdvisory": {"status": "broken",
            "message": "Update Codex to 0.170 or later.", "recommendedVersion": "0.170.0", "recommendedRange": null}}),
        ),
        provider(
            "Registry",
            json!({"driver": "acpRegistry", "installed": false, "status": "warning",
            "auth": {"status": "unknown"}}),
        ),
    ];
    let groups = group(Some(&providers), Some(&computer));
    assert_eq!(names(&groups.found), ["Broken", "Codex", "Registry"]);
    assert_eq!(names(&groups.available), ["Pi"]);
    assert_eq!(groups.ready_count, 0);
    assert!(!groups.checking);
}

#[test]
fn agents_the_policy_does_not_allow_are_left_out_and_no_list_is_still_checking() {
    let providers = [provider("Codex", json!({})), provider("Blocked", json!({}))];
    let groups = group_welcome_agents(
        Some(&providers),
        None,
        name_of,
        |p| p.driver.0 != "blocked",
        str::cmp,
    );
    assert_eq!(names(&groups.found), ["Codex"]);
    assert_eq!(groups.ready_count, 1);
    let none = group(None, None);
    assert!(
        none.checking
            && none.found.is_empty()
            && none.available.is_empty()
            && none.ready_count == 0
    );
}

#[test]
fn the_cli_step_waits_for_its_check_then_opens_the_terminal_unless_codex_is_ready() {
    use CliTerminalStep::*;
    let step = |overrides| cli_terminal_step(&provider("Codex", overrides), None);
    assert_eq!(
        step(
            json!({"setup": {"canAuthenticate": true, "canInstall": true}, "auth": {"status": "unauthenticated"}})
        ),
        Wait
    );
    assert_eq!(
        step(json!({"installed": false, "status": "warning", "auth": {"status": "unknown"}})),
        Wait
    );
    assert_eq!(
        step(json!({"installed": false, "status": "error", "auth": {"status": "unknown"}})),
        Open
    );
    assert_eq!(step(json!({"auth": {"status": "unauthenticated"}})), Open);
    assert_eq!(
        step(json!({"status": "error", "auth": {"status": "unknown"}})),
        Open
    );
    let broken = json!({"status": "broken", "message": "Update Codex.", "recommendedVersion": null, "recommendedRange": null});
    assert_eq!(step(json!({"compatibilityAdvisory": broken})), Done);
    assert_eq!(
        step(json!({"compatibilityAdvisory": broken, "auth": {"status": "unauthenticated"}})),
        Done
    );
    assert_eq!(
        step(
            json!({"compatibilityAdvisory": broken, "status": "error", "auth": {"status": "unknown"}})
        ),
        Done
    );
    assert_eq!(step(json!({"status": "warning"})), Done);
    assert_eq!(step(json!({"installed": false, "status": "error"})), Open);
    assert_eq!(step(json!({})), Done);
}

#[test]
fn turning_an_agent_on_also_clears_an_older_off_inside_its_config() {
    let plain: ProviderInstanceConfig =
        serde_json::from_value(json!({"driver": "pi"})).expect("config");
    assert_eq!(turned_on_instance(&plain).enabled, Some(true));
    let migrated: ProviderInstanceConfig = serde_json::from_value(
        json!({"driver": "pi", "config": {"enabled": false, "binaryPath": "pi"}}),
    )
    .expect("config");
    let on = turned_on_instance(&migrated);
    assert_eq!(on.enabled, Some(true));
    assert_eq!(
        on.config,
        Some(json!({"enabled": true, "binaryPath": "pi"}))
    );
}

#[test]
fn the_codex_setup_card_opens_only_while_codex_is_not_signed_in() {
    let setup = |overrides| codex_needs_setup(&provider("Codex", overrides), None);
    assert!(setup(
        json!({"installed": false, "status": "error", "auth": {"status": "unknown"}})
    ));
    assert!(setup(json!({"auth": {"status": "unauthenticated"}})));
    assert!(setup(
        json!({"status": "error", "auth": {"status": "unknown"}})
    ));
    assert!(!setup(json!({"installed": false, "status": "error"})));
    assert!(!setup(
        json!({"auth": {"status": "unauthenticated"}, "compatibilityAdvisory":
        {"status": "broken", "message": null, "recommendedVersion": null, "recommendedRange": null}})
    ));
    assert!(!setup(json!({})));
    assert!(!codex_needs_setup(
        &provider("Claude", json!({"auth": {"status": "unauthenticated"}})),
        None
    ));
}

#[test]
fn the_card_keeps_codexs_last_status_while_a_rebuild_drops_it_and_closes_once_signed_in() {
    let signed_out = provider("Codex", json!({"auth": {"status": "unauthenticated"}}));
    let other = provider("Other", json!({}));
    let ready = [provider("Codex", json!({}))];
    let listed = [signed_out.clone()];
    let card = |providers: &[ServerProvider], last: Option<&ServerProvider>| {
        codex_card_provider(Some(providers), Some("codex"), last, None).map(name_of)
    };
    assert_eq!(card(&listed, None).as_deref(), Some("Codex"));
    assert_eq!(card(&[], Some(&signed_out)).as_deref(), Some("Codex"));
    assert_eq!(card(&[], Some(&other)), None);
    assert_eq!(card(&ready, None), None);
    assert_eq!(
        codex_card_provider(Some(&listed), None, None, None).map(name_of),
        None
    );
}

#[test]
fn the_phone_is_offered_only_through_t3_connect_on_the_primary_computer_when_allowed() {
    let policy = |limits: Value| -> ManagedPolicySummary {
        serde_json::from_value(
            json!({"organization": "Acme", "lockedSettings": [], "limits": limits}),
        )
        .expect("policy")
    };
    assert!(welcome_offers_phone(true, true, true, None));
    assert!(
        !welcome_offers_phone(true, false, true, None),
        "no pairing-code fallback without T3 Connect"
    );
    assert!(!welcome_offers_phone(true, true, false, None));
    assert!(!welcome_offers_phone(
        true,
        true,
        true,
        Some(&policy(json!({"connect": false})))
    ));
    assert!(!welcome_offers_phone(
        true,
        true,
        true,
        Some(&policy(json!({"clients": ["desktop"]})))
    ));
    assert!(!welcome_offers_phone(false, true, true, None));
    assert_eq!(
        managed_capability_denial(
            Some(&policy(json!({"connect": false}))),
            ManagedCapability::Connect
        )
        .as_deref(),
        Some("Acme's policy does not allow T3 Connect.")
    );
}
