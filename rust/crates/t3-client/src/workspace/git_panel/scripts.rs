use super::*;
use crate::{
    platform::{StorageArea, StorageKey, StorageWrite},
    rpc::{RpcCallError, RpcEndpoint},
    workspace::{ProjectKey, project_ops::scripts as policy},
};
use futures::{future::LocalBoxFuture, stream};
use serde_json::json;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    project::{ProjectReadFileInput, ProjectReadFileResult, ProjectScript, ProjectScriptIcon},
    rpc::*,
    server::ServerConfig,
    settings::ServerSettings,
    t3_project_file::{T3ProjectFile, T3ProjectFileScript},
    terminal::{TerminalOpenInput, TerminalWriteInput},
};

#[derive(Clone, Debug)]
pub struct NewProjectScriptInput {
    pub script: policy::ProjectScriptInput,
    pub keybinding: Option<Option<String>>,
}
#[derive(Clone, Debug)]
pub struct FailedImport {
    pub input: NewProjectScriptInput,
    pub error: String,
}
type SavedScriptSettings = (Rc<ServerConfig>, Rc<ServerSettings>);

#[derive(Default)]
pub(crate) struct ScriptsState {
    saved: RefCell<BTreeMap<EnvironmentId, SavedScriptSettings>>,
    failed_imports: RefCell<BTreeMap<ThreadKey, FailedImport>>,
    changed: PanelChanges,
    scheduler: super::super::vcs::VcsCommandScheduler,
}
#[derive(Clone, Debug)]
pub struct ProjectScriptsSnapshot {
    pub project: ProjectKey,
    pub scripts: Vec<ProjectScript>,
    pub file_scripts: Vec<T3ProjectFileScript>,
    pub importable_scripts: Vec<T3ProjectFileScript>,
    pub primary_script_id: Option<String>,
    pub preferred_script_id: Option<String>,
    pub can_run: bool,
    pub can_edit: bool,
    pub can_edit_keybindings: bool,
    pub failed_import: Option<FailedImport>,
    pub file_error: Option<String>,
    pub keybindings: Rc<t3_contracts::keybindings::ResolvedKeybindingsConfig>,
}
#[derive(Clone)]
pub struct ProjectScripts<P: Platform + Clone + 'static> {
    service: GitPanel<P>,
    key: ThreadKey,
    file: Option<Rc<crate::composition::QueryLease<ProjectReadFileResult>>>,
}
fn preferred_key() -> StorageKey {
    StorageKey::new("preferences", None, "t3code:last-invoked-script-by-project")
}

pub fn resolve_project_scripts(
    settings: &ServerSettings,
    project: &t3_contracts::orchestration_project::OrchestrationProjectShell,
) -> Vec<ProjectScript> {
    if let Some(scripts) = settings
        .project_settings_overrides
        .get(&project.id)
        .and_then(|s| s.default_project_scripts.as_ref())
    {
        return scripts.clone();
    }
    if settings.project_settings_folded {
        return settings.default_project_scripts.clone();
    }
    match settings.project_script_overrides.get(&project.id) {
        Some(Some(scripts)) => scripts.clone(),
        Some(None) => settings.default_project_scripts.clone(),
        None => {
            if project.scripts.is_empty() {
                settings.default_project_scripts.clone()
            } else {
                project.scripts.clone()
            }
        }
    }
}
pub fn importable_scripts(
    file: &[T3ProjectFileScript],
    scripts: &[ProjectScript],
) -> Vec<T3ProjectFileScript> {
    file.iter()
        .filter(|f| {
            !scripts.iter().any(|s| {
                s.command == f.command || s.name.0.to_lowercase() == f.name.0.to_lowercase()
            })
        })
        .cloned()
        .collect()
}
pub fn import_input(
    file: T3ProjectFileScript,
    can_edit_keybindings: bool,
) -> NewProjectScriptInput {
    let has_preview = file.preview_url.is_some();
    let setup = file.run_on_worktree_create == Some(true);
    NewProjectScriptInput {
        script: policy::ProjectScriptInput {
            name: file.name,
            command: file.command,
            icon: file.icon.unwrap_or(ProjectScriptIcon::Play),
            run_on_worktree_create: setup,
            wait_for_setup: setup && file.r#async == Some(false),
            run_on_settle: file.run_on_settle.unwrap_or(false),
            stop_on_settle: file.stop_on_settle.unwrap_or(false),
            preview_url: file.preview_url,
            auto_open_preview: has_preview && file.auto_open_preview.unwrap_or(false),
        },
        keybinding: can_edit_keybindings.then_some(None),
    }
}
pub fn parse_project_file(contents: &str) -> Option<T3ProjectFile> {
    let mut clean = String::new();
    let mut chars = contents.chars().peekable();
    let (mut quoted, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if quoted {
            clean.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
            clean.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            while chars.peek().is_some_and(|c| !matches!(c, '\r' | '\n')) {
                chars.next();
            }
            clean.push(' ');
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                return None;
            }
            clean.push(' ');
            continue;
        }
        clean.push(c);
    }
    let mut output = String::new();
    let chars = clean.chars().collect::<Vec<_>>();
    let (mut quoted, mut escaped) = (false, false);
    for (i, c) in chars.iter().copied().enumerate() {
        if !quoted
            && c == ','
            && chars[i + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| matches!(c, '}' | ']'))
        {
            continue;
        }
        output.push(c);
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        }
    }
    serde_json::from_str(&output).ok()
}
pub fn reusable_script_terminal(
    drawer: &[String],
    active: &str,
    panel: &std::collections::BTreeSet<String>,
    running: &[String],
) -> Option<String> {
    let idle = |id: &String| !panel.contains(id) && !running.contains(id);
    drawer
        .iter()
        .filter(|id| idle(id))
        .find(|id| id.as_str() == active)
        .or_else(|| drawer.iter().find(|id| idle(id)))
        .cloned()
}
fn binding_key(binding: &t3_contracts::keybindings::ResolvedKeybindingRule) -> String {
    let s = &binding.shortcut;
    let mut parts = Vec::new();
    for (enabled, label) in [
        (s.mod_key, "mod"),
        (s.ctrl_key, "ctrl"),
        (s.meta_key, "meta"),
        (s.alt_key, "alt"),
        (s.shift_key, "shift"),
    ] {
        if enabled {
            parts.push(label.to_owned());
        }
    }
    parts.push(
        match s.key.as_str() {
            " " => "space",
            "escape" => "esc",
            key => key,
        }
        .into(),
    );
    parts.join("+")
}

impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn project_scripts(&self, key: &ThreadKey) -> ProjectScripts<P> {
        let file = self.workspace_context(key, None).map(|c| {
            Rc::new(self.runtime.query::<WsProjectsReadFileRpc>(
                &key.environment,
                ProjectReadFileInput {
                    cwd: c.project.workspace_root.clone(),
                    relative_path: "t3.json".into(),
                    allow_missing: Some(Some(true)),
                },
                ResourcePolicy::default(),
            ))
        });
        ProjectScripts {
            service: self.clone(),
            key: key.clone(),
            file,
        }
    }
}
impl<P: Platform + Clone + 'static> ProjectScripts<P> {
    fn settings(&self, config: &Rc<ServerConfig>) -> Rc<ServerSettings> {
        self.service
            .state
            .scripts
            .saved
            .borrow()
            .get(&self.key.environment)
            .filter(|(base, _)| Rc::ptr_eq(base, config))
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| Rc::new(config.settings.clone()))
    }
    pub fn snapshot(&self) -> LocalBoxFuture<'static, Result<ProjectScriptsSnapshot, ClientError>> {
        let scripts = self.clone();
        async move {
            let context = scripts
                .service
                .workspace_context(&scripts.key, None)
                .ok_or(ClientError::Cancelled)?;
            let config = super::details::environment_config(
                scripts.service.runtime.store(),
                &scripts.key.environment,
            )
            .ok_or(ClientError::Disconnected)?;
            let settings = scripts.settings(&config);
            let saved = resolve_project_scripts(&settings, &context.project);
            let preferred = scripts
                .service
                .platform
                .get(StorageArea::Ordinary, preferred_key())
                .await?
                .and_then(|b| serde_json::from_slice::<BTreeMap<String, String>>(&b).ok())
                .and_then(|p| p.get(&context.project.id.0).cloned());
            let primary = preferred
                .as_ref()
                .and_then(|id| saved.iter().find(|s| s.id.0 == *id))
                .or_else(|| policy::primary_project_script(&saved))
                .map(|s| s.id.0.clone());
            let file = scripts.file.as_ref().map(|q| q.resource().snapshot());
            let file_scripts = file
                .as_ref()
                .and_then(|f| f.value.as_ref())
                .filter(|f| !f.truncated)
                .and_then(|f| parse_project_file(&f.contents))
                .and_then(|f| f.scripts)
                .unwrap_or_default();
            let importable_scripts = importable_scripts(&file_scripts, &saved);
            let endpoint = scripts.service.session(&scripts.key.environment).ok();
            let update = serde_json::from_value(json!({"patch":{}}))?;
            let can_edit = endpoint
                .as_ref()
                .is_some_and(|e| e.available::<WsServerUpdateSettingsRpc>(&update));
            let input = TerminalWriteInput {
                thread_id: TrimmedNonEmptyString(scripts.key.thread.0.clone()),
                terminal_id: TrimmedNonEmptyString("availability".into()),
                data: String::new(),
            };
            let can_run = endpoint
                .as_ref()
                .is_some_and(|e| e.available::<WsTerminalWriteRpc>(&input));
            let open = serde_json::from_value(json!({
                "threadId":scripts.key.thread,"terminalId":"availability",
                "cwd":context.worktree_path.as_deref().unwrap_or(&context.project.workspace_root.0),
                "stopOnSettle":false
            }))?;
            let can_run = can_run
                && endpoint
                    .as_ref()
                    .is_some_and(|e| e.available::<WsTerminalOpenRpc>(&open));
            let keybinding =
                serde_json::from_value(json!({"key":"mod+r","command":"script.availability.run"}))?;
            let can_edit_keybindings = endpoint
                .as_ref()
                .is_some_and(|e| e.available::<WsServerUpsertKeybindingRpc>(&keybinding));
            Ok(ProjectScriptsSnapshot {
                project: ProjectKey {
                    environment: scripts.key.environment.clone(),
                    project: context.project.id.clone(),
                },
                scripts: saved,
                file_scripts,
                importable_scripts,
                primary_script_id: primary,
                preferred_script_id: preferred,
                can_run,
                can_edit,
                can_edit_keybindings,
                failed_import: scripts
                    .service
                    .state
                    .scripts
                    .failed_imports
                    .borrow()
                    .get(&scripts.key)
                    .cloned(),
                file_error: file.and_then(|f| f.error),
                keybindings: Rc::new(config.keybindings.clone()),
            })
        }
        .boxed_local()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, Result<ProjectScriptsSnapshot, ClientError>> {
        let scripts = self.clone();
        let mut changes = stream::select(
            scripts.service.changes(),
            scripts.service.state.scripts.changed.subscribe(),
        )
        .boxed_local();
        if let Some(file) = &scripts.file {
            changes = stream::select(
                changes,
                file.resource()
                    .observe(scripts.service.platform.monotonic_ms())
                    .map(|_| ())
                    .boxed_local(),
            )
            .boxed_local();
        }
        stream::unfold(
            (true, scripts, changes),
            |(first, scripts, mut changes)| async move {
                if !first {
                    changes.next().await?;
                }
                let value = scripts.snapshot().await;
                Some((value, (false, scripts, changes)))
            },
        )
        .boxed_local()
    }
    async fn persist(
        &self,
        before: Vec<ProjectScript>,
        after: Vec<ProjectScript>,
        script_id: &str,
        keybinding: Option<Option<String>>,
    ) -> Result<(), ClientError> {
        let config =
            super::details::environment_config(self.service.runtime.store(), &self.key.environment)
                .ok_or(ClientError::Disconnected)?;
        let settings = self.settings(&config);
        let c = self
            .service
            .workspace_context(&self.key, None)
            .ok_or(ClientError::Cancelled)?;
        let command = format!("script.{script_id}.run");
        let previous = config
            .keybindings
            .0
            .iter()
            .rev()
            .find(|b| b.command.0 == command)
            .map(binding_key);
        let deleting = !after.iter().any(|s| s.id.0 == script_id);
        let endpoint = self.service.session(&self.key.environment)?;
        let normalized = keybinding
            .as_ref()
            .and_then(|v| v.as_ref())
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        let changed = keybinding.is_some() && normalized != previous;
        let probe = serde_json::from_value(json!({"key":"mod+r","command":command}))?;
        let can_keys = endpoint.available::<WsServerUpsertKeybindingRpc>(&probe);
        let changed = changed && (!deleting || can_keys);
        if changed && !can_keys {
            return Err(ClientError::PermissionDenied {
                method: "server.upsertKeybinding".into(),
            });
        }
        if changed
            && normalized
                .as_ref()
                .is_some_and(|k| crate::view_logic::shell::keybindings::parse_shortcut(k).is_none())
        {
            return Err(ClientError::InvalidPayload("Invalid keybinding.".into()));
        }
        let patch = if config.environment.capabilities.project_settings_overrides == Some(true) {
            let mut entry = settings
                .project_settings_overrides
                .get(&c.project.id)
                .map(serde_json::to_value)
                .transpose()?
                .unwrap_or(json!({}));
            entry["defaultProjectScripts"] = serde_json::to_value(&after)?;
            json!({"projectSettingsOverrides":{c.project.id.0.clone():entry}})
        } else {
            json!({"projectScriptOverrides":{c.project.id.0.clone():after}})
        };
        let input = serde_json::from_value(json!({"patch":patch}))?;
        let saved = endpoint
            .command::<WsServerUpdateSettingsRpc>(input)
            .await
            .map_err(RpcCallError::into_client_error)?;
        self.service.state.scripts.saved.borrow_mut().insert(
            self.key.environment.clone(),
            (config.clone(), Rc::new(saved)),
        );
        self.service.state.scripts.changed.notify();
        if !changed {
            return Ok(());
        }
        if !endpoint.available::<WsServerUpsertKeybindingRpc>(&probe) {
            return Err(ClientError::Rejected{message:if deleting{"The script was deleted, but its keyboard shortcut could not be removed because permission changed."}else{"The script was saved, but this connection can no longer change keyboard shortcuts."}.into(),detail:None});
        }
        if normalized.is_none() && !before.iter().any(|s| s.id.0 == script_id) {
            return Ok(());
        }
        let retained = deleting
            && (settings
                .default_project_scripts
                .iter()
                .any(|s| s.id.0 == script_id)
                || settings
                    .project_settings_overrides
                    .iter()
                    .any(|(id, entry)| {
                        id != &c.project.id
                            && entry
                                .default_project_scripts
                                .as_ref()
                                .is_some_and(|scripts| scripts.iter().any(|s| s.id.0 == script_id))
                    })
                || self
                    .service
                    .runtime
                    .store()
                    .shared_shell(&self.key.environment)
                    .is_some_and(|shell| {
                        shell
                            .projects
                            .iter()
                            .filter(|p| p.id != c.project.id)
                            .any(|p| {
                                resolve_project_scripts(&settings, p)
                                    .iter()
                                    .any(|s| s.id.0 == script_id)
                            })
                    }));
        if normalized.is_none() && retained {
            return Ok(());
        }
        let prior = config
            .keybindings
            .0
            .iter()
            .filter(|b| b.command.0 == command && b.when_ast.as_ref().is_none_or(Option::is_none))
            .map(binding_key)
            .collect::<Vec<_>>();
        let remove = if normalized.is_some() {
            prior.len().saturating_sub(1)
        } else {
            prior.len()
        };
        for key in prior.iter().take(remove) {
            endpoint
                .command::<WsServerRemoveKeybindingRpc>(serde_json::from_value(
                    json!({"key":key,"command":command}),
                )?)
                .await
                .map_err(RpcCallError::into_client_error)?;
        }
        if let Some(key) = normalized {
            let mut input = json!({"key":key,"command":command});
            if let Some(previous) = prior.last().filter(|p| **p != key) {
                input["replace"] = json!({"key":previous,"command":command});
            }
            endpoint
                .command::<WsServerUpsertKeybindingRpc>(serde_json::from_value(input)?)
                .await
                .map_err(RpcCallError::into_client_error)?;
        }
        Ok(())
    }
    pub fn add(
        &self,
        input: NewProjectScriptInput,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let scripts = self.clone();
        async move {
            let _lease = scripts
                .service
                .state
                .scripts
                .scheduler
                .acquire(CheckoutKey {
                    environment: scripts.key.environment.clone(),
                    cwd: "settings".into(),
                })
                .await;
            let before = scripts.snapshot().await?.scripts;
            let id = policy::next_project_script_id(
                &input.script.name.0,
                before.iter().map(|s| s.id.0.as_str()),
                scripts.service.platform.now_epoch_ms(),
            );
            let mut after = before
                .iter()
                .map(|s| policy::release_claimed_roles(s, &input.script))
                .collect::<Vec<_>>();
            after.push(policy::build_project_script(
                TrimmedNonEmptyString(id.clone()),
                input.script,
            ));
            scripts.persist(before, after, &id, input.keybinding).await
        }
        .boxed_local()
    }
    pub fn update(
        &self,
        id: String,
        input: NewProjectScriptInput,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let scripts = self.clone();
        async move {
            let _lease = scripts
                .service
                .state
                .scripts
                .scheduler
                .acquire(CheckoutKey {
                    environment: scripts.key.environment.clone(),
                    cwd: "settings".into(),
                })
                .await;
            let before = scripts.snapshot().await?.scripts;
            if !before.iter().any(|s| s.id.0 == id) {
                return Err(ClientError::Rejected {
                    message: "Script not found.".into(),
                    detail: None,
                });
            }
            let replacement = policy::build_project_script(
                TrimmedNonEmptyString(id.clone()),
                input.script.clone(),
            );
            let after = before
                .iter()
                .map(|s| {
                    if s.id.0 == id {
                        replacement.clone()
                    } else {
                        policy::release_claimed_roles(s, &input.script)
                    }
                })
                .collect();
            scripts.persist(before, after, &id, input.keybinding).await
        }
        .boxed_local()
    }
    pub fn delete(&self, id: String) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let scripts = self.clone();
        async move {
            let _lease = scripts
                .service
                .state
                .scripts
                .scheduler
                .acquire(CheckoutKey {
                    environment: scripts.key.environment.clone(),
                    cwd: "settings".into(),
                })
                .await;
            let before = scripts.snapshot().await?.scripts;
            let after = before.iter().filter(|s| s.id.0 != id).cloned().collect();
            scripts.persist(before, after, &id, Some(None)).await
        }
        .boxed_local()
    }
    pub fn import(
        &self,
        file: T3ProjectFileScript,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let scripts = self.clone();
        async move {
            let snapshot = scripts.snapshot().await?;
            let input = import_input(file, snapshot.can_edit_keybindings);
            let result = scripts.add(input.clone()).await;
            match &result {
                Ok(()) => {
                    scripts
                        .service
                        .state
                        .scripts
                        .failed_imports
                        .borrow_mut()
                        .remove(&scripts.key);
                }
                Err(ClientError::Cancelled) => (),
                Err(error) => {
                    scripts
                        .service
                        .state
                        .scripts
                        .failed_imports
                        .borrow_mut()
                        .insert(
                            scripts.key.clone(),
                            FailedImport {
                                input,
                                error: error.to_string(),
                            },
                        );
                }
            }
            scripts.service.state.scripts.changed.notify();
            result
        }
        .boxed_local()
    }
    pub fn run(&self, id: String) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        let scripts = self.clone();
        async move {
            let snapshot = scripts.snapshot().await?;
            let script = snapshot
                .scripts
                .iter()
                .find(|s| s.id.0 == id)
                .ok_or(ClientError::Cancelled)?;
            if !snapshot.can_run {
                return Err(ClientError::PermissionDenied {
                    method: "terminal.open".into(),
                });
            }
            let c = scripts
                .service
                .workspace_context(&scripts.key, None)
                .ok_or(ClientError::Cancelled)?;
            let endpoint = scripts.service.session(&scripts.key.environment)?;
            let mut preferred = scripts
                .service
                .platform
                .get(StorageArea::Ordinary, preferred_key())
                .await?
                .and_then(|b| serde_json::from_slice::<BTreeMap<String, String>>(&b).ok())
                .unwrap_or_default();
            preferred.insert(c.project.id.0.clone(), id);
            scripts
                .service
                .platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: preferred_key(),
                        value: Some(serde_json::to_vec(&preferred)?),
                    }],
                )
                .await?;
            let terminal_id = format!(
                "script-{}",
                crate::operations::allocate_command_id(&scripts.service.platform, None)?.0
            );
            let cwd = c
                .worktree_path
                .clone()
                .unwrap_or_else(|| c.project.workspace_root.0.clone());
            let mut env = BTreeMap::from([(
                "T3CODE_PROJECT_ROOT".into(),
                c.project.workspace_root.0.clone(),
            )]);
            if let Some(path) = &c.worktree_path {
                env.insert("T3CODE_WORKTREE_PATH".into(), path.clone());
            }
            endpoint
                .command::<WsTerminalOpenRpc>(TerminalOpenInput {
                    thread_id: TrimmedNonEmptyString(scripts.key.thread.0.clone()),
                    terminal_id: TrimmedNonEmptyString(terminal_id.clone()),
                    cwd: TrimmedNonEmptyString(cwd),
                    worktree_path: Some(c.worktree_path.map(TrimmedNonEmptyString)),
                    cols: Some(Some(120)),
                    rows: Some(Some(30)),
                    env: Some(Some(env)),
                    provider_instance_id: None,
                    stop_on_settle: script.stop_on_settle,
                })
                .await
                .map_err(RpcCallError::into_client_error)?;
            endpoint
                .command::<WsTerminalWriteRpc>(TerminalWriteInput {
                    thread_id: TrimmedNonEmptyString(scripts.key.thread.0.clone()),
                    terminal_id: TrimmedNonEmptyString(terminal_id.clone()),
                    data: format!(" {}\r", script.command.0),
                })
                .await
                .map_err(RpcCallError::into_client_error)?;
            scripts.service.state.scripts.changed.notify();
            Ok(terminal_id)
        }
        .boxed_local()
    }
}
