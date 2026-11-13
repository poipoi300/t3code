#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{fs, path::Path};
use t3_contracts::provider_usage_limits::{ServerProviderUsageLimits, ServerProviderUsageWindow};
use t3_usage::{
    Error, UsageProviderKind as P,
    limits::{self, UsageLimitProbe},
    sources::{ProbeFuture, ProviderInstance, SourceKind, resolve_sources},
};
const AT: &str = "2026-08-01T10:00:00.000Z";
#[test]
fn malformed_previous_codex_limits_cannot_panic_on_an_update() {
    let update = json!({"primary":{"usedPercent":50}});
    for previous in [json!("invalid"), json!([]), json!(false)] {
        assert_eq!(
            limits::merge_codex_limits(Some(previous), &update),
            Some(update.clone())
        );
    }
}

#[test]
fn limit_window_sort_uses_the_same_collation_as_buckets() {
    let windows =
        ["z", "é", "A", "a"].map(|id| json!({"id":id,"kind":"weekly","label":id,"usedPercent":0}));
    let result = json!(limits::make_limits(AT, windows.to_vec()).unwrap());
    assert_eq!(
        result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "A", "é", "z"]
    );
}
#[test]
fn provider_directory_rules_use_explicit_fixture_homes_and_environment_only() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    for (provider, default, suffix) in [
        (P::Claude, ".claude", "projects"),
        (P::Codex, ".codex", "sessions"),
        (P::Grok, ".grok", "sessions"),
        (P::Pi, ".pi/agent", "sessions"),
    ] {
        let instance = ProviderInstance::new(provider);
        let sources = resolve_sources(home, &instance, None);
        assert_eq!(sources[0].directory, home.join(default).join(suffix));
    }
    let mut claude = ProviderInstance::new(P::Claude);
    claude.environment.insert(
        "CLAUDE_CONFIG_DIR".into(),
        home.join("claude-env").display().to_string(),
    );
    assert_eq!(
        resolve_sources(home, &claude, None)[0].directory,
        home.join("claude-env/projects")
    );
    claude.home_path = Some(Path::new("~/custom").into());
    assert_eq!(
        resolve_sources(home, &claude, None)[0].directory,
        home.join("custom/projects")
    );
    let mut codex = ProviderInstance::new(P::Codex);
    codex.environment.insert(
        "CODEX_HOME".into(),
        home.join("codex-env").display().to_string(),
    );
    assert_eq!(
        resolve_sources(home, &codex, None)[0].directory,
        home.join("codex-env/sessions")
    );
    codex.managed = true;
    assert_eq!(
        resolve_sources(home, &codex, None)[0].directory,
        home.join(".codex/sessions")
    );
    let mut pi = ProviderInstance::new(P::Pi);
    pi.environment.insert(
        "PI_CODING_AGENT_DIR".into(),
        home.join("agent-dir").display().to_string(),
    );
    pi.pi_session_dir = Some(home.join("launched"));
    let sources = resolve_sources(home, &pi, None);
    assert_eq!(sources[0].directory, home.join("launched"));
    assert_eq!(sources[1].directory, home.join("agent-dir/sessions"));
    pi.environment.insert(
        "PI_CODING_AGENT_SESSION_DIR".into(),
        home.join("direct-sessions").display().to_string(),
    );
    assert_eq!(
        resolve_sources(home, &pi, None)[1].directory,
        home.join("direct-sessions")
    );
    let grok = resolve_sources(home, &ProviderInstance::new(P::Grok), None);
    assert_eq!(
        grok[0].kind,
        SourceKind::Transcripts {
            file_name: Some("updates.jsonl".into())
        }
    );
    let pending = resolve_sources(home, &ProviderInstance::new(P::Muse), None);
    assert!(pending[0].refreshing);
    let resolved = resolve_sources(home, &ProviderInstance::new(P::Muse), Some(home));
    assert_eq!(
        resolved[0].directory,
        dunce::canonicalize(home).unwrap().join("sessions")
    );
    assert_eq!(
        resolved[0].kind,
        SourceKind::Transcripts {
            file_name: Some("session.jsonl".into())
        }
    );
}
#[test]
fn sqlite_root_lists_and_antigravity_nested_conversation_directories() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let mut oc = ProviderInstance::new(P::Opencode);
    oc.environment.insert(
        "OPENCODE_DATA_DIR".into(),
        format!(
            "{}, {}",
            home.join("one").display(),
            home.join("two").display()
        ),
    );
    let dirs = resolve_sources(home, &oc, None);
    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs[1].directory, home.join("two"));
    let mut ag = ProviderInstance::new(P::Antigravity);
    ag.environment.insert(
        "ANTIGRAVITY_DATA_DIR".into(),
        home.join("ag").display().to_string(),
    );
    fs::create_dir_all(home.join("ag/conversations")).unwrap();
    let source = resolve_sources(home, &ag, None);
    assert_eq!(source[0].directory.file_name().unwrap(), "conversations");
}
#[test]
fn limit_updates_preserve_reset_fields_and_healthy_probe_state() {
    let previous=limits::make_limits(AT,vec![json!({"id":"primary","kind":"session","label":"Session","usedPercent":10,"resetsAt":"2026-08-01T12:00:00.000Z","windowDurationMins":300})]).unwrap();
    let update: ServerProviderUsageWindow = serde_json::from_value(
        json!({"id":"primary","kind":"session","label":"Session","usedPercent":150}),
    )
    .unwrap();
    let merged = limits::apply_limits_update(Some(previous.clone()), vec![update], AT)
        .unwrap()
        .unwrap();
    let v = json!(merged);
    assert_eq!(v["windows"][0]["usedPercent"], 100.0);
    assert_eq!(v["windows"][0]["windowDurationMins"], 300);
    assert_eq!(v["windows"][0]["resetsAt"], "2026-08-01T12:00:00.000Z");
    let failed = limits::unavailable(AT, "probeFailed", None).unwrap();
    assert_eq!(
        limits::resolve_after_probe(Some(previous.clone()), Some(failed)).unwrap(),
        Some(previous)
    );
}
#[test]
fn codex_continuation_waits_for_all_exhausted_windows() {
    let mut snapshot = json!({"primary":{"usedPercent":100,"resetsAt":1785580000},"secondary":{"usedPercent":100,"resetsAt":1785590000}});
    assert_eq!(
        limits::codex_limit_reset_at(&snapshot),
        Some("2026-08-01T13:13:20.000Z".into())
    );
    snapshot["secondary"]
        .as_object_mut()
        .unwrap()
        .remove("resetsAt");
    assert_eq!(limits::codex_limit_reset_at(&snapshot), None);
}
struct FakeProbe {
    fail: bool,
}
impl UsageLimitProbe for FakeProbe {
    fn read<'a>(
        &'a self,
        provider: &'a P,
        at: &'a str,
    ) -> ProbeFuture<'a, ServerProviderUsageLimits> {
        Box::pin(async move {
            if self.fail {
                return Err(Error::Probe("fixture error".into()));
            }
            match provider {
                P::Codex => limits::codex_limits(
                    &json!({"primary":{"usedPercent":42,"resetsAt":1785580000}}),
                    None,
                    None,
                    at,
                ),
                _ => limits::unavailable(at, "unsupported", None),
            }
        })
    }
}
#[tokio::test]
async fn network_cli_probe_seam_is_tested_with_a_fake() {
    let success = limits::read_provider_limits(&FakeProbe { fail: false }, &P::Codex, AT)
        .await
        .unwrap();
    assert_eq!(json!(success)["windows"][0]["usedPercent"], 42.0);
    let failed = limits::read_provider_limits(&FakeProbe { fail: true }, &P::Codex, AT)
        .await
        .unwrap();
    assert_eq!(json!(failed)["unavailable"]["reason"], "probeFailed");
    assert!(!json!(failed).to_string().contains("fixture error"));
    let unsupported = limits::read_provider_limits(&FakeProbe { fail: false }, &P::Pi, AT)
        .await
        .unwrap();
    assert_eq!(json!(unsupported)["unavailable"]["reason"], "unsupported");
}
struct PendingProbe;
impl UsageLimitProbe for PendingProbe {
    fn read<'a>(&'a self, _: &'a P, _: &'a str) -> ProbeFuture<'a, ServerProviderUsageLimits> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn stalled_provider_limits_are_reported_as_unavailable() {
    let limits = json!(
        limits::read_provider_limits(&PendingProbe, &P::Codex, AT)
            .await
            .unwrap()
    );
    assert_eq!(limits["unavailable"]["reason"], "probeFailed");
}
#[test]
fn muse_opencode_and_antigravity_report_only_available_windows() {
    let muse=limits::muse_limits(&json!({"observedAtMs":1785578400000u64,"window":{"usedPercent":30,"resetsAtMs":1785588400000u64,"windowDurationMins":300},"weekly":{"usedPercent":50,"resetsAtMs":1786183200000u64}})).unwrap();
    assert_eq!(muse.windows.len(), 2);
    assert_eq!(
        limits::muse_limit_reset_at(
            None,
            1785578400000.0,
            Some(&json!({"message":"İ quota RESETS AT 2026-08-08T10:00:00Z."}))
        ),
        Some("2026-08-08T10:00:00.000Z".into())
    );
    let go=limits::opencode_limits(&json!({"usage":{"rolling":{"percent":10,"resetsAt":AT},"weekly":{"percent":20,"resetsAt":AT},"monthly":{"percent":30,"resetsAt":AT}}}),AT).unwrap();
    assert_eq!(go.windows.len(), 3);
    let ag=limits::antigravity_limits(&json!({"buckets":[{"bucketId":"gemini-5h","window":"5h","remainingFraction":1.0,"resetTime":"2026-08-01T15:00:00Z"},{"bucketId":"3p-weekly","window":"weekly","remainingFraction":0.1,"resetTime":"2026-08-08T10:00:00Z"}]}),AT,1785578400000.0).unwrap();
    let v: Value = json!(ag);
    assert!(v["windows"][0].get("resetsAt").is_none());
    assert_eq!(v["windows"][1]["usedPercent"], 90.0);
}
