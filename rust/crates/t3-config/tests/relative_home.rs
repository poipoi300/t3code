use std::path::Path;
use t3_config::{environment::resolve_worktree_home, home::resolve_dev_home};

// This integration-test process has one test: changing cwd cannot race other tests.
#[test]
fn relative_and_absolute_worktree_homes_override_shared_home() {
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let _restore = Restore(std::env::current_dir().unwrap());
    let root = temp.path().join("worktree");
    std::fs::create_dir_all(root.join("src/nested")).unwrap();
    std::fs::write(root.join(".git"), "gitdir: ../main/.git/worktrees/test\n").unwrap();
    std::env::set_current_dir(&root).unwrap();
    for cwd in [
        Path::new("src/nested"),
        Path::new("./src/nested"),
        Path::new("."),
        Path::new(""),
        &root.join("src/nested"),
        Path::new("src/../src/nested"),
    ] {
        let detected = resolve_worktree_home(cwd);
        assert_eq!(detected, Some(root.join(".t3")), "{}", cwd.display());
        assert_eq!(
            resolve_dev_home(
                None,
                detected.as_deref(),
                Some("shared"),
                temp.path(),
                &root
            )
            .0,
            root.join(".t3")
        );
    }
}
