use super::super::{Registry, register};
use t3_contracts::environment::*;
pub fn register_module(registry: &mut Registry) {
    register::<EnvironmentMachineKind>(registry, "EnvironmentMachineKind");
    register::<ExecutionEnvironmentCapabilities>(registry, "ExecutionEnvironmentCapabilities");
    register::<ExecutionEnvironmentDescriptor>(registry, "ExecutionEnvironmentDescriptor");
    register::<ExecutionEnvironmentPlatform>(registry, "ExecutionEnvironmentPlatform");
    register::<ExecutionEnvironmentPlatformArch>(registry, "ExecutionEnvironmentPlatformArch");
    register::<ExecutionEnvironmentPlatformOs>(registry, "ExecutionEnvironmentPlatformOs");
    register::<RepositoryIdentity>(registry, "RepositoryIdentity");
    register::<RepositoryIdentityLocator>(registry, "RepositoryIdentityLocator");
    register::<RepositoryOrigin>(registry, "RepositoryOrigin");
    register::<ScopedProjectRef>(registry, "ScopedProjectRef");
    register::<ScopedThreadRef>(registry, "ScopedThreadRef");
    register::<ServerInstallation>(registry, "ServerInstallation");
    register::<ServerSelfUpdateCapability>(registry, "ServerSelfUpdateCapability");
    register::<ServerSelfUpdateMethod>(registry, "ServerSelfUpdateMethod");
    register::<ThreadEnvMode>(registry, "ThreadEnvMode");
    register::<WorktreeSubmodules>(registry, "WorktreeSubmodules");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("environment", register_module)
}

#[test]
fn capabilities_default_and_unknown_optional_shapes_are_omitted() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let capabilities: ExecutionEnvironmentCapabilities = serde_json::from_value(json!({
     "serverInstallation": {"kind": "future-installer"}, "serverBrowser": false
    }))?;
    assert_eq!(
        serde_json::to_value(capabilities)?,
        json!({"repositoryIdentity": false, "serverBrowser": false})
    );
    let platform: ExecutionEnvironmentPlatform =
        serde_json::from_value(json!({"os": "linux", "arch": "x64", "machine": "future-machine"}))?;
    assert_eq!(
        serde_json::to_value(platform)?,
        json!({"os": "linux", "arch": "x64"})
    );
    assert!(
        serde_json::from_value::<ExecutionEnvironmentCapabilities>(
            json!({"attachmentUploads": null})
        )
        .is_err()
    );
    Ok(())
}
