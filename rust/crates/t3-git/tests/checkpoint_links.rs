mod common;
use common::{Repository, TestResult};
use std::path::{Path, PathBuf};
use t3_git::{ExecuteOptions, GitError, checkpoint_ref};

#[derive(Clone, Copy, Debug)]
enum LinkKind {
    #[cfg(windows)]
    Junction,
    Directory,
    File,
}
impl LinkKind {
    fn all() -> Vec<Self> {
        vec![
            #[cfg(windows)]
            Self::Junction,
            Self::Directory,
            Self::File,
        ]
    }
    async fn create(self, target: &Path, link: &Path) -> TestResult {
        std::fs::create_dir_all(link.parent().expect("link parent"))?;
        match self {
            #[cfg(windows)]
            Self::Junction => {
                let mut process = t3_process::ProcessOptions::new("cmd.exe");
                process.args = vec![
                    "/c".into(),
                    "mklink".into(),
                    "/J".into(),
                    link.to_string_lossy().replace('/', "\\").into(),
                    target.to_string_lossy().replace('/', "\\").into(),
                ];
                t3_process::run(process).await?;
            }
            #[cfg(windows)]
            Self::Directory => std::os::windows::fs::symlink_dir(target, link)?,
            #[cfg(windows)]
            Self::File => std::os::windows::fs::symlink_file(target.join("marker"), link)?,
            #[cfg(unix)]
            Self::Directory => std::os::unix::fs::symlink(target, link)?,
            #[cfg(unix)]
            Self::File => std::os::unix::fs::symlink(target.join("marker"), link)?,
        }
        Ok(())
    }
    fn remove(self, link: &Path) -> std::io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Junction | Self::Directory => std::fs::remove_dir(link),
            _ => std::fs::remove_file(link),
        }
    }
}
fn outside(repo: &Repository) -> Result<PathBuf, std::io::Error> {
    let outside = repo.temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(
        outside.join("marker"),
        b"external marker must never be hashed\0\xff",
    )?;
    Ok(outside)
}
async fn assert_marker_not_hashed(repo: &Repository, outside: &Path) -> TestResult {
    let marker = std::fs::read(outside.join("marker"))?;
    let oid = repo
        .git
        .execute(
            &["hash-object", "--stdin"],
            ExecuteOptions {
                stdin: Some(marker),
                ..Default::default()
            },
        )
        .await?
        .stdout_text();
    let object = repo
        .git
        .execute(
            &["cat-file", "-e", oid.trim()],
            ExecuteOptions {
                allow_nonzero: true,
                ..Default::default()
            },
        )
        .await?;
    assert_ne!(
        object.code,
        Some(0),
        "outside marker was read into the object store"
    );
    Ok(())
}
async fn assert_safe_capture(
    repo: &Repository,
    git: &t3_git::Git,
    reference: &str,
    link: &str,
    outside: &Path,
    restore_allowed: bool,
) -> TestResult {
    let index = repo.index()?;
    git.capture_checkpoint(reference).await?;
    let tree = repo
        .command(&["ls-tree", "-r", "--name-only", reference])
        .await?;
    assert!(
        !tree.lines().any(|path| path.eq_ignore_ascii_case(link)
            || path
                .to_lowercase()
                .starts_with(&format!("{}/", link.to_lowercase()))),
        "unsafe tree: {tree}"
    );
    assert_eq!(repo.show(reference, "safe").await?, "base\n");
    assert_marker_not_hashed(repo, outside).await?;
    let marker = std::fs::read(outside.join("marker"))?;
    std::fs::write(outside.join("later"), b"preserve later\0")?;
    let result = git.restore_checkpoint(reference, false).await;
    if restore_allowed {
        assert!(matches!(result, Ok(true)), "{result:?}");
    } else {
        assert!(
            matches!(result, Err(GitError::UnsafeRestore(_))),
            "{result:?}"
        );
        assert_eq!(repo.index()?, index);
    }
    assert_eq!(std::fs::read(outside.join("marker"))?, marker);
    assert_eq!(std::fs::read(outside.join("later"))?, b"preserve later\0");
    assert_eq!(std::fs::read_dir(outside)?.count(), 2);
    Ok(())
}

