use crate::{DeviceService, Error, Result, host::OpenContext, service::Admission};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{base_schemas::ThreadId, device::*};
use t3_mcp::{CallContext, DecodedInput, McpError, scope::Capability};

#[async_trait]
pub trait Threads: Send + Sync {
    async fn shell(&self, thread: &ThreadId) -> t3_mcp::Result<t3_mcp::access::ThreadShell>;
    async fn workspace(&self, thread: &ThreadId) -> t3_mcp::Result<PathBuf>;
}
pub struct DeviceAdapter {
    pub service: DeviceService,
    pub threads: Arc<dyn Threads>,
}
struct Check {
    context: CallContext,
    threads: Arc<dyn Threads>,
}
#[async_trait]
impl Admission for Check {
    async fn check(&self) -> Result<()> {
        self.context
            .recheck_environment()
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        let caller = self
            .context
            .scope
            .require_thread("Device tools")
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        let shell = self
            .threads
            .shell(&caller.thread_id)
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        self.context
            .check_dispatch(&shell)
            .map_err(|e| Error::Unavailable(e.to_string()))
    }
}
fn caller(context: &CallContext) -> t3_mcp::Result<ThreadId> {
    context.scope.require_capability(Capability::Device).map_err(|_|mapped(Error::Unavailable("Agent device access is turned off for this environment. Ask the user to turn on Agent device access in Settings > Integrations > Devices; it applies when the agent session next starts, such as in a new thread.".into())))?;
    Ok(context
        .scope
        .require_thread("Device tools")?
        .thread_id
        .clone())
}
fn mapped(error: Error) -> McpError {
    match error {
        Error::Device { wire, reason } => {
            let fields = serde_json::to_value(wire).unwrap_or(Value::Null);
            McpError::Domain {
                tag: fields["_tag"]
                    .as_str()
                    .unwrap_or("DeviceOperationError")
                    .into(),
                message: reason,
                fields,
            }
        }
        error => McpError::Domain {
            tag: "DeviceToolUnavailableError".into(),
            message: error.to_string(),
            fields: json!({"reason":error.to_string()}),
        },
    }
}
pub fn resolve_host(state: &DeviceServiceState, selector: Option<&str>) -> Result<Option<String>> {
    let Some(selector) = selector.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Some(host) = state.hosts.iter().find(|h| h.id.as_ref() == selector) {
        return Ok(Some(host.id.as_ref().into()));
    }
    let matches = state
        .hosts
        .iter()
        .filter(|h| h.label.as_ref().to_lowercase() == selector.to_lowercase())
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Ok(Some(matches[0].id.as_ref().into()));
    }
    let candidates = if matches.is_empty() {
        state.hosts.iter().collect()
    } else {
        matches
    };
    let mut listed = candidates
        .iter()
        .take(10)
        .map(|h| {
            format!(
                "{} ({})",
                h.label.as_ref().chars().take(64).collect::<String>(),
                h.id.as_ref()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if candidates.len() > 10 {
        listed.push_str(&format!(", {} more", candidates.len() - 10));
    }
    let selector = selector.chars().take(128).collect::<String>();
    let duplicate = state
        .hosts
        .iter()
        .filter(|h| h.label.as_ref().to_lowercase() == selector.to_lowercase())
        .count()
        > 1;
    Err(Error::Unavailable(if duplicate {
        format!("Several device hosts are named {selector}; pass one id: {listed}.")
    } else {
        format!(
            "No device host {selector}. Known hosts: {}.",
            if listed.is_empty() { "none" } else { &listed }
        )
    }))
}
pub fn png_dimensions(bytes: &[u8]) -> (i64, i64) {
    if bytes.len() < 24
        || bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10]
        || &bytes[12..16] != b"IHDR"
    {
        return (0, 0);
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (i64::from(width), i64::from(height))
}
impl DeviceAdapter {
    async fn enabled(&self) -> t3_mcp::Result<DeviceServiceState> {
        let state = self.service.list().await.map_err(mapped)?;
        if state.host_status == DeviceHostStatus::Disabled {
            return Err(mapped(Error::Unavailable("Device support is off. Ask the user to enable it in the Device panel before installing or starting device tools.".into())));
        }
        Ok(state)
    }
}
#[async_trait]
impl t3_mcp::services::DeviceService for DeviceAdapter {
    async fn device_list(
        &self,
        context: CallContext,
        input: DecodedInput,
    ) -> t3_mcp::Result<DeviceToolListResult> {
        let thread = caller(&context)?;
        let state = self.enabled().await?;
        let host = resolve_host(&state, input.value()["hostId"].as_str()).map_err(mapped)?;
        Ok(DeviceToolListResult {
            host_statuses: state
                .host_statuses
                .into_iter()
                .filter(|(id, _)| host.as_ref().is_none_or(|h| h == id.as_ref()))
                .collect(),
            hosts: state
                .hosts
                .into_iter()
                .filter(|h| host.as_ref().is_none_or(|id| id == h.id.as_ref()))
                .collect(),
            devices: state
                .devices
                .into_iter()
                .filter(|d| host.as_ref().is_none_or(|id| id == d.host_id.as_ref()))
                .collect(),
            open: state
                .sessions
                .into_iter()
                .filter(|s| s.thread_id == thread)
                .map(|s| DeviceToolOpenSession {
                    host_id: s.host_id,
                    device_id: s.device_id,
                })
                .collect(),
        })
    }
    async fn device_open(
        &self,
        context: CallContext,
        input: DeviceToolOpenInput,
    ) -> t3_mcp::Result<DeviceToolOpenResult> {
        let thread = caller(&context)?;
        let state = self.enabled().await?;
        let host = resolve_host(
            &state,
            input
                .host_id
                .as_ref()
                .and_then(|v| v.as_ref())
                .map(AsRef::as_ref),
        )
        .map_err(mapped)?;
        let platform = input.platform.flatten();
        let target = if let Some(id) = input.device_id.flatten() {
            self.service
                .locate_device(host.as_deref(), id.as_ref())
                .await
                .map_err(mapped)?
                .ok_or_else(|| {
                    let named = |host: &DeviceHostSummary| {
                        format!("'{}' (hostId {})", host.label.as_ref(), host.id.as_ref())
                    };
                    let target = host.as_ref().map(|id| {
                        state
                            .hosts
                            .iter()
                            .find(|h| h.id.as_ref() == id)
                            .map(named)
                            .unwrap_or_else(|| format!("hostId {id}"))
                    });
                    let failures = state
                        .hosts
                        .iter()
                        .filter(|h| host.as_ref().is_none_or(|id| id == h.id.as_ref()))
                        .filter_map(|h| {
                            state
                                .host_statuses
                                .get(&h.id)
                                .filter(|s| s.status == DeviceHostStatus::Failed)
                                .and_then(|s| s.detail.as_ref().and_then(|d| d.as_ref()))
                                .map(|detail| format!("{} failed to start: {detail}", named(h)))
                        })
                        .collect::<Vec<_>>();
                    let mut parts = vec![format!(
                        "No device {} on {}.",
                        id.as_ref(),
                        target.as_deref().unwrap_or("any host")
                    )];
                    parts.extend(failures);
                    parts.push("Call device_list for current ids.".into());
                    mapped(Error::Unavailable(parts.join(" ")))
                })?
        } else {
            let id = host.as_deref().unwrap_or(LOCAL_DEVICE_HOST_ID);
            let candidates = state
                .devices
                .iter()
                .filter(|d| {
                    d.host_id.as_ref() == id && platform.as_ref().is_none_or(|p| *p == d.platform)
                })
                .collect::<Vec<_>>();
            if platform.is_none()
                && candidates.iter().any(|d| d.platform == DevicePlatform::Ios)
                && candidates
                    .iter()
                    .any(|d| d.platform == DevicePlatform::Android)
            {
                return Err(mapped(Error::Unavailable(
                    "Both iOS and Android devices are available; pass platform or deviceId.".into(),
                )));
            }
            candidates.iter().find(|d|d.booted).or(candidates.first()).map(|d|(*d).clone()).ok_or_else(||mapped(Error::Unavailable(state.host_statuses.get(&DeviceHostId::from(id)).and_then(|s|s.detail.clone().flatten()).unwrap_or_else(||"No simulators or emulators were found. Call device_list to see why.".into()))))?
        };
        if self
            .service
            .agent_readiness_if_supported(Some(target.host_id.as_ref()))
            .await
            .map_err(mapped)?
            .is_none()
        {
            return Err(mapped(Error::Unavailable("Agent device access requires enabled device support, agent access, and an available simulator platform on this host.".into())));
        }
        let workspace = self.threads.workspace(&thread).await?;
        let check = Arc::new(Check {
            context: context.clone(),
            threads: self.threads.clone(),
        });
        let opened = self
            .service
            .open_authorized(
                DeviceOpenInput {
                    thread_id: thread.clone(),
                    host_id: Some(Some(target.host_id.clone())),
                    device_id: target.id.clone(),
                    platform: target.platform.clone(),
                    boot: None,
                },
                OpenContext {
                    workspace_root: Some(workspace),
                },
                check,
            )
            .await
            .map_err(|e| context.recorded_refusal().unwrap_or_else(|| mapped(e)))?;
        let after = self.service.state().await;
        let device = after
            .devices
            .into_iter()
            .find(|d| d.host_id == opened.host_id && d.id == opened.device_id)
            .unwrap_or(target);
        let agent = self
            .service
            .agent_target(&thread, opened.host_id.as_ref(), opened.device_id.as_ref())
            .await
            .map_err(mapped)?;
        let mut args = Vec::new();
        if let Some(id) = &agent.agent_device_id {
            args.extend([
                "--platform".into(),
                if device.platform == DevicePlatform::Ios {
                    "ios"
                } else {
                    "android"
                }
                .into(),
                if device.platform == DevicePlatform::Ios {
                    "--udid"
                } else {
                    "--serial"
                }
                .into(),
                id.clone(),
            ]);
        }
        args.extend(agent.args.clone());
        let command=self.service.agent_launcher(&thread,opened.host_id.as_ref(),opened.device_id.as_ref(),&agent,&args).await.map_err(mapped)?.ok_or_else(||mapped(Error::Unavailable("The device was closed while device_open was preparing it. Call device_open again only if you still need it.".into())))?;
        let kind = after
            .hosts
            .iter()
            .find(|h| h.id == device.host_id)
            .map(|h| &h.kind);
        let quick_start = quick_start(&device, &command, kind, agent.install_directory.as_deref());
        Ok(DeviceToolOpenResult {
            device,
            agent_device: DeviceAgentCommand {
                command,
                target_args: args,
                session: agent.session,
            },
            quick_start,
        })
    }
    async fn device_screenshot(
        &self,
        context: CallContext,
        input: DeviceToolTargetInput,
    ) -> t3_mcp::Result<DeviceToolScreenshotResult> {
        let thread = caller(&context)?;
        let state = self.service.state().await;
        let host = resolve_host(
            &state,
            input
                .host_id
                .as_ref()
                .and_then(|v| v.as_ref())
                .map(AsRef::as_ref),
        )
        .map_err(mapped)?;
        let session = if let Some(id) = input.device_id.flatten() {
            self.service
                .locate_session(&thread, host.as_deref(), id.as_ref())
                .await
                .map_err(mapped)?
        } else {
            self.service
                .sessions_for_thread(&thread)
                .await
                .into_iter()
                .rev()
                .find(|s| host.as_ref().is_none_or(|h| h == s.host_id.as_ref()))
        }
        .ok_or_else(|| {
            mapped(Error::Unavailable(
                "No device is open in this thread. Call device_open first.".into(),
            ))
        })?;
        let (device, png) = self
            .service
            .screenshot(Some(session.host_id.as_ref()), session.device_id.as_ref())
            .await
            .map_err(mapped)?;
        let (width, height) = png_dimensions(&png);
        Ok(DeviceToolScreenshotResult {
            device,
            screenshot: DeviceScreenshot {
                mime_type: DeviceScreenshotMimeType::Png,
                data: base64::engine::general_purpose::STANDARD.encode(png),
                width,
                height,
            },
        })
    }
    async fn device_close(
        &self,
        context: CallContext,
        input: DeviceToolCloseInput,
    ) -> t3_mcp::Result<Value> {
        let thread = caller(&context)?;
        let state = self.service.state().await;
        let host = resolve_host(
            &state,
            input
                .host_id
                .as_ref()
                .and_then(|v| v.as_ref())
                .map(AsRef::as_ref),
        )
        .map_err(mapped)?;
        let id = input.device_id.flatten();
        let located = if let Some(id) = &id {
            self.service
                .locate_session(&thread, host.as_deref(), id.as_ref())
                .await
                .map_err(mapped)?
        } else {
            None
        };
        let host = located
            .map(|s| s.host_id)
            .or_else(|| host.map(|s| s.into()));
        self.service
            .close_authorized(
                DeviceCloseInput {
                    thread_id: thread,
                    host_id: host.map(Some),
                    device_id: id.map(Some),
                    shutdown: input.shutdown,
                },
                Arc::new(Check {
                    context: context.clone(),
                    threads: self.threads.clone(),
                }),
            )
            .await
            .map_err(|e| context.recorded_refusal().unwrap_or_else(|| mapped(e)))?;
        Ok(json!({}))
    }
}
pub fn quick_start(
    device: &DeviceSummary,
    command: &str,
    kind: Option<&DeviceHostKind>,
    install_directory: Option<&std::path::Path>,
) -> String {
    let executable = if !command.is_empty()
        && command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:-".contains(&b))
    {
        command.to_owned()
    } else {
        crate::ssh::quote(command)
    };
    let run = |args: &str| format!("{executable} {args}");
    let notes=match kind{
        Some(DeviceHostKind::Simlock)=>format!("This device is leased from a Simlock pool (alpha in T3 Code). The lease returns it to the pool, wiped, when it ends, so call device_close with shutdown: true when you are done. It starts with no apps installed.{}",install_directory.map(|dir|format!(" agent-device accepts install for this lease only when run from {}.",dir.display())).unwrap_or_default()),
        Some(DeviceHostKind::Eas)=>"This is an EAS Simulator session in the cloud. It bills the Expo account per minute until it stops, so call device_close with shutdown: true when you are done. It starts with no apps installed: install-from-source <url> installs a build from a URL (iOS needs a .tar.gz containing a .app), and install uploads one from this machine.".into(),
        _=>if device.platform==DevicePlatform::Ios{"First use builds an XCTest runner and can take a couple of minutes; later commands are fast."}else{"The Android snapshot helper installs itself on first use."}.into(),
    };
    vec![
        format!("The user is watching {} ({}) in the Device panel.",device.name.as_ref(),device.version),
        format!("Drive it with {executable}. It is bound to this device and session, so it needs no --platform, --udid, --serial, --config or --session flags. Use this exact path; login shells may reset PATH."),
        "Every app launch, deep link, tap, text entry, alert, setting, screenshot and recording on this device goes through it. Run open <bundle-or-package-id> (apps lists them) before snapshots, CLI screenshots or input; it starts the session they need. Use device_screenshot just to look.".into(),
        format!("  {}  # restarts the app; Expo dev clients connect to their Metro, --metro-port <port> picks it",run("open <bundle-or-package-id> --relaunch")),
        format!("  {}",run("open <bundle-or-package-id> <deep-link-url>")),
        format!("  {}  # accessibility tree with @eN refs",run("snapshot -i")),
        format!("  {}",run("press @e3 --settle")),
        format!("  {}",run("fill @e5 \"text\" --settle")),
        format!("  {}",run("alert accept")),
        format!("  {}  # also text-size, permission grant <name>",run("settings appearance dark")),
        format!("  {}  # {}or call device_screenshot to look",run("screenshot /tmp/shot.png"),if device.platform==DevicePlatform::Ios{"iOS saves 1x points; add --pixel-density <n> (3 on most iPhones) for device resolution, "}else{""}),
        format!("  {}  # then record stop",run("record start --scope device /tmp/run.mp4")),
        format!("  {}",run("install <app> <path-to-.app-or-.apk>")),
        format!("Prefer snapshot refs over coordinates. Run {executable} help <command> for flags and {executable} help workflow for guides."),
        "Do not use simctl, idb, adb or ssh for those actions. They stay fine for builds, installs, code sync, Metro, port forwards, logs, test runners and what this command cannot do; come back to it for every UI action and screenshot so the user can watch.".into(),
        "Each device_open returns its own command; opening another device does not retarget this one.".into(),
        "The command is removed when the device closes or T3 Code restarts. If it is missing, call device_open again.".into(),
        notes,
    ].join("\n")
}
