mod common;
use common::{Repository, TestResult};
use serde_json::json;
use t3_contracts::{
    git::{VcsListRefsInput, VcsListRefsInputRefKind},
    review::*,
};
use t3_git::LocalStatusOptions;

fn preview(
    repo: &Repository,
    extra: serde_json::Value,
) -> Result<ReviewDiffPreviewInput, serde_json::Error> {
    let mut input = json!({"cwd":repo.root()});
    input
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    serde_json::from_value(input)
}

#[tokio::test]
async fn review_commit_list_caps_at_one_hundred_and_rejects_a_shallow_boundary() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("base", "base\n")?;
    repo.commit("initial").await?;
    let mut import = String::new();
    for index in 0..101 {
        let subject = format!("Feature {index}");
        import.push_str(&format!("commit refs/heads/feature\ncommitter Test <test@example.invalid> {} +0000\ndata {}\n{subject}\n",1_700_000_000+index,subject.len()));
        if index == 0 {
            import.push_str("from refs/heads/main\n");
        }
        import.push('\n');
    }
    repo.git
        .execute(
            &["fast-import", "--quiet"],
            t3_git::ExecuteOptions {
                stdin: Some(import.into_bytes()),
                ..Default::default()
            },
        )
        .await?;
    repo.git.switch_ref("feature").await?;
    let input: ReviewBranchCommitsInput =
        serde_json::from_value(json!({"cwd":repo.root(),"baseRef":"main"}))?;
    let commits = repo.git.review_branch_commits(&input).await?;
    assert_eq!(commits.commits.len(), 100);
    assert!(commits.truncated);
    assert_eq!(commits.commits[0].subject, "Feature 100");
    let shallow = repo.temp.path().join("shallow");
    let url = url::Url::from_directory_path(repo.root()).map_err(|_| "file URL")?;
    repo.command(&[
        "clone",
        "--depth=1",
        "--branch",
        "feature",
        url.as_str(),
        &shallow.to_string_lossy(),
    ])
    .await?;
    let sha = repo.git.at(&shallow).resolve_commit("HEAD").await?;
    let input: ReviewDiffPreviewInput =
        serde_json::from_value(json!({"cwd":shallow,"commitSha":sha}))?;
    let error = repo
        .git
        .review_diff_preview(&input)
        .await
        .expect_err("missing shallow parent");
    assert!(
        error
            .to_string()
            .contains("parent is not available locally")
    );
    Ok(())
}

#[tokio::test]
async fn status_combines_staged_and_unstaged_counts_and_reports_repository_relative_paths()
-> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "first\nsecond\n")?;
    repo.write("nested/keep", "keep\n")?;
    repo.commit("initial").await?;
    repo.write("a", "staged\nsecond\n")?;
    repo.command(&["add", "a"]).await?;
    repo.write("a", "working\nsecond\nthird\n")?;
    repo.write("nested/é space", "new\n")?;
    let status = repo
        .git
        .at(repo.root().join("nested"))
        .status_details_local(LocalStatusOptions::default())
        .await?;
    assert!(status.is_repo && status.has_working_tree_changes && status.is_default_branch);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.working_tree.insertions, 2);
    assert_eq!(status.working_tree.deletions, 1);
    assert!(
        status
            .working_tree
            .files
            .iter()
            .any(|file| file.path == "a" && file.insertions == 2 && file.deletions == 1)
    );
    assert!(
        status
            .working_tree
            .files
            .iter()
            .any(|file| file.path == "nested/é space")
    );
    assert!(
        status
            .working_tree
            .root_path
            .expect("root")
            .expect("value")
            .ends_with("repo")
    );
    Ok(())
}

#[tokio::test]
async fn unborn_status_and_review_include_staged_and_untracked_files_without_creating_real_index()
-> TestResult {
    let repo = Repository::new().await?;
    repo.write("tracked", "line one\nline two\n")?;
    repo.command(&["add", "tracked"]).await?;
    repo.write("untracked", "new\n")?;
    let before = repo.index()?;
    let status = repo
        .git
        .status_details_local(LocalStatusOptions {
            include_branch_changes: true,
            ..Default::default()
        })
        .await?;
    assert_eq!(status.working_tree.insertions, 2);
    assert_eq!(status.branch_changes.expect("totals").insertions, 3);
    let result = repo
        .git
        .review_diff_preview(&preview(&repo, json!({}))?)
        .await?;
    assert_eq!(result.sources.len(), 2);
    assert!(result.sources[0].diff.contains("+new"));
    assert_eq!(result.sources[0].files.as_ref().expect("files").len(), 2);
    assert_eq!(repo.index()?, before);
    Ok(())
}

