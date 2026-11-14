//! First-run decision and project import (`onboarding/firstRun.logic.ts`,
//! `onboarding/projectImport.logic.ts`). Time and locale ordering are supplied by the caller.
use crate::types::{EnvironmentId, ProjectId};
use crate::workspace::{
    project_ops::paths::find_project_by_path, projects::normalize_project_path_for_comparison,
};
use std::{cmp::Ordering, collections::BTreeSet};
use t3_contracts::agent_sessions::AgentSessionProjectCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRunDecision {
    Pending,
    App,
    Wizard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstRunGate {
    pub decision: FirstRunDecision,
    pub stalled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRunGateEvent {
    Evidence(FirstRunDecision),
    Timeout,
}

pub fn first_run_provenance_authoritative(
    welcome_received: bool,
    bootstrap_pending: Option<bool>,
) -> bool {
    welcome_received && bootstrap_pending != Some(true)
}

impl FirstRunGate {
    pub fn transition(self, event: FirstRunGateEvent) -> Self {
        match event {
            FirstRunGateEvent::Timeout => {
                if self.decision == FirstRunDecision::Pending && !self.stalled {
                    Self {
                        stalled: true,
                        ..self
                    }
                } else {
                    self
                }
            }
            FirstRunGateEvent::Evidence(decision) => {
                if decision == FirstRunDecision::Pending || decision == self.decision {
                    self
                } else {
                    Self {
                        decision,
                        stalled: false,
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FirstRunProject<'a> {
    pub id: &'a str,
    pub environment: &'a str,
    pub workspace_root: &'a str,
}

#[derive(Debug, Clone)]
pub struct FirstRunThread<'a> {
    pub id: &'a str,
    pub project: &'a str,
    pub environment: &'a str,
    pub has_latest_run: bool,
    pub has_user_message: bool,
    pub has_runtime: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FirstRunWorkspace<'a> {
    pub primary_environment: Option<&'a str>,
    pub server_cwd: Option<&'a str>,
    pub bootstrap_project: Option<&'a str>,
    pub bootstrap_thread: Option<&'a str>,
    pub bootstrap_project_created: bool,
    pub bootstrap_thread_created: bool,
    pub projects: Vec<FirstRunProject<'a>>,
    pub threads: Vec<FirstRunThread<'a>>,
}

/// Whether the workspace holds only what the server's first start created.
pub fn is_fresh_first_run_workspace(input: &FirstRunWorkspace<'_>) -> bool {
    if input.projects.len() > 1 || input.threads.len() > 1 {
        return false;
    }
    let project = input.projects.first();
    if let Some(project) = project {
        let Some(cwd) = input.server_cwd else {
            return false;
        };
        if !input.bootstrap_project_created
            || input.bootstrap_project != Some(project.id)
            || Some(project.environment) != input.primary_environment
            || normalize_project_path_for_comparison(project.workspace_root)
                != normalize_project_path_for_comparison(cwd)
        {
            return false;
        }
    }
    let Some(thread) = input.threads.first() else {
        return true;
    };
    project.is_some_and(|project| {
        input.bootstrap_thread_created
            && input.bootstrap_thread == Some(thread.id)
            && Some(thread.environment) == input.primary_environment
            && thread.project == project.id
            && !thread.has_latest_run
            && !thread.has_user_message
            && !thread.has_runtime
    })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FirstRunInput {
    pub enabled: bool,
    pub hydrated: bool,
    pub completed: bool,
    pub bootstrapped: bool,
    pub authoritative: bool,
    pub workspace_authoritative: bool,
    pub workspace_provenance_authoritative: bool,
    pub catalog_ready: bool,
    pub server_config_available: bool,
    pub workspace_fresh: bool,
    pub project_count: usize,
    pub thread_count: usize,
}

/// The decision, and whether to persist first-run completion with it.
pub fn resolve_first_run_decision(input: &FirstRunInput) -> (FirstRunDecision, bool) {
    if !input.enabled || (input.hydrated && input.completed) {
        return (FirstRunDecision::App, false);
    }
    if !input.hydrated {
        return (FirstRunDecision::Pending, false);
    }
    if input.project_count > 1 || input.thread_count > 1 {
        return (
            FirstRunDecision::App,
            input.bootstrapped
                && input.authoritative
                && input.workspace_authoritative
                && input.catalog_ready
                && input.server_config_available,
        );
    }
    if !input.bootstrapped
        || !input.authoritative
        || !input.workspace_provenance_authoritative
        || !input.catalog_ready
        || !input.server_config_available
    {
        return (FirstRunDecision::Pending, false);
    }
    if input.workspace_fresh {
        (FirstRunDecision::Wizard, false)
    } else {
        (FirstRunDecision::App, input.workspace_authoritative)
    }
}

pub fn resolve_hosted_first_run_decision(hydrated: bool) -> (FirstRunDecision, bool) {
    (
        if hydrated {
            FirstRunDecision::App
        } else {
            FirstRunDecision::Pending
        },
        false,
    )
}

const RECENT_PROJECT_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const DEFAULT_SELECTION_MIN_THREADS: u64 = 3;

/// The candidates selected by default: git repositories with enough history active in the last
/// 30 days. Candidates without a git field (older servers) count as repositories.
pub fn recent_onboarding_projects(
    candidates: &[AgentSessionProjectCandidate],
    now_epoch_ms: i64,
) -> Vec<&AgentSessionProjectCandidate> {
    let cutoff = now_epoch_ms - RECENT_PROJECT_WINDOW_MS;
    candidates
        .iter()
        .filter(|candidate| {
            !matches!(candidate.git, Some(None))
                && candidate.thread_count >= DEFAULT_SELECTION_MIN_THREADS
                && candidate
                    .last_active_at
                    .as_ref()
                    .and_then(|at| crate::time::epoch_ms(&at.0))
                    .is_some_and(|at| at >= cutoff && at <= now_epoch_ms)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingProjectGroup<'a> {
    pub key: String,
    pub label: String,
    pub repository: Option<String>,
    pub candidates: Vec<&'a AgentSessionProjectCandidate>,
    pub thread_count: u64,
    pub last_active_at: Option<String>,
}

/// Repositories grouped by remote (or path), most recent first, ties by `compare_labels`
/// (`localeCompare` in the web app); non-git folders separately, in input order.
pub fn group_onboarding_projects<'a>(
    candidates: &'a [AgentSessionProjectCandidate],
    compare_labels: impl Fn(&str, &str) -> Ordering,
) -> (
    Vec<OnboardingProjectGroup<'a>>,
    Vec<&'a AgentSessionProjectCandidate>,
) {
    let mut groups: Vec<OnboardingProjectGroup<'a>> = Vec::new();
    let mut other = Vec::new();
    for candidate in candidates {
        let (remote, repository) = match &candidate.git {
            Some(None) => {
                other.push(candidate);
                continue;
            }
            Some(Some(git)) => (git.remote_key.clone(), git.repository.clone()),
            None => (None, None),
        };
        let key = remote.map_or_else(
            || format!("path:{}", candidate.path.0),
            |remote| format!("remote:{remote}"),
        );
        let active = candidate.last_active_at.as_ref().map(|at| at.0.clone());
        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.candidates.push(candidate);
            group.thread_count += candidate.thread_count;
            group.last_active_at = match (group.last_active_at.take(), active) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
            continue;
        }
        groups.push(OnboardingProjectGroup {
            key,
            label: repository
                .clone()
                .unwrap_or_else(|| candidate.title.0.clone()),
            repository,
            candidates: vec![candidate],
            thread_count: candidate.thread_count,
            last_active_at: active,
        });
    }
    groups.sort_by(
        |left, right| match (&left.last_active_at, &right.last_active_at) {
            (l, r) if l == r => compare_labels(&left.label, &right.label),
            (None, _) => Ordering::Greater,
            (_, None) => Ordering::Less,
            (Some(l), Some(r)) => r.cmp(l),
        },
    );
    (groups, other)
}

pub struct ImportProject<'a> {
    pub id: &'a ProjectId,
    pub environment: &'a EnvironmentId,
    pub workspace_root: &'a str,
}

/// The server's project match first; the client snapshot can lag behind the scan.
pub fn resolve_onboarding_project_id(
    projects: &[ImportProject<'_>],
    environment: &EnvironmentId,
    candidate: &AgentSessionProjectCandidate,
) -> Option<ProjectId> {
    if let Some(project) = &candidate.project_id {
        return project.clone();
    }
    let in_environment: Vec<_> = projects
        .iter()
        .filter(|project| project.environment == environment)
        .collect();
    find_project_by_path(&in_environment, &candidate.path.0, |project| {
        project.workspace_root
    })
    .map(|project| project.id.clone())
}

/// Whether candidates are not projects yet, by `resolve_onboarding_project_id`'s rules, reading
/// the project list once.
pub fn new_onboarding_project_filter(
    projects: &[ImportProject<'_>],
) -> impl Fn(&EnvironmentId, &AgentSessionProjectCandidate) -> bool + use<> {
    let roots: BTreeSet<_> = projects
        .iter()
        .map(|project| {
            (
                project.environment.clone(),
                normalize_project_path_for_comparison(project.workspace_root),
            )
        })
        .collect();
    move |environment, candidate| {
        if candidate.project_id.is_some() {
            return false;
        }
        let path = normalize_project_path_for_comparison(&candidate.path.0);
        path.is_empty() || !roots.contains(&(environment.clone(), path))
    }
}

/// Splits a path into its last segment and the parent shown under it; drive and share roots
/// stay whole.
pub fn split_onboarding_project_path(path: &str) -> (String, String) {
    let bytes = path.as_bytes();
    let separator = |b: u8| b == b'/' || b == b'\\';
    let drive_root = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2..].iter().all(|b| separator(*b));
    let share_root = {
        let rest = path.trim_start_matches(['/', '\\']);
        path.len() - rest.len() == 2 && {
            let mut parts = rest.split(['/', '\\']);
            let (host, share) = (parts.next(), parts.next());
            host.is_some_and(|h| !h.is_empty())
                && share.is_some_and(|s| !s.is_empty())
                && parts.all(str::is_empty)
        }
    };
    if drive_root || share_root {
        return (path.into(), String::new());
    }
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return (path.into(), String::new());
    }
    match trimmed.rfind(['/', '\\']) {
        None => (trimmed.into(), String::new()),
        Some(index) => (trimmed[index + 1..].into(), trimmed[..=index].into()),
    }
}

pub fn onboarding_project_key(environment: &EnvironmentId, path: &str) -> String {
    serde_json::json!([environment.0, path]).to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate(path: &str, extra: serde_json::Value) -> AgentSessionProjectCandidate {
        let mut value = json!({"title":path.trim_end_matches('/').rsplit('/').next().unwrap(),"path":path,"sources":["codex"],"threadCount":3,"lastActiveAt":"2026-08-20T12:00:00.000Z","alreadyImported":false,"git":{"remoteKey":null,"repository":null}});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }
    const NOW: i64 = 1_787_400_000_000; // 2026-08-22T12:00:00.000Z

    #[test]
    fn the_default_selection_is_recent_git_history_with_enough_threads() {
        let recent = candidate("/projects/recent", json!({}));
        let older = candidate(
            "/projects/older",
            json!({"lastActiveAt":"2026-07-01T12:00:00.000Z"}),
        );
        let thin = candidate("/projects/thin", json!({"threadCount":2}));
        let folder = candidate("/projects/folder", json!({"git":null}));
        let legacy = {
            let mut value = serde_json::to_value(candidate("/projects/legacy", json!({}))).unwrap();
            value.as_object_mut().unwrap().remove("git");
            serde_json::from_value(value).unwrap()
        };
        let all = [recent, older, thin, folder, legacy];
        let selected: Vec<_> = recent_onboarding_projects(&all, NOW)
            .into_iter()
            .map(|c| c.path.0.as_str())
            .collect();
        assert_eq!(selected, ["/projects/recent", "/projects/legacy"]);
    }

    #[test]
    fn repositories_group_by_remote_newest_first_and_folders_stay_apart() {
        let github = |repo: &str| json!({"git":{"remoteKey":format!("github.com/{}", repo.to_lowercase()),"repository":repo}});
        let candidates = [
            candidate("/a/app", github("Org/App")),
            candidate("/b/app", {
                let mut v = github("Org/App");
                v["lastActiveAt"] = json!("2026-08-21T12:00:00.000Z");
                v
            }),
            candidate("/c/lib", github("Org/Lib")),
            candidate("/d/notes", json!({"git":null})),
        ];
        let (groups, other) = group_onboarding_projects(&candidates, |a, b| a.cmp(b));
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, "Org/App");
        assert_eq!(groups[0].thread_count, 6);
        assert_eq!(
            groups[0].last_active_at.as_deref(),
            Some("2026-08-21T12:00:00.000Z")
        );
        assert_eq!(groups[1].label, "Org/Lib");
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn first_run_reaches_the_wizard_only_on_a_fresh_authoritative_workspace() {
        let ready = FirstRunInput {
            enabled: true,
            hydrated: true,
            bootstrapped: true,
            authoritative: true,
            workspace_authoritative: true,
            workspace_provenance_authoritative: true,
            catalog_ready: true,
            server_config_available: true,
            workspace_fresh: true,
            ..FirstRunInput::default()
        };
        assert_eq!(
            resolve_first_run_decision(&ready),
            (FirstRunDecision::Wizard, false)
        );
        assert_eq!(
            resolve_first_run_decision(&FirstRunInput {
                workspace_fresh: false,
                ..ready
            }),
            (FirstRunDecision::App, true)
        );
        assert_eq!(
            resolve_first_run_decision(&FirstRunInput {
                catalog_ready: false,
                ..ready
            }),
            (FirstRunDecision::Pending, false)
        );
        assert_eq!(
            resolve_first_run_decision(&FirstRunInput {
                project_count: 2,
                ..ready
            }),
            (FirstRunDecision::App, true)
        );
        assert_eq!(
            resolve_first_run_decision(&FirstRunInput {
                completed: true,
                ..ready
            }),
            (FirstRunDecision::App, false)
        );
        let gate = FirstRunGate {
            decision: FirstRunDecision::Pending,
            stalled: false,
        };
        let stalled = gate.transition(FirstRunGateEvent::Timeout);
        assert!(stalled.stalled);
        let decided = stalled.transition(FirstRunGateEvent::Evidence(FirstRunDecision::App));
        assert_eq!(
            decided,
            FirstRunGate {
                decision: FirstRunDecision::App,
                stalled: false
            }
        );
        assert_eq!(decided.transition(FirstRunGateEvent::Timeout), decided);
    }

    #[test]
    fn a_bootstrap_project_and_untouched_thread_are_still_fresh() {
        let project = FirstRunProject {
            id: "p",
            environment: "local",
            workspace_root: "/repo/",
        };
        let thread = FirstRunThread {
            id: "t",
            project: "p",
            environment: "local",
            has_latest_run: false,
            has_user_message: false,
            has_runtime: false,
        };
        let mut input = FirstRunWorkspace {
            primary_environment: Some("local"),
            server_cwd: Some("/repo"),
            bootstrap_project: Some("p"),
            bootstrap_thread: Some("t"),
            bootstrap_project_created: true,
            bootstrap_thread_created: true,
            projects: vec![project],
            threads: vec![thread],
        };
        assert!(is_fresh_first_run_workspace(&input));
        input.threads[0].has_user_message = true;
        assert!(!is_fresh_first_run_workspace(&input));
        input.threads.clear();
        input.bootstrap_project_created = false;
        assert!(!is_fresh_first_run_workspace(&input));
    }

    #[test]
    fn project_paths_split_into_name_and_parent_and_keys_are_json() {
        assert_eq!(
            split_onboarding_project_path("/a/b/"),
            ("b".into(), "/a/".into())
        );
        assert_eq!(
            split_onboarding_project_path("C:\\"),
            ("C:\\".into(), String::new())
        );
        assert_eq!(
            split_onboarding_project_path("\\\\host\\share\\"),
            ("\\\\host\\share\\".into(), String::new())
        );
        assert_eq!(
            split_onboarding_project_path("name"),
            ("name".into(), String::new())
        );
        assert_eq!(
            onboarding_project_key(&EnvironmentId("e".into()), "/a"),
            r#"["e","/a"]"#
        );
        let id = ProjectId("p".into());
        let environment = EnvironmentId("e".into());
        let projects = [ImportProject {
            id: &id,
            environment: &environment,
            workspace_root: "/repo",
        }];
        let is_new = new_onboarding_project_filter(&projects);
        assert!(!is_new(&environment, &candidate("/repo/", json!({}))));
        assert!(is_new(
            &EnvironmentId("other".into()),
            &candidate("/repo", json!({}))
        ));
        assert_eq!(
            resolve_onboarding_project_id(&projects, &environment, &candidate("/repo", json!({}))),
            Some(id)
        );
    }
}
