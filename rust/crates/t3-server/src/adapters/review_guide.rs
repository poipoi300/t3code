//! The production review-guide generator (`PullRequestReviewGuideService.ts`): TypeScript's
//! enablement and model policy, an isolated checkout of the pull request head, and generation
//! through the text-generation adapter (a coding agent that investigates read-only).
//!
//! The checkout never runs repository code and leaves nothing behind: host-provided branch
//! names pass `git check-ref-format --branch` and carry no leading `+` and no `:`; fetched refs
//! land under `refs/t3/review-guide/<runId>/`; the detached worktree is added with
//! `core.hooksPath` pointed at an empty server-owned directory (t3-git's disabled-hooks path);
//! the worktree, its folder and the refs are force-removed after success, failure and
//! cancellation.
use crate::adapters::{git::GitAdapter, textgen::TextGenAdapter};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use t3_contracts::{
    base_schemas::ProjectId, model_selection::ModelSelection, pull_request::PullRequestDetail,
    pull_request_review_guide::PullRequestReviewGuideStop,
};
use t3_git::{ExecuteOptions, Git, RepositoryBinding};
use t3_pull_request::{
    Error, PullRequestService, Result,
    context::Context,
    guide::{GuideGenerator, PreparedGuide, parse_review_guide_change},
};
use t3_textgen::{
    CancellationToken,
    generation::{Generated, GenerationContext, GenerationInput},
    prompts::{ReviewInput, ReviewLayer},
};

const OPERATION: &str = "generateReviewGuide";
const SETTLED_REMARK_CHARS: usize = 200;
const REF_ROOT: &str = "refs/t3/review-guide/";
const DIRECTORY_PREFIX: &str = "review-guide-";
const BINDING_FILE: &str = "repository.json";

pub struct ReviewGuideGenerator {
    textgen: Arc<TextGenAdapter>,
    git: Arc<GitAdapter>,
    projects: t3_project::ProjectService,
    /// `config.worktreesDir`: the managed worktree folder that holds the scratch checkouts.
    worktrees: PathBuf,
    service: OnceLock<PullRequestService>,
    /// Test seam: each finished cleanup's run id (`StartOptions::review_guide_cleanups`).
    cleanups: OnceLock<tokio::sync::mpsc::UnboundedSender<String>>,
    runs: Mutex<HashMap<String, Run>>,
    startup: tokio::sync::OnceCell<()>,
    /// At most `CONCURRENT_RUNS` checkouts and agents at once; a run holds its slot from
    /// `prepare` until its cleanup, and later runs wait for one.
    slots: Arc<tokio::sync::Semaphore>,
}

/// Each run is a full checkout plus an agent allowed 900 s (review L2).
const CONCURRENT_RUNS: usize = 2;

/// What a run created, so `cleanup` (or a failed `prepare`) can remove exactly that.
#[derive(Default)]
struct Run {
    git: Option<Git>,
    refs: Vec<String>,
    /// `<worktrees>/review-guide-<runId>`, holding the journal and checkout.
    directory: Option<PathBuf>,
    git_attr_source: Option<String>,
    project: Option<ProjectId>,
    selection: Option<ModelSelection>,
    instructions: Option<String>,
    stack: Vec<ReviewLayer>,
    slot: Option<tokio::sync::OwnedSemaphorePermit>,
}

fn failed(detail: &str) -> Error {
    Error::invalid(OPERATION, detail)
}

impl ReviewGuideGenerator {
    pub fn new(
        context: &crate::services::DomainContext<'_>,
        textgen: Arc<TextGenAdapter>,
        git: Arc<GitAdapter>,
        projects: t3_project::ProjectService,
    ) -> Arc<Self> {
        let worktrees = context.local.config.paths.worktrees();
        let worktrees = t3_workspace::paths::canonicalize(&worktrees).unwrap_or(worktrees);
        let generator = Arc::new(Self {
            textgen,
            git,
            projects,
            worktrees,
            service: OnceLock::new(),
            cleanups: OnceLock::new(),
            runs: Mutex::new(HashMap::new()),
            startup: tokio::sync::OnceCell::new(),
            slots: Arc::new(tokio::sync::Semaphore::new(CONCURRENT_RUNS)),
        });
        let boot = generator.clone();
        tokio::spawn(async move { boot.finish_startup().await });
        generator
    }

