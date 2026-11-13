#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};
use t3_provider::{account_usage::*, modes, probe_models};
const CHECKED: &str = "2026-11-13T12:00:00.000Z";
#[test]
fn codex_allowances_follow_plan_duration_scope_and_sparse_notification_rules() {
    let snapshot = json!({"planType":"free","primary":{"usedPercent":120,"resetsAt":1800000000},"secondary":{"usedPercent":25,"windowDurationMins":10080},"individualLimit":{"remainingPercent":40,"resetsAt":1800000050}});
    let limits = serde_json::to_value(codex_limits(&snapshot, CHECKED).unwrap()).unwrap();
    assert_eq!(limits["windows"][0]["kind"], "weekly");
    assert_eq!(limits["windows"][1]["kind"], "monthly");
    assert_eq!(limits["windows"][1]["id"], "individual");
    assert_eq!(limits["windows"][2]["usedPercent"].as_f64(), Some(100.));
    assert_eq!(codex_update(&snapshot, Some(&snapshot)).unwrap(), None);
    let scoped = json!({"limitId":"model-specific","primary":{"usedPercent":90}});
    assert_eq!(
        merge_codex(Some(&snapshot), &scoped),
        Some(snapshot.clone())
    );
    assert!(codex_limits(&scoped, CHECKED).unwrap().windows.is_empty());
    let merged = merge_codex(Some(&snapshot), &json!({"primary":null})).unwrap();
    assert!(merged["primary"].is_null());
    assert_eq!(merged["secondary"], snapshot["secondary"]);
    assert_eq!(
        codex_reset_at(&snapshot).as_deref(),
        Some("2027-01-15T08:00:00.000Z")
    );
    assert!(codex_reset_at(&json!({"primary":{"usedPercent":100}})).is_none());
}
#[test]
fn claude_captured_spend_budget_uses_minor_units_and_falls_back_only_to_enabled_valid_budgets() {
    let response = json!({"rate_limits_available":true,"rate_limits":{"spend":{"enabled":true,"used":{"amount_minor":4631,"currency":"USD","exponent":2},"limit":{"amount_minor":50000,"currency":"USD","exponent":2}},"extra_usage":{"is_enabled":true,"used_credits":30000,"monthly_limit":50000,"utilization":60},"five_hour":{"utilization":12,"resets_at":null},"model_scoped":[{"display_name":"Opus 5.5","utilization":33,"resets_at":"2026-11-14T00:00:00Z"}]}});
    let (limits, names) = claude_limits(&response, CHECKED).unwrap();
    let limits = serde_json::to_value(limits).unwrap();
    let windows = limits["windows"].as_array().unwrap();
    let spend = windows.iter().find(|w| w["id"] == "monthly_spend").unwrap()["usedPercent"]
        .as_f64()
        .unwrap();
    assert!((spend - 9.262).abs() < 1e-10);
    assert_eq!(names.overage_included.as_deref(), Some("Opus 5.5"));
    let event = json!({"rateLimitType":"seven_day_overage_included","utilization":0.5,"resetsAt":1800000000});
    assert!(
        claude_update(&event, &ClaudeScopedLimitNames::default())
            .unwrap()
            .is_none()
    );
    let update = serde_json::to_value(claude_update(&event, &names).unwrap().unwrap()).unwrap();
    assert_eq!(update["windows"][0]["id"], "seven_day_opus_5_5");
    assert_eq!(update["windows"][0]["usedPercent"].as_f64(), Some(50.));
    for bad in [
        json!({"currency":"EUR","exponent":2,"amount_minor":50000}),
        json!({"currency":"USD","exponent":3,"amount_minor":50000}),
        json!({"currency":"USD","exponent":2,"amount_minor":0}),
    ] {
        let mut input = response.clone();
        input["rate_limits"]["spend"]["limit"] = bad;
        let (limits, _) = claude_limits(&input, CHECKED).unwrap();
        let value = serde_json::to_value(limits).unwrap();
        assert_eq!(
            value["windows"].as_array().unwrap().last().unwrap()["usedPercent"].as_f64(),
            Some(60.)
        );
    }
    assert_eq!(
        serde_json::to_value(
            claude_limits(&json!({"rate_limits_available":false}), CHECKED)
                .unwrap()
                .0
        )
        .unwrap()["unavailable"]["reason"],
        "unsupported"
    );
    assert_eq!(
        serde_json::to_value(
            claude_limits(&json!({"rate_limits_available":true}), CHECKED)
                .unwrap()
                .0
        )
        .unwrap()["unavailable"]["reason"],
        "probeFailed"
    );
}
#[test]
fn codex_and_claude_modes_preserve_sandbox_and_explicit_approval_precedence() {
    for (mode, approval, reviewer, sandbox) in [
        (
            RuntimeMode::ApprovalRequired,
            "untrusted",
            "user",
            "readOnly",
        ),
        (
            RuntimeMode::AutoAcceptEdits,
            "on-request",
            "user",
            "workspaceWrite",
        ),
        (
            RuntimeMode::Auto,
            "on-request",
            "auto_review",
            "workspaceWrite",
        ),
        (RuntimeMode::FullAccess, "never", "user", "dangerFullAccess"),
    ] {
        let result = modes::codex(mode, None, None);
        assert_eq!(result["approvalPolicy"], approval);
        assert_eq!(result["approvalsReviewer"], reviewer);
        assert_eq!(result["sandboxPolicy"]["type"], sandbox);
    }
    let read_only = modes::claude(
        RuntimeMode::FullAccess,
        ProviderInteractionMode::Default,
        None,
        Some("readOnly"),
        false,
        false,
    );
    assert_eq!(read_only.permission_mode, "dontAsk");
    assert!(read_only.read_only);
    assert_eq!(
        modes::claude(
            RuntimeMode::FullAccess,
            ProviderInteractionMode::Default,
            None,
            None,
            true,
            true
        )
        .permission_mode,
        "default"
    );
    assert_eq!(
        modes::claude(
            RuntimeMode::FullAccess,
            ProviderInteractionMode::Default,
            Some("on-request"),
            Some("dangerFullAccess"),
            false,
            false
        )
        .permission_mode,
        "default"
    );
    let plan = modes::claude(
        RuntimeMode::FullAccess,
        ProviderInteractionMode::Plan,
        Some("never"),
        None,
        false,
        false,
    );
    assert_eq!(plan.permission_mode, "plan");
    assert!(plan.install_permission_callback);
}
#[test]
fn account_metadata_keeps_custom_endpoints_separate_from_native_quota_and_auth() {
    let account = json!({"account":{"type":"chatgpt","email":"ada@example.com","planType":"plus"},"requiresOpenaiAuth":true});
    assert!(probe_models::codex_native_quota(&account, &json!({})));
    assert!(!probe_models::codex_native_quota(
        &account,
        &json!({"openai_base_url":"https://proxy.example/v1"})
    ));
    assert!(!probe_models::codex_native_quota(&account, &Value::Null));
    assert_eq!(
        probe_models::codex_plan("promax"),
        Some("ChatGPT Pro 500 Subscription")
    );
    let (auth, status) =
        probe_models::claude_auth(&json!({"tokenSource":"none","apiKeySource":"none"}), false)
            .unwrap();
    assert_eq!(status, "error");
    assert_eq!(
        serde_json::to_value(auth).unwrap()["status"],
        "unauthenticated"
    );
    let (auth, status) = probe_models::claude_auth(&json!({"tokenSource":"none"}), true).unwrap();
    assert_eq!(status, "ready");
    assert_eq!(
        serde_json::to_value(auth).unwrap()["type"],
        "customEndpoint"
    );
    let (auth, _) = probe_models::claude_auth(&json!({"apiProvider":"bedrock"}), false).unwrap();
    assert_eq!(
        serde_json::to_value(auth).unwrap()["label"],
        "Amazon Bedrock"
    );
}
