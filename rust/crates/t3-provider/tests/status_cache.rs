#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_contracts::server::ServerProvider;
use t3_provider::{catalog, status_cache::*};
fn provider(id: &str, driver: &str) -> ServerProvider {
    serde_json::from_value(json!({"instanceId":id,"driver":driver,"enabled":true,"installed":true,"version":"1.0.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-04-10T00:00:00.000Z","models":[],"skills":[],"slashCommands":[]})).unwrap()
}
#[tokio::test]
async fn persisted_status_excludes_progress_and_workspaces_and_hydration_cannot_resurrect_old_configuration()
 {
    let root = tempfile::tempdir().unwrap();
    let cache = ProviderStatusCache::new(root.path().into());
    let mut cached = provider("work", "codex");
    cached.models = vec![
        catalog::model("old-custom", "Old custom", None),
        catalog::model("cached", "Cached", None),
        catalog::model("same", "Stale name", None),
    ];
    cached.models[0].is_custom = true;
    cached.update_state =
        Some(serde_json::from_value(json!({"status":"running","message":"Updating","startedAt":null,"finishedAt":null,"output":null})).unwrap());
    cached.workspace_snapshots = Some(vec![]);
    cached.message = Some(Some("Cached status".into()));
    cache.write(&cached).await.unwrap();
    let bytes = std::fs::read(path(root.path(), &"work".into()).unwrap()).unwrap();
    let persisted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(persisted.get("updateState").is_none());
    assert!(persisted.get("workspaceSnapshots").is_none());
    let restored = cache.read(&"work".into()).await.unwrap();
    let mut fallback = provider("work", "codex");
    fallback.installed = false;
    fallback.version = None;
    fallback.models = vec![
        catalog::model("same", "Current name", None),
        catalog::model("new-custom", "New custom", None),
    ];
    fallback.models[1].is_custom = true;
    fallback.message = Some(Some("Checking".into()));
    let hydrated = hydrate(&restored, fallback.clone());
    assert!(hydrated.installed);
    assert_eq!(
        hydrated
            .models
            .iter()
            .map(|model| model.slug.0.as_str())
            .collect::<Vec<_>>(),
        ["same", "new-custom", "cached"]
    );
    assert_eq!(hydrated.models[0].name.0, "Current name");
    assert_eq!(hydrated.message, restored.message);
    for (id, driver, enabled) in [
        ("other", "codex", true),
        ("work", "claudeAgent", true),
        ("work", "codex", false),
    ] {
        let mut changed = fallback.clone();
        changed.instance_id = id.into();
        changed.driver = driver.into();
        changed.enabled = enabled;
        assert_eq!(hydrate(&restored, changed.clone()), changed);
    }
    std::fs::write(path(root.path(), &"work".into()).unwrap(), b"{unfinished").unwrap();
    assert!(cache.read(&"work".into()).await.is_none());
    assert!(cache.read(&"missing".into()).await.is_none());
    for id in [
        "../escape",
        "..\\escape",
        "C:never-format-secret",
        ".",
        "trailing.",
    ] {
        let error = path(root.path(), &id.into()).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("never-format-secret"));
    }
}
#[test]
fn snapshot_order_uses_driver_order_then_names_and_null_names_sort_like_absent_names() {
    let mut z = provider("z", "codex");
    z.display_name = Some(None);
    let a = provider("a", "codex");
    let mut named = provider("b", "codex");
    named.display_name = Some(Some("Work".into()));
    let mut providers = vec![
        provider("other", "custom"),
        z,
        provider("claude", "claudeAgent"),
        named,
        a,
    ];
    order(&mut providers);
    assert_eq!(
        providers
            .iter()
            .map(|provider| provider.instance_id.0.as_str())
            .collect::<Vec<_>>(),
        ["a", "z", "b", "claude", "other"]
    );
}
