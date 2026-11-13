use super::super::{Registry, register};
use t3_contracts::editor::*;

pub fn register_module(registry: &mut Registry) {
    register::<EditorId>(registry, "EditorId");
    register::<EditorLaunchStyle>(registry, "EditorLaunchStyle");
    register::<ExternalLauncherBrowserSpawnError>(registry, "ExternalLauncherBrowserSpawnError");
    register::<ExternalLauncherCommandNotFoundError>(
        registry,
        "ExternalLauncherCommandNotFoundError",
    );
    register::<ExternalLauncherEditorSpawnError>(registry, "ExternalLauncherEditorSpawnError");
    register::<ExternalLauncherError>(registry, "ExternalLauncherError");
    register::<ExternalLauncherUnknownEditorError>(registry, "ExternalLauncherUnknownEditorError");
    register::<ExternalLauncherUnsupportedEditorError>(
        registry,
        "ExternalLauncherUnsupportedEditorError",
    );
    register::<ExternalLauncherUnsupportedTargetError>(
        registry,
        "ExternalLauncherUnsupportedTargetError",
    );
    register::<FileManagerRevealKind>(registry, "FileManagerRevealKind");
    register::<LaunchEditorInput>(registry, "LaunchEditorInput");
    register::<RemoteOpenTarget>(registry, "RemoteOpenTarget");
    register::<RemoteOpenTargetKind>(registry, "RemoteOpenTargetKind");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("editor", register_module)
}