#[tokio::test]
async fn changes_uses_merge_base_including_commits_working_edits_and_untracked_files() -> TestResult
{
    let repo = Repository::new().await?;
    repo.write("base", "base\n")?;
    repo.commit("initial").await?;
    repo.git.create_ref("feature", true).await?;
    repo.write("committed", "feature\n")?;
    repo.commit("feature").await?;
    repo.write("base", "working\n")?;
    repo.write("untracked", "new\n")?;
    let status = repo
        .git
        .status_details_local(LocalStatusOptions {
            include_branch_changes: true,
            ..Default::default()
        })
        .await?;
    assert_eq!(status.ahead_count, 1);
    assert_eq!(status.ahead_of_default_count, 1);
    let totals = status.branch_changes.expect("totals");
    assert_eq!(totals.base_ref.expect("base").0, "main");
    assert_eq!((totals.insertions, totals.deletions), (3, 1));
    let result = repo
        .git
        .review_diff_preview(&preview(&repo, json!({"metadataOnly":true}))?)
        .await?;
    let dirty = &result.sources[0];
    let changes = &result.sources[1];
    assert_eq!(dirty.files.as_ref().expect("files").len(), 2);
    assert_eq!(changes.files.as_ref().expect("files").len(), 3);
    assert!(dirty.diff.is_empty() && changes.diff.is_empty());
    assert_eq!(changes.patches_deferred, Some(true));
    let full = repo
        .git
        .review_diff_preview(&preview(&repo, json!({"sourceKind":"branch-range"}))?)
        .await?;
    assert_eq!(full.sources.len(), 1);
    assert!(full.sources[0].diff.contains("+feature"));
    assert!(full.sources[0].diff.contains("+working"));
    assert!(full.sources[0].diff.contains("+new"));
    Ok(())
}

#[tokio::test]
async fn review_keeps_literal_paths_binary_statistics_and_client_patch_format() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("old name", "one\ntwo\nthree\n")?;
    repo.write("binary", [0, 1, 2])?;
    repo.commit("initial").await?;
    repo.command(&["config", "core.splitIndex", "true"]).await?;
    repo.command(&["update-index", "--split-index"]).await?;
    std::fs::rename(repo.root().join("old name"), repo.root().join("new name"))?;
    repo.write("new name", "one\ntwo\nthree\nfour\n")?;
    repo.write("binary", [0, 2, 3])?;
    repo.write("[magic]", "literal\n")?;
    repo.write("m", "other\n")?;
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.suppressBlankEmpty", "true"),
        ("diff.submodule", "log"),
        ("diff.mnemonicPrefix", "true"),
    ] {
        repo.command(&["config", key, value]).await?;
    }
    let before = repo.index()?;
    let result = repo
        .git
        .review_diff_preview(&preview(&repo, json!({"sourceKind":"working-tree"}))?)
        .await?;
    let source = &result.sources[0];
    let files = source.files.as_ref().expect("manifest");
    assert!(
        files.iter().any(
            |file| file.path == "new name" && file.previous_path.as_deref() == Some("old name")
        )
    );
    assert!(files.iter().any(|file| file.path == "binary"));
    assert!(source.diff.contains("diff --git a/[magic] b/[magic]"));
    let file = repo
        .git
        .review_diff_preview(&preview(
            &repo,
            json!({"file":{"sourceKind":"working-tree","path":"[magic]","previousPath":null}}),
        )?)
        .await?;
    assert!(file.sources[0].diff.contains("literal"));
    assert!(!file.sources[0].diff.contains("+other"));
    assert_eq!(repo.index()?, before);
    Ok(())
}

#[tokio::test]
async fn review_statistics_survive_patch_truncation_and_hashes_are_stable() -> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    repo.write("large", "0123456789abcdef\n".repeat(12_000))?;
    repo.write("z-last", "last\n")?;
    let input = preview(&repo, json!({"sourceKind":"working-tree"}))?;
    let result = repo.git.review_diff_preview(&input).await?;
    let source = &result.sources[0];
    assert!(source.truncated);
    assert!(source.diff.len() <= 120_000);
    assert!(!source.diff.ends_with("[truncated]"));
    assert!(
        source
            .files
            .as_ref()
            .expect("manifest")
            .iter()
            .any(|file| file.path == "z-last")
    );
    let again = repo.git.review_diff_preview(&input).await?;
    assert_eq!(again.sources[0].diff_hash, source.diff_hash);
    Ok(())
}

