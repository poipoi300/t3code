#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use serde_json::json;
use std::sync::atomic::Ordering;
use t3_contracts::{base_schemas::ThreadId, device::*};
use t3_device::{Error, host::OpenContext};

#[tokio::test]
async fn consent_gates_list_readiness_and_agent_start() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let host = FakeHost::new("local", ops, FakeCommands::encoder(), None);
    let service = service(vec![host.clone()], false, false).await;
    assert_eq!(
        service.list().await.unwrap().host_status,
        DeviceHostStatus::Disabled
    );
    assert!(service.readiness(None).await.is_err());
    assert!(
        service
            .agent_readiness_if_supported(None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    let configured = service
        .configure(serde_json::from_value(json!({"enabled":true})).unwrap())
        .await
        .unwrap();
    assert_eq!(configured.devices.len(), 1);
    assert_eq!(host.agents.load(Ordering::SeqCst), 0);
    service
        .configure(serde_json::from_value(json!({"agentAccessEnabled":true})).unwrap())
        .await
        .unwrap();
    assert_eq!(host.agents.load(Ordering::SeqCst), 1);
    service
        .configure(
            serde_json::from_value(json!({"agentAccessEnabled":false,"onboardingCompleted":true}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(host.agent_stops.load(Ordering::SeqCst), 1);
    assert!(service.state().await.onboarding_completed);
    let state = service
        .configure(serde_json::from_value(json!({"enabled":false})).unwrap())
        .await
        .unwrap();
    assert!(state.devices.is_empty());
    assert!(state.sessions.is_empty());
    assert_eq!(host.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn android_boot_replaces_avd_with_authoritative_serial_and_clears_progress() {
    let ops = FakeOps::new("local", DevicePlatform::Android, false);
    let host = FakeHost::new("local", ops.clone(), FakeCommands::encoder(), None);
    let service = service(vec![host], true, false).await;
    let mut stream = service.state_stream().await;
    assert_eq!(stream.recv().await.unwrap().revision, 0);
    let opened = service
        .open(
            open("local", "phone", "t", DevicePlatform::Android),
            OpenContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(opened.device_id.as_ref(), "booted-phone");
    assert_eq!(ops.boots.load(Ordering::SeqCst), 1);
    let state = service.state().await;
    assert_eq!(state.devices.len(), 1);
    assert_eq!(state.devices[0].id, opened.device_id);
    assert_eq!(state.booting_devices, Some(Some(vec![])));
    let mut saw_booting = false;
    while let Ok(update) = stream.try_recv() {
        if update
            .booting_devices
            .as_ref()
            .and_then(|v| v.as_ref())
            .is_some_and(|v| !v.is_empty())
        {
            saw_booting = true;
        }
    }
    assert!(saw_booting);
}
#[tokio::test]
async fn failing_boot_always_clears_progress_and_adds_no_session() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, false);
    ops.fail_boot.store(true, Ordering::SeqCst);
    let host = FakeHost::new("local", ops, FakeCommands::encoder(), None);
    let service = service(vec![host], true, false).await;
    assert!(
        service
            .open(
                open("local", "phone", "t", DevicePlatform::Ios),
                OpenContext::default()
            )
            .await
            .is_err()
    );
    let state = service.state().await;
    assert!(state.sessions.is_empty());
    assert_eq!(state.booting_devices, Some(Some(vec![])));
}
#[tokio::test]
async fn disabled_during_boot_cannot_restore_devices_or_sessions() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, false);
    let gate = Gate::new();
    *ops.boot_gate.lock().unwrap() = Some(gate.clone());
    let host = FakeHost::new("local", ops, FakeCommands::encoder(), None);
    let service = service(vec![host], true, false).await;
    let copy = service.clone();
    let opening = tokio::spawn(async move {
        copy.open(
            open("local", "phone", "t", DevicePlatform::Ios),
            OpenContext::default(),
        )
        .await
    });
    gate.entered.notified().await;
    service
        .configure(serde_json::from_value(json!({"enabled":false})).unwrap())
        .await
        .unwrap();
    gate.release.notify_one();
    assert!(opening.await.unwrap().is_err());
    let state = service.state().await;
    assert!(state.sessions.is_empty());
    assert!(state.devices.is_empty());
    assert_eq!(state.host_status, DeviceHostStatus::Disabled);
}
#[tokio::test]
async fn emulator_encoder_check_refuses_missing_and_similarly_named_encoders() {
    for (code, stdout, should_pass) in [
        (127, "", false),
        (1, " V....D libx264", false),
        (0, " V....D libx264rgb libx264 H.264", false),
        (0, " V....D libx264", true),
    ] {
        let ops = FakeOps::new("local", DevicePlatform::Android, false);
        let commands = FakeCommands::new(move |_| {
            Ok(t3_device::io::CommandOutput {
                code,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        });
        let host = FakeHost::new("local", ops.clone(), commands, None);
        let service = service(vec![host], true, false).await;
        let result = service
            .open(
                open("local", "phone", "t", DevicePlatform::Android),
                OpenContext::default(),
            )
            .await;
        assert_eq!(result.is_ok(), should_pass, "{stdout}");
        assert_eq!(ops.boots.load(Ordering::SeqCst), usize::from(should_pass));
        assert_eq!(
            service.state().await.sessions.len(),
            usize::from(should_pass)
        );
    }
}
#[tokio::test]
async fn incomplete_discovery_retains_only_the_unlisted_platform_and_then_recovers() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    ops.listing.lock().await.devices.push(device(
        "local",
        "android",
        DevicePlatform::Android,
        true,
    ));
    let host = FakeHost::new("local", ops.clone(), FakeCommands::encoder(), None);
    let service = service(vec![host], true, false).await;
    service.list().await.unwrap();
    {
        let mut list = ops.listing.lock().await;
        list.devices.clear();
        list.detail = Some("simctl failed".into());
        list.incomplete_platforms = vec![DevicePlatform::Ios];
    }
    let retained = service.list().await.unwrap();
    assert_eq!(
        retained
            .devices
            .iter()
            .map(|d| d.id.as_ref())
            .collect::<Vec<_>>(),
        ["phone"]
    );
    assert_eq!(
        retained.host_status_detail,
        Some(Some("simctl failed".into()))
    );
    ops.listing.lock().await.incomplete_platforms.clear();
    ops.listing.lock().await.detail = None;
    let recovered = service.list().await.unwrap();
    assert!(recovered.devices.is_empty());
    assert!(recovered.host_status_detail.is_none());
}
#[tokio::test]
async fn close_releases_only_callers_claim_and_keeps_device_powered() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let release = FakeRelease::new();
    let host = FakeHost::new(
        "local",
        ops.clone(),
        FakeCommands::encoder(),
        Some(release.clone()),
    );
    let service = service(vec![host], true, false).await;
    for thread in ["a", "b"] {
        service
            .open(
                open("local", "phone", thread, DevicePlatform::Ios),
                OpenContext::default(),
            )
            .await
            .unwrap();
    }
    service.close(close("a", false)).await.unwrap();
    service.close(close("a", false)).await.unwrap();
    let state = service.state().await;
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].thread_id.as_ref(), "b");
    assert!(state.devices[0].booted);
    assert_eq!(ops.shutdowns.load(Ordering::SeqCst), 0);
    assert_eq!(
        release.calls.lock().await.as_slice(),
        [t3_device::agent::session("a", "local", "phone")]
    );
}
#[tokio::test]
async fn failed_release_prevents_shutdown_and_preserves_records_for_retry() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let release = FakeRelease::new();
    release.fail.store(true, Ordering::SeqCst);
    let host = FakeHost::new(
        "local",
        ops.clone(),
        FakeCommands::encoder(),
        Some(release.clone()),
    );
    let service = service(vec![host], true, false).await;
    service
        .open(
            open("local", "phone", "a", DevicePlatform::Ios),
            OpenContext::default(),
        )
        .await
        .unwrap();
    assert!(service.close(close("a", true)).await.is_err());
    assert_eq!(ops.shutdowns.load(Ordering::SeqCst), 0);
    assert_eq!(service.state().await.sessions.len(), 1);
    release.fail.store(false, Ordering::SeqCst);
    service.close(close("a", true)).await.unwrap();
    assert_eq!(ops.shutdowns.load(Ordering::SeqCst), 1);
    assert!(service.state().await.sessions.is_empty());
    assert_eq!(release.calls.lock().await.len(), 2);
}
#[tokio::test]
async fn close_shutdown_releases_peer_claims_before_poweroff() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let release = FakeRelease::new();
    let host = FakeHost::new(
        "local",
        ops.clone(),
        FakeCommands::encoder(),
        Some(release.clone()),
    );
    let service = service(vec![host], true, false).await;
    for thread in ["a", "b"] {
        service
            .open(
                open("local", "phone", thread, DevicePlatform::Ios),
                OpenContext::default(),
            )
            .await
            .unwrap();
    }
    service.close(close("a", true)).await.unwrap();
    assert_eq!(release.calls.lock().await.len(), 2);
    assert!(service.state().await.sessions.is_empty());
    assert_eq!(ops.shutdowns.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn reopen_waits_for_old_close_claim_release() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let release = FakeRelease::new();
    let gate = Gate::new();
    let host = FakeHost::new("local", ops, FakeCommands::encoder(), Some(release.clone()));
    let service = service(vec![host], true, false).await;
    service
        .open(
            open("local", "phone", "a", DevicePlatform::Ios),
            OpenContext::default(),
        )
        .await
        .unwrap();
    *release.gate.lock().unwrap() = Some(gate.clone());
    let s = service.clone();
    let closing = tokio::spawn(async move { s.close(close("a", false)).await });
    gate.entered.notified().await;
    let s = service.clone();
    let opening = tokio::spawn(async move {
        s.open(
            open("local", "phone", "a", DevicePlatform::Ios),
            OpenContext::default(),
        )
        .await
    });
    gate.release.notify_one();
    closing.await.unwrap().unwrap();
    opening.await.unwrap().unwrap();
    assert_eq!(service.state().await.sessions.len(), 1);
    assert_eq!(release.calls.lock().await.len(), 1);
}
#[tokio::test]
async fn host_collisions_are_ambiguous_and_explicit_wrong_host_never_falls_back() {
    let a = FakeOps::new("a", DevicePlatform::Ios, true);
    let b = FakeOps::new("b", DevicePlatform::Ios, true);
    b.listing
        .lock()
        .await
        .devices
        .push(device("b", "only-b", DevicePlatform::Ios, true));
    let a = FakeHost::new("a", a, FakeCommands::encoder(), None);
    let b = FakeHost::new("b", b, FakeCommands::encoder(), None);
    let service = service(vec![a, b], true, false).await;
    service.list().await.unwrap();
    assert!(matches!(
        service.locate_device(None, "phone").await,
        Err(Error::Ambiguous { .. })
    ));
    assert!(matches!(
        service.locate_device(Some("a"), "only-b").await,
        Err(Error::OtherHost { .. })
    ));
    assert_eq!(
        service
            .locate_device(Some("b"), "phone")
            .await
            .unwrap()
            .unwrap()
            .host_id
            .as_ref(),
        "b"
    );
    assert!(
        service
            .locate_device(None, "absent")
            .await
            .unwrap()
            .is_none()
    );
    for host in ["a", "b"] {
        service
            .open(
                open(host, "phone", "t", DevicePlatform::Ios),
                OpenContext::default(),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        service
            .locate_session(&ThreadId::from("t"), None, "phone")
            .await,
        Err(Error::Ambiguous { .. })
    ));
    service
        .close(serde_json::from_value(json!({"threadId":"t","hostId":"a"})).unwrap())
        .await
        .unwrap();
    assert_eq!(
        service.sessions_for_thread(&"t".into()).await[0]
            .host_id
            .as_ref(),
        "b"
    );
}
#[tokio::test]
async fn inspection_is_read_only_and_failed_hosts_do_not_hide_healthy_hosts() {
    let good = FakeHost::new(
        "good",
        FakeOps::new("good", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    let bad = FakeHost::new(
        "bad",
        FakeOps::new("bad", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    bad.fail.store(true, Ordering::SeqCst);
    bad.inspection_fail.store(true, Ordering::SeqCst);
    let service = service(vec![good.clone(), bad.clone()], false, false).await;
    let inspected = service.inspect().await.unwrap();
    assert_eq!(inspected.host_status, DeviceHostStatus::Disabled);
    assert_eq!(good.starts.load(Ordering::SeqCst), 0);
    assert!(inspected.hosts[1].tool_inspection_error.is_some());
    let listed = service
        .configure(serde_json::from_value(json!({"enabled":true})).unwrap())
        .await
        .unwrap();
    assert_eq!(listed.devices.len(), 1);
    assert_eq!(listed.devices[0].host_id.as_ref(), "good");
    assert_eq!(
        listed.host_statuses[&DeviceHostId::from("bad")].status,
        DeviceHostStatus::Failed
    );
}
#[tokio::test]
async fn screenshot_discovers_before_any_list() {
    let ops = FakeOps::new("local", DevicePlatform::Ios, true);
    let host = FakeHost::new("local", ops, FakeCommands::encoder(), None);
    let service = service(vec![host], true, false).await;
    assert!(service.state().await.devices.is_empty());
    let (device, png) = service.screenshot(None, "phone").await.unwrap();
    assert_eq!(device.id.as_ref(), "phone");
    assert_eq!(png, [137, 80, 78, 71]);
}
#[tokio::test]
async fn state_subscription_contains_initial_snapshot_then_all_revisions() {
    let service = service(vec![], false, false).await;
    let mut rx = service.state_stream().await;
    let initial = rx.recv().await.unwrap();
    service
        .configure(serde_json::from_value(json!({"onboardingCompleted":true})).unwrap())
        .await
        .unwrap();
    let update = rx.recv().await.unwrap();
    assert!(update.onboarding_completed);
    assert_eq!(update.revision, initial.revision + 1);
}

#[tokio::test]
async fn background_host_reconnects_reach_state_subscribers() {
    let host = FakeHost::new("local", FakeOps::new("local", DevicePlatform::Ios, true), FakeCommands::encoder(), None);
    let service = service(vec![host.clone()], true, false).await;
    service.readiness(None).await.unwrap();
    let mut updates = service.state_stream().await;
    let initial = updates.recv().await.unwrap();
    host.phase.lock().unwrap().as_ref().unwrap()(DeviceHostStatus::Starting, Some("Reconnecting".into()));
    let update = updates.recv().await.unwrap();
    assert_eq!(update.host_status, DeviceHostStatus::Starting);
    assert_eq!(update.host_status_detail.flatten().as_deref(), Some("Reconnecting"));
    assert_eq!(update.revision, initial.revision + 1);
    host.phase.lock().unwrap().as_ref().unwrap()(DeviceHostStatus::Ready, None);
    assert_eq!(updates.recv().await.unwrap().host_status, DeviceHostStatus::Ready);
}

#[tokio::test]
async fn cloud_summaries_are_split_for_wire_compatibility() {
    let mut state = service(vec![], false, false).await.state().await;
    for (id, kind) in [("local", DeviceHostKind::Local), ("ssh", DeviceHostKind::Ssh), ("eas", DeviceHostKind::Eas), ("pool", DeviceHostKind::Simlock)] {
        state.hosts.push(t3_device::host::summary(id, id, kind, vec![]));
    }
    let wire = serde_json::to_value(t3_device::service::wire_state(state)).unwrap();
    assert_eq!(wire["hosts"].as_array().unwrap().iter().map(|h|h["id"].as_str().unwrap()).collect::<Vec<_>>(), ["local", "ssh"]);
    assert_eq!(wire["easHosts"][0]["id"], "eas");
    assert_eq!(wire["simlockHosts"][0]["id"], "pool");
}
