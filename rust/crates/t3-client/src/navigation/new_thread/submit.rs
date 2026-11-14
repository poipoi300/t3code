use super::*;

struct Busy(Rc<NewThreadState>, String);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.borrow_mut().remove(&self.1);
        self.0.changed.invalidate();
    }
}
impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
    pub async fn submit(
        &self,
        id: String,
        revision: u64,
        mode: ComposerDispatchMode,
    ) -> Result<ThreadKey, ClientError> {
        let result = self.submit_inner(&id, revision, mode).await;
        if let Err(error) = &result {
            self.state
                .errors
                .borrow_mut()
                .insert(Target::Draft(id), error.to_string());
            self.state.changed.invalidate();
        }
        result
    }
    async fn submit_inner(
        &self,
        id: &str,
        revision: u64,
        mode: ComposerDispatchMode,
    ) -> Result<ThreadKey, ClientError> {
        self.drafts.load().await?;
        let authority = self.drafts.view(id);
        if authority.revision != revision {
            return Err(ClientError::Cancelled);
        }
        if !authority.writable {
            return Err(ClientError::InvalidPayload(
                "The composer draft is not writable.".into(),
            ));
        }
        let key = self.draft_key(id).ok_or(ClientError::Cancelled)?;
        if !self.can_operate(&key) {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        if !self.runtime.pending_messages(&key).is_empty()
            || !self.state.busy.borrow_mut().insert(id.into())
        {
            return Err(ClientError::InvalidPayload(
                "This draft is already being sent.".into(),
            ));
        }
        let _busy = Busy(self.state.clone(), id.into());
        self.state.changed.invalidate();
        let document = self.drafts.document();
        let saved = document.borrow().clone();
        let session = &saved["draftThreadsByThreadKey"][id];
        let project = self.project(&ProjectKey {
            environment: key.environment.clone(),
            project: crate::types::ProjectId(
                session["projectId"].as_str().unwrap_or_default().into(),
            ),
        })?;
        let config = self
            .runtime
            .store()
            .snapshot()
            .environments
            .get(&key.environment)
            .and_then(|e| e.config.clone())
            .ok_or(ClientError::Disconnected)?;
        let settings = policy::settings(
            &config,
            &project,
            &saved["draftsByThreadKey"][id],
            session,
            self.platform.now_epoch_ms(),
        )?;
        let attachments = self
            .drafts
            .attachments(&ComposerTarget::Draft {
                draft_id: id.into(),
                environment_id: Some(key.environment.clone()),
            })?
            .draft_attachments()?;
        let send = self
            .runtime
            .messages(&key.environment)
            .ok_or(ClientError::Disconnected)?;
        let mut changes =
            futures::stream::select(send.changes(), self.runtime.store().subscribe().map(|_| ()))
                .boxed_local();
        let creation = QueuedCreation {
            project_id: project.project.id,
            project_title: Some(project.project.title.0),
            project_cwd: Some(project.project.workspace_root.0),
            workspace_mode: serde_json::from_value(session["envMode"].clone())
                .unwrap_or(t3_contracts::environment::ThreadEnvMode::Local),
            branch: session["branch"].as_str().map(str::to_owned),
            worktree_path: session["worktreePath"].as_str().map(str::to_owned),
            start_from_origin: session["startFromOrigin"].as_bool(),
            pull_request_reference: session["checkoutPullRequest"]["reference"]
                .as_str()
                .map(str::to_owned),
        };
        if self.drafts.view(id).revision != revision {
            return Err(ClientError::Cancelled);
        }
        let message = send
            .submit(
                &key,
                authority.prompt.to_string(),
                SendOptions {
                    attachments,
                    settings: Some(settings),
                    creation: Some(creation),
                    dispatch_mode: Some(match mode {
                        ComposerDispatchMode::Auto | ComposerDispatchMode::Start => {
                            DispatchMode::Auto
                        }
                        ComposerDispatchMode::Queue => DispatchMode::Queue,
                        ComposerDispatchMode::Steer => DispatchMode::Steer,
                        ComposerDispatchMode::Restart => DispatchMode::Restart,
                    }),
                    ..Default::default()
                },
            )
            .await?;
        self.drafts.clear_revision(id, revision)?;
        let id_owned = id.to_owned();
        self.drafts.update(Box::new(move |doc| {
            web_drafts::mark_promoting(doc, &id_owned, None);
            Ok(())
        }))?;
        loop {
            if let Some(SendState::Restored { draft_key, reason }) = send.state(&message) {
                let id = id.to_owned();
                let session = session.clone();
                let original = saved["draftsByThreadKey"][&id].clone();
                self.drafts.update(Box::new(move |doc| {
                    web_drafts::mark_promoting(doc, &id, Some(&Value::Null));
                    if !web_has_user_content(&doc["draftsByThreadKey"][&id]) {
                        doc["draftsByThreadKey"][&id] = doc["draftsByThreadKey"]
                            .get(&draft_key)
                            .cloned()
                            .unwrap_or(original);
                    } else if doc["draftThreadsByThreadKey"].get(&draft_key).is_none() {
                        let mut recovery = session;
                        recovery["promotedTo"] = Value::Null;
                        doc["draftThreadsByThreadKey"][&draft_key] = recovery;
                    }
                    Ok(())
                }))?;
                self.drafts.flush().await?;
                send.dismiss_message(&message);
                return Err(ClientError::Rejected {
                    message: reason,
                    detail: None,
                });
            }
            if matches!(send.state(&message), Some(SendState::Accepted { .. }))
                && self
                    .shell(&key)
                    .as_ref()
                    .is_some_and(|s| web_drafts::promotion_ready(Some(s), false))
            {
                let id = id.to_owned();
                self.drafts.update(Box::new(move |doc| {
                    web_drafts::finalize_promotion(doc, &id);
                    Ok(())
                }))?;
                self.drafts.flush().await?;
                return Ok(key);
            }
            if changes.next().await.is_none() {
                return Err(ClientError::Disconnected);
            }
        }
    }
}
