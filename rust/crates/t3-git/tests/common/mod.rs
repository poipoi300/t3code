#![allow(dead_code)]
use std::path::Path;
use t3_git::{ExecuteOptions, Git, OutputMode};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub struct Repository {
    pub temp: tempfile::TempDir,
    pub git: Git,
}
impl Repository {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        std::fs::create_dir(&home)?;
        let root = temp.path().join("repo");
        std::fs::create_dir(&root)?;
        let mut env = t3_process::Environment::new();
        for (key, value) in [
            ("HOME", home.to_string_lossy().into_owned()),
            ("USERPROFILE", home.to_string_lossy().into_owned()),
            ("XDG_CONFIG_HOME", home.to_string_lossy().into_owned()),
            (
                "GIT_CONFIG_GLOBAL",
                home.join("no-global-config").to_string_lossy().into_owned(),
            ),
            (
                "GIT_CONFIG_SYSTEM",
                home.join("no-system-config").to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("GIT_ATTR_NOSYSTEM", "1".into()),
            ("GIT_CONFIG_COUNT", "0".into()),
            ("GIT_AUTHOR_NAME", "Test".into()),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid".into()),
            ("GIT_COMMITTER_NAME", "Test".into()),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid".into()),
        ] {
            env.insert(key.into(), Some(value.into()));
        }
        for key in ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG", "GIT_TEMPLATE_DIR"] {
            env.insert(key.into(), None);
        }
        let git = Git::new(root).with_environment(env);
        let repository = Self { temp, git };
        repository
            .command(&["init", "--initial-branch=main"])
            .await?;
        repository
            .command(&["config", "core.autocrlf", "false"])
            .await?;
        Ok(repository)
    }
    pub fn root(&self) -> &Path {
        self.git.cwd()
    }
    pub fn write(&self, path: &str, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
        let path = self.root().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }
    pub async fn command(&self, args: &[&str]) -> Result<String, t3_git::GitError> {
        Ok(self
            .git
            .execute(
                args,
                ExecuteOptions {
                    max_output_bytes: 32 * 1024 * 1024,
                    output_mode: OutputMode::Error,
                    ..Default::default()
                },
            )
            .await?
            .stdout_text())
    }
    pub async fn commit(&self, message: &str) -> Result<String, t3_git::GitError> {
        self.command(&["add", "."]).await?;
        self.command(&["commit", "--allow-empty", "-m", message])
            .await?;
        Ok(self.command(&["rev-parse", "HEAD"]).await?.trim().into())
    }
    pub async fn show(&self, reference: &str, path: &str) -> Result<String, t3_git::GitError> {
        self.command(&["show", &format!("{reference}:{path}")])
            .await
    }
    pub fn index(&self) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.root().join(".git/index"))
    }
}
