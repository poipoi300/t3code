use super::super::{Registry, register};
use t3_contracts::device::*;
pub fn register_module(registry: &mut Registry) {
    register::<DeviceActionInput>(registry, "DeviceActionInput");
    register::<DeviceActionUnavailableError>(registry, "DeviceActionUnavailableError");
    register::<DeviceAppearance>(registry, "DeviceAppearance");
    register::<DeviceBootError>(registry, "DeviceBootError");
    register::<DeviceCloseInput>(registry, "DeviceCloseInput");
    register::<DeviceColorFilter>(registry, "DeviceColorFilter");
    register::<DeviceConfigureInput>(registry, "DeviceConfigureInput");
    register::<DeviceDetail>(registry, "DeviceDetail");
    register::<DeviceDetailInput>(registry, "DeviceDetailInput");
    register::<DeviceEasAccount>(registry, "DeviceEasAccount");
    register::<DeviceEasAccountError>(registry, "DeviceEasAccountError");
    register::<DeviceEasAccountInput>(registry, "DeviceEasAccountInput");
    register::<DeviceError>(registry, "DeviceError");
    register::<DeviceForegroundApp>(registry, "DeviceForegroundApp");
    register::<DeviceHostId>(registry, "DeviceHostId");
    register::<DeviceHostStatus>(registry, "DeviceHostStatus");
    register::<DeviceHostStatuses>(registry, "DeviceHostStatuses");
    register::<DeviceHostSummary>(registry, "DeviceHostSummary");
    register::<DeviceHostUnavailableError>(registry, "DeviceHostUnavailableError");
    register::<DeviceId>(registry, "DeviceId");
    register::<DeviceListInput>(registry, "DeviceListInput");
    register::<DeviceNotFoundError>(registry, "DeviceNotFoundError");
    register::<DeviceOpenInput>(registry, "DeviceOpenInput");
    register::<DeviceOperationError>(registry, "DeviceOperationError");
    register::<DeviceOrientation>(registry, "DeviceOrientation");
    register::<DevicePermission>(registry, "DevicePermission");
    register::<DevicePlatform>(registry, "DevicePlatform");
    register::<DevicePlatformAvailability>(registry, "DevicePlatformAvailability");
    register::<DevicePlatformUnavailableError>(registry, "DevicePlatformUnavailableError");
    register::<DeviceServiceState>(registry, "DeviceServiceState");
    register::<DeviceSession>(registry, "DeviceSession");
    register::<DeviceSettings>(registry, "DeviceSettings");
    register::<DeviceShutdownInput>(registry, "DeviceShutdownInput");
    register::<DeviceSimlockIdentity>(registry, "DeviceSimlockIdentity");
    register::<DeviceSimlockIdentityError>(registry, "DeviceSimlockIdentityError");
    register::<DeviceSimlockIdentityInput>(registry, "DeviceSimlockIdentityInput");
    register::<DeviceSummary>(registry, "DeviceSummary");
    register::<DeviceTextSize>(registry, "DeviceTextSize");
    register::<DeviceToolCloseInput>(registry, "DeviceToolCloseInput");
    register::<DeviceToolError>(registry, "DeviceToolError");
    register::<DeviceToolListResult>(registry, "DeviceToolListResult");
    register::<DeviceToolOpenInput>(registry, "DeviceToolOpenInput");
    register::<DeviceToolOpenResult>(registry, "DeviceToolOpenResult");
    register::<DeviceToolScreenshotResult>(registry, "DeviceToolScreenshotResult");
    register::<DeviceToolTargetInput>(registry, "DeviceToolTargetInput");
    register::<DeviceToolUnavailableError>(registry, "DeviceToolUnavailableError");
    register::<DeviceToolVersion>(registry, "DeviceToolVersion");
    register::<DeviceToolVersions>(registry, "DeviceToolVersions");
    register::<EasDeviceHostConfig>(registry, "EasDeviceHostConfig");
    register::<EasDeviceHostConfigs>(registry, "EasDeviceHostConfigs");
    register::<SimlockDeviceHostConfig>(registry, "SimlockDeviceHostConfig");
    register::<SimlockDeviceHostConfigs>(registry, "SimlockDeviceHostConfigs");
    register::<SshDeviceHostConfig>(registry, "SshDeviceHostConfig");
    register::<SshDeviceHostConfigs>(registry, "SshDeviceHostConfigs");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("device", register_module)
}

#[test]
fn legacy_hosts_decode_with_current_defaults() -> Result<(), serde_json::Error> {
    use serde_json::json;
    for (input, expected) in [
        (
            json!({"id":"remote","label":"Remote","target":"host"}),
            json!({"kind":"ssh","id":"remote","label":"Remote","target":"host"}),
        ),
        (
            json!({"kind":"eas","id":"cloud","label":"Cloud"}),
            json!({"kind":"eas","id":"cloud","label":"Cloud","accessToken":"","maxSessionMinutes":30,"idleMinutes":10,"maxConcurrentSessions":2}),
        ),
        (
            json!({"kind":"simlock","id":"gateway","label":"Gateway","gatewayUrl":"https://example.com"}),
            json!({"kind":"simlock","id":"gateway","label":"Gateway","gatewayUrl":"https://example.com","agentToken":"","maxLeases":2,"maxLeaseMinutes":60}),
        ),
    ] {
        let decoded: DeviceHostConfig = serde_json::from_value(input)?;
        assert_eq!(serde_json::to_value(decoded)?, expected);
    }
    Ok(())
}
