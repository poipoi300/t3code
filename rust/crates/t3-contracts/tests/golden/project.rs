use super::super::{Registry, register};
use t3_contracts::project::*;
pub fn register_module(registry: &mut Registry) {
    register::<Project>(registry, "Project");
    register::<ProjectContentMatch>(registry, "ProjectContentMatch");
    register::<ProjectContentMatchRange>(registry, "ProjectContentMatchRange");
    register::<ProjectCreateFromThreadInput>(registry, "ProjectCreateFromThreadInput");
    register::<ProjectCreateFromThreadResult>(registry, "ProjectCreateFromThreadResult");
    register::<ProjectCreateNewInput>(registry, "ProjectCreateNewInput");
    register::<ProjectCreateNewResult>(registry, "ProjectCreateNewResult");
    register::<ProjectCreatePayload>(registry, "ProjectCreatePayload");
    register::<ProjectEnsureScratchResult>(registry, "ProjectEnsureScratchResult");
    register::<ProjectEntriesFailure>(registry, "ProjectEntriesFailure");
    register::<ProjectEntry>(registry, "ProjectEntry");
    register::<ProjectEntryKind>(registry, "ProjectEntryKind");
    register::<ProjectFaviconPath>(registry, "ProjectFaviconPath");
    register::<ProjectFileFailure>(registry, "ProjectFileFailure");
    register::<ProjectFileOperation>(registry, "ProjectFileOperation");
    register::<ProjectIconColor>(registry, "ProjectIconColor");
    register::<ProjectIconOverride>(registry, "ProjectIconOverride");
    register::<ProjectIgnoredFile>(registry, "ProjectIgnoredFile");
    register::<ProjectListEntriesError>(registry, "ProjectListEntriesError");
    register::<ProjectListEntriesInput>(registry, "ProjectListEntriesInput");
    register::<ProjectListEntriesResult>(registry, "ProjectListEntriesResult");
    register::<ProjectListIgnoredFilesInput>(registry, "ProjectListIgnoredFilesInput");
    register::<ProjectListIgnoredFilesResult>(registry, "ProjectListIgnoredFilesResult");
    register::<ProjectMonogramText>(registry, "ProjectMonogramText");
    register::<ProjectMutation>(registry, "ProjectMutation");
    register::<ProjectMutation>(registry, "ProjectMutation.project.create");
    register::<ProjectMutation>(registry, "ProjectMutation.project.delete");
    register::<ProjectMutation>(registry, "ProjectMutation.project.update");
    register::<ProjectMutationError>(registry, "ProjectMutationError");
    register::<ProjectReadFileError>(registry, "ProjectReadFileError");
    register::<ProjectReadFileInput>(registry, "ProjectReadFileInput");
    register::<ProjectReadFileResult>(registry, "ProjectReadFileResult");
    register::<ProjectScript>(registry, "ProjectScript");
    register::<ProjectScriptIcon>(registry, "ProjectScriptIcon");
    register::<ProjectSearchContentsError>(registry, "ProjectSearchContentsError");
    register::<ProjectSearchContentsInput>(registry, "ProjectSearchContentsInput");
    register::<ProjectSearchContentsResult>(registry, "ProjectSearchContentsResult");
    register::<ProjectSearchEntriesError>(registry, "ProjectSearchEntriesError");
    register::<ProjectSearchEntriesInput>(registry, "ProjectSearchEntriesInput");
    register::<ProjectSearchEntriesResult>(registry, "ProjectSearchEntriesResult");
    register::<ProjectSnapshot>(registry, "ProjectSnapshot");
    register::<ProjectUpdatePayload>(registry, "ProjectUpdatePayload");
    register::<ProjectWriteFileError>(registry, "ProjectWriteFileError");
    register::<ProjectWriteFileInput>(registry, "ProjectWriteFileInput");
    register::<ProjectWriteFileResult>(registry, "ProjectWriteFileResult");
    register::<ReceivedProjectIcon>(registry, "ReceivedProjectIcon");
    register::<StoredProjectIcon>(registry, "StoredProjectIcon");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("project", register_module)
}

