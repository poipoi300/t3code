//! Composes the model picker from the existing catalog, draft and guarded destination commands.
use super::{
    model_traits::{ModelTraitsInput, ModelTraitsView, resolve_model_traits},
    send::Changes,
    web_models::{self, ModelPicks},
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint, SessionEndpoint},
    types::{EnvironmentId, ThreadKey},
    view_logic::{chat_view, shell::models},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{
    model::*,
    model_selection::ModelSelection,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::ProviderInstanceId,
    rpc::{
        WsOrchestrationV2DispatchCommandRpc, WsServerRefreshProvidersRpc, WsServerUpdateSettingsRpc,
    },
    server::{ServerConfig, ServerProvider},
    settings::ModelFavorites,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelCatalogState {
    Ready,
    Loading,
    Offline,
    Denied(String),
    Unsupported(String),
    Error(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerModelProvider {
    pub snapshot: ServerProvider,
    pub enabled: bool,
    pub available: bool,
    pub ready: bool,
    pub display_name: String,
    pub continuation_group: Option<String>,
    pub accent: Option<String>,
    pub acp_agent: Option<String>,
    pub acp_icon: Option<String>,
    pub options: Vec<models::ModelOption>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerModelsView {
    pub providers: Vec<ComposerModelProvider>,
    pub selection: ModelSelection,
    pub favorites: ModelFavorites,
    pub locked_provider: Option<String>,
    pub locked_continuation_group: Option<String>,
    pub disabled_models: BTreeMap<(ProviderInstanceId, String), String>,
    pub traits: ModelTraitsView,
    pub state: ModelCatalogState,
    pub can_refresh: bool,
}
#[derive(Clone)]
struct Favorite {
    desired: bool,
    pending: bool,
    baseline: Option<ModelFavorites>,
    serial: u64,
}
#[derive(Default)]
pub struct ComposerModelState {
    picks: RefCell<ModelPicks>,
    applied: RefCell<BTreeMap<ThreadKey, Rc<ModelSelection>>>,
    lanes: RefCell<BTreeMap<(EnvironmentId, String), crate::services::resources::LocalLimiter>>,
    refresh_unavailable: RefCell<BTreeMap<EnvironmentId,crate::types::Generation>>,
    favorites: RefCell<BTreeMap<(EnvironmentId, ProviderInstanceId, String), Favorite>>,
    failures: RefCell<BTreeMap<EnvironmentId, (crate::types::Generation, ModelCatalogState)>>,
    views: RefCell<BTreeMap<ThreadKey, Weak<ComposerModelsView>>>,
    serial: Cell<u64>,
    changes: Changes,
}
struct FavoriteAttempt {
    state: Rc<ComposerModelState>,
    key: (EnvironmentId, ProviderInstanceId, String),
    serial: u64,
    prior: Option<Favorite>,
    settled: bool,
}
impl Drop for FavoriteAttempt {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let mut favorites = self.state.favorites.borrow_mut();
        if favorites
            .get(&self.key)
            .is_some_and(|p| p.serial == self.serial)
        {
            if let Some(prior) = self.prior.clone().filter(|p| !p.pending) {
                favorites.insert(self.key.clone(), prior);
            } else {
                favorites.remove(&self.key);
            }
        }
        drop(favorites);
        self.state.changes.notify();
    }
}
#[derive(Clone)]
pub struct ComposerModelsSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    key: ThreadKey,
    state: Rc<ComposerModelState>,
}
impl<P: Platform + Clone + 'static> ComposerModelsSession<P> {
    pub fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        key: ThreadKey,
        state: Rc<ComposerModelState>,
    ) -> Self {
        Self {
            runtime,
            platform,
            key,
            state,
        }
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        futures::stream::select_all(vec![
            self.runtime
                .store()
                .subscribe_environment(&self.key.environment)
                .map(|_| ())
                .boxed_local(),
            self.runtime.composer_drafts().changes(),
            self.runtime.composer_preferences().changes(),
            self.state.changes.subscribe(),
        ])
        .boxed_local()
    }
    fn endpoint(&self) -> Result<SessionEndpoint, ClientError> {
        self.runtime
            .client(&self.key.environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)
    }
    fn config(&self) -> Option<Rc<ServerConfig>> {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(&self.key.environment)?
            .config
            .clone()
    }
    fn failure(&self, error: &ClientError) {
        let state = match error {
            ClientError::Unsupported(reason) => ModelCatalogState::Unsupported((*reason).into()),
            _ => return,
        };
        self.state
            .failures
            .borrow_mut()
            .insert(self.key.environment.clone(), (self.generation(), state));
        self.state.changes.notify();
    }
    fn generation(&self) -> crate::types::Generation {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(&self.key.environment)
            .map_or(crate::types::Generation(0), |e| e.session)
    }
    fn permit(&self, lane: String) -> crate::services::resources::AcquirePermit {
        self.state
            .lanes
            .borrow_mut()
            .entry((self.key.environment.clone(), lane))
            .or_insert_with(|| crate::services::resources::LocalLimiter::new(1))
            .acquire()
    }
    pub fn snapshot(&self) -> Rc<ComposerModelsView> {
        let draft_id=self.runtime.composer_drafts().thread_draft_key(&self.key);
        let config = self.config();
        let thread = self.runtime.store().shared_thread(&self.key);
        let pick = self.state.applied.borrow().get(&self.key).cloned();
        if let (Some(thread), Some(pick)) = (&thread, pick) {
            let mut released = false;
            let _ = self.runtime.composer_drafts().update(|document| {
                released = self.state.picks.borrow_mut().reconcile(
                    document,
                    &draft_id,
                    &pick,
                    web_models::Observation {
                        live: true,
                        pending: false,
                        visible: Some(&thread.thread.model_selection),
                        authoritative: Some(&thread.thread.model_selection),
                    },
                );
                Ok(())
            });
            if released {
                self.state.applied.borrow_mut().remove(&self.key);
            }
        }
        let document = self.runtime.composer_drafts().document();
        let document = document.borrow();
        let key = draft_id;
        let draft = &document["draftsByThreadKey"][&key];
        let server_selection = thread.as_ref().map(|p| &p.thread.model_selection);
        let default_selection = ModelSelection {
            instance_id: ProviderInstanceId("t3code_no_provider".into()),
            model: String::new(),
            options: None,
        };
        let mut selection = draft["activeProvider"]
            .as_str()
            .and_then(|id| {
                serde_json::from_value(draft["modelSelectionByProvider"][id].clone()).ok()
            })
            .unwrap_or_else(|| server_selection.cloned().unwrap_or(default_selection));
        let legacy = self.runtime.composer_preferences().snapshot();
        let project = thread.as_ref().and_then(|p| match &p.thread.project_id {
            IdOrIndex::Id(id) => Some(id),
            _ => None,
        });
        let projection = self
            .runtime
            .store()
            .snapshot()
            .environments
            .get(&self.key.environment)
            .and_then(|e| e.threads.get(&self.key.thread))
            .and_then(|t| t.projection.clone());
        let started = projection.as_ref().is_some_and(|p| {
            !p.runs.is_empty() || !p.turn_items.is_empty() || !p.provider_sessions.is_empty()
        });
        let handoff = crate::thread_views::threads::workflows::thread_supports_provider_handoff(
            projection.as_deref(),
        );
        let locked_provider = config
            .as_ref()
            .and_then(|c| {
                chat_view::derive_locked_provider(
                    started,
                    projection.as_ref().map_or(0, |p| p.turn_items.len() as u64),
                    started,
                    None,
                    None,
                    server_selection.map(|s| s.instance_id.0.as_str()),
                    &c.providers.0,
                )
            })
            .filter(|_| !handoff)
            .map(|p| p.0);
        let mut providers = vec![];
        let mut disabled = BTreeMap::new();
        let mut favorites = vec![];
        let mut locked_group = None;
        let mut descriptors = vec![];
        let mut unavailable = false;
        if let Some(config) = &config {
            let settings = models::Settings {
                server: &config.settings,
                legacy_preferences: Some(&legacy.provider_model_preferences),
            };
            let picks = draft["modelSelectionByProvider"].as_object().map(|p| {
                p.iter()
                    .filter_map(|(id, v)| {
                        serde_json::from_value::<ModelSelection>(v.clone())
                            .ok()
                            .map(|v| (ProviderInstanceId(id.clone()), v))
                    })
                    .collect::<BTreeMap<_, _>>()
            });
            let entries = models::entries_for(&config.providers.0, &config.settings, project);
            let enabled = entries
                .iter()
                .map(|entry| {
                    let mut snapshot = entry.snapshot.clone();
                    snapshot.enabled = entry.enabled;
                    snapshot
                })
                .collect::<Vec<_>>();
            let candidates = enabled
                .iter()
                .map(|snapshot| chat_view::ProviderInstanceEntry {
                    snapshot,
                    is_available: snapshot.availability
                        != Some(Some(
                            t3_contracts::server::ServerProviderAvailability::Unavailable,
                        )),
                    continuation_group_key: snapshot
                        .continuation
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|c| c.group_key.0.as_str()),
                })
                .collect::<Vec<_>>();
            let requested = selection.instance_id.clone();
            let locked = locked_provider
                .as_ref()
                .map(|driver| t3_contracts::provider_instance::ProviderDriverKind(driver.clone()));
            let resolved = chat_view::resolve_composer_provider_selection(
                &candidates,
                &[Some(&requested), server_selection.map(|s| &s.instance_id)],
                locked.as_ref(),
                server_selection.map(|s| &s.instance_id),
            );
            locked_group = resolved.locked_continuation_group_key.map(str::to_owned);
            let selected = resolved
                .selected_provider_entry
                .map(|entry| entry.snapshot.instance_id.clone());
            let provider = selected
                .as_ref()
                .and_then(|id| config.providers.0.iter().find(|p| &p.instance_id == id));
            if let Some(provider) = provider {
                selection.instance_id = provider.instance_id.clone();
                let project_selection =
                    models::project_default(&config.providers.0, &config.settings, project);
                let effective = models::composer_model_state(models::ComposerModelInput {
                    draft_selections: picks.as_ref(),
                    providers: &config.providers.0,
                    driver: &provider.driver.0,
                    selected_instance: Some(&selection.instance_id),
                    requested_instance: Some(&requested),
                    thread_selection: server_selection,
                    project_selection: project_selection.as_ref(),
                    settings: &settings,
                });
                selection.model = effective.selected_model;
                selection.options = effective.options.get(&selection.instance_id).cloned();
                descriptors = models::model_capabilities(provider, Some(&selection.model), false)
                    .option_descriptors
                    .and_then(|v| v)
                    .unwrap_or_default();
                unavailable = matches!(provider.driver.0.as_str(), "opencode" | "antigravity")
                    && !provider.models.iter().any(|m| m.slug.0 == selection.model);
            } else {
                selection = models::no_provider_selection();
            }
            let options = models::options_by_instance(
                &settings,
                &config.providers.0,
                Some(&selection),
                false,
            );
            for entry in models::entries_for(&config.providers.0, &config.settings, project) {
                for option in options
                    .get(&entry.snapshot.instance_id)
                    .into_iter()
                    .flatten()
                {
                    let next = ModelSelection {
                        instance_id: entry.snapshot.instance_id.clone(),
                        model: option.slug.clone(),
                        options: None,
                    };
                    if let Some(current) = server_selection
                        && let Some(reason) = chat_view::model_disabled_reason(
                            started,
                            handoff,
                            current,
                            Some(&current.instance_id),
                            &next,
                        )
                    {
                        disabled.insert((next.instance_id, next.model), reason);
                    }
                }
                providers.push(ComposerModelProvider {
                    snapshot: entry.snapshot.clone(),
                    enabled: entry.enabled,
                    available: entry.is_available,
                    ready: entry.ready(),
                    display_name: entry.display_name,
                    continuation_group: entry.continuation_group_key.map(str::to_owned),
                    accent: entry.accent,
                    acp_agent: entry.acp_agent,
                    acp_icon: entry.acp_icon,
                    options: options
                        .get(&entry.snapshot.instance_id)
                        .cloned()
                        .unwrap_or_default(),
                });
            }
            favorites = config
                .settings
                .favorites
                .clone()
                .unwrap_or_else(|| legacy.favorites.clone());
            self.state
                .favorites
                .borrow_mut()
                .retain(|(env, instance, model), pending| {
                    if env != &self.key.environment {
                        return true;
                    }
                    if !pending.pending && pending.baseline != config.settings.favorites {
                        return false;
                    }
                    favorites.retain(|favorite| {
                        &favorite.provider != instance || &favorite.model.0 != model
                    });
                    if pending.desired {
                        favorites.push(t3_contracts::settings::ModelFavorite {
                            provider: instance.clone(),
                            model: t3_contracts::base_schemas::TrimmedNonEmptyString(model.clone()),
                        });
                    }
                    true
                });
        }
        if unavailable {
            descriptors=selection.options.as_ref().into_iter().flat_map(|o|&o.0).filter_map(|o| {
                let raw=match &o.value{ProviderOptionSelectionValue::Boolean(value)=>json!({"id":o.id,"type":"boolean","label":o.id,"currentValue":value}),ProviderOptionSelectionValue::String(value)=>json!({"id":o.id,"type":"select","label":o.id,"currentValue":value,"options":[{"id":value,"label":value}]})};serde_json::from_value(raw).ok()
            }).collect();
        }
        let reported = projection.as_ref().and_then(|p| {
            crate::thread_views::threads::execution::derive_reported_model_selection(p)
        });
        let traits = resolve_model_traits(&ModelTraitsInput {
            descriptors: &descriptors,
            selection: Some(&selection),
            reported,
            prompt: draft["prompt"].as_str().unwrap_or_default(),
            allow_prompt_injected_effort: true,
            unavailable,
        });
        let state = if self.endpoint().is_err() {
            ModelCatalogState::Offline
        } else if config.is_none() {
            ModelCatalogState::Loading
        } else if let Some((_, failure)) = self
            .state
            .failures
            .borrow()
            .get(&self.key.environment)
            .filter(|(generation, _)| *generation == self.generation())
        {
            failure.clone()
        } else {
            let command = serde_json::from_value(
                json!({"type":"thread.model-selection.set","commandId":"availability","threadId":self.key.thread,"modelSelection":selection}),
            );
            if command.ok().is_some_and(|input| {
                self.endpoint().is_ok_and(|endpoint| {
                    endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&input)
                })
            }) {
                ModelCatalogState::Ready
            } else {
                ModelCatalogState::Denied("This connection cannot change threads.".into())
            }
        };
        let can_refresh = config.as_ref().is_some_and(|config| {
            config.environment.capabilities.model_catalog_refresh == Some(true)
        }) && !self
            .state
            .refresh_unavailable
            .borrow()
              .get(&self.key.environment).is_some_and(|generation|*generation==self.generation())
            && self.endpoint().is_ok_and(|endpoint| {
                serde_json::from_value(json!({"modelCatalogOnly":true}))
                    .is_ok_and(|input| endpoint.available::<WsServerRefreshProvidersRpc>(&input))
            });
        let view = ComposerModelsView {
            providers,
            selection,
            favorites,
            locked_provider,
            locked_continuation_group: locked_group,
            disabled_models: disabled,
            traits,
            state,
            can_refresh,
        };
        let mut views = self.state.views.borrow_mut();
        if let Some(prior) = views
            .get(&self.key)
            .and_then(Weak::upgrade)
            .filter(|prior| **prior == view)
        {
            return prior;
        }
        let view = Rc::new(view);
        views.retain(|_, view| view.strong_count() > 0);
        views.insert(self.key.clone(), Rc::downgrade(&view));
        view
    }
    pub fn set_model_selection(
        &self,
        selection: ModelSelection,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.set_selection(selection, true)
    }
    pub fn set_model_options(
        &self,
        selection: ModelSelection,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        if self.snapshot().selection.instance_id != selection.instance_id
            || self.snapshot().selection.model != selection.model
        {
            return futures::future::ready(Err(ClientError::Cancelled)).boxed_local();
        }
        self.set_selection(selection, false)
    }
    pub fn change_model_trait(
        &self,
        instance: ProviderInstanceId,
        model: String,
        change: ProviderOptionSelection,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        async move {
            let view = this.snapshot();
            if view.selection.instance_id != instance || view.selection.model != model {
                return Err(ClientError::Cancelled);
            }
            let drafts = this.runtime.composer_drafts();
            let draft = drafts.view(&this.key);
            let planned = super::model_traits::plan_model_trait_change(
                &ModelTraitsInput {
                    descriptors: &view.traits.descriptors,
                    selection: Some(&view.selection),
                    reported: None,
                    prompt: &draft.prompt,
                    allow_prompt_injected_effort: true,
                    unavailable: view.traits.unavailable,
                },
                change,
            )
            .ok_or(ClientError::Cancelled)?;
            let mut selection = view.selection.clone();
            selection.options = planned.options;
            if planned.prompt != draft.prompt.as_ref() {
                let removed = draft
                    .prompt
                    .encode_utf16()
                    .count()
                    .saturating_sub(planned.prompt.encode_utf16().count());
                drafts.set_prompt(
                    &this.key,
                    planned.prompt,
                    draft.cursor.saturating_sub(removed),
                )?;
            }
            this.set_model_options(selection).await
        }
        .boxed_local()
    }
    fn set_selection(
        &self,
        mut selection: ModelSelection,
        replace: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        async move {
            let endpoint=this.endpoint()?;let view=this.snapshot();
            if !replace && (view.selection.instance_id!=selection.instance_id || view.selection.model!=selection.model) {return Err(ClientError::Cancelled);}
            if replace && selection.options.is_none() && view.selection.instance_id==selection.instance_id && view.selection.model==selection.model {selection.options=view.selection.options.clone();}
            let command_id=crate::operations::allocate_command_id(&this.platform,None)?;
            let command:OrchestrationV2Command=serde_json::from_value(json!({"type":"thread.model-selection.set","commandId":command_id,"threadId":this.key.thread,"modelSelection":selection}))?;
            if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command){return Err(ClientError::PermissionDenied{method:"orchestration.dispatchCommand".into()});}
            if let Some(reason)=view.disabled_models.get(&(selection.instance_id.clone(),selection.model.clone())){return Err(ClientError::Rejected{message:reason.clone(),detail:None});}
            let provider=view.providers.iter().find(|p|p.snapshot.instance_id==selection.instance_id&&p.enabled&&p.available).ok_or(ClientError::Unsupported("model selection"))?;
            if !provider.options.iter().any(|m|m.slug==selection.model) && !(view.selection.instance_id==selection.instance_id&&view.selection.model==selection.model){return Err(ClientError::Cancelled);}
            let drafts=this.runtime.composer_drafts();let key=drafts.thread_draft_key(&this.key);
            if replace&&selection.options.is_none(){selection.options=serde_json::from_value(drafts.document().borrow()["stickyOptionsByModelByProvider"][&selection.instance_id.0][&selection.model].clone()).ok();}
            let mut pick=None;
            drafts.update(|document| {pick=this.state.picks.borrow_mut().set(document,&key,Some(&selection),replace,true);web_models::set_sticky(document,&selection);Ok(())})?;
            let same_instance=this.runtime.store().shared_thread(&this.key).is_some_and(|p|p.thread.model_selection.instance_id==selection.instance_id);
            if !same_instance {drafts.flush().await?;return Ok(());}
            let command=serde_json::from_value(json!({"type":"thread.model-selection.set","commandId":command_id,"threadId":this.key.thread,"modelSelection":selection}))?;
            let _permit=this.permit(format!("thread:{}",this.key.thread.0)).await;
            match this.endpoint()?.command::<WsOrchestrationV2DispatchCommandRpc>(command).await {
                Ok(_)=>{this.state.failures.borrow_mut().remove(&this.key.environment);if let Some(pick)=pick {this.state.applied.borrow_mut().insert(this.key.clone(),pick);}drafts.flush().await?;this.state.changes.notify();Ok(())},
                Err(error)=>{let error=error.into_client_error();this.failure(&error);Err(error)}
            }
        }.boxed_local()
    }
    pub fn toggle_model_favorite(
        &self,
        instance: ProviderInstanceId,
        model: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        async move {
            let config=this.config().ok_or(ClientError::Disconnected)?;
            if config.settings.favorites.is_none()||config.settings.provider_model_preferences.is_none(){this.runtime.composer_preferences().load().await?;}
            let legacy=this.runtime.composer_preferences().snapshot();let id=(this.key.environment.clone(),instance.clone(),model.clone());
            let prior=this.state.favorites.borrow().get(&id).cloned();
            let favorite=prior.as_ref().filter(|p|p.pending||p.baseline==config.settings.favorites).map_or_else(||config.settings.favorites.as_ref().unwrap_or(&legacy.favorites).iter().any(|f|f.provider==instance&&f.model.0==model),|p|p.desired);
            let serial=this.state.serial.get()+1;this.state.serial.set(serial);
            let mut patch=json!({"setModelFavorites":[{"provider":instance,"model":model,"favorite":!favorite}]});
            let mut migrate=json!({});
            if config.settings.favorites.is_none()&&!legacy.favorites.is_empty(){migrate["favorites"]=json!(legacy.favorites);}
            if config.settings.provider_model_preferences.is_none()&&!legacy.provider_model_preferences.is_empty(){migrate["providerModelPreferences"]=json!(legacy.provider_model_preferences);}
            if migrate.as_object().is_some_and(|m|!m.is_empty()){patch["migrateModelPreferences"]=migrate;}
            let input=serde_json::from_value(json!({"patch":patch}))?;let endpoint=this.endpoint()?;
            if !endpoint.available::<WsServerUpdateSettingsRpc>(&input){return Err(ClientError::PermissionDenied{method:"server.updateSettings".into()});}
            this.state.favorites.borrow_mut().insert(id.clone(),Favorite{desired:!favorite,pending:true,baseline:config.settings.favorites.clone(),serial});this.state.changes.notify();
            let mut attempt=FavoriteAttempt{state:this.state.clone(),key:id.clone(),serial,prior:prior.clone(),settled:false};
            let _permit=this.permit("config".into()).await;
            let result=match this.endpoint(){Ok(endpoint)=>endpoint.command::<WsServerUpdateSettingsRpc>(input).await.map_err(RpcCallError::into_client_error),Err(error)=>Err(error)};
            let mut favorites=this.state.favorites.borrow_mut();
            if favorites.get(&id).is_some_and(|p|p.serial==serial){if result.is_ok(){if let Some(pending)=favorites.get_mut(&id){pending.pending=false;}}else if let Some(prior)=prior.filter(|p|!p.pending){favorites.insert(id,prior);}else{favorites.remove(&id);}}
            drop(favorites);attempt.settled=true;this.state.changes.notify();result.map(|_|())
        }.boxed_local()
    }
    pub fn refresh_model_catalog(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let this = self.clone();
        async move {
            let config = this.config().ok_or(ClientError::Disconnected)?;
            if config.environment.capabilities.model_catalog_refresh != Some(true)
                || this.state.refresh_unavailable.borrow().get(&this.key.environment).is_some_and(|generation|*generation==this.generation()) {
                return Err(ClientError::Unsupported("model catalog refresh"));
            }
            let endpoint = this.endpoint()?;
            let input = serde_json::from_value(json!({"modelCatalogOnly":true}))?;
            if !endpoint.available::<WsServerRefreshProvidersRpc>(&input) {
                return Err(ClientError::PermissionDenied {
                    method: "server.refreshProviders".into(),
                });
            }
            match endpoint.command::<WsServerRefreshProvidersRpc>(input).await {
                Ok(_) => {
                    this.state
                        .failures
                        .borrow_mut()
                        .remove(&this.key.environment);
                    this.state.changes.notify();
                    Ok(())
                }
                Err(error) => {
                    let error = error.into_client_error();
                    if matches!(error, ClientError::Unsupported(_)) {
                        this.state
                            .refresh_unavailable
                            .borrow_mut()
                            .insert(this.key.environment.clone(),this.generation());
                        this.state.changes.notify();
                    }
                    Err(error)
                }
            }
        }
        .boxed_local()
    }
}
