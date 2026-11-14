use super::command_policy_test::{change, projection};
use serde_json::json;
use t3_contracts::provider_policy::RuntimeMode::*;
use t3_orchestration::ports::decisions::{
    dispatch_mode_limit::RuntimeModeCeiling, runtime_policy::*,
};

#[test]
fn unsupported_provider_modes_fall_back_to_supervised_and_unknown_lists_preserve_mode() {
    let grok = [ApprovalRequired, Auto, FullAccess];
    let antigravity = [ApprovalRequired, AutoAcceptEdits, FullAccess];
    assert_eq!(
        provider_runtime_mode(Auto, Some(&antigravity)),
        ApprovalRequired
    );
    assert_eq!(
        provider_runtime_mode(AutoAcceptEdits, Some(&antigravity)),
        AutoAcceptEdits
    );
    assert_eq!(
        provider_runtime_mode(FullAccess, Some(&antigravity)),
        FullAccess
    );
    assert_eq!(
        provider_runtime_mode(AutoAcceptEdits, Some(&grok)),
        ApprovalRequired
    );
    assert_eq!(provider_runtime_mode(Auto, Some(&grok)), Auto);
    assert_eq!(provider_runtime_mode(FullAccess, Some(&grok)), FullAccess);
    assert_eq!(
        provider_runtime_mode(AutoAcceptEdits, None),
        AutoAcceptEdits
    );
    assert_eq!(
        provider_runtime_mode(AutoAcceptEdits, Some(&[])),
        AutoAcceptEdits
    );
}

#[test]
fn runtime_policy_uses_worktree_then_project_root_and_applies_overrides_last() {
    let thread = projection().thread;
    let policy = resolve(
        &thread,
        Some("/project-root"),
        None,
        None,
        &RuntimePolicyOverride::default(),
    )
    .unwrap();
    assert_eq!(policy.cwd.unwrap().to_str(), Some("/project-root"));
    let thread = change(&thread, json!({"worktreePath":"/worktree"}));
    assert_eq!(
        resolve(
            &thread,
            Some("/project-root"),
            None,
            None,
            &RuntimePolicyOverride::default()
        )
        .unwrap()
        .cwd
        .unwrap()
        .to_str(),
        Some("/worktree")
    );
    let overrides = RuntimePolicyOverride {
        cwd: Some("/override".into()),
        approval_policy: Some(json!({"approval":false})),
        sandbox_policy: Some(json!("sandbox")),
    };
    let policy = resolve(&thread, None, None, None, &overrides).unwrap();
    assert_eq!(policy.cwd.unwrap().to_str(), Some("/override"));
    assert_eq!(policy.approval_policy, overrides.approval_policy);
    assert_eq!(policy.sandbox_policy, overrides.sandbox_policy);
    assert!(resolve(&projection().thread, None, None, None, &overrides).is_err());
}

#[test]
fn managed_ceiling_refuses_stale_high_modes_before_provider_fallback() {
    let ceiling = RuntimeModeCeiling {
        organization: "Example Corp".into(),
        max_runtime_mode: AutoAcceptEdits,
    };
    let thread = change(
        &projection().thread,
        json!({"runtimeMode":"full-access","worktreePath":"/work"}),
    );
    let error = resolve(
        &thread,
        None,
        Some(&[ApprovalRequired]),
        Some(&ceiling),
        &RuntimePolicyOverride::default(),
    )
    .unwrap_err();
    assert!(error.to_string().ends_with(
        "Example Corp limits threads to Auto-accept edits. Switch this thread's mode to continue."
    ));
    let thread = change(&thread, json!({"runtimeMode":"auto-accept-edits"}));
    let policy = resolve(
        &thread,
        None,
        None,
        Some(&ceiling),
        &RuntimePolicyOverride::default(),
    )
    .unwrap();
    assert_eq!(policy.runtime_mode, AutoAcceptEdits);
    assert_eq!(policy.max_runtime_mode, Some(AutoAcceptEdits));
}

