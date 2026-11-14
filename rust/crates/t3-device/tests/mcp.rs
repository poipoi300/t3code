#![cfg(feature = "mcp")]
#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, atomic::Ordering},
};
use t3_contracts::{base_schemas::ThreadId, device::*, provider_policy::RuntimeMode};
use t3_device::{
    DeviceService,
    host::{AgentEndpoint, OpenContext},
    io::Secret,
    mcp::{DeviceAdapter, Threads},
};
use t3_mcp::{
    AccessRule, AccessStore, CallContext, InvocationScope, Modes, ThreadShell,
    services::DeviceService as McpDeviceService,
};

struct ThreadAccess {
    shell: StdMutex<ThreadShell>,
}
impl ThreadAccess {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            shell: StdMutex::new(ThreadShell {
                id: "thread".into(),
                modes: Modes {
                    runtime_mode: RuntimeMode::ApprovalRequired,
                    ..Modes::full()
                },
                deleted: false,
                archived: false,
                active: true,
                provider_instance_id: Some("provider".into()),
                background_subagent_providers: vec![],
                link_origin: None,
            }),
        })
    }
}
#[async_trait]
impl AccessStore for ThreadAccess {
    async fn thread_shell(&self, _: &ThreadId) -> t3_mcp::Result<Option<ThreadShell>> {
        Ok(Some(self.shell.lock().unwrap().clone()))
    }
}
#[async_trait]
impl Threads for ThreadAccess {
    async fn shell(&self, _: &ThreadId) -> t3_mcp::Result<ThreadShell> {
        Ok(self.shell.lock().unwrap().clone())
    }
    async fn workspace(&self, _: &ThreadId) -> t3_mcp::Result<PathBuf> {
        Ok("/workspace".into())
    }
}
async fn context(access: Arc<ThreadAccess>, tool: &str, device_access: bool) -> CallContext {
    let scope:InvocationScope=serde_json::from_value(json!({"environmentId":"env","requestNamespace":"test","issuedAt":0,"capabilities":if device_access{vec!["device"]}else{vec![]},"thread":{"threadId":"thread","providerSessionId":"session","providerInstanceId":"provider"}})).unwrap();
    t3_mcp::access::authorize(
        access,
        scope,
        if ["device_open", "device_close"].contains(&tool) {
            AccessRule::ActsAsCaller
        } else {
            AccessRule::ReadsAsCaller
        },
        tool,
        &json!({}),
        false,
    )
    .await
    .unwrap()
}
async fn fixture(
    booted: bool,
) -> (
    DeviceAdapter,
    Arc<ThreadAccess>,
    Arc<FakeOps>,
    Arc<FakeHost>,
    Arc<FakeLaunchers>,
) {
    let ops = FakeOps::new("local", DevicePlatform::Ios, booted);
    let mut host = FakeHost::new("local", ops.clone(), FakeCommands::encoder(), None);
    Arc::get_mut(&mut host).unwrap().ready.agent = Some(AgentEndpoint {
        base_url: "http://agent.test".into(),
        token: Secret::new("secret"),
        entry_path: "/agent.mjs".into(),
        agent_device_id: None,
        session: None,
        state_dir: None,
        install_directory: None,
    });
    let access = ThreadAccess::new();
    let launchers = FakeLaunchers::new();
    let service = DeviceService::new(
        vec![host.clone()],
        settings(true, true),
        None,
        Some(launchers.clone()),
        None,
        None,
    )
    .await
    .unwrap();
    (
        DeviceAdapter {
            service,
            threads: access.clone(),
        },
        access,
        ops,
        host,
        launchers,
    )
}