#[test]
fn legacy_icons_normalize_and_future_icons_remain_decode_only() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let canonical = json!({"kind": "monogram", "text": "AB", "color": "blue"});
    for legacy in [
        json!({"kind": "lucide", "name": "folder", "color": "blue", "monogram": "AB"}),
        json!({"kind": "lucide", "name": "folder", "color": "blue", "monogramText": "AB", "monogram": "CD"}),
        json!({"kind": "lucide", "name": "folder", "color": "blue", "monogramText": null, "monogram": "AB"}),
    ] {
        let stored: StoredProjectIcon = serde_json::from_value(legacy.clone())?;
        assert_eq!(serde_json::to_value(stored)?, canonical);
        let received: ReceivedProjectIcon = serde_json::from_value(legacy)?;
        assert_eq!(serde_json::to_value(received)?, canonical);
    }
    let future: ReceivedProjectIcon = serde_json::from_value(json!({"kind": "future"}))?;
    assert!(future.0.is_none());
    assert!(serde_json::to_value(future).is_err());
    assert!(serde_json::from_value::<ReceivedProjectIcon>(json!({"kind": "lucide"})).is_err());
    assert!(serde_json::from_value::<StoredProjectIcon>(json!({"kind": "future"})).is_err());
    Ok(())
}

#[test]
fn project_snapshots_clear_future_icons() -> Result<(), serde_json::Error> {
    use serde_json::json;
    let mut value = json!({"id": "project", "title": "Project", "workspaceRoot": "/repo", "projectIcon": {"kind": "future"}, "defaultModelSelection": null, "scripts": [], "createdAt": "", "updatedAt": "", "deletedAt": null});
    let project: Project = serde_json::from_value(value.clone())?;
    value["projectIcon"] = json!(null);
    assert_eq!(serde_json::to_value(project)?, value);
    value.as_object_mut().expect("object").remove("deletedAt");
    value["projectIcon"] = json!({"kind": "future"});
    let shell: t3_contracts::orchestration_project::OrchestrationProjectShell =
        serde_json::from_value(value.clone())?;
    value["projectIcon"] = json!(null);
    assert_eq!(serde_json::to_value(shell)?, value);
    Ok(())
}

#[test]
fn mutations_normalize_legacy_selections_and_reject_invalid_sources()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let mut value = json!({"type": "project.create", "commandId": "command", "projectId": "project", "title": "Project", "workspaceRoot": "/repo", "defaultModelSelection": {"provider": "codex", "model": "model"}});
    let mutation: ProjectMutation = serde_json::from_value(value.clone())?;
    value["defaultModelSelection"] = json!({"instanceId": "codex", "model": "model"});
    assert_eq!(serde_json::to_value(mutation)?, value);
    for selection in [json!({"model": {}}), json!({"model": "model"})] {
        value["defaultModelSelection"] = selection;
        assert!(serde_json::from_value::<ProjectMutation>(value.clone()).is_err());
    }
    value["defaultModelSelection"] = json!(null);
    for blank in ["", " \t\n", "\u{feff}\u{a0}"] {
        for tag in ["project.create", "project.update"] {
            for key in ["title", "workspaceRoot"] {
                let mut invalid = value.clone();
                invalid["type"] = json!(tag);
                invalid[key] = json!(blank);
                assert!(serde_json::from_value::<ProjectMutation>(invalid).is_err());
            }
        }
        let script = json!({"id": "script", "name": "Build", "command": "build", "icon": "build", "runOnWorktreeCreate": false});
        for key in ["id", "name", "command", "previewUrl"] {
            let mut invalid = script.clone();
            invalid[key] = json!(blank);
            assert!(serde_json::from_value::<ProjectScript>(invalid).is_err());
        }
        let mut invalid = value.clone();
        invalid["type"] = json!("project.update");
        invalid["faviconPath"] = json!(blank);
        assert!(serde_json::from_value::<ProjectMutation>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn ignored_file_query_trims_and_enforces_utf16_length() -> Result<(), serde_json::Error> {
    use serde_json::json;
    for query in [json!(null), json!("x".repeat(257)), json!("😀".repeat(129))] {
        assert!(
            serde_json::from_value::<ProjectListIgnoredFilesInput>(
                json!({"cwd":"/repo", "query":query})
            )
            .is_err()
        );
    }
    let input: ProjectListIgnoredFilesInput = serde_json::from_value(
        json!({"cwd":"/repo", "query":format!("  {}  ", "😀".repeat(128))}),
    )?;
    assert_eq!(input.query.map(|query| query.0), Some("😀".repeat(128)));
    Ok(())
}
