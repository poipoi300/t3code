use crate::{Error, ProjectService, Result};
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{base_schemas::ProjectId, project::*};
use t3_git::{ExecuteOptions, Git};
use tokio::sync::Mutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub fn scratch_folder_root(base_dir: &Path) -> PathBuf {
    base_dir.join("scratch")
}

/// Required workspace extension: these operations are not exported by t3-workspace yet.
#[async_trait]
pub trait ManagedFolderIo: Send + Sync {
    async fn create_directory(&self, path: &Path, recursive: bool) -> Result<bool>;
    async fn write_file(&self, path: &Path, contents: &str) -> Result<()>;
    /// Remove only this claimed directory, without following directory links/reparse points.
    async fn remove_directory(&self, path: &Path) -> Result<()>;
}
#[derive(Clone)]
pub struct ManagedProjectFolders {
    pub base_dir: PathBuf,
    pub named_projects_root: PathBuf,
    projects: ProjectService,
    io: Arc<dyn ManagedFolderIo>,
    probe: Arc<Mutex<Option<Option<PathBuf>>>>,
    git_environment: t3_process::Environment,
    tasks: TaskTracker,
    stop: CancellationToken,
}
impl ManagedProjectFolders {
    pub fn new(
        base_dir: PathBuf,
        projects: ProjectService,
        io: Arc<dyn ManagedFolderIo>,
        git_environment: t3_process::Environment,
    ) -> Self {
        Self {
            named_projects_root: base_dir.join("projects"),
            base_dir,
            projects,
            io,
            probe: Default::default(),
            git_environment,
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        }
    }
    fn git(&self, cwd: &Path) -> Git {
        self.projects
            .git()
            .at(cwd)
            .with_environment(self.git_environment.clone())
    }
    pub async fn scratch_root(&self) -> Option<PathBuf> {
        let mut probe = self.probe.lock().await;
        if probe.is_none() {
            let result = self.git(&self.base_dir).detect_repository().await;
            let available = matches!(
                result,
                Ok(None)
                    | Err(t3_git::GitError::Process(
                        t3_process::ProcessError::NotFound { .. }
                    ))
            );
            *probe = Some(available.then(|| scratch_folder_root(&self.base_dir)));
        }
        let root = probe.as_ref()?.clone()?;
        drop(probe);
        if let Ok(real) = t3_workspace::paths::canonicalize(&root)
            && real != root
            && self
                .projects
                .get_by_workspace_root(&real.to_string_lossy(), false)
                .await
                .ok()
                .flatten()
                .is_some()
        {
            return Some(real);
        }
        Some(root)
    }
    pub async fn is_scratch_project(&self, id: &ProjectId) -> bool {
        let Some(root) = self.scratch_root().await else {
            return false;
        };
        self.projects
            .get_by_id(id, false)
            .await
            .ok()
            .flatten()
            .is_some_and(|p| Path::new(&p.workspace_root.0) == root)
    }
    pub async fn ensure_scratch(&self) -> Result<ProjectEnsureScratchResult> {
        let root = self.scratch_root().await.ok_or(Error::Unavailable(
            "Threads without a project are not available on this environment.",
        ))?;
        self.io.create_directory(&root, true).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let input = serde_json::from_value(
            serde_json::json!({"type":"project.create","commandId":format!("scratch-project:{id}"),"projectId":id,"title":"No project","workspaceRoot":root}),
        )?;
        let (project_id, created) = match self.projects.bootstrap(input).await {
            Ok((project, created)) => (project.id, created),
            Err(Error::Conflict {
                conflicting_project_id,
                ..
            }) => (ProjectId(conflicting_project_id), false),
            Err(e) => return Err(e),
        };
        if created {
            self.projects.update(serde_json::from_value(serde_json::json!({"type":"project.update","commandId":format!("scratch-project-icon:{id}"),"projectId":project_id,"projectIcon":{"kind":"lucide","name":"message-square-dashed","color":"gray"}}))?).await?;
        }
        Ok(ProjectEnsureScratchResult { project_id })
    }
    pub async fn folder_for_thread(
        &self,
        project: &ProjectId,
        thread: &t3_contracts::base_schemas::ThreadId,
        text: &str,
    ) -> Result<Option<PathBuf>> {
        if !self.is_scratch_project(project).await {
            return Ok(None);
        }
        let Some(root) = self.scratch_root().await else {
            return Ok(None);
        };
        self.io.create_directory(&root, true).await?;
        let words = folder_words(text);
        let now = self.projects.now();
        let parsed = chrono::DateTime::parse_from_rfc3339(&now.0)
            .map_err(|_| Error::Invalid("Invalid project clock timestamp".into()))?;
        let date = parsed.to_utc().format("%Y-%m-%d").to_string();
        let id: String = thread
            .0
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let path_for = |id: &str| {
            root.join(
                [date.as_str(), &words, id]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("-"),
            )
        };
        for attempt in 0.. {
            let path = match attempt {
                0 => path_for(&id[id.len().saturating_sub(8)..]),
                1 => path_for(&id),
                _ => PathBuf::from(format!(
                    "{}-{}",
                    path_for(&id).display(),
                    &uuid::Uuid::new_v4().to_string()[..8]
                )),
            };
            if self.io.create_directory(&path, false).await? {
                return Ok(Some(path));
            }
        }
        unreachable!()
    }
    pub async fn create_new(&self, input: ProjectCreateNewInput) -> Result<ProjectCreateNewResult> {
        if self.stop.is_cancelled() {
            return Err(Error::Unavailable("ManagedProjectFolders is shut down"));
        }
        let cancel = self.stop.child_token();
        struct CancelOnDrop(CancellationToken);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        let this = self.clone();
        self.tasks
            .spawn(async move { this.create_new_inner(input, cancel).await })
            .await?
    }
    async fn create_new_inner(
        &self,
        input: ProjectCreateNewInput,
        cancel: CancellationToken,
    ) -> Result<ProjectCreateNewResult> {
        self.io
            .create_directory(&self.named_projects_root, true)
            .await?;
        let slug = folder_name(&input.name.0);
        let mut claimed = None;
        for attempt in 1..=100 {
            let name = if attempt == 1 {
                slug.clone()
            } else {
                format!("{slug}-{attempt}")
            };
            let path = self.named_projects_root.join(name);
            if self.io.create_directory(&path, false).await? {
                claimed = Some(path);
                break;
            }
        }
        let root = claimed
            .ok_or_else(|| Error::Invalid(format!("Every folder name for \"{slug}\" is taken.")))?;
        let prepared = tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(Error::Cancelled),
            result=self.scaffold(&root,&input.name.0,cancel.clone())=>result,
        };
        let commit_error = match prepared {
            Ok(e) => e,
            Err(e) => {
                self.cleanup_unowned(&root).await;
                return Err(e);
            }
        };
        let id = uuid::Uuid::new_v4().to_string();
        let created=self.projects.create(serde_json::from_value(serde_json::json!({"type":"project.create","commandId":format!("named-project:{id}"),"projectId":id,"title":input.name,"workspaceRoot":root}))?).await;
        match created {
            Ok(project) => Ok(ProjectCreateNewResult {
                project_id: project.id,
                workspace_root: project.workspace_root,
                commit_error: commit_error.map(Into::into),
            }),
            Err(error) => {
                if !matches!(error, Error::Conflict { .. }) {
                    self.cleanup_unowned(&root).await;
                }
                Err(error)
            }
        }
    }
    pub async fn cleanup_unowned(&self, path: &Path) {
        let inspect = async {
            let parent = t3_workspace::paths::canonicalize(&self.named_projects_root)
                .map_err(|_| Error::Invalid("Cannot inspect managed root".into()))?;
            let real = t3_workspace::paths::canonicalize(path)
                .map_err(|_| Error::Invalid("Cannot inspect managed folder".into()))?;
            if !t3_workspace::paths::within(&real, &parent)
                || real == parent
                || real.parent() != Some(parent.as_path())
            {
                return Err(Error::Invalid(
                    "Managed folder resolves outside its parent".into(),
                ));
            }
            if self
                .projects
                .get_by_workspace_root(&path.to_string_lossy(), false)
                .await?
                .is_none()
            {
                self.io.remove_directory(path).await?;
            }
            Ok::<_, Error>(())
        }
        .await;
        let _ = inspect;
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
    async fn scaffold(
        &self,
        root: &Path,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<Option<String>> {
        let git = self.git(root).with_cancellation(cancel);
        let config = git
            .execute(
                &["config", "--get", "init.defaultBranch"],
                ExecuteOptions {
                    allow_nonzero: true,
                    ..Default::default()
                },
            )
            .await;
        let branch = config
            .ok()
            .filter(|r| r.code == Some(0))
            .map(|r| r.stdout_text().trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or("main".into());
        let short = || ExecuteOptions {
            timeout: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        git.execute(&["init", &format!("--initial-branch={branch}")], short())
            .await?;
        self.io.write_file(&root.join("README.md"),&format!("<img src=\"assets/icon.svg\" width=\"64\" height=\"64\" alt=\"\">\n\n# {name}\n\nCreated in [T3 Code](https://t3.codes).\n")).await?;
        self.io
            .create_directory(&root.join("assets"), false)
            .await?;
        self.io
            .write_file(&root.join("assets/icon.svg"), &named_icon_svg(name))
            .await?;
        git.execute(
            &[
                "-c",
                "core.safecrlf=false",
                "add",
                "--force",
                "--",
                "README.md",
                "assets/icon.svg",
            ],
            short(),
        )
        .await?;
        let commit = git
            .execute(
                &["commit", "--message", "Initial commit"],
                ExecuteOptions {
                    allow_nonzero: true,
                    timeout: Some(Duration::from_secs(30)),
                    ..Default::default()
                },
            )
            .await;
        Ok(match commit {
            Ok(r) if r.code == Some(0) => None,
            Ok(r) => Some(describe_commit_failure(&r.stderr_text())),
            Err(e) => Some(e.to_string()),
        })
    }
}
pub fn folder_words(text: &str) -> String {
    let words = text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    words
        .chars()
        .take(48)
        .collect::<String>()
        .trim_end_matches('-')
        .into()
}
pub fn folder_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let decomposed: String = name
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect();
    let mut slug = decomposed
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug.truncate(slug.len().min(64));
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        return "project".into();
    }
    let reserved = ["con", "prn", "aux", "nul"].contains(&slug)
        || ["com", "lpt"].into_iter().any(|p| {
            slug.strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
        });
    if reserved {
        format!("{slug}-project")
    } else {
        slug.into()
    }
}
fn named_icon_svg(name: &str) -> String {
    const COLORS: [&str; 15] = [
        "#dc2626", "#ea580c", "#d97706", "#16a34a", "#059669", "#0d9488", "#0891b2", "#0284c7",
        "#2563eb", "#4f46e5", "#7c3aed", "#9333ea", "#c026d3", "#db2777", "#e11d48",
    ];
    let initials = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(2)
        .filter_map(|s| s.chars().next())
        .collect::<String>()
        .to_uppercase();
    let initials = if initials.is_empty() {
        name.trim().chars().next().unwrap_or('?').to_string()
    } else {
        initials
    };
    let font = if initials.encode_utf16().count() > 1 {
        26
    } else {
        32
    };
    let hash = name
        .chars()
        .fold(0u32, |hash, c| hash.wrapping_mul(31).wrapping_add(c as u32));
    let initials = initials
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">\n  <rect width=\"64\" height=\"64\" rx=\"14\" fill=\"{}\"/>\n  <text x=\"32\" y=\"32\" dy=\"0.35em\" text-anchor=\"middle\" font-family=\"ui-sans-serif, system-ui, -apple-system, sans-serif\" font-size=\"{font}\" font-weight=\"600\" fill=\"#ffffff\">{initials}</text>\n</svg>\n",
        COLORS[hash as usize % COLORS.len()]
    )
}
fn describe_commit_failure(stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if [
        "identity unknown",
        "tell me who you are",
        "no name was given",
        "no email was given",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        return "Git has no name or email on this machine. Set user.name and user.email, then commit.".into();
    }
    stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("Git could not make the first commit.")
        .into()
}