#[test]
fn quick_start_matches_typescript_generated_goldens() {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("quick_start.json").trim_start_matches('\u{feff}'))
            .unwrap();
    for row in rows {
        let device = serde_json::from_value(row["device"].clone()).unwrap();
        let kind = row["kind"]
            .as_str()
            .map(|s| serde_json::from_value::<DeviceHostKind>(json!(s)).unwrap());
        let directory = row["installDirectory"].as_str().map(std::path::Path::new);
        assert_eq!(
            t3_device::mcp::quick_start(
                &device,
                row["command"].as_str().unwrap(),
                kind.as_ref(),
                directory
            ),
            row["expected"].as_str().unwrap()
        );
    }
}
#[tokio::test]
async fn missing_device_messages_keep_platform_and_explicit_target() {
    let (adapter, access, _, _, _) = fixture(true).await;
    let error = adapter
        .device_open(
            context(access.clone(), "device_open", true).await,
            serde_json::from_value(json!({"platform":"android"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("No android devices were found on host local. Call device_list to see why.")
    );
    let error = adapter
        .device_screenshot(
            context(access, "device_screenshot", true).await,
            serde_json::from_value(json!({"deviceId":"missing"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Device missing is not open in this thread. Call device_open first.")
    );
}
#[tokio::test]
async fn mcp_open_returns_post_boot_binding_and_screenshot_and_close_target_the_thread() {
    let (adapter, access, _ops, _host, launchers) = fixture(false).await;
    let opened = adapter
        .device_open(
            context(access.clone(), "device_open", true).await,
            serde_json::from_value(json!({"deviceId":"phone"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(opened.device.id.as_ref(), "booted-phone");
    assert!(
        opened
            .agent_device
            .target_args
            .windows(2)
            .any(|v| v == ["--udid", "booted-phone"])
    );
    assert_eq!(
        opened.agent_device.session,
        t3_device::agent::session("thread", "local", "booted-phone")
    );
    assert_eq!(launchers.bound.lock().await.len(), 1);
    let listed = adapter
        .device_list(
            context(access.clone(), "device_list", true).await,
            t3_mcp::decode_input("device_list", json!({"hostId":"LOCAL"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.open[0].device_id, opened.device.id);
    let shot = adapter
        .device_screenshot(
            context(access.clone(), "device_screenshot", true).await,
            serde_json::from_value(json!({})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shot.device.id, opened.device.id);
    assert_eq!(shot.screenshot.data, "iVBORw==");
    assert_eq!(shot.screenshot.width, 0);
    adapter
        .service
        .open(
            open("local", "booted-phone", "other-thread", DevicePlatform::Ios),
            OpenContext::default(),
        )
        .await
        .unwrap();
    adapter
        .device_close(
            context(access.clone(), "device_close", true).await,
            serde_json::from_value(json!({})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        adapter.service.state().await.sessions[0].thread_id.as_ref(),
        "other-thread"
    );
    assert_eq!(launchers.released.lock().await.len(), 1);
    assert!(
        adapter
            .device_screenshot(
                context(access, "device_screenshot", true).await,
                serde_json::from_value(json!({})).unwrap()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn mcp_missing_capability_refuses_before_discovery_or_helpers() {
    let (adapter, access, ops, host, _) = fixture(false).await;
    let error = adapter
        .device_open(
            context(access, "device_open", false).await,
            serde_json::from_value(json!({"deviceId":"phone"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Agent device access is turned off")
    );
    assert_eq!(host.starts.load(Ordering::SeqCst), 0);
    assert_eq!(ops.boots.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn mcp_rechecks_modes_at_session_commit_after_device_boot() {
    let (adapter, access, ops, _host, _) = fixture(false).await;
    let gate = Gate::new();
    *ops.boot_gate.lock().unwrap() = Some(gate.clone());
    let ctx = context(access.clone(), "device_open", true).await;
    let service = adapter.service.clone();
    let opening = tokio::spawn(async move {
        adapter
            .device_open(
                ctx,
                serde_json::from_value(json!({"deviceId":"phone"})).unwrap(),
            )
            .await
    });
    gate.entered.notified().await;
    access.shell.lock().unwrap().modes = Modes::full();
    gate.release.notify_one();
    let error = opening.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("now runs in"));
    assert!(service.state().await.sessions.is_empty());
}
#[tokio::test]
async fn mcp_reports_failed_host_setup_when_explicit_device_is_unlisted() {
    let (adapter, access, _ops, host, _) = fixture(true).await;
    host.fail.store(true, Ordering::SeqCst);
    let error = adapter
        .device_open(
            context(access, "device_open", true).await,
            serde_json::from_value(json!({"deviceId":"missing"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("failed to start: offline"));
}
#[test]
fn png_dimensions_require_signature_ihdr_and_complete_header() {
    let mut bytes = vec![0; 24];
    bytes[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&1170u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&2532u32.to_be_bytes());
    assert_eq!(t3_device::mcp::png_dimensions(&bytes), (1170, 2532));
    bytes[12] = 0;
    assert_eq!(t3_device::mcp::png_dimensions(&bytes), (0, 0));
    assert_eq!(t3_device::mcp::png_dimensions(&bytes[..23]), (0, 0));
}
