use crate::{
    Error, Result,
    host::{DeviceOperations, Listing, OpenContext},
    io::{Command, Commands, Http, HttpRequest, json_request, request_bytes},
};
use async_trait::async_trait;
use base64::Engine;
use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::device::*;

pub struct HubOperations {
    pub host_id: String,
    pub origin: String,
    pub http: Arc<dyn Http>,
    pub commands: Arc<dyn Commands>,
    pub platforms: Vec<DevicePlatform>,
}
#[derive(Deserialize)]
struct HubList {
    simulators: Vec<HubDevice>,
    emulators: Vec<HubDevice>,
    #[serde(default)]
    errors: Vec<Warning>,
}
#[derive(Deserialize, Serialize)]
struct HubDevice {
    id: String,
    name: String,
    version: String,
    platform: DevicePlatform,
    booted: bool,
    physical: bool,
}
#[derive(Deserialize)]
struct Warning {
    message: String,
    error: Option<String>,
}

impl HubOperations {
    fn req(&self, method: Method, path: &str, seconds: u64) -> Result<HttpRequest> {
        HttpRequest::new(
            method,
            &format!("{}{path}", self.origin),
            Duration::from_secs(seconds),
        )
    }
    async fn post(&self, path: &str, body: Value, op: &str, seconds: u64) -> Result<Value> {
        json_request(
            self.http.as_ref(),
            self.req(Method::POST, path, seconds)?.json(&body)?,
            op,
        )
        .await
    }
    async fn start_stream(&self, id: &str) -> Result<()> {
        self.post(
            "/vendor/serve-sim/grid/api/start",
            json!({"udid":id}),
            "attach stream",
            180,
        )
        .await?;
        Ok(())
    }
    async fn post_shutdown(&self, path: &str, body: Value) -> Result<()> {
        let result = self.post(path, body, "shutdown", 15).await?;
        if result["ok"] != true {
            return Err(Error::operation(
                "shutdown",
                DeviceOperationErrorReason::HubRejected,
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl DeviceOperations for HubOperations {
    async fn list(&self) -> Result<Listing> {
        let requested = tokio::time::timeout(
            Duration::from_secs(40),
            json_request(
                self.http.as_ref(),
                self.req(Method::GET, "/api/devices", 40)?,
                "list",
            ),
        )
        .await;
        let value = match requested {
            Err(_) => {
                return Ok(Listing {
                    detail: Some("The device hub did not answer within 40 seconds.".into()),
                    incomplete_platforms: self.platforms.clone(),
                    ..Default::default()
                });
            }
            Ok(Err(Error::Unavailable(ref reason))) if reason.contains("timed out") => {
                return Ok(Listing {
                    detail: Some("The device hub did not answer within 40 seconds.".into()),
                    incomplete_platforms: self.platforms.clone(),
                    ..Default::default()
                });
            }
            Ok(value) => value?,
        };
        let list: HubList =
            serde_json::from_value(value).map_err(|_| crate::error::invalid("list"))?;
        let ios_missing = list.simulators.is_empty()
            && list
                .errors
                .iter()
                .any(|w| simulator_list_failed(&w.message));
        let mut devices = Vec::new();
        for device in list.simulators.into_iter().chain(list.emulators) {
            crate::error::validate_selector(&device.id, "device id")?;
            crate::error::validate_selector(&device.name, "device name")?;
            devices.push(DeviceSummary {
                host_id: self.host_id.as_str().into(),
                id: device.id.into(),
                name: device.name.into(),
                version: device.version,
                platform: device.platform,
                booted: device.booted,
                physical: device.physical,
            });
        }
        let android = self.platforms.contains(&DevicePlatform::Android);
        let mut details = Vec::new();
        for warning in list.errors {
            if (!android && warning.message.starts_with("[android-utils] "))
                || deleted_plist(&warning)
            {
                continue;
            }
            let head = warning
                .error
                .as_deref()
                .unwrap_or("")
                .lines()
                .take_while(|l| !l.trim_start().starts_with("at "))
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            details.push(crate::error::bounded(
                &format!(
                    "{}{}{}",
                    warning.message,
                    if head.is_empty() { "" } else { " " },
                    head
                ),
                2000,
            ));
        }
        let mut incomplete = false;
        if android {
            let result = self
                .commands
                .run(Command::new("emulator", ["-list-avds"]))
                .await?;
            if result.code != 0 {
                incomplete = true;
                details.push(format!("Could not list Android virtual devices (emulator -list-avds, exit code {}). Discovered devices remain available.",result.code));
            } else {
                for name in result
                    .stdout
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    crate::error::validate_selector(name, "AVD name")?;
                    if !devices
                        .iter()
                        .any(|d| d.platform == DevicePlatform::Android && d.name.as_ref() == name)
                    {
                        devices.push(DeviceSummary {
                            host_id: self.host_id.as_str().into(),
                            id: name.into(),
                            name: name.into(),
                            version: "Android".into(),
                            platform: DevicePlatform::Android,
                            booted: false,
                            physical: false,
                        });
                    }
                }
            }
        }
        Ok(Listing {
            devices,
            detail: if details.is_empty() {
                None
            } else {
                Some(details.join("\n"))
            },
            android_discovery_incomplete: incomplete,
            incomplete_platforms: if ios_missing {
                vec![DevicePlatform::Ios]
            } else {
                vec![]
            },
        })
    }
    async fn boot(&self, device: &DeviceSummary, _: &OpenContext) -> Result<String> {
        crate::error::validate_selector(device.id.as_ref(), "boot")?;
        let result = self
            .post(
                "/api/devices/boot",
                json!({"platform":device.platform,"id":device.id,"name":device.name}),
                "boot",
                180,
            )
            .await?;
        if result["ok"] != true {
            let mut diagnostics = Vec::new();
            if let Some(s) = result["error"].as_str().filter(|s| !s.trim().is_empty()) {
                diagnostics.push(s);
            }
            if let Some(errors) = result["errors"].as_array() {
                for error in errors {
                    for key in ["message", "error"] {
                        if let Some(s) = error[key].as_str().filter(|s| !s.trim().is_empty()) {
                            diagnostics.push(s);
                        }
                    }
                }
            }
            let diagnostics = diagnostics.join("\n\n");
            let lower = diagnostics.to_lowercase();
            let reason = if lower.contains("no space left")
                || ((lower.contains("insufficient") || lower.contains("not enough"))
                    && (lower.contains("disk") || lower.contains("space")))
            {
                DeviceBootErrorReason::DiskSpace
            } else if lower.contains("timeout") || lower.contains("timed out") {
                DeviceBootErrorReason::Timeout
            } else {
                DeviceBootErrorReason::LaunchFailed
            };
            return Err(Error::boot(
                &self.host_id,
                device.id.as_ref(),
                reason,
                (!diagnostics.is_empty()).then_some(diagnostics),
            ));
        }
        if device.platform == DevicePlatform::Ios {
            self.start_stream(device.id.as_ref()).await?;
        }
        let id = result["serial"]
            .as_str()
            .or(result["id"].as_str())
            .unwrap_or(device.id.as_ref());
        crate::error::validate_selector(id, "boot result")?;
        Ok(id.into())
    }
    async fn attach(&self, device: &DeviceSummary) -> Result<()> {
        crate::error::validate_selector(device.id.as_ref(), "device id")?;
        if device.platform == DevicePlatform::Ios {
            self.start_stream(device.id.as_ref()).await?;
        }
        Ok(())
    }
    async fn shutdown(&self, id: &str, platform: DevicePlatform) -> Result<()> {
        crate::error::validate_selector(id, "shutdown")?;
        if platform == DevicePlatform::Ios {
            if let Err(error) = self
                .post_shutdown("/vendor/serve-sim/api/shutdown", json!({"udid":id}))
                .await
                && !self
                    .list()
                    .await?
                    .devices
                    .iter()
                    .any(|d| d.id.as_ref() == id && !d.booted)
            {
                return Err(error);
            }
            Ok(())
        } else {
            self.post_shutdown(
                "/api/devices/shutdown",
                json!({"platform":platform,"id":id}),
            )
            .await
        }
    }
    async fn screenshot(&self, device: &DeviceSummary) -> Result<Vec<u8>> {
        crate::error::validate_selector(device.id.as_ref(), "device id")?;
        let prefix = if device.platform == DevicePlatform::Ios {
            "serve-sim"
        } else {
            "serve-emu"
        };
        let mut request = self.req(
            Method::POST,
            &format!("/vendor/{prefix}/api/screenshot"),
            20,
        )?;
        request
            .url
            .query_pairs_mut()
            .append_pair("device", device.id.as_ref());
        let (status, bytes) = request_bytes(self.http.as_ref(), request).await?;
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        let old_ios = device.platform == DevicePlatform::Ios
            && device
                .version
                .strip_prefix("iOS ")
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse::<u32>().ok())
                .is_some_and(|v| v <= 26);
        if !old_ios {
            return Err(Error::operation(
                "screenshot",
                DeviceOperationErrorReason::HubRejected,
            ));
        }
        const CAPTURE: &str = "f=$(mktemp -t t3-screenshot) || exit 1\ntrap 'rm -f \"$f\"' 0\nxcrun simctl io \"$1\" screenshot --type=png \"$f\" >/dev/null && base64 -i \"$f\"";
        let mut command = Command::new("sh", ["-c", CAPTURE, "sh", device.id.as_ref()]);
        command.timeout = Duration::from_secs(20);
        command.max_bytes = 64 * 1024 * 1024;
        let result = self.commands.run(command).await?;
        if result.code != 0 {
            return Err(Error::command("screenshot", result.code));
        }
        let encoded: String = result
            .stdout
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| crate::error::invalid("screenshot"))
    }
}

fn simulator_list_failed(message: &str) -> bool {
    message.starts_with("[apple-utils] Failed to run `xcrun simctl list devices")
        || message.starts_with("[apple-utils] Failed to parse JSON output:")
        || message.starts_with("[apple-utils] Failed to list devices:")
        || message
            .starts_with("[apple-utils] `xcrun simctl list devices --json` did not respond within ")
}
fn deleted_plist(warning: &Warning) -> bool {
    warning
        .message
        .starts_with("[apple-utils] Failed to read CoreSimulator plist at ")
        && warning.message.contains("/CoreSimulator/Devices/")
        && warning.message.ends_with("/device.plist:")
        && warning.error.as_deref().is_some_and(|s| {
            s.starts_with("Error: ENOENT:")
                || s.contains("code: 'ENOENT'")
                || s.contains("code: \"ENOENT\"")
        })
}