#[tokio::test]
async fn project_policy_and_wrappers_keep_ceiling_precedence_and_reasoning_effort() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use t3_orchestration::{
        api::RuntimePolicyResolver,
        stores::projects::{ProjectRow, upsert},
        testing::{TEST_TIME, TestDatabase},
    };
    struct Modes(AtomicUsize);
    #[async_trait::async_trait]
    impl ProviderRuntimeModeSource for Modes {
        async fn supported_modes(
            &self,
            _: t3_contracts::provider_instance::ProviderInstanceId,
        ) -> t3_orchestration::error::Result<Option<Vec<t3_contracts::provider_policy::RuntimeMode>>>
        {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Some(vec![ApprovalRequired]))
        }
    }
    let database = TestDatabase::memory().unwrap();
    let modes = Arc::new(Modes(AtomicUsize::new(0)));
    let project:ProjectRow=serde_json::from_value(json!({"projectId":"project","title":"Project","hasCustomTitle":false,"workspaceRoot":"C:/workspace","defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null})).unwrap();
    database
        .stores
        .transaction(move |tx| upsert(tx, &project))
        .await
        .unwrap();
    let base = Arc::new(ProjectRuntimePolicy {
        database: database.stores.clone(),
        providers: modes.clone(),
        ceiling: Some(RuntimeModeCeiling {
            organization: "Example".into(),
            max_runtime_mode: Auto,
        }),
    });
    let mut thread = projection().thread;
    let selection = thread.model_selection.clone();
    thread = change(&thread, json!({"runtimeMode":"full-access"}));
    let error = base
        .resolve(thread.clone(), selection.clone())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        t3_orchestration::error::Error::Service {
            operation: "runtime-policy.above-ceiling",
            ..
        }
    ));
    assert_eq!(modes.0.load(Ordering::Relaxed), 0);
    thread = change(&thread, json!({"runtimeMode":"auto"}));
    let override_policy = OverriddenRuntimePolicy {
        base,
        overrides: RuntimePolicyOverride {
            cwd: Some("C:/override".into()),
            ..Default::default()
        },
        reasoning_effort: Some("high".into()),
    };
    let configured = override_policy.resolve(thread, selection).await.unwrap();
    assert_eq!(configured.policy.runtime_mode, ApprovalRequired);
    assert_eq!(
        configured.policy.cwd.unwrap().to_string_lossy(),
        "C:/override"
    );
    assert_eq!(configured.options.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(modes.0.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn configured_policy_reads_live_instance_modes_and_project_paths_with_managed_limits() {
    use std::sync::Arc;
    use t3_contracts::{managed_policy::ManagedPolicyDocument, server::ServerProvider};
    use t3_orchestration::{
        stores::projects::{ProjectRow, upsert},
        testing::{TEST_TIME, TestDatabase},
    };
    let database = TestDatabase::memory().unwrap();
    let project: ProjectRow = serde_json::from_value(json!({"projectId":"project","title":"Project","hasCustomTitle":false,"workspaceRoot":"C:/project-root","defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null})).unwrap();
    database
        .stores
        .transaction(move |tx| upsert(tx, &project))
        .await
        .unwrap();
    let provider: ServerProvider = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerProvider.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let mut provider = change(
        &provider,
        json!({"instanceId":"codex","supportedRuntimeModes":["approval-required"]}),
    );
    let (sender, receiver) = tokio::sync::watch::channel(Arc::new(vec![provider.clone()]));
    let modes = Arc::new(SnapshotRuntimeModes::new(receiver));
    let managed: ManagedPolicyDocument = serde_json::from_value(
        json!({"schemaVersion":1,"organization":"Example","limits":{"maxRuntimeMode":"auto"}}),
    )
    .unwrap();
    let resolver = ProjectRuntimePolicy::configured(
        database.stores.clone(),
        modes.clone(),
        Some(&managed),
        RuntimePolicyOverride {
            approval_policy: Some(json!("on-request")),
            ..Default::default()
        },
        Some("high".into()),
    );
    let mut thread = change(
        &projection().thread,
        json!({"runtimeMode":"auto","worktreePath":null}),
    );
    let selection = thread.model_selection.clone();
    let configured = resolver
        .resolve(thread.clone(), selection.clone())
        .await
        .unwrap();
    assert_eq!(configured.policy.runtime_mode, ApprovalRequired);
    assert_eq!(
        configured.policy.cwd.unwrap().to_string_lossy(),
        "C:/project-root"
    );
    assert_eq!(configured.policy.max_runtime_mode, Some(Auto));
    assert_eq!(configured.policy.approval_policy, Some(json!("on-request")));
    assert_eq!(configured.options.reasoning_effort.as_deref(), Some("high"));
    provider = change(&provider, json!({"supportedRuntimeModes":["auto"]}));
    sender.send(Arc::new(vec![provider.clone()])).unwrap();
    thread = change(&thread, json!({"worktreePath":"C:/worktree"}));
    let configured = resolver
        .resolve(thread.clone(), selection.clone())
        .await
        .unwrap();
    assert_eq!(configured.policy.runtime_mode, Auto);
    assert_eq!(
        configured.policy.cwd.unwrap().to_string_lossy(),
        "C:/worktree"
    );
    provider = change(&provider, json!({"supportedRuntimeModes":null}));
    sender.send(Arc::new(vec![provider])).unwrap();
    assert_eq!(
        resolver
            .resolve(thread.clone(), selection.clone())
            .await
            .unwrap()
            .policy
            .runtime_mode,
        Auto
    );
    assert_eq!(
        modes
            .supported_modes(t3_contracts::provider_instance::ProviderInstanceId(
                "missing".into()
            ))
            .await
            .unwrap(),
        None
    );
    thread = change(&thread, json!({"runtimeMode":"full-access"}));
    assert!(
        resolver
            .resolve(thread, selection)
            .await
            .unwrap_err()
            .to_string()
            .contains("Example limits threads to Auto")
    );
}