#[tokio::test]
async fn commit_review_handles_root_and_merge_first_parent_and_rejects_invalid_sha() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "root\n")?;
    let root = repo.commit("Root").await?;
    let result = repo
        .git
        .review_diff_preview(&preview(&repo, json!({"commitSha":root}))?)
        .await?;
    assert_eq!(result.sources[0].kind, ReviewDiffPreviewSourceKind::Commit);
    assert!(result.sources[0].diff.contains("+root"));
    let parent = result.sources[0]
        .base_ref
        .as_ref()
        .expect("empty tree")
        .0
        .clone();
    let contents: ReviewDiffFileContentsInput = serde_json::from_value(
        json!({"cwd":repo.root(),"sourceKind":"commit","changeType":"new","baseRef":parent,"headRef":root,"oldPath":"a","newPath":"a"}),
    )?;
    assert_eq!(
        repo.git
            .review_diff_file_contents(&contents)
            .await?
            .new_contents,
        "root\n"
    );
    repo.git.create_ref("feature", true).await?;
    repo.write("feature", "feature\n")?;
    repo.commit("Feature").await?;
    repo.git.switch_ref("main").await?;
    repo.write("main", "main\n")?;
    repo.commit("Main").await?;
    let first_parent = repo.command(&["rev-parse", "HEAD"]).await?;
    repo.command(&["merge", "--no-ff", "-m", "Merge", "feature"])
        .await?;
    let merge = repo.command(&["rev-parse", "HEAD"]).await?;
    let result = repo
        .git
        .review_diff_preview(&preview(&repo, json!({"commitSha":merge.trim()}))?)
        .await?;
    assert_eq!(
        result.sources[0].base_ref.as_ref().expect("parent").0,
        first_parent.trim()
    );
    assert_eq!(result.sources[0].files.as_ref().expect("files").len(), 1);
    assert!(
        repo.git
            .review_diff_preview(&preview(&repo, json!({"commitSha":"HEAD"}))?)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn file_expansion_checks_binary_size_traversal_and_internal_state() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("a", "before\n")?;
    repo.commit("initial").await?;
    repo.write("a", "after\n")?;
    let input = |path: &str, kind: &str| {
        serde_json::from_value::<ReviewDiffFileContentsInput>(
            json!({"cwd":repo.root(),"sourceKind":"working-tree","changeType":kind,"baseRef":null,"headRef":null,"oldPath":path,"newPath":path}),
        )
    };
    assert!(
        repo.git
            .review_diff_file_contents(&input("a", "change")?)
            .await
            .is_err(),
        "unconfigured working-file access must fail closed"
    );
    let git = repo
        .git
        .clone()
        .with_internal_paths(repo.root().join("state"), repo.root().join("secrets"));
    let result = git
        .review_diff_file_contents(&input("a", "change")?)
        .await?;
    assert_eq!(result.old_contents, "before\n");
    assert_eq!(result.new_contents, "after\n");
    repo.write("binary", [0, 3])?;
    assert!(
        git.review_diff_file_contents(&input("binary", "new")?)
            .await
            .is_err()
    );
    repo.write("large", vec![b'a'; 1024 * 1024 + 1])?;
    assert!(
        git.review_diff_file_contents(&input("large", "new")?)
            .await
            .is_err()
    );
    assert!(
        git.review_diff_file_contents(&input("../outside", "new")?)
            .await
            .is_err()
    );
    repo.write("state/db.sqlite-wal", "private\n")?;
    let git = repo
        .git
        .clone()
        .with_internal_paths(repo.root().join("state"), repo.root().join("secrets"));
    assert!(
        git.review_diff_file_contents(&input("state/db.sqlite-wal", "new")?)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn branch_listing_paginates_filters_dedupes_origin_and_invalidates_after_mutations()
-> TestResult {
    let repo = Repository::new().await?;
    repo.commit("initial").await?;
    let remote = repo.temp.path().join("remote.git");
    repo.command(&["init", "--bare", &remote.to_string_lossy()])
        .await?;
    repo.command(&["remote", "add", "origin", &remote.to_string_lossy()])
        .await?;
    repo.command(&["push", "-u", "origin", "main"]).await?;
    repo.command(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ])
    .await?;
    let input = |extra: serde_json::Value| {
        let mut value = json!({"cwd":repo.root()});
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        serde_json::from_value::<VcsListRefsInput>(value)
    };
    let result = repo.git.list_branches(&input(json!({}))?).await?;
    assert_eq!(result.total_count, 1);
    assert!(result.refs[0].current && result.refs[0].is_default);
    let all = repo
        .git
        .list_branches(&input(json!({"includeMatchingRemoteRefs":true}))?)
        .await?;
    assert_eq!(all.total_count, 2);
    repo.git.create_ref("Feature/A", false).await?;
    let result = repo
        .git
        .list_branches(&input(json!({"query":"FEATURE","limit":1}))?)
        .await?;
    assert_eq!(result.total_count, 1);
    assert_eq!(result.refs[0].name.0, "Feature/A");
    let first = repo.git.list_branches(&input(json!({"limit":1}))?).await?;
    assert_eq!(first.next_cursor, Some(1));
    let next = repo
        .git
        .list_branches(&input(json!({"limit":1,"cursor":1}))?)
        .await?;
    assert_eq!(next.refs[0].name.0, "Feature/A");
    assert_eq!(next.next_cursor, None);
    let mut remote_only = input(json!({"includeMatchingRemoteRefs":true}))?;
    remote_only.ref_kind = Some(Some(VcsListRefsInputRefKind::Remote));
    assert_eq!(
        repo.git.list_branches(&remote_only).await?.refs[0].name.0,
        "origin/main"
    );
    repo.git
        .rename_branch("Feature/A", "feature/b", true)
        .await?;
    assert!(
        repo.git
            .list_branches(&input(json!({}))?)
            .await?
            .refs
            .iter()
            .any(|reference| reference.name.0 == "feature/b")
    );
    Ok(())
}