#[tokio::test]
async fn ignored_links_use_all_git_ignore_sources_without_exposing_targets() -> TestResult {
    for kind in LinkKind::all() {
        for (layout, link, ignore, rule) in [
            ("top", "node_modules", ".gitignore", "node_modules\n"),
            ("nested", "pkg/node_modules", ".gitignore", "node_modules\n"),
            ("parent", "vendor/cache/link", ".gitignore", "vendor/\n"),
            (
                "nested-ignore",
                "pkg/cache/link",
                "pkg/.gitignore",
                "cache/\n",
            ),
            ("info", "private/link", ".git/info/exclude", "private/\n"),
            ("global", "global/link", "", "global/\n"),
        ] {
            let repo = Repository::new().await?;
            repo.write("safe", "base\n")?;
            repo.commit("base").await?;
            let mut git = repo.git.clone();
            if ignore.is_empty() {
                let excludes = repo.temp.path().join("global-excludes");
                let config = repo.temp.path().join("global-config");
                std::fs::write(&excludes, rule)?;
                std::fs::write(
                    &config,
                    format!(
                        "[core]\n excludesFile = {}\n",
                        excludes.to_string_lossy().replace('\\', "/")
                    ),
                )?;
                git = git.with_environment(
                    [("GIT_CONFIG_GLOBAL".into(), Some(config.into_os_string()))]
                        .into_iter()
                        .collect(),
                );
            } else {
                repo.write(ignore, rule)?;
            }
            let target = outside(&repo)?;
            kind.create(&target, &repo.root().join(link)).await?;
            let result =
                assert_safe_capture(&repo, &git, &checkpoint_ref(layout, 0), link, &target, true)
                    .await;
            assert!(result.is_ok(), "{kind:?} {layout}: {result:?}");
            let cleanup = kind.remove(&repo.root().join(link));
            // Git may safely unlink a directory symlink while cleaning its empty parent.
            assert!(
                cleanup.is_ok()
                    || cleanup
                        .as_ref()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
                "{kind:?} {layout}: {cleanup:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn restore_refuses_an_untracked_nonignored_external_link() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("safe", "base\n")?;
    repo.commit("base").await?;
    let reference = checkpoint_ref("visible-unsafe", 0);
    repo.git.capture_checkpoint(&reference).await?;
    let target = outside(&repo)?;
    #[cfg(windows)]
    let kind = LinkKind::Junction;
    #[cfg(unix)]
    let kind = LinkKind::Directory;
    for name in ["visible-link", "nested/visible-link"] {
        let link = repo.root().join(name);
        kind.create(&target, &link).await?;
        let result = repo.git.restore_checkpoint(&reference, false).await;
        let cleanup = kind.remove(&link);
        if cleanup
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        {
            cleanup?;
        }
        #[cfg(windows)]
        assert!(
            matches!(result, Err(GitError::UnsafeRestore(_))),
            "{name}: {result:?}"
        );
        #[cfg(unix)]
        assert!(matches!(result, Ok(true)), "{name}: {result:?}");
    }
    assert_eq!(
        std::fs::read(target.join("marker"))?,
        b"external marker must never be hashed\0\xff"
    );
    Ok(())
}

#[tokio::test]
async fn capture_preserves_deletions_literals_and_nested_repositories() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("safe", "base\n")?;
    repo.write("gone", "delete\n")?;
    repo.write("tracked-ignored", "before\n")?;
    repo.commit("base").await?;
    repo.write(
        ".gitignore",
        "node_modules\ntracked-ignored\nignored-file\n",
    )?;
    repo.write("ignored-file", "do not add\n")?;
    repo.write("tracked-ignored", "after\n")?;
    std::fs::remove_file(repo.root().join("gone"))?;
    for path in ["[literal]", "-leading", "with spaces", "é"] {
        repo.write(path, "literal content\n")?;
    }
    repo.command(&["init", "empty nested"]).await?;
    repo.command(&["init", "empty nested/deeper repo"]).await?;
    repo.command(&["init", "born nested"]).await?;
    repo.command(&["init", "born nested/deeper born"]).await?;
    let born = repo.git.at(repo.root().join("born nested"));
    born.execute(
        &["commit", "--allow-empty", "-m", "nested"],
        ExecuteOptions::default(),
    )
    .await?;
    let deeper = repo.git.at(repo.root().join("born nested/deeper born"));
    deeper
        .execute(
            &["commit", "--allow-empty", "-m", "deeper"],
            ExecuteOptions::default(),
        )
        .await?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file("safe", repo.root().join("safe-alias"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("safe", repo.root().join("safe-alias"))?;
    repo.command(&["config", "core.symlinks", "true"]).await?;
    let target = outside(&repo)?;
    let kind = LinkKind::all()[0];
    let link = repo.root().join("node_modules");
    kind.create(&target, &link).await?;
    let reference = checkpoint_ref("safe-file-semantics", 0);
    let result =
        assert_safe_capture(&repo, &repo.git, &reference, "node_modules", &target, true).await;
    kind.remove(&link)?;
    result?;
    for path in ["[literal]", "-leading", "with spaces", "é"] {
        assert_eq!(repo.show(&reference, path).await?, "literal content\n");
    }
    assert_eq!(repo.show(&reference, "tracked-ignored").await?, "after\n");
    assert!(
        repo.command(&["ls-tree", &reference, "--", "safe-alias"])
            .await?
            .starts_with("120000 blob ")
    );
    assert!(
        repo.command(&[
            "ls-tree",
            &reference,
            "--",
            "gone",
            "ignored-file",
            "empty nested"
        ])
        .await?
        .is_empty()
    );
    assert!(
        repo.command(&["ls-tree", &reference, "--", "born nested"])
            .await?
            .starts_with("160000 commit ")
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn capture_keeps_backslashes_and_newlines_in_file_names() -> TestResult {
    let repo = Repository::new().await?;
    repo.write("safe", "base\n")?;
    repo.commit("base").await?;
    repo.write(".gitignore", "link\n")?;
    let path = "back\\slash\nname";
    repo.write(path, "literal\n")?;
    let target = outside(&repo)?;
    LinkKind::Directory
        .create(&target, &repo.root().join("link"))
        .await?;
    let reference = checkpoint_ref("posix-paths", 0);
    repo.git.capture_checkpoint(&reference).await?;
    assert_eq!(repo.show(&reference, path).await?, "literal\n");
    Ok(())
}
