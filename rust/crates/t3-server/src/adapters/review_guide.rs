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
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::{
    base_schemas::ProjectId, model_selection::ModelSelection, pull_request::PullRequestDetail,
    pull_request_review_guide::PullRequestReviewGuideStop,
};
use t3_git::{ExecuteOptions, Git};
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
    counter: AtomicU64,
}

/// What a run created, so `cleanup` (or a failed `prepare`) can remove exactly that.
#[derive(Default)]
struct Run {
    workspace_root: PathBuf,
    refs: Vec<String>,
    worktree: Option<PathBuf>,
    /// `<worktrees>/review-guide-<runId>-<random>`, holding the checkout.
    directory: Option<PathBuf>,
    project: Option<ProjectId>,
    selection: Option<ModelSelection>,
    instructions: Option<String>,
    stack: Vec<ReviewLayer>,
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
        Arc::new(Self {
            textgen,
            git,
            projects,
            worktrees: context.local.config.paths.worktrees(),
            service: OnceLock::new(),
            cleanups: OnceLock::new(),
            runs: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        })
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
        // Cleanup also runs while the server shuts down, when the environment's Git root is
        // already cancelled; these commands are bounded by their own timeouts instead.
        let git = self
            .git
            .at(&run.workspace_root)
            .with_cancellation(CancellationToken::new());
        if let Some(worktree) = &run.worktree
            && !force_remove_worktree(&git, worktree).await
        {
            tracing::debug!("review guide worktree removal failed; pruning");
        }
        // Removes the folder even when Git never registered the worktree.
        if let Some(directory) = &run.directory {
            let directory = directory.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(directory)).await;
        }
        let _ = git.execute(&["worktree", "prune"], options(60)).await;
        for reference in &run.refs {
            let _ = git
                .execute(&["update-ref", "-d", reference], options(30))
                .await;
        }
        if let Some(sender) = self.cleanups.get() {
            let _ = sender.send(id.to_owned());
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
        let detail_value = serde_json::to_value(detail).unwrap_or_default();
        let checkout = Checkout {
            generator: self,
            id,
            git: self.git.at(&context.project.workspace_root),
            provider: detail_value["provider"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        let remote = checkout.remote(&remote_name, &remote_url).await?;
        let base_branch = detail_value["baseBranch"].as_str().unwrap_or_default();
        checkout.check_branch(base_branch).await?;
        let base_ref = format!("refs/t3/review-guide/{id}/base");
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
        std::fs::create_dir_all(&self.worktrees)
            .map_err(|_| failed("Could not create the review checkout."))?;
        let directory = self.worktrees.join(format!(
            "review-guide-{id}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        self.update(id, |run| run.directory = Some(directory.clone()));
        std::fs::create_dir(&directory)
            .map_err(|_| failed("Could not create the review checkout."))?;
        let path = directory.join("checkout");
        self.update(id, |run| run.worktree = Some(path.clone()));
        let path_text = path.to_string_lossy().into_owned();
        // A scratch checkout for reading the change: no repository hook may run.
        checkout
            .git
            .execute(
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "worktree",
                    "add",
                    "--detach",
                    "--quiet",
                    &path_text,
                    &resolved_head,
                ],
                options(300),
            )
            .await
            .map_err(|_| failed("Could not check out the pull request head."))?;
        let worktree = self.git.at(&path);
        let merge_base = worktree
            .execute(&["merge-base", &base_tip, "HEAD"], options(60))
            .await
            .map_err(|_| failed("Could not find where the pull request branched off."))?
            .stdout_text()
            .trim()
            .to_owned();
        let change = read_change(&worktree, &merge_base).await?;
        self.update(id, |run| {
            run.project = Some(project);
            run.selection = Some(selection);
            run.instructions = instructions;
            run.stack = layers;
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

/// Removes a review worktree's registration and files.
async fn force_remove_worktree(git: &Git, worktree: &std::path::Path) -> bool {
    let path = worktree.to_string_lossy().into_owned();
    git.execute(&["worktree", "remove", "--force", "--", &path], options(60))
        .await
        .is_ok()
}

fn options(seconds: u64) -> ExecuteOptions {
    ExecuteOptions {
        timeout: Some(Duration::from_secs(seconds)),
        ..Default::default()
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
                &["fetch", "--quiet", "--no-tags", "--", remote, refspec],
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
        let head_ref = format!("refs/t3/review-guide/{}/head-{number}", self.id);
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
) -> Result<t3_pull_request::guide::ReviewGuideChange> {
    let range = format!("{base}...HEAD");
    let diff = |extra: &'static [&'static str]| {
        let mut args = vec![
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
                        timeout: Some(Duration::from_secs(60)),
                        max_output_bytes: 64 * 1024 * 1024,
                        ..Default::default()
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
        let id = self.counter.fetch_add(1, Ordering::AcqRel).to_string();
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).insert(
            id.clone(),
            Run {
                workspace_root: PathBuf::from(&context.project.workspace_root),
                ..Default::default()
            },
        );
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
        let (project, selection, instructions, stack) = {
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
            &["worktree", "add", "-q", "--detach", &checkout.to_string_lossy(), "HEAD"],
        );
        git(
            &repo,
            temp.path(),
            &["worktree", "lock", "--reason", "initializing", &checkout.to_string_lossy()],
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
