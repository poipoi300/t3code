use super::*;
use crate::connection::Phase;
use t3_contracts::project::ProjectReadFileInput;

impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
    fn logical_project(&self, project: &EnvironmentProject) -> String {
        match projects::logical_project_key(project, &self.state.grouping.borrow(), false) {
            LogicalProjectKey::Repository(key) => key,
            LogicalProjectKey::Physical(key) => key.legacy_preference_key(),
            LogicalProjectKey::Scratch => "\0no-project".into(),
        }
    }
    pub(super) fn shell(&self, key: &ThreadKey) -> Option<Value> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(&key.environment)?
            .shell
            .snapshot
            .as_ref()?
            .threads
            .iter()
            .find(|t| t.id == key.thread)
            .and_then(|t| serde_json::to_value(t).ok())
    }
    fn reusable(&self, session: &Value) -> bool {
        session["promotedTo"].is_null()
            && policy::session_key(session).is_some_and(|key| self.shell(&key).is_none())
    }
    /// The caller owns route cancellation by dropping this future when its route leaves.
    pub async fn open_project(
        &self,
        key: ProjectKey,
        origin: Option<&str>,
        prompt: Option<String>,
    ) -> Result<String, ClientError> {
        self.drafts.load().await?;
        let project = self.project(&key)?;
        let snapshot = self.runtime.store().snapshot();
        let state = snapshot
            .environments
            .get(&key.environment)
            .ok_or(ClientError::Disconnected)?;
        let config = state.config.clone().ok_or(ClientError::Disconnected)?;
        let settings = policy::effective_settings(&config, &project)?;
        let logical = self.logical_project(&project);
        let document = self.drafts.document();
        let before = document.borrow().clone();
        let stored =
            web_sessions::by_logical_project(&before, &logical).map(|(id, _)| id.to_owned());
        let origin_session = origin.map(|id| before["draftThreadsByThreadKey"][id].clone());
        let ticket = self.state.next.get().wrapping_add(1);
        self.state.next.set(ticket);
        if let Some(id) = origin {
            self.state.attempts.borrow_mut().insert(id.into(), ticket);
        }
        let file = if settings.default_thread_env_mode.is_none()
            && state
                .connection
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Connected)
        {
            if let Some(endpoint) = self
                .runtime
                .client(&key.environment)
                .and_then(|c| c.session())
            {
                ProjectCommands::default()
                    .read_file(
                        &key.environment,
                        &endpoint,
                        &self.platform,
                        ProjectReadFileInput {
                            cwd: project.project.workspace_root.clone(),
                            relative_path: "t3.json".into(),
                            allow_missing: Some(Some(true)),
                        },
                    )
                    .await
                    .ok()
                    .filter(|f| f.exists != Some(Some(false)) && !f.truncated)
                    .and_then(|f| {
                        serde_json::from_str::<t3_contracts::t3_project_file::T3ProjectFile>(
                            &f.contents,
                        )
                        .ok()
                    })
                    .and_then(|f| serde_json::to_value(f).ok())
            } else {
                None
            }
        } else {
            None
        };
        self.project(&key)?;
        let current = document.borrow().clone();
        if let Some(id) = origin {
            let previous = origin_session.as_ref().unwrap_or(&Value::Null);
            let latest = &current["draftThreadsByThreadKey"][id];
            if self.state.attempts.borrow().get(id) != Some(&ticket)
                || latest["environmentId"] != previous["environmentId"]
                || latest["projectId"] != previous["projectId"]
            {
                return Err(ClientError::Cancelled);
            }
        }
        let destination = web_sessions::by_logical_project(&current, &logical)
            .filter(|(_, session)| self.reusable(session));
        let raced = destination.filter(|(id, _)| Some(*id) != stored.as_deref());
        let empty =
            destination.filter(|(id, _)| !web_has_user_content(&current["draftsByThreadKey"][*id]));
        let invested_origin = origin.filter(|id| {
            self.reusable(&current["draftThreadsByThreadKey"][*id])
                && web_has_user_content(&current["draftsByThreadKey"][*id])
        });
        let same_origin = origin.filter(|id| {
            self.reusable(&current["draftThreadsByThreadKey"][*id])
                && current["draftThreadsByThreadKey"][*id]["logicalProjectKey"] == logical
        });
        let id = invested_origin
            .or_else(|| empty.map(|(id, _)| id))
            .or(same_origin)
            .or_else(|| raced.map(|(id, _)| id))
            .map(|id| Ok(id.to_owned()))
            .unwrap_or_else(|| {
                operations::allocate_command_id(&self.platform, None).map(|id| id.0)
            })?;
        let existing = current["draftThreadsByThreadKey"].get(&id);
        let already_open = origin == Some(id.as_str());
        let thread = existing
            .and_then(|s| s["threadId"].as_str())
            .map(|id| Ok(id.to_owned()))
            .unwrap_or_else(|| {
                operations::allocate_command_id(&self.platform, None).map(|id| id.0)
            })?;
        let has_threads = snapshot
            .environments
            .get(&key.environment)
            .and_then(|e| e.shell.snapshot.as_ref())
            .is_some_and(|s| s.threads.iter().any(|t| t.project_id == key.project));
        let mode = policy::env_mode(
            &settings,
            &project,
            file.as_ref(),
            config.new_projects_root.as_ref().map(|r| r.0.as_str()),
            has_threads,
        );
        let now = crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms());
        let mut options = json!({"threadId":thread});
        if !already_open && raced.is_none() {
            options["branch"] = Value::Null;
            options["worktreePath"] = Value::Null;
            options["envMode"] = json!(mode);
            options["startFromOrigin"] = json!(
                mode == t3_contracts::environment::ThreadEnvMode::Worktree
                    && settings.new_worktrees_start_from_origin
            );
            options["runtimeMode"] = json!(settings.default_runtime_mode);
        }
        let source_shell = origin
            .and_then(|id| {
                snapshot.environments.iter().find_map(|(env, state)| {
                    state
                        .shell
                        .snapshot
                        .as_ref()?
                        .threads
                        .iter()
                        .find(|thread| format!("{}:{}", env.0, thread.id.0) == id)
                })
            })
            .cloned();
        let carry = origin
            .and_then(|id| web_models::ModelPicks::default().capture(&current, id))
            .map(|p| (*p).clone())
            .or_else(|| {
                source_shell
                    .as_ref()
                    .map(|thread| thread.model_selection.clone())
            });
        let carry_interaction = origin
            .and_then(|id| current["draftsByThreadKey"][id].get("interactionMode"))
            .cloned()
            .or_else(|| {
                source_shell
                    .as_ref()
                    .and_then(|thread| serde_json::to_value(thread.interaction_mode).ok())
            })
            .or_else(|| {
                origin.and_then(|id| {
                    current["draftThreadsByThreadKey"][id]
                        .get("interactionMode")
                        .cloned()
                })
            });
        if let Some(mode) = carry_interaction {
            options["interactionMode"] = mode;
        }
        let reference = ScopedProjectRef {
            environment_id: key.environment,
            project_id: key.project,
        };
        let result = id.clone();
        let default = if settings.default_model_selection.is_some()
            || carry.is_none() && current["stickyActiveProvider"].is_null()
        {
            policy::settings(
                &config,
                &project,
                &Value::Null,
                &Value::Null,
                self.platform.now_epoch_ms(),
            )
            .ok()
            .map(|s| s.model_selection)
            .filter(|pick| {
                settings.default_model_selection.is_some()
                    || crate::view_logic::shell::models::project_default(
                        &config.providers.0,
                        &settings,
                        Some(&project.project.id),
                    )
                    .as_ref()
                        != Some(pick)
            })
        } else {
            None
        };
        self.drafts.update(Box::new(move |doc| {
            web_sessions::set_logical_project(
                doc,
                &logical,
                &reference,
                &id,
                &options,
                &now,
                &BTreeSet::new(),
            );
            if doc["draftsByThreadKey"][&id]["modelSelectionExplicit"] != true {
                let mut models = web_models::ModelPicks::default();
                let own = models.capture(doc, &id);
                if own.is_none() {
                    models.apply_sticky(doc, &id);
                }
                let selection = default.or_else(|| (!already_open).then_some(carry).flatten());
                if let Some(selection) = selection {
                    models.set(doc, &id, Some(&selection), true, false);
                }
            }
            if let Some(prompt) = prompt
                && !web_has_user_content(&doc["draftsByThreadKey"][&id])
            {
                web_models::set_prompt(doc, &id, &prompt);
            }
            Ok(())
        }))?;
        self.drafts.flush().await?;
        Ok(result)
    }
}