    /// Reports every finished cleanup (tests wait on it instead of polling).
    pub fn report_cleanups(&self, sender: tokio::sync::mpsc::UnboundedSender<String>) {
        let _ = self.cleanups.set(sender);
    }

    /// The pull-request service (activity, stack); bound once it has started.
    pub fn bind(&self, service: PullRequestService) {
        let _ = self.service.set(service);
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut Run)) {
        if let Some(run) = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(id)
        {
            change(run);
        }
    }

    /// Force-removes the run's worktree and its folder, and deletes its refs.
    async fn remove(&self, id: &str) {
        let Some(run) = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        else {
            return;
        };
        // Cleanup also runs while the server shuts down. Keep the prepare-time identity
        // while replacing the cancelled token with one bounded by the command timeouts.
        if let (Some(git), Some(directory)) = (run.git, run.directory) {
            let git = git.with_cancellation(CancellationToken::new());
            finish_checkout(&git, &directory, &run.refs).await;
        }
        if let Some(sender) = self.cleanups.get() {
            let _ = sender.send(id.to_owned());
        }
    }

    async fn finish_startup(&self) {
        self.startup.get_or_init(|| self.sweep_leftovers()).await;
    }

    async fn sweep_leftovers(&self) {
        let mut legacy = HashSet::new();
        if let Ok(entries) = fs::read_dir(&self.worktrees) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let name = entry.file_name();
                let Some(id) = name
                    .to_str()
                    .and_then(|name| name.strip_prefix(DIRECTORY_PREFIX))
                else {
                    continue;
                };
                let directory = entry.path();
                if generated_uuid(id) {
                    self.recover_checkout(id, &directory).await;
                } else if legacy_directory(id) {
                    legacy.insert(directory);
                }
            }
        }
        self.sweep_legacy(legacy).await;
    }

    async fn recover_checkout(&self, id: &str, directory: &Path) {
        let journal = directory.join(BINDING_FILE);
        if !fs::symlink_metadata(&journal).is_ok_and(|entry| entry.is_file()) {
            // A crash before journaling can leave an empty run directory.
            let _ = fs::remove_dir(directory);
            return;
        }
        let Some(binding) = fs::read(journal)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RepositoryBinding>(&bytes).ok())
        else {
            return;
        };
        let Ok(git) = self
            .git
            .at(binding.top_level.clone())
            .with_repository_binding(binding)
            .await
        else {
            tracing::debug!("Could not rebind an interrupted review checkout");
            return;
        };
        let prefix = format!("{REF_ROOT}{id}/");
        if let Some(refs) = read_guide_refs(&git, &prefix).await {
            finish_checkout(&git, directory, &refs).await;
        }
    }

    /// Before review runs had journals, only the exact scratch directory and numeric ref
    /// formats identified leftovers. Discover those registrations from saved project roots.
    async fn sweep_legacy(&self, mut directories: HashSet<PathBuf>) {
        let Ok(projects) = self
            .projects
            .database()
            .read(|db| t3_orchestration::stores::projects::list_shells(db, None))
            .await
        else {
            return;
        };
        let mut seen = HashSet::new();
        let mut listings_complete = true;
        for project in projects {
            let root = self.git.at(project.workspace_root.0);
            let Ok(Some(binding)) = root.bind_repository().await else {
                continue;
            };
            if seen.contains(&binding.common_dir) {
                continue;
            }
            let common_dir = binding.common_dir.clone();
            let Ok(git) = root.with_repository_binding(binding).await else {
                continue;
            };
            let Ok(worktrees) = git.list_worktrees().await else {
                listings_complete = false;
                continue;
            };
            seen.insert(common_dir);
            for worktree in worktrees {
                let Some(directory) = worktree.path.parent() else {
                    continue;
                };
                if worktree
                    .path
                    .file_name()
                    .is_some_and(|name| name == "checkout")
                    && directory.parent() == Some(self.worktrees.as_path())
                    && directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_prefix(DIRECTORY_PREFIX))
                        .is_some_and(legacy_directory)
                {
                    // Leave any registered directory for which Git refuses targeted removal.
                    directories.remove(directory);
                    if (worktree.detached || worktree.locked.as_deref() == Some("initializing"))
                        && directory_or_missing(directory)
                        && directory_or_missing(&worktree.path)
                        && contains_only_checkout(directory)
                        && force_remove_worktree(&git, &worktree.path).await
                    {
                        remove_run_files(directory, false).await;
                    }
                }
            }
            if let Some(refs) = read_guide_refs(&git, REF_ROOT).await {
                delete_refs(&git, &refs).await;
            }
        }
        if listings_complete {
            for directory in directories {
                // An undiscovered linked checkout might belong to a removed or unavailable
                // project. Leave its files when we cannot remove its Git registration safely.
                let checkout = directory.join("checkout");
                if directory_or_missing(&checkout)
                    && fs::symlink_metadata(checkout.join(".git"))
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    && contains_only_checkout(&directory)
                {
                    remove_run_files(&directory, false).await;
                }
            }
        }
    }

    /// The project's repository identity (`requireOwnRepository`'s locator), resolved when it
    /// is not cached yet.
    async fn locator(&self, workspace_root: &str) -> Result<(String, String)> {
        let identity = self
            .projects
            .enrichment()
            .read_repository_identity(workspace_root)
            .await
            .map(|identity| serde_json::to_value(identity).unwrap_or_default())
            .ok_or_else(|| failed("Guides are written from this project's checkout, and this pull request is in another repository. Write it from a project that is a checkout of that repository."))?;
        let locator = &identity["locator"];
        Ok((
            locator["remoteName"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            locator["remoteUrl"].as_str().unwrap_or_default().to_owned(),
        ))
    }

    async fn prepare_inner(
        &self,
        id: &str,
        context: &Context,
        detail: &PullRequestDetail,
    ) -> Result<PreparedGuide> {
        let service = self.service.get().ok_or_else(|| {
            failed("Review guide generation is not available on this server yet.")
        })?;
        let project = context.project.id.clone();
        let (remote_name, remote_url) = self.locator(&context.project.workspace_root).await?;
        let activity = service.activity(context.reference.clone()).await?;
        let stack = service
            .stack(context.reference.clone(), false)
            .await
            .ok()
            .flatten();
        let settings = self
            .textgen
            .settings()
            .await
            .map_err(|_| failed("Could not read the server settings."))?;
        let settings =
            t3_shared::project_settings::resolve_project_settings(&settings, Some(&project), None)
                .map_err(|_| failed("Could not read the server settings."))?
                .settings;
        let providers = self.textgen.providers().await;
        let selection = crate::adapters::git_actions::review_guide_selection(
            &settings,
            &providers,
            Some(&project),
        );
        let instructions = Some(settings.review_guide_instructions.0.clone());
        let head = detail
            .head_sha
            .clone()
            .flatten()
            .ok_or_else(|| failed("This host did not report the pull request's head commit."))?
            .to_string();
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| failed("Review guide generation is shutting down."))?;
        self.update(id, |run| run.slot = Some(slot));
        let detail_value = serde_json::to_value(detail).unwrap_or_default();
        // t3-git's repository binding: a repository replaced or redirected at the project's
        // root since it was first bound refuses instead of being used (review L3).
        let root = self.git.at(&context.project.workspace_root);
        let identity = root
            .bind_repository()
            .await
            .map_err(|_| failed("This project's checkout changed; trust the repository again."))?
            .ok_or_else(|| failed("This project's folder is not a Git repository."))?;
        let bound = root
            .with_repository_binding(identity)
            .await
            .map_err(|_| failed("This project's checkout changed; trust the repository again."))?;
        self.update(id, |run| run.git = Some(bound.clone()));
        let empty_tree = empty_tree(&bound).await?;
        let directory = self.worktrees.join(format!("{DIRECTORY_PREFIX}{id}"));
        fs::create_dir_all(&self.worktrees)
            .and_then(|()| fs::create_dir(&directory))
            .map_err(|_| failed("Could not create the review checkout."))?;
        self.update(id, |run| run.directory = Some(directory.clone()));
        // Journal the bound identity before the first ref or worktree is created so startup
        // can recover even when a crash predated checkout or the project was since removed.
        let binding = bound
            .recorded_repository()
            .flatten()
            .ok_or_else(|| failed("Could not identify the review repository."))?;
        let journal = serde_json::to_vec(binding)
            .map_err(|_| failed("Could not record the review checkout."))?;
        fs::write(directory.join(BINDING_FILE), journal)
            .map_err(|_| failed("Could not record the review checkout."))?;
        let checkout = Checkout {
            generator: self,
            id,
            git: bound,
            provider: detail_value["provider"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        let remote = checkout.remote(&remote_name, &remote_url).await?;
        let base_branch = detail_value["baseBranch"].as_str().unwrap_or_default();
        checkout.check_branch(base_branch).await?;
        let base_ref = format!("{REF_ROOT}{id}/base");
        self.update(id, |run| run.refs.push(base_ref.clone()));
        checkout
            .fetch(&remote, &format!("+refs/heads/{base_branch}:{base_ref}"))
            .await?;
        let base_tip = checkout.resolve(&base_ref).await?;
        let head_branch = detail_value["headBranch"].as_str().unwrap_or_default();
        let resolved_head = checkout
            .fetch_head(&remote, detail.number, &head, head_branch)
            .await?;
        let mut layers = Vec::new();
        if let Some(stack) = stack {
            let stack = serde_json::to_value(stack).unwrap_or_default();
            for layer in stack["layers"].as_array().cloned().unwrap_or_default() {
                let number = layer["number"].as_u64().unwrap_or_default();
                let head_sha = if number == detail.number {
                    Some(head.clone())
                } else if let Some(sha) = layer["headSha"].as_str() {
                    checkout
                        .fetch_head(
                            &remote,
                            number,
                            sha,
                            layer["headBranch"].as_str().unwrap_or_default(),
                        )
                        .await
                        .ok()
                } else {
                    None
                };
                layers.push(ReviewLayer {
                    number,
                    title: layer["title"].as_str().map(str::to_owned),
                    head_sha,
                    state: layer["state"].as_str().unwrap_or_default().to_owned(),
                });
            }
        }
        let path = directory.join("checkout");
        let path_text = path.to_string_lossy().into_owned();
        // A scratch checkout for reading the change: no repository hook runs, and no attribute
        // from the fork selects a filter, eol conversion or driver (git-lfs ships enabled on
        // Windows; a fork's `.lfsconfig` would otherwise send requests where it chooses).
        let attr_source = format!("--attr-source={empty_tree}");
        checkout
            .git
            .execute(
                &[
                    attr_source.as_str(),
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "core.fsmonitor=false",
                    "worktree",
                    "add",
                    "--detach",
                    "--quiet",
                    &path_text,
                    &resolved_head,
                ],
                isolated(300),
            )
            .await
            .map_err(|_| failed("Could not check out the pull request head."))?;
        let worktree = self.git.at(&path);
        let merge_base = worktree
            .execute(
                &[
                    attr_source.as_str(),
                    "-c",
                    "core.fsmonitor=false",
                    "merge-base",
                    &base_tip,
                    "HEAD",
                ],
                isolated(60),
            )
            .await
            .map_err(|_| failed("Could not find where the pull request branched off."))?
            .stdout_text()
            .trim()
            .to_owned();
        let change = read_change(&worktree, &merge_base, &attr_source).await?;
        strip_provider_files(&path)
            .map_err(|_| failed("Could not isolate the review checkout."))?;
        self.update(id, |run| {
            run.project = Some(project);
            run.selection = Some(selection);
            run.instructions = instructions;
            run.stack = layers;
            run.git_attr_source = Some(empty_tree);
        });
        Ok(PreparedGuide {
            change,
            detail: detail.clone(),
            activity,
            base_commit: merge_base,
            cwd: path_text,
            cleanup_id: id.to_owned(),
        })
    }
}

/// Codex discovers project layers in trusted linked worktrees. Remove the fork's
/// provider config directories before either CLI starts.
fn strip_provider_files(checkout: &Path) -> std::io::Result<()> {
    for name in [
        ".codex",
        ".claude",
        "CLAUDE.md",
        "CLAUDE.local.md",
        ".mcp.json",
    ] {
        remove_checkout_entry(&checkout.join(name))?;
    }
    Ok(())
}

fn remove_checkout_entry(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if metadata.file_type().is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn generated_number(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|number| value == number.to_string())
}

fn generated_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|uuid| uuid.get_version_num() == 4 && value == uuid.simple().to_string())
}

