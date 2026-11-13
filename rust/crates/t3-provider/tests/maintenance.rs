#![allow(clippy::unwrap_used)]
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
    time::Duration,
};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_provider::{
    environment::{Environment, Platform},
    maintenance::*,
};

async fn is_pending<F: Future>(mut future: Pin<&mut F>) -> bool {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await
}

#[tokio::test(start_paused = true)]
async fn concurrent_version_misses_share_one_lookup_and_positive_results_expire_after_an_hour() {
    let cache = LatestVersions::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, rx) = tokio::sync::watch::channel(false);
    let lookup = || {
        let mut rx = rx.clone();
        let calls = calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            rx.wait_for(|v| *v).await.unwrap();
            Ok::<_, ()>(Some("2.0.0".into()))
        }
    };
    let mut first = Box::pin(cache.cached("cli", false, lookup));
    let mut second = Box::pin(cache.cached("cli", false, lookup));
    assert!(is_pending(first.as_mut()).await);
    assert!(is_pending(second.as_mut()).await);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(true).unwrap();
    assert_eq!(first.await.unwrap(), Some("2.0.0".into()));
    assert_eq!(second.await.unwrap(), Some("2.0.0".into()));
    assert_eq!(
        cache.cached("cli", false, lookup).await.unwrap(),
        Some("2.0.0".into())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(3600)).await;
    cache.cached("cli", false, lookup).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn canceling_version_lookup_owner_frees_key_and_waiter_retries_its_own_lookup() {
    let cache = LatestVersions::default();
    let mut owner = Box::pin(cache.cached("cli", false, || {
        std::future::pending::<Result<Option<String>, ()>>()
    }));
    assert!(is_pending(owner.as_mut()).await);
    let calls = AtomicUsize::new(0);
    let mut waiter = Box::pin(cache.cached("cli", false, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(Some("2.0.0".into()))
    }));
    assert!(is_pending(waiter.as_mut()).await);
    drop(owner);
    assert_eq!(waiter.await.unwrap(), Some("2.0.0".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failing_owner_and_canceled_waiter_do_not_poison_shared_version_lookup() {
    let cache = LatestVersions::default();
    let (release, rx) = tokio::sync::watch::channel(false);
    let mut owner = Box::pin(cache.cached("cli", false, || {
        let mut rx = rx.clone();
        async move {
            rx.wait_for(|v| *v).await.unwrap();
            Err::<Option<String>, _>("offline")
        }
    }));
    assert!(is_pending(owner.as_mut()).await);
    let mut canceled = Box::pin(cache.cached("cli", false, || async {
        Ok::<_, &str>(Some("wrong".into()))
    }));
    assert!(is_pending(canceled.as_mut()).await);
    drop(canceled);
    let mut waiter = Box::pin(cache.cached("cli", false, || async {
        Ok::<_, &str>(Some("2.0.0".into()))
    }));
    assert!(is_pending(waiter.as_mut()).await);
    release.send(true).unwrap();
    assert_eq!(owner.await, Err("offline"));
    assert_eq!(waiter.await.unwrap(), Some("2.0.0".into()));
}

#[tokio::test(start_paused = true)]
async fn missing_versions_expire_in_a_minute_and_invalidated_old_lookups_cannot_overwrite_fresh_ones()
 {
    let cache = LatestVersions::default();
    let calls = AtomicUsize::new(0);
    let lookup = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ()>(None)
    };
    cache.cached("missing", false, lookup).await.unwrap();
    cache.cached("missing", false, lookup).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(60)).await;
    cache.cached("missing", false, lookup).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let (release, rx) = tokio::sync::watch::channel(false);
    let mut old = Box::pin(cache.cached("cli", false, || {
        let mut rx = rx.clone();
        async move {
            rx.wait_for(|v| *v).await.unwrap();
            Ok::<_, ()>(Some("old".into()))
        }
    }));
    assert!(is_pending(old.as_mut()).await);
    cache.invalidate(Some("cli"));
    assert_eq!(
        cache
            .cached("cli", false, || async { Ok::<_, ()>(Some("new".into())) })
            .await
            .unwrap(),
        Some("new".into())
    );
    release.send(true).unwrap();
    assert_eq!(old.await.unwrap(), Some("old".into()));
    assert_eq!(
        cache
            .cached("cli", false, || async { Ok::<_, ()>(None) })
            .await
            .unwrap(),
        Some("new".into())
    );
}

