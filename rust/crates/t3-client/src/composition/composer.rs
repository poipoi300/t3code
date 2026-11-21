use super::*;
use crate::thread_views::composer::{
    actions::ComposerActionsSession, draft_service::ComposerDraftService,
};

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    pub fn composer_citation_comments(
        &self,
        key: &ThreadKey,
    ) -> crate::thread_views::composer::citation_comments::ComposerCitationCommentContext<P> {
        crate::thread_views::composer::citation_comments::ComposerCitationCommentContext::new(
            self.composer_drafts(),
            key.clone(),
        )
    }
    pub fn composer_voice(
        &self,
    ) -> crate::thread_views::composer::voice_service::ComposerVoiceSession {
        crate::thread_views::composer::voice_service::ComposerVoiceSession::default()
    }
    pub fn composer_chips(
        &self,
        target: crate::thread_views::composer::target::ComposerTarget,
    ) -> crate::thread_views::composer::chips_service::ComposerChipsSession<P> {
        crate::thread_views::composer::chips_service::ComposerChipsSession::new(
            self.clone(),
            target,
        )
    }
    pub fn pending_user_input(
        &self,
        key: &ThreadKey,
    ) -> crate::thread_views::composer::questions_service::PendingUserInputSession<P> {
        if !self.inner.composer_questions.driver_bound() {
            let (handle, registration) = AbortHandle::new_pair();
            if self.inner.composer_questions.bind_driver(handle) {
                let weak = Rc::downgrade(&self.inner);
                let mut changes = futures::stream::select(
                    self.store().subscribe().map(Some),
                    self.composer_drafts().changes().map(|_| None),
                );
                (self.inner.spawn)(Abortable::new(async move {
                    let mut last_ready=false;
                    while let Some(change)=changes.next().await {
                        let Some(inner)=weak.upgrade() else {break;};
                        let runtime=ClientRuntime{inner};
                        let ready=runtime.composer_drafts().writable();
                        if change.is_none()&&ready==last_ready {continue;}
                        last_ready=ready;
                        for key in runtime.inner.composer_questions.active_keys() {
                            let relevant=change.as_ref().is_none_or(|change|change.scopes.iter().any(|scope|match scope {
                                crate::store::Scope::All=>true,
                                crate::store::Scope::Thread(changed)=>changed==&key,
                                crate::store::Scope::Environment(environment)=>environment==&key.environment,
                                crate::store::Scope::Shell(_)=>false,
                            }));
                            if !relevant {continue;}
                            crate::thread_views::composer::questions_service::PendingUserInputSession::new(
                                runtime.clone(),runtime.inner.platform.clone(),key,
                                runtime.inner.composer_questions.clone(),runtime.inner.spawn.clone(),
                            ).view();
                        }
                    }
                },registration).map(|_|()).boxed_local());
            }
        }
        crate::thread_views::composer::questions_service::PendingUserInputSession::new(
            self.clone(),
            self.inner.platform.clone(),
            key.clone(),
            self.inner.composer_questions.clone(),
            self.inner.spawn.clone(),
        )
    }
    pub fn composer_suggestions(
        &self,
        key: &ThreadKey,
        cwd: Option<String>,
    ) -> crate::thread_views::composer::suggestions::ComposerSuggestionsSession<P> {
        crate::thread_views::composer::suggestions::ComposerSuggestionsSession::new(
            self.clone(),
            key.clone(),
            cwd,
            self.inner.platform.now_epoch_ms() as f64,
        )
    }
    pub fn prompt_stash(
        &self,
        target: crate::thread_views::composer::target::ComposerTarget,
    ) -> crate::thread_views::composer::stash_service::PromptStashSession<P> {
        let state = self.inner.composer_stash.clone();
        let platform = self.inner.platform.clone();
        (self.inner.spawn)(
            async move {
                let _ = state.load(&platform).await;
            }
            .boxed_local(),
        );
        crate::thread_views::composer::stash_service::PromptStashSession::new(
            self.clone(),
            self.inner.platform.clone(),
            target,
            self.inner.composer_stash.clone(),
        )
    }
    pub fn composer_server_updates(&self) -> crate::services::updates::ServerUpdateService {
        self.inner.composer_updates.clone()
    }
    pub(crate) fn composer_update_dismissals(
        &self,
    ) -> crate::thread_views::composer::banners::UpdateDismissals {
        self.inner.composer_update_dismissals.clone()
    }
    pub fn update_composer_server(
        &self,
        environment: &EnvironmentId,
        input: t3_contracts::server::ServerSelfUpdateInput,
    ) -> LocalBoxFuture<'static, Result<t3_contracts::server::ServerSelfUpdateResult, ClientError>>
    {
        let this = self.clone();
        let environment = environment.clone();
        async move {
            let host = super::composer_updates::ComposerUpdateHost {
                runtime: this.clone(),
                environment: environment.clone(),
            };
            this.inner
                .composer_updates
                .run(environment, &host, &this.inner.platform, input)
                .await
        }
        .boxed_local()
    }
    pub fn composer_banners(
        &self,
        target: crate::thread_views::composer::target::ComposerTarget,
    ) -> crate::thread_views::composer::banners::ComposerBannersSession<P> {
        let session = crate::thread_views::composer::banners::ComposerBannersSession::new(
            self.clone(),
            self.inner.platform.clone(),
            target,
        );
        let load = session.clone();
        (self.inner.spawn)(async move { load.load_client_version().await }.boxed_local());
        session
    }
    pub(crate) fn set_composer_environment_enabled(
        &self,
        environment: &EnvironmentId,
        enabled: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        let environment = environment.clone();
        async move {
            let _lease = this.inner.registry.lock().await;
            let mut registry = this.load_registry().await?;
            let was_enabled = registry
                .get(&environment)
                .ok_or(ClientError::EnvironmentMismatch)?
                .registration()
                .enabled;
            registry
                .set_enabled_persisted(
                    &this.inner.platform,
                    &environment,
                    enabled,
                    Time::from_platform(&this.inner.platform),
                )
                .await?;
            // Disabling ends the driver's event loop. A fresh supervisor reads the
            // persisted registration when the user switches the machine back on.
            if enabled && !was_enabled {
                this.start_saved(&environment).await?;
            } else if let Some(client) = this.client(&environment) {
                if client.set_enabled(enabled).is_err() && enabled {
                    this.start_saved(&environment).await?;
                }
            } else if enabled {
                this.start_saved(&environment).await?;
            }
            Ok(())
        }
        .boxed_local()
    }
    pub fn composer_models(
        &self,
        key: &ThreadKey,
    ) -> crate::thread_views::composer::model_service::ComposerModelsSession<P> {
        crate::thread_views::composer::model_service::ComposerModelsSession::new(
            self.clone(),
            self.inner.platform.clone(),
            key.clone(),
            self.inner.composer_models.clone(),
        )
    }
    pub fn composer_drafts(&self) -> Rc<ComposerDraftService<P>> {
        self.inner.composer_drafts.clone()
    }
    pub fn composer_preferences(
        &self,
    ) -> Rc<crate::thread_views::composer::preferences::ComposerPreferences<P>> {
        self.inner.composer_preferences.clone()
    }

    pub fn composer_primary_actions(&self, key: &ThreadKey) -> ComposerActionsSession<P> {
        ComposerActionsSession::new(self.clone(), key.clone())
    }

    pub fn resume_thread(
        &self,
        key: &ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let runtime = self.clone();
        let key = key.clone();
        async move {
            crate::thread_views::composer::resume::resume_thread(
                &runtime,
                &runtime.inner.platform,
                &key,
                runtime.inner.source.clone(),
                &runtime.inner.composer_resumes,
            )
            .await
        }
        .boxed_local()
    }

    pub fn interrupt_thread(
        &self,
        key: &ThreadKey,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let runtime = self.clone();
        let key = key.clone();
        async move {
            let endpoint = runtime
                .client(&key.environment)
                .and_then(|c| c.session())
                .ok_or(ClientError::Disconnected)?;
            let supports = runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .and_then(|e| e.config.as_ref())
                .is_some_and(|c| c.environment.capabilities.thread_stop == Some(true));
            crate::thread_views::composer::interrupt::interrupt_thread(
                &runtime.inner.platform,
                runtime.store(),
                endpoint,
                &key,
                supports,
            )
            .await
        }
        .boxed_local()
    }
}