fn legacy_directory(suffix: &str) -> bool {
    suffix
        .split_once('-')
        .is_some_and(|(id, random)| generated_number(id) && generated_uuid(random))
}

fn directory_or_missing(directory: &Path) -> bool {
    match fs::symlink_metadata(directory) {
        Ok(entry) => entry.is_dir(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn contains_only_checkout(directory: &Path) -> bool {
    match fs::read_dir(directory) {
        Ok(mut entries) => {
            entries.all(|entry| entry.is_ok_and(|entry| entry.file_name() == "checkout"))
        }
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

async fn read_guide_refs(git: &Git, prefix: &str) -> Option<Vec<String>> {
    let output = git
        .execute(
            &[
                "for-each-ref",
                "--format=%(refname)%00%(symref)",
                "--",
                prefix,
            ],
            options(30),
        )
        .await
        .ok()?;
    Some(
        output
            .stdout_text()
            .lines()
            .filter_map(|line| {
                let (name, symref) = line.split_once('\0')?;
                if !symref.is_empty() {
                    return None;
                }
                let suffix = name.strip_prefix(prefix)?;
                let leaf = if prefix == REF_ROOT {
                    let (id, leaf) = suffix.split_once('/')?;
                    if !generated_number(id) {
                        return None;
                    }
                    leaf
                } else {
                    suffix
                };
                (leaf == "base" || leaf.strip_prefix("head-").is_some_and(generated_number))
                    .then(|| name.to_owned())
            })
            .collect(),
    )
}

async fn delete_refs(git: &Git, refs: &[String]) -> bool {
    let mut removed = true;
    for reference in refs {
        if git
            .execute(&["update-ref", "--no-deref", "-d", reference], options(30))
            .await
            .is_err()
        {
            removed = false;
        }
    }
    removed
}

async fn finish_checkout(git: &Git, directory: &Path, refs: &[String]) {
    let checkout = directory.join("checkout");
    if directory_or_missing(directory)
        && directory_or_missing(&checkout)
        && force_remove_worktree(git, &checkout).await
        && delete_refs(git, refs).await
    {
        remove_run_files(directory, true).await;
    } else {
        tracing::debug!("Could not remove a review checkout; kept its recovery journal");
    }
}

async fn remove_run_files(directory: &Path, journaled: bool) {
    let directory = directory.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        remove_checkout_entry(&directory.join("checkout"))?;
        // Keep the journal until the checkout is gone, so a partial removal can be retried.
        if journaled
            && let Err(error) = fs::remove_file(directory.join(BINDING_FILE))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error);
        }
        fs::remove_dir(directory)
    })
    .await;
}

/// Git can remove a registered worktree even if its directory has gone. An unregistered
/// path is already gone from Git's perspective. Repository-wide prune could drop user state.
async fn force_remove_worktree(git: &Git, worktree: &Path) -> bool {
    let args = [
        "worktree".as_ref(),
        "remove".as_ref(),
        "--force".as_ref(),
        "--force".as_ref(),
        "--".as_ref(),
        worktree.as_os_str(),
    ];
    git.execute(
        &args,
        ExecuteOptions {
            allow_nonzero: true,
            env: [("LC_ALL".into(), Some("C".into()))].into_iter().collect(),
            ..options(60)
        },
    )
    .await
    .is_ok_and(|output| {
        output.code == Some(0)
            || output
                .stderr_text()
                .trim_end()
                .ends_with("is not a working tree")
    })
}

fn options(seconds: u64) -> ExecuteOptions {
    ExecuteOptions {
        timeout: Some(Duration::from_secs(seconds)),
        ..Default::default()
    }
}

/// Options for commands that read the fork's tree: git-lfs never fetches (second guard beside
/// the empty attribute source).
fn isolated(seconds: u64) -> ExecuteOptions {
    ExecuteOptions {
        env: [(
            std::ffi::OsString::from("GIT_LFS_SKIP_SMUDGE"),
            Some(std::ffi::OsString::from("1")),
        )]
        .into_iter()
        .collect(),
        ..options(seconds)
    }
}

/// The repository's empty tree (SHA-1 or SHA-256), for `--attr-source`.
async fn empty_tree(git: &Git) -> Result<String> {
    let tree = git
        .execute(
            &["hash-object", "-t", "tree", "--stdin"],
            ExecuteOptions {
                stdin: Some(Vec::new()),
                ..options(30)
            },
        )
        .await
        .map_err(|_| failed("Could not prepare the review checkout."))?
        .stdout_text()
        .trim()
        .to_owned();
    if matches!(tree.len(), 40 | 64) && tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(tree)
    } else {
        Err(failed("Could not prepare the review checkout."))
    }
}