#[tokio::test]
async fn installation_writer_waits_for_running_work_and_blocks_new_work_without_blocking_other_instances()
 {
    let gates = InstallationGate::default();
    let id = ProviderInstanceId("a".into());
    let other = ProviderInstanceId("b".into());
    let (release, rx) = tokio::sync::oneshot::channel::<()>();
    let mut execution = Box::pin(gates.execute(&id, async {
        rx.await.unwrap();
    }));
    assert!(is_pending(execution.as_mut()).await);
    let (writer_release, writer_rx) = tokio::sync::oneshot::channel::<()>();
    let (started, mut started_rx) = tokio::sync::oneshot::channel();
    let ids = [id.clone(), id.clone()];
    let mut update = Box::pin(gates.update(&ids, async {
        started.send(()).unwrap();
        writer_rx.await.unwrap();
    }));
    assert!(is_pending(update.as_mut()).await);
    let mut later = Box::pin(gates.execute(&id, async { "later" }));
    assert!(is_pending(later.as_mut()).await);
    assert_eq!(
        gates.execute(&other, async { "independent" }).await,
        "independent"
    );
    release.send(()).unwrap();
    execution.await;
    assert!(is_pending(update.as_mut()).await);
    started_rx.try_recv().unwrap();
    assert!(is_pending(later.as_mut()).await);
    writer_release.send(()).unwrap();
    update.await;
    assert_eq!(later.await, "later");
}

#[test]
fn update_actions_pin_pnpm_latest_and_quote_windows_executables_for_powershell() {
    let update = action(
        "C:\\Program Files\\node\\npm.cmd",
        vec!["install".into(), "@openai/codex@latest".into()],
        "pnpm-global",
        Environment::new(),
        Platform::Windows,
    );
    assert!(
        update
            .command
            .starts_with("& 'C:\\Program Files\\node\\npm.cmd' ")
    );
    let caps = MaintenanceCapabilities {
        driver: "codex".into(),
        package: Some("@openai/codex".into()),
        update: Some(update),
        install_path: None,
        manual_command: None,
    };
    let targeted = latest_update(&caps, Some("1.2.3")).unwrap();
    assert_eq!(targeted.args[1], "@openai/codex@1.2.3");
    assert!(targeted.command.ends_with("@openai/codex@1.2.3"));
    assert!(targeted_update(&caps, "1.2.3;evil").is_none());
    assert_eq!(
        pnpm_update_args("pkg", Some("10.4.0")),
        ["add", "-g", "--allow-build=pkg", "pkg@latest"]
    );
    assert_eq!(
        pnpm_update_args("pkg", Some("10.3.9")),
        ["add", "-g", "pkg@latest"]
    );
}

#[test]
fn maintenance_requires_npm_ownership_and_reports_manual_updates_without_executable_actions() {
    assert_eq!(
        npm_global_prefix(
            "/usr/local/lib/node_modules/@openai/codex/bin/codex.js",
            "@openai/codex"
        )
        .as_deref(),
        Some("/usr/local")
    );
    assert!(
        npm_global_prefix(
            "/project/node_modules/tool/lib/node_modules/pkg/bin/main.js",
            "pkg"
        )
        .is_none()
    );
    assert!(
        npm_global_prefix(
            "/home/a/.local/share/mise/installs/codex/1.2/lib/node_modules/pkg/main.js",
            "pkg"
        )
        .is_none()
    );
    let owner = homebrew_ownership("/opt/homebrew/Cellar/codex/1.2.3/bin/codex").unwrap();
    assert!(!owner.cask);
    assert_eq!(owner.prefix, "/opt/homebrew");
    assert_eq!(
        parse_homebrew_latest(r#"{"casks":[{"version":"1.2.3,build"}]}"#, true).as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        parse_chocolatey_latest("Claude-Code|2.0.0\n", "claude-code").as_deref(),
        Some("2.0.0")
    );
    assert_eq!(parse_npm_view("[\"1.2.3\"]").as_deref(), Some("1.2.3"));
    assert!(parse_npm_view("[\"1\",\"2\"]").is_none());
    let caps = MaintenanceCapabilities {
        driver: "fx".into(),
        package: None,
        update: None,
        install_path: Some("/bin/fx".into()),
        manual_command: None,
    };
    let advisory = version_advisory(
        &caps,
        Some("0.0.1"),
        Some("0.0.2"),
        Some("2026-11-13T00:00:00Z"),
    );
    assert!(!advisory.can_update);
    assert!(advisory.message.unwrap().0.contains("T3 Code runs /bin/fx"));
}
