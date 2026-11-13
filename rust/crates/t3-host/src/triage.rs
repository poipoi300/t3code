use crate::{Error, Result, background::Clock};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    auth::AuthClientPresentationMetadata,
    server::{ServerPrepareTriageInput, ServerPrepareTriageResult},
};
use tokio::sync::Mutex;

pub const TRIAGE_PLAYBOOK: &str = include_str!("triage-playbook.md");
pub const TRIAGE_PROMPT_FILE_NAME: &str = "triage-playbook.md";
pub const REPOSITORY_URL: &str = "https://github.com/pingdotgg/t3code";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub reference: String,
    pub commit: String,
}
pub fn pick_source_ref(listed: &str, version: &str) -> Option<SourceRef> {
    let refs: std::collections::BTreeMap<_, _> = listed
        .lines()
        .filter_map(|l| l.trim().split_once('\t'))
        .map(|(c, r)| (r, c))
        .collect();
    let tag = format!("v{version}");
    let selected = refs
        .get(format!("refs/tags/{tag}^{{}}").as_str())
        .or_else(|| refs.get(format!("refs/tags/{tag}").as_str()))
        .map(|c| (tag, *c))
        .or_else(|| refs.get("refs/heads/main").map(|c| ("main".into(), *c)))?;
    if !matches!(selected.1.len(), 40 | 64) || !selected.1.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(SourceRef {
        reference: selected.0,
        commit: selected.1.into(),
    })
}
#[derive(Debug, Clone)]
pub struct TriagePaths {
    pub base_dir: PathBuf,
    pub state_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
    pub logs_dir: PathBuf,
    pub server_trace_path: PathBuf,
    pub provider_event_log_path: PathBuf,
    pub terminal_logs_dir: PathBuf,
    pub provider_status_cache_dir: PathBuf,
    pub secrets_dir: PathBuf,
}
#[derive(Debug, Clone)]
pub struct TriageMachine {
    pub os: String,
    pub runtime_version: String,
    pub server_version: String,
    pub pid: u32,
}
#[derive(Debug, Clone)]
pub enum TriageEntry {
    Cli {
        cli_version: String,
        launched_as: String,
    },
    App {
        client: String,
    },
}
#[derive(Debug, Clone)]
pub struct TriageSession {
    pub session_dir: PathBuf,
    pub context_file_path: PathBuf,
    pub prompt_file_path: PathBuf,
}
#[derive(Debug, Clone)]
pub enum SourceClone {
    Cloned {
        source: SourceRef,
        directory: PathBuf,
    },
    Cloning {
        source: SourceRef,
        directory: PathBuf,
    },
    Failed {
        reason: String,
    },
}
struct State {
    staging_cleaned: bool,
    release: Option<SourceRef>,
    cloning: BTreeSet<String>,
    tasks: tokio::task::JoinSet<(String, Result<()>)>,
}
pub struct TriageService {
    paths: TriagePaths,
    machine: TriageMachine,
    clock: Arc<dyn Clock>,
    repository: String,
    environment: t3_process::Environment,
    state: Mutex<State>,
    prepare_gate: Mutex<()>,
}
impl TriageService {
    pub fn new(paths: TriagePaths, machine: TriageMachine, clock: Arc<dyn Clock>) -> Self {
        Self::with_repository(paths, machine, clock, REPOSITORY_URL.into())
    }
    pub fn with_repository(
        paths: TriagePaths,
        machine: TriageMachine,
        clock: Arc<dyn Clock>,
        repository: String,
    ) -> Self {
        Self {
            paths,
            machine,
            clock,
            repository,
            environment: t3_process::Environment::new(),
            state: Mutex::new(State {
                staging_cleaned: false,
                release: None,
                cloning: BTreeSet::new(),
                tasks: tokio::task::JoinSet::new(),
            }),
            prepare_gate: Mutex::new(()),
        }
    }
    pub fn with_git_environment(mut self, environment: t3_process::Environment) -> Self {
        self.environment = environment;
        self
    }
    pub async fn prepare(
        &self,
        input: ServerPrepareTriageInput,
    ) -> Result<ServerPrepareTriageResult> {
        let source = self
            .start_source_clone()
            .await
            .unwrap_or(SourceClone::Failed {
                reason: "The source repository could not be read.".into(),
            });
        let client = describe_client(&input.client);
        let server = format!(
            "running (pid {}, version {}); this session runs inside it",
            self.machine.pid, self.machine.server_version
        );
        let session = self
            .write_session(&TriageEntry::App { client }, &server, Some(&source))
            .await?;
        Ok(ServerPrepareTriageResult {
            prompt: format!(
                "Read {} and follow it. ",
                composer_file_link(&session.prompt_file_path)
            ),
        })
    }
    pub async fn write_session(
        &self,
        entry: &TriageEntry,
        server: &str,
        source: Option<&SourceClone>,
    ) -> Result<TriageSession> {
        let _guard = self.prepare_gate.lock().await;
        let now = chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .ok_or(Error::Timestamp)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let directory = self
            .paths
            .state_dir
            .join("triage")
            .join(now.replace([':', '.'], "-"));
        fs::create_dir_all(&directory).map_err(|_| Error::Io)?;
        let context = directory.join("context.md");
        let prompt = directory.join(TRIAGE_PROMPT_FILE_NAME);
        fs::write(&context, self.build_context(&now, entry, server, source))
            .map_err(|_| Error::Io)?;
        fs::write(&prompt, build_seed_prompt(&context, entry)).map_err(|_| Error::Io)?;
        Ok(TriageSession {
            session_dir: directory,
            context_file_path: context,
            prompt_file_path: prompt,
        })
    }
    async fn start_source_clone(&self) -> Result<SourceClone> {
        let source_dir = self.paths.base_dir.join("source");
        fs::create_dir_all(&source_dir).map_err(|_| Error::Io)?;
        let mut state = self.state.lock().await;
        if !state.staging_cleaned {
            let base = cap_std::fs::Dir::open_ambient_dir(
                &self.paths.base_dir,
                cap_std::ambient_authority(),
            )
            .map_err(|_| Error::Io)?;
            match base.remove_dir_all("source-staging") {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(Error::Refused(
                        "stale source staging could not be removed safely",
                    ));
                }
            }
            state.staging_cleaned = true;
        }
        while let Some(result) = state.tasks.try_join_next() {
            if let Ok((commit, _)) = result {
                state.cloning.remove(&commit);
            }
        }
        let source = if let Some(source) = &state.release {
            source.clone()
        } else {
            let tag = format!("refs/tags/v{}", self.machine.server_version);
            let output = git(
                &source_dir,
                &[
                    "ls-remote",
                    &self.repository,
                    &tag,
                    &format!("{tag}^{{}}"),
                    "refs/heads/main",
                ],
                5,
                &self.environment,
            )
            .await?;
            let Some(source) = pick_source_ref(&output, &self.machine.server_version) else {
                return Ok(SourceClone::Failed {
                    reason: "the repository listed neither the release tag nor main.".into(),
                });
            };
            if source.reference != "main" {
                state.release = Some(source.clone());
            }
            source
        };
        let directory = source_dir.join(&source.commit);
        if directory.exists() {
            let head = if directory.join(".git").exists() {
                git(&directory, &["rev-parse", "HEAD"], 10, &self.environment)
                    .await
                    .unwrap_or_default()
            } else {
                String::new()
            };
            return Ok(if head.trim() == source.commit {
                SourceClone::Cloned { source, directory }
            } else {
                SourceClone::Failed {
                    reason: format!(
                        "{} exists but is not a complete checkout of {}. Remove it first.",
                        directory.display(),
                        source.commit
                    ),
                }
            });
        }
        if state.cloning.insert(source.commit.clone()) {
            let repository = self.repository.clone();
            let commit = source.commit.clone();
            let stage = self.paths.base_dir.join("source-staging");
            let environment = self.environment.clone();
            state.tasks.spawn(async move {
                let result =
                    clone_source(stage, source_dir, repository, &commit, &environment).await;
                (commit, result)
            });
        }
        Ok(SourceClone::Cloning { source, directory })
    }
    pub async fn drain(&self) {
        let mut state = self.state.lock().await;
        while let Some(result) = state.tasks.join_next().await {
            if let Ok((commit, _)) = result {
                state.cloning.remove(&commit);
            }
        }
    }
    pub fn build_context(
        &self,
        generated_at: &str,
        entry: &TriageEntry,
        server: &str,
        source: Option<&SourceClone>,
    ) -> String {
        let p = &self.paths;
        let source = match source {
            None => String::new(),
            Some(SourceClone::Failed { reason }) => format!(
                "\n## Source\n\nThe server could not clone the source for its version ({}): {reason}\nClone it as step 4 describes.\n",
                self.machine.server_version
            ),
            Some(
                clone @ (SourceClone::Cloned { source, directory }
                | SourceClone::Cloning { source, directory }),
            ) => {
                let verb = if matches!(source.reference.as_str(), "main") {
                    format!(
                        "main, since its version ({}) has no release tag",
                        self.machine.server_version
                    )
                } else {
                    format!(
                        "the source for its own version ({})",
                        self.machine.server_version
                    )
                };
                let action = if matches!(clone, SourceClone::Cloned { .. }) {
                    "The server cloned"
                } else {
                    "The server is cloning"
                };
                let wait = if matches!(clone, SourceClone::Cloned { .. }) {
                    ""
                } else {
                    "\nThe folder appears only when the clone is complete. Wait for it instead of\ncloning again. If it has not appeared after a few minutes, clone as step 4\ndescribes.\n"
                };
                format!(
                    "\n## Source\n\n{action} {verb}:\n\n    {}  ({}, commit {})\n{wait}\nUse it when the bug happened on this server's build. When it happened on another\nbuild, clone that version as step 4 describes.\n",
                    directory.display(),
                    source.reference,
                    source.commit
                )
            }
        };
        let (generated_by, before, after, in_app) = match entry {
            TriageEntry::Cli {
                cli_version,
                launched_as,
            } => (
                "`t3 triage`",
                format!(
                    "- Triage CLI version: {cli_version} (the `t3` that ran this command; the bug may have happened on another build)"
                ),
                format!("- CLI launched as: {launched_as}\n"),
                "",
            ),
            TriageEntry::App { client } => (
                "the T3 Code app",
                format!(
                    "- Started from: {client} (the client the user pressed \"Triage an issue\" in; the bug may have happened on another device)"
                ),
                String::new(),
                "\n## This session runs inside T3 Code\n\nYou are a thread inside the T3 Code server described above. Stopping,\nrestarting, or killing that server, its service, or the desktop app ends this\nsession. When a fix needs one of those, give the user the exact steps and let\nthem do it. The database is in use by that server: read it, never lock it.\n",
            ),
        };
        format!(
            "# T3 Code triage context\n\nGenerated by {generated_by} at {generated_at}.\n\n{before}\n- OS: {}\n- Node: {}\n{after}- Server process: {server}\n- Repo: https://github.com/pingdotgg/t3code\n{in_app}{source}\n## Paths\n\n- State dir: {}\n- Database (SQLite; write only with the user's explicit permission): {}\n- Settings: {}\n- Logs dir: {}\n- Service log (systemd/launchd service installs only): {}\n- Desktop backend logs (glob; one file per backend plus rotated copies; written only when a backend crashes or fails to start): {}\n- T3 Connect tunnel log (ndjson; what the server logged about its cloudflared connector, with rotated copies `.1` and `.2`): {}\n- Server trace (ndjson): {}\n- Provider event log: {}\n- Terminal logs: {}\n- Provider status cache: {}\n- Secrets dir (NEVER read this): {}\n- Source cache dir (clone the repo here): {}\n",
            self.machine.os,
            self.machine.runtime_version,
            p.state_dir.display(),
            p.db_path.display(),
            p.settings_path.display(),
            p.logs_dir.display(),
            p.logs_dir.join("boot-service.log").display(),
            p.logs_dir.join("server-child*.log*").display(),
            p.logs_dir.join("relay-client.ndjson").display(),
            p.server_trace_path.display(),
            p.provider_event_log_path.display(),
            p.terminal_logs_dir.display(),
            p.provider_status_cache_dir.display(),
            p.secrets_dir.display(),
            p.base_dir.join("source").display()
        )
    }
}
pub fn build_seed_prompt(context: &Path, entry: &TriageEntry) -> String {
    let (start, first) = match entry {
        TriageEntry::Cli { .. } => (
            "with `t3 triage`",
            "Follow the playbook below, starting by asking the user what went wrong.",
        ),
        TriageEntry::App { .. } => (
            "from the T3 Code app",
            "Follow the playbook below. The message that sent you here may already describe what went wrong; if it does not, start by asking.",
        ),
    };
    format!(
        "A T3 Code user is having a problem with their install and started this session {start}.\n\nMachine facts (versions, OS, paths, server liveness) are in the triage context file:\n\n    {}\n\n{first}\n\n---\n\n{TRIAGE_PLAYBOOK}",
        context.display()
    )
}
pub fn describe_server_process(path: &Path) -> String {
    let Some(state) = t3_config::runtime::read_runtime_state(path) else {
        return "not running (no server-runtime.json; the server may never have started here)"
            .into();
    };
    let version = state
        .server_version
        .as_ref()
        .map_or_else(|| "version not recorded".into(), |v| format!("version {v}"));
    if !t3_config::runtime::is_process_alive(state.pid) {
        return format!(
            "not running (state file is stale: pid {} is dead; last origin {}, {version})",
            state.pid, state.origin
        );
    }
    format!("running (pid {}, {}, {version})", state.pid, state.origin)
}
async fn git(
    cwd: &Path,
    args: &[&str],
    seconds: u64,
    environment: &t3_process::Environment,
) -> Result<String> {
    let mut options = t3_git::ExecuteOptions {
        timeout: Some(Duration::from_secs(seconds)),
        max_output_bytes: 1024 * 1024,
        output_mode: t3_git::OutputMode::Error,
        ..Default::default()
    };
    for (k, v) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_ASKPASS", ""),
        ("LANGUAGE", "C"),
    ] {
        options.env.insert(k.into(), Some(v.into()));
    }
    t3_git::Git::new(cwd)
        .with_environment(environment.clone())
        .execute(args, options)
        .await
        .map(|o| o.stdout_text())
        .map_err(|_| Error::Service)
}
async fn clone_source(
    stage: PathBuf,
    source: PathBuf,
    repository: String,
    commit: &str,
    environment: &t3_process::Environment,
) -> Result<()> {
    fs::create_dir_all(&stage).map_err(|_| Error::Io)?;
    let temp = tempfile::Builder::new()
        .prefix("clone-")
        .tempdir_in(stage)
        .map_err(|_| Error::Io)?;
    git(temp.path(), &["init", "--quiet"], 10, environment).await?;
    git(
        temp.path(),
        &["remote", "add", "origin", &repository],
        10,
        environment,
    )
    .await?;
    git(
        temp.path(),
        &[
            "fetch",
            "--depth",
            "1",
            "--filter=blob:none",
            "origin",
            commit,
        ],
        600,
        environment,
    )
    .await?;
    git(
        temp.path(),
        &["checkout", "--quiet", "FETCH_HEAD"],
        600,
        environment,
    )
    .await?;
    let destination = source.join(commit);
    if fs::rename(temp.path(), &destination).is_err()
        && (!destination.join(".git").exists()
            || git(&destination, &["rev-parse", "HEAD"], 10, environment)
                .await?
                .trim()
                != commit)
    {
        return Err(Error::Io);
    }
    Ok(())
}
fn describe_client(client: &AuthClientPresentationMetadata) -> String {
    let value = t3_contracts::stored::to_value(client).unwrap_or_default();
    let mut parts = vec![
        value["label"]
            .as_str()
            .unwrap_or("an unknown client")
            .to_owned(),
    ];
    for (key, prefix) in [
        ("appVersion", "version "),
        ("os", "on "),
        ("browser", "in "),
    ] {
        if let Some(v) = value[key].as_str() {
            parts.push(format!("{prefix}{v}"));
        }
    }
    if value["webDeployment"] == "hosted" {
        parts.push("(app.t3.codes)".into());
    }
    parts.join(" ")
}
fn composer_file_link(path: &Path) -> String {
    let path = path.to_string_lossy();
    let label = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&path)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    let mut destination = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b";/@&=+$,-_.!~*':".contains(&byte) {
            destination.push(char::from(byte));
        } else {
            destination.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("[{label}]({destination})")
}