struct Checkout<'a> {
    generator: &'a ReviewGuideGenerator,
    id: &'a str,
    git: Git,
    provider: String,
}

impl Checkout<'_> {
    /// The remote whose effective fetch URL is the project's (identity names can be stale).
    async fn remote(&self, preferred: &str, url: &str) -> Result<String> {
        let names = self
            .git
            .execute(&["remote"], options(30))
            .await
            .map_err(|_| failed("Could not list the checkout's remotes."))?
            .stdout_text();
        let candidates = std::iter::once(preferred.to_owned()).chain(
            names
                .lines()
                .filter(|name| !name.is_empty() && *name != preferred)
                .map(str::to_owned),
        );
        for name in candidates {
            if name.is_empty() {
                continue;
            }
            let matches = self
                .git
                .execute(&["remote", "get-url", "--", &name], options(30))
                .await
                .is_ok_and(|output| output.stdout_text().trim() == url);
            if matches {
                return Ok(name);
            }
        }
        Err(failed(
            "This project's checkout no longer has a remote for the pull request's repository.",
        ))
    }

    /// Host-provided branch names: valid for Git, and no refspec operators.
    async fn check_branch(&self, branch: &str) -> Result<()> {
        let valid = !branch.starts_with('+')
            && !branch.contains(':')
            && self
                .git
                .execute(&["check-ref-format", "--branch", branch], options(30))
                .await
                .is_ok();
        if valid {
            Ok(())
        } else {
            Err(failed(
                "The pull request's base branch name is not one git can fetch.",
            ))
        }
    }

    async fn fetch(&self, remote: &str, refspec: &str) -> Result<()> {
        self.git
            .execute(
                &[
                    "-c",
                    "core.fsmonitor=false",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--",
                    remote,
                    refspec,
                ],
                options(300),
            )
            .await
            .map(|_| ())
            .map_err(|_| failed("Could not fetch from the pull request's repository."))
    }

    async fn resolve(&self, revision: &str) -> Result<String> {
        self.git
            .execute(
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{revision}^{{commit}}"),
                ],
                options(30),
            )
            .await
            .map(|output| output.stdout_text().trim().to_owned())
            .map_err(|_| failed("Could not resolve the pull request's commits."))
    }

    /// `fetchHead`: the reported head from local objects, the commit itself, the head branch
    /// (into a per-run ref), or the host's head ref (into a per-run ref).
    async fn fetch_head(
        &self,
        remote: &str,
        number: u64,
        reported: &str,
        branch: &str,
    ) -> Result<String> {
        let sha = reported.to_ascii_lowercase();
        if !(7..=64).contains(&sha.len()) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(failed(&format!(
                "The host reported a head for #{number} that is not a commit hash."
            )));
        }
        let head = || async {
            let commit = self.resolve(&sha).await?;
            // rev-parse prefers same-named branches and tags over short hashes.
            if commit.starts_with(&sha) {
                Ok(commit)
            } else {
                Err(failed(
                    "The head names a branch or tag rather than the commit.",
                ))
            }
        };
        if let Ok(commit) = head().await {
            return Ok(commit);
        }
        if matches!(sha.len(), 40 | 64)
            && self.fetch(remote, &sha).await.is_ok()
            && let Ok(commit) = head().await
        {
            return Ok(commit);
        }
        let head_ref = format!("{REF_ROOT}{}/head-{number}", self.id);
        self.generator
            .update(self.id, |run| run.refs.push(head_ref.clone()));
        if !branch.is_empty()
            && !branch.starts_with('+')
            && !branch.contains(':')
            && self
                .git
                .execute(
                    &["check-ref-format", &format!("refs/heads/{branch}")],
                    options(30),
                )
                .await
                .is_ok()
            && self
                .fetch(remote, &format!("+refs/heads/{branch}:{head_ref}"))
                .await
                .is_ok()
            && let Ok(commit) = head().await
        {
            return Ok(commit);
        }
        let host_ref = match self.provider.as_str() {
            "github" | "forgejo" => Some(format!("refs/pull/{number}/head")),
            "gitlab" => Some(format!("refs/merge-requests/{number}/head")),
            _ => None,
        };
        if let Some(host_ref) = host_ref
            && self
                .fetch(remote, &format!("+{host_ref}:{head_ref}"))
                .await
                .is_ok()
            && let Ok(commit) = head().await
        {
            return Ok(commit);
        }
        Err(failed(&format!("Could not find head {sha} of #{number}.")))
    }
}

