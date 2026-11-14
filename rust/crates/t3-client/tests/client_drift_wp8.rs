use serde_json::json;
use t3_client::workspace::{
    branch::*,
    ignored_files::{self, FileSelection},
    projects::*,
};
use t3_contracts::{
    environment::ExecutionEnvironmentCapabilities, git::VcsRef, project::ProjectIgnoredFile,
};

fn reference(name: &str, default: bool, current: bool, path: Option<&str>) -> VcsRef {
    serde_json::from_value(
        json!({"name":name,"isDefault":default,"current":current,"worktreePath":path}),
    )
    .expect("ref")
}
#[test]
fn clone_bases_only_use_listed_default_or_current_refs() {
    assert_eq!(default_worktree_base_branch(&[]), None);
    assert_eq!(
        default_worktree_base_branch(&[
            reference("feature", false, true, None),
            reference("staging", true, false, None)
        ]),
        Some("staging")
    );
    assert_eq!(
        default_worktree_base_branch(&[
            reference("other", false, false, None),
            reference("feature", false, true, None)
        ]),
        Some("feature")
    );
}
#[test]
fn stale_branch_metadata_cannot_skip_an_active_checkout() {
    for (path, active, expected_path, stays) in [
        (
            Some("/secondary"),
            Some("/current"),
            Some("/secondary"),
            false,
        ),
        (Some("/repo"), None, None, true),
        (Some("/repo"), Some("/current"), None, false),
        (None, Some("/current"), Some("/current"), true),
    ] {
        let target = branch_selection_target(
            "/repo",
            Some("/repo"),
            active,
            &reference("feature", false, false, path),
        );
        assert_eq!(target.next_worktree_path.as_deref(), expected_path);
        assert_eq!(target.stays_in_active_checkout, stays);
        assert_eq!(target.reuse_existing_worktree, path.is_some());
    }
}
#[test]
fn ignored_file_search_negotiates_separately_and_bounds_the_query() {
    for (listing, search, query, expected) in [
        (true, Some(true), "  z-last.env  ", Some("z-last.env")),
        (true, None, "z-last.env", None),
        (true, Some(false), "z-last.env", None),
        (true, Some(true), "   ", None),
    ] {
        let capabilities: ExecutionEnvironmentCapabilities = serde_json::from_value(json!({"repositoryIdentity":true,"ignoredFileListing":listing,"ignoredFileSearch":search.unwrap_or(false)})).expect("capabilities");
        let input = ignored_files::request("/repo", query, &capabilities).expect("listing");
        assert_eq!(input.query.as_ref().map(|query| query.0.as_str()), expected);
    }
    let old: ExecutionEnvironmentCapabilities =
        serde_json::from_value(json!({"ignoredFileListing":true})).expect("old server");
    assert!(
        serde_json::to_value(ignored_files::request("/repo", "query", &old).expect("fallback"))
            .expect("wire")
            .get("query")
            .is_none()
    );
    let capable: ExecutionEnvironmentCapabilities =
        serde_json::from_value(json!({"ignoredFileListing":true,"ignoredFileSearch":true}))
            .expect("new server");
    let input = ignored_files::request("/repo", &format!("  {}😀  ", "a".repeat(255)), &capable)
        .expect("search");
    assert_eq!(input.query.expect("query").0, "a".repeat(255));
    let unavailable: ExecutionEnvironmentCapabilities =
        serde_json::from_value(json!({})).expect("unsupported");
    assert!(ignored_files::request("/repo", "query", &unavailable).is_none());
}
#[test]
fn ignored_file_picks_survive_query_changes_with_one_copy_ceiling() {
    let file = |path: String| {
        serde_json::from_value::<ProjectIgnoredFile>(json!({"path":path,"byteLength":4}))
            .expect("file")
    };
    let first = file("z-last.env".into());
    let second = file("a-other.env".into());
    let mut selected = FileSelection::default();
    selected.select(&first, true);
    selected.select(&second, true);
    assert_eq!(selected.files(), [first.clone(), second.clone()]);
    for index in 0..300 {
        selected.select(&file(format!("file-{index}")), true);
    }
    assert_eq!(
        selected.files().len(),
        ignored_files::COPIED_FILES_MAX_COUNT
    );
    selected.toggle_shown(&[first.clone(), second.clone()]);
    assert_eq!(selected.files().len(), 198);
    selected.toggle_shown(&[first.clone(), second.clone()]);
    assert_eq!(selected.files().len(), 200);
    selected.select(&first, false);
    assert_eq!(selected.files().len(), 199);
}
#[test]
fn project_filters_resolve_legacy_scratch_and_drop_archived_or_unknown_scopes() {
    let mut scratch = EnvironmentProject {environment:t3_client::types::EnvironmentId("local".into()),is_scratch:true,project:serde_json::from_value(json!({"id":"scratch","title":"Scratch","workspaceRoot":"/scratch","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"})).expect("project")};
    let projects = [scratch.clone()];
    let groups = build_project_groups(&projects, &GroupingSettings::default(), None, true);
    assert_eq!(
        home_project_filter_scope(
            &groups,
            &projects,
            Some(LogicalProjectKey::Physical(scratch.physical_key())),
            GroupingMode::Repository
        )
        .map(|group| &group.key),
        Some(&LogicalProjectKey::Scratch)
    );
    assert!(
        home_project_filter_scope(
            &groups,
            &projects,
            Some(LogicalProjectKey::Repository("removed".into())),
            GroupingMode::Repository
        )
        .is_none()
    );
    scratch.project.archived_at = Some(Some(scratch.project.created_at.clone()));
    let projects = [scratch];
    let groups = build_project_groups(&projects, &GroupingSettings::default(), None, true);
    assert!(
        home_project_filter_scope(
            &groups,
            &projects,
            Some(LogicalProjectKey::Scratch),
            GroupingMode::Repository
        )
        .is_none()
    );
}