/// `readChange`: the files and hunks between the merge base and the head.
async fn read_change(
    worktree: &Git,
    base: &str,
    attr_source: &str,
) -> Result<t3_pull_request::guide::ReviewGuideChange> {
    let range = format!("{base}...HEAD");
    let diff = |extra: &'static [&'static str]| {
        let mut args = vec![
            attr_source,
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=false",
            "diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ];
        args.extend_from_slice(extra);
        args.push(&range);
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        async move {
            worktree
                .execute(
                    &args,
                    ExecuteOptions {
                        max_output_bytes: 64 * 1024 * 1024,
                        ..isolated(60)
                    },
                )
                .await
                .map(|output| output.stdout_text())
                .map_err(|_| failed("Could not read the pull request's change."))
        }
    };
    let (names, patch) = tokio::try_join!(
        diff(&["--name-only", "-z"]),
        diff(&[
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv"
        ]),
    )?;
    Ok(parse_review_guide_change(&names, &patch))
}

/// `settledReviewSummary`: one line per resolved or outdated thread, by its opener.
fn settled_review(activity: &Value) -> String {
    activity["reviewThreads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|thread| thread["isResolved"] == true || thread["isOutdated"] == true)
        .filter_map(|thread| {
            let opener = thread["comments"].get(0)?;
            let who = opener["author"]["login"].as_str().unwrap_or("ghost");
            let state = if thread["isResolved"] == true {
                "resolved"
            } else {
                "outdated"
            };
            let body = without_html_comments(opener["body"].as_str().unwrap_or_default());
            let flat = body
                .split(is_js_space)
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let flat = if t3_textgen::text::utf16_len(&flat) > SETTLED_REMARK_CHARS {
                format!("{}…", t3_textgen::text::head(&flat, SETTLED_REMARK_CHARS))
            } else {
                flat
            };
            Some(format!(
                "- {} ({state}, {who}): {flat}",
                thread["path"].as_str().unwrap_or_default()
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text.replace(/<!--[\s\S]*?-->/g, "")`.
fn without_html_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let Some(end) = rest[start + 4..].find("-->") else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + 4 + end + 3..];
    }
    out.push_str(rest);
    out
}

/// JavaScript's `\s`.
fn is_js_space(ch: char) -> bool {
    ch.is_whitespace() && ch != '\u{85}' || ch == '\u{feff}'
}

#[async_trait::async_trait]
impl GuideGenerator for ReviewGuideGenerator {
    async fn check_enabled(&self, _: &Context) -> Result<()> {
        self.finish_startup().await;
        let settings = self
            .textgen
            .settings()
            .await
            .map_err(|_| failed("Could not read the server settings."))?;
        if settings.review_guides_enabled {
            Ok(())
        } else {
            Err(failed(
                "Review guides are off. Turn them on in Settings → Source Control.",
            ))
        }
    }

    async fn prepare(
        &self,
        context: &Context,
        detail: PullRequestDetail,
        cancel: CancellationToken,
    ) -> Result<PreparedGuide> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), Run::default());
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(failed("Review guide generation canceled.")),
            result = self.prepare_inner(&id, context, &detail) => result,
        };
        if result.is_err() {
            self.remove(&id).await;
        }
        result
    }

    async fn generate(
        &self,
        input: &PreparedGuide,
        cancel: CancellationToken,
    ) -> Result<(Vec<PullRequestReviewGuideStop>, ModelSelection)> {
        let (project, selection, instructions, stack, git_attr_source) = {
            let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
            let run = runs
                .get(&input.cleanup_id)
                .ok_or_else(|| failed("The review checkout is gone."))?;
            (
                run.project.clone(),
                run.selection
                    .clone()
                    .ok_or_else(|| failed("The review checkout is gone."))?,
                run.instructions.clone(),
                run.stack.clone(),
                run.git_attr_source.clone(),
            )
        };
        let detail = serde_json::to_value(&input.detail).unwrap_or_default();
        let activity = serde_json::to_value(&input.activity).unwrap_or_default();
        let review = ReviewInput {
            number: input.detail.number,
            title: detail["title"].as_str().unwrap_or_default().to_owned(),
            description: detail["body"].as_str().unwrap_or_default().to_owned(),
            base_branch: detail["baseBranch"].as_str().unwrap_or_default().to_owned(),
            base_commit: input.base_commit.clone(),
            settled_review: settled_review(&activity),
            instructions,
            stack,
            git_attr_source,
        };
        let generated = self
            .textgen
            .generate(
                GenerationContext {
                    cwd: PathBuf::from(&input.cwd),
                    model_selection: selection.clone(),
                    project_id: project,
                },
                GenerationInput::Review(review),
                cancel,
            )
            .await
            .map_err(|error| failed(&error.to_string()))?;
        match generated {
            Generated::Review { stops } => Ok((stops, selection)),
            _ => Err(failed("The model returned invalid structured output.")),
        }
    }

    async fn cleanup(&self, input: PreparedGuide) {
        self.remove(&input.cleanup_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settled_review_lists_resolved_and_outdated_openers() {
        let activity = json!({"reviewThreads": [
            {"isResolved": true, "isOutdated": false, "path": "a.rs", "comments": [{"author": {"login": "ann"}, "body": "Rename <!-- hidden\nnote --> this\n\n  please"}]},
            {"isResolved": false, "isOutdated": true, "path": "b.rs", "comments": [{"author": null, "body": "x".repeat(250)}]},
            {"isResolved": false, "isOutdated": false, "path": "c.rs", "comments": [{"author": {"login": "bo"}, "body": "open"}]},
        ]});
        let summary = settled_review(&activity);
        let lines: Vec<_> = summary.lines().collect();
        assert_eq!(lines[0], "- a.rs (resolved, ann): Rename this please");
        assert!(lines[1].starts_with("- b.rs (outdated, ghost): xxx"));
        assert!(lines[1].ends_with('…'));
        assert_eq!(lines.len(), 2);
    }

    fn git(cwd: &std::path::Path, home: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=t3", "-c", "user.email=t3@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", home.join("empty-config"))
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A `worktree add` killed by its timeout or a shutdown leaves its registration locked as
    /// "initializing", which a single `--force` refuses (review L1). The lock written here is the
    /// one the killed add leaves.
    #[tokio::test]
    async fn a_worktree_left_locked_by_a_killed_add_is_still_removed() {
        let temp = tempfile::tempdir().expect("temp");
        std::fs::write(temp.path().join("empty-config"), "").expect("config");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).expect("repo");
        git(&repo, temp.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a\n").expect("file");
        git(&repo, temp.path(), &["add", "."]);
        git(&repo, temp.path(), &["commit", "-q", "-m", "init"]);
        let checkout = temp.path().join("rg-1").join("checkout");
        git(
            &repo,
            temp.path(),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &checkout.to_string_lossy(),
                "HEAD",
            ],
        );
        git(
            &repo,
            temp.path(),
            &[
                "worktree",
                "lock",
                "--reason",
                "initializing",
                &checkout.to_string_lossy(),
            ],
        );
        let handle = Git::new(&repo).with_environment(
            [
                ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
                (
                    "GIT_CONFIG_GLOBAL".into(),
                    Some(temp.path().join("empty-config").into_os_string()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        assert!(force_remove_worktree(&handle, &checkout).await);
        let listed = git(&repo, temp.path(), &["worktree", "list", "--porcelain"]);
        assert_eq!(listed.matches("worktree ").count(), 1, "{listed}");
        assert!(!checkout.exists());
    }
}
