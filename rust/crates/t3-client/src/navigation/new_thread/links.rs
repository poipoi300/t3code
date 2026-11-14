use super::*;
use crate::{
    navigation::{
        link_targets,
        prompt_links::{self, LinkedPrompt, NewThreadLink, PullRequest, Repository},
    },
    rpc::RpcEndpoint,
    workspace::project_ops::source_control::SourceControl,
};
use t3_contracts::rpc::{WsGitPreparePullRequestThreadRpc, WsPullRequestsLinkedThreadsRpc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    CloneRepository,
    UseWithoutProject,
    CheckoutPullRequest,
    ReplaceDraft,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkText {
    pub key: String,
    pub arguments: Vec<(String, String)>,
}
impl LinkText {
    fn key(key: &str) -> Self {
        Self {
            key: key.into(),
            arguments: vec![],
        }
    }
    fn arg(mut self, key: &str, value: String) -> Self {
        self.arguments.push((key.into(), value));
        self
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkChoice {
    pub action: LinkAction,
    pub label: LinkText,
    pub disabled_reason: Option<LinkText>,
}
#[derive(Clone)]
pub(super) enum LinkDecision {
    MissingRepository(Repository),
    Checkout(ProjectKey, PullRequest),
    Replace {
        destination: Destination,
        draft_id: String,
        prompt: LinkedPrompt,
        notes: Vec<String>,
    },
    Message {
        title: LinkText,
        message: LinkText,
    },
}
#[derive(Clone)]
pub(super) struct LinkSession {
    decision: LinkDecision,
    busy: bool,
}
impl LinkSession {
    fn view(&self) -> NewThreadView {
        if self.busy && !matches!(self.decision, LinkDecision::Checkout(..)) {
            return NewThreadView::Loading;
        }
        let choice = |action, label| LinkChoice {
            action,
            label,
            disabled_reason: self.busy.then(|| LinkText::key("Checking out...")),
        };
        let (title, message, choices) = match &self.decision {
            LinkDecision::MissingRepository(repository) => {
                let name = format!("{}/{}", repository.owner, repository.name);
                (
                    LinkText::key("You don't have {name}").arg("name", name.clone()),
                    LinkText::key("Clone it to work on it here, or start without a project."),
                    vec![
                        choice(
                            LinkAction::CloneRepository,
                            LinkText::key("Clone {name}").arg("name", name),
                        ),
                        choice(
                            LinkAction::UseWithoutProject,
                            LinkText::key("Use without project"),
                        ),
                    ],
                )
            }
            LinkDecision::Checkout(_, pr) => (
                LinkText::key("Check out {name}?").arg(
                    "name",
                    format!(
                        "{}/{}#{}",
                        pr.repository.owner, pr.repository.name, pr.number
                    ),
                ),
                LinkText::key(
                    "This puts the pull request's code in a new worktree and runs your project's setup script on it.",
                ),
                vec![choice(
                    LinkAction::CheckoutPullRequest,
                    LinkText::key(if self.busy {
                        "Checking out..."
                    } else {
                        "Check out"
                    }),
                )],
            ),
            LinkDecision::Replace { .. } => (
                LinkText::key("Linked prompt ready"),
                LinkText::key(
                    "This thread already has a draft. Replace it with the linked prompt?",
                ),
                vec![choice(
                    LinkAction::ReplaceDraft,
                    LinkText::key("Replace draft"),
                )],
            ),
            LinkDecision::Message { title, message } => (title.clone(), message.clone(), vec![]),
        };
        NewThreadView::LinkChoice {
            title,
            message,
            choices,
        }
    }
}
struct LinkBusy(Rc<NewThreadState>, String);
impl Drop for LinkBusy {
    fn drop(&mut self) {
        if let Some(session) = self.0.links.borrow_mut().get_mut(&self.1) {
            session.busy = false;
        }
        self.0.changed.invalidate();
    }
}
fn repository(link: &NewThreadLink) -> Option<&Repository> {
    match link {
        NewThreadLink::New { repository, .. } => repository.as_ref(),
        NewThreadLink::PullRequest { pull_request, .. } => Some(&pull_request.repository),
    }
}
fn prompt(link: &NewThreadLink) -> Option<&LinkedPrompt> {
    match link {
        NewThreadLink::New { prompt, .. } => Some(prompt),
        NewThreadLink::PullRequest { prompt, .. } => prompt.as_ref(),
    }
}

impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
    fn link_decision(&self, search: &str, decision: LinkDecision) -> NewThreadView {
        let session = LinkSession {
            decision,
            busy: false,
        };
        let view = session.view();
        self.state.links.borrow_mut().insert(search.into(), session);
        view
    }
    fn last_active_environment(&self) -> Option<EnvironmentId> {
        let snapshot = self.runtime.store().snapshot();
        let mut latest = None;
        for (environment, state) in &snapshot.environments {
            for thread in state.shell.snapshot.iter().flat_map(|shell| &shell.threads) {
                if latest
                    .as_ref()
                    .is_none_or(|(_, date): &(EnvironmentId, String)| thread.updated_at.0 > *date)
                {
                    latest = Some((environment.clone(), thread.updated_at.0.clone()));
                }
            }
        }
        latest.map(|(environment, _)| environment).or_else(|| {
            self.runtime
                .environments()
                .into_iter()
                .find(|e| {
                    e.routes
                        .iter()
                        .any(|route| route.mode == crate::connection::ConnectionMode::Local)
                })
                .map(|e| e.id)
        })
    }
    pub(super) async fn resolve_link(&self, search: &str) -> Result<NewThreadView, ClientError> {
        let link = match prompt_links::parse_new_thread_link(search) {
            Ok(link) => link,
            Err(message) => {
                return Ok(NewThreadView::LinkFailed {
                    title: "Could not open the link".into(),
                    message: message.into(),
                });
            }
        };
        if let Some(session) = self.state.links.borrow().get(search) {
            return Ok(session.view());
        }
        let snapshot = self.runtime.store().snapshot();
        if snapshot.environments.is_empty()
            || snapshot
                .environments
                .values()
                .any(|e| e.shell.status != SyncStatus::Live)
        {
            return Ok(NewThreadView::Loading);
        }
        let projects = self
            .projects()
            .into_iter()
            .filter(|p| !p.is_scratch)
            .collect::<Vec<_>>();
        if let NewThreadLink::PullRequest { pull_request, .. } = &link {
            let local = link_targets::find_pull_request_thread(
                snapshot.environments.iter().flat_map(|(env, state)| {
                    state.shell.snapshot.iter().flat_map(move |shell| {
                        shell.threads.iter().map(move |thread| (env, thread))
                    })
                }),
                pull_request,
            );
            let mut found = local;
            if found.is_none() {
                for project in link_targets::find_projects(&projects, &pull_request.repository) {
                    if snapshot
                        .environments
                        .get(&project.environment)
                        .is_none_or(|e| e.shell.status != SyncStatus::Live)
                    {
                        continue;
                    }
                    let Some(endpoint) = self
                        .runtime
                        .client(&project.environment)
                        .and_then(|c| c.session())
                    else {
                        continue;
                    };
                    let input = serde_json::from_value(
                        json!({"projectId":project.project.id,"host":"github.com","repository":format!("{}/{}",pull_request.repository.owner,pull_request.repository.name),"number":pull_request.number}),
                    )?;
                    if let Ok(reply) = endpoint.call::<WsPullRequestsLinkedThreadsRpc>(input).await
                        && let Some(thread) = reply
                            .threads
                            .iter()
                            .find(|thread| thread.archived_at.is_none())
                    {
                        found = Some(ThreadKey {
                            environment: project.environment.clone(),
                            thread: thread.id.clone(),
                        });
                        break;
                    }
                }
            }
            if let Some(key) = found {
                let destination = Destination::Thread(key.clone());
                let document = self.drafts.document();
                let id = web_sessions::resolve(
                    &document.borrow(),
                    web_sessions::Target::Thread(&t3_contracts::environment::ScopedThreadRef {
                        environment_id: key.environment,
                        thread_id: key.thread,
                    }),
                )
                .ok_or(ClientError::Cancelled)?;
                return self
                    .place_link_prompt(search, &link, id, destination, vec![], false)
                    .await;
            }
        }
        let Some(repository) = repository(&link) else {
            return match self
                .open_scratch_project(self.last_active_environment().as_ref())
                .await
            {
                Ok(project) => self.open_link_draft(search, &link, &project, true).await,
                Err(_) => Ok(self.link_decision(
                    search,
                    LinkDecision::Message {
                        title: LinkText::key("Could not open the link"),
                        message: LinkText::key(
                            "None of your machines can start a thread without a project right now.",
                        ),
                    },
                )),
            };
        };
        let Some(project) = link_targets::find_projects(&projects, repository)
            .first()
            .copied()
        else {
            return Ok(
                self.link_decision(search, LinkDecision::MissingRepository(repository.clone()))
            );
        };
        if let NewThreadLink::PullRequest { pull_request, .. } = link {
            return Ok(
                self.link_decision(search, LinkDecision::Checkout(project.key(), pull_request))
            );
        }
        self.open_link_draft(search, &link, project, false).await
    }
    async fn open_link_draft(
        &self,
        search: &str,
        link: &NewThreadLink,
        project: &EnvironmentProject,
        extra_pr_url: bool,
    ) -> Result<NewThreadView, ClientError> {
        let id = self.open_project(project.key(), None, None).await?;
        let mut notes = vec![];
        if let NewThreadLink::New {
            provider: Some(provider),
            model,
            ..
        } = link
        {
            let resolved = self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&project.environment)
                .and_then(|e| e.config.as_ref())
                .and_then(|c| {
                    link_targets::model_selection(&c.providers.0, provider, model.as_deref())
                });
            match resolved {
                Some((selection,exact))=>{
                    let draft_id=id.clone();self.drafts.update(Box::new(move|doc|{web_models::ModelPicks::default().set(doc,&draft_id,Some(&selection),true,true);Ok(())}))?;
                    if !exact {notes.push(format!("The link asked for {}, which {provider} does not offer here, so its default model is used.",model.as_deref().unwrap_or_default()));}
                },
                None=>notes.push(format!("The link asked for {provider}, which is not set up here, so your default model is used.")),
            }
        }
        self.place_link_prompt(
            search,
            link,
            id.clone(),
            Destination::Draft(id),
            notes,
            extra_pr_url,
        )
        .await
    }
    async fn place_link_prompt(
        &self,
        search: &str,
        link: &NewThreadLink,
        id: String,
        destination: Destination,
        mut notes: Vec<String>,
        extra_pr_url: bool,
    ) -> Result<NewThreadView, ClientError> {
        let mut linked = prompt(link).cloned();
        if let NewThreadLink::PullRequest { pull_request, .. } = link
            && extra_pr_url
        {
            let extra = pull_request.url();
            let prompt = linked.get_or_insert(LinkedPrompt {
                text: String::new(),
                hidden_characters: 0,
            });
            if !prompt.text.is_empty() {
                prompt.text.push_str("\n\n");
            }
            prompt.text.push_str(&extra);
        }
        if let Some(prompt) = linked {
            if prompt.hidden_characters > 0 {
                notes.push(format!("This prompt had hidden characters, it may be malicious. T3 Code removed {} hidden character{}. Read the prompt before you send it.",prompt.hidden_characters,if prompt.hidden_characters==1{""}else{"s"}));
            }
            let current = self.drafts.view(&id);
            if !current.prompt.trim().is_empty() && current.prompt.as_ref() != prompt.text {
                return Ok(self.link_decision(
                    search,
                    LinkDecision::Replace {
                        destination,
                        draft_id: id,
                        prompt,
                        notes,
                    },
                ));
            }
            self.drafts
                .set_prompt(&id, prompt.text.clone(), prompt.text.encode_utf16().count())?;
            self.state.errors.borrow_mut().insert(
                Target::Draft(id),
                [
                    vec!["Prompt added from a link. Read it before you send.".into()],
                    notes,
                ]
                .concat()
                .join(" "),
            );
            self.drafts.flush().await?;
        }
        self.state
            .destinations
            .borrow_mut()
            .insert(Target::Link(search.into()), destination.clone());
        Ok(NewThreadView::Navigate(destination))
    }
    pub async fn link_action(
        &self,
        search: String,
        action: LinkAction,
    ) -> Result<Option<Destination>, ClientError> {
        let link = prompt_links::parse_new_thread_link(&search)
            .map_err(|message| ClientError::InvalidPayload(message.into()))?;
        let session = self
            .state
            .links
            .borrow()
            .get(&search)
            .cloned()
            .ok_or(ClientError::Cancelled)?;
        let allowed = matches!(
            (&session.decision, action),
            (
                LinkDecision::MissingRepository(_),
                LinkAction::CloneRepository | LinkAction::UseWithoutProject
            ) | (LinkDecision::Checkout(..), LinkAction::CheckoutPullRequest)
                | (LinkDecision::Replace { .. }, LinkAction::ReplaceDraft)
        );
        if session.busy || !allowed {
            return Err(ClientError::Cancelled);
        }
        if let Some(session) = self.state.links.borrow_mut().get_mut(&search) {
            session.busy = true;
        }
        let _busy = LinkBusy(self.state.clone(), search.clone());
        self.state.changed.invalidate();
        let view = match (session.decision, action) {
            (LinkDecision::MissingRepository(repository), LinkAction::CloneRepository) => {
                self.clone_link_repository(&search, &link, &repository)
                    .await?
            }
            (LinkDecision::MissingRepository(_), LinkAction::UseWithoutProject) => {
                match self.open_scratch_project(self.last_active_environment().as_ref()).await {
                    Ok(project)=>self.open_link_draft(&search,&link,&project,true).await?,
                    Err(_)=>self.link_decision(&search,LinkDecision::Message{title:LinkText::key("Could not open the link"),message:LinkText::key("None of your machines can start a thread without a project right now.")}),
                }
            }
            (LinkDecision::Checkout(project, pr), LinkAction::CheckoutPullRequest) => {
                self.checkout_link(&search, &link, project, pr).await?
            }
            (
                LinkDecision::Replace {
                    destination,
                    draft_id,
                    prompt,
                    notes,
                },
                LinkAction::ReplaceDraft,
            ) => {
                self.drafts.set_prompt(
                    &draft_id,
                    prompt.text.clone(),
                    prompt.text.encode_utf16().count(),
                )?;
                self.state.errors.borrow_mut().insert(
                    Target::Draft(draft_id),
                    [
                        vec!["Prompt added from a link. Read it before you send.".into()],
                        notes,
                    ]
                    .concat()
                    .join(" "),
                );
                self.drafts.flush().await?;
                NewThreadView::Navigate(destination)
            }
            _ => return Err(ClientError::Cancelled),
        };
        if let NewThreadView::Navigate(destination) = view {
            self.state
                .destinations
                .borrow_mut()
                .insert(Target::Link(search), destination.clone());
            Ok(Some(destination))
        } else {
            Ok(None)
        }
    }
    async fn wait_link_project(
        &self,
        key: ProjectKey,
        generation: Option<Generation>,
        mut changes: crate::store::Subscription,
    ) -> Result<EnvironmentProject, ClientError> {
        let timeout = self.platform.sleep(5_000);
        futures::pin_mut!(timeout);
        loop {
            if self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .map(|e| e.session)
                != generation
            {
                return Err(ClientError::Cancelled);
            }
            if let Ok(project) = self.project(&key) {
                return Ok(project);
            }
            match futures::future::select(changes.next(), timeout.as_mut()).await {
                futures::future::Either::Left((Some(_), _)) => {}
                futures::future::Either::Left((None, _)) => return Err(ClientError::Cancelled),
                futures::future::Either::Right(_) => {
                    return Err(ClientError::Rejected {
                        message: "The clone started, but its project did not appear.".into(),
                        detail: None,
                    });
                }
            }
        }
    }
    async fn clone_link_repository(
        &self,
        search: &str,
        link: &NewThreadLink,
        repository: &Repository,
    ) -> Result<NewThreadView, ClientError> {
        let environment = self.last_active_environment();
        let snapshot = self.runtime.store().snapshot();
        let config = environment
            .as_ref()
            .and_then(|env| snapshot.environments.get(env))
            .and_then(|e| e.config.as_ref());
        let Some((environment, config, root)) =
            environment.as_ref().zip(config).and_then(|(env, config)| {
                config
                    .new_projects_root
                    .as_ref()
                    .map(|root| (env, config, root))
            })
        else {
            return Ok(self.link_decision(search,LinkDecision::Message{title:LinkText::key("Could not open the link"),message:LinkText::key("This machine cannot clone from a link. Update T3 Code on it, or clone the repository from the command palette.")}));
        };
        let endpoint = self
            .runtime
            .client(environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)?;
        let remote = format!(
            "https://github.com/{}/{}.git",
            repository.owner, repository.name
        );
        let ghq = config.settings.ghq_clone_layout;
        let base = if ghq {
            let base = config.settings.add_project_base_directory.0.trim();
            if base.is_empty() {
                "~/".into()
            } else {
                crate::workspace::project_ops::paths::ensure_browse_directory_path(base)
            }
        } else {
            root.0.clone()
        };
        let path = operations::get_clone_destination_path(
            &base,
            Some(&operations::get_clone_directory_path(&remote, ghq, None)),
        );
        let id = crate::types::ProjectId(operations::allocate_command_id(&self.platform, None)?.0);
        let input = serde_json::from_value(
            json!({"projectId":id,"title":repository.name,"createdAt":crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms()),"remoteUrl":remote,"destinationPath":path}),
        )?;
        let changes = self.runtime.store().subscribe();
        let generation = snapshot.environments.get(environment).map(|e| e.session);
        let reply = SourceControl::default()
            .start_project_clone(environment.clone(), &endpoint, input)
            .await
            .map_err(crate::rpc::RpcCallError::into_client_error)?;
        let project = self
            .wait_link_project(
                ProjectKey {
                    environment: environment.clone(),
                    project: reply.project_id,
                },
                generation,
                changes,
            )
            .await?;
        if matches!(link, NewThreadLink::PullRequest { .. }) {
            return Ok(self.link_decision(
                search,
                LinkDecision::Message {
                    title: LinkText::key("Cloning {name}")
                        .arg("name", format!("{}/{}", repository.owner, repository.name)),
                    message: LinkText::key("Open this link again when the clone finishes."),
                },
            ));
        }
        self.open_link_draft(search, link, &project, false).await
    }
    async fn checkout_link(
        &self,
        search: &str,
        link: &NewThreadLink,
        key: ProjectKey,
        pr: PullRequest,
    ) -> Result<NewThreadView, ClientError> {
        let project = self.project(&key)?;
        let id = self.open_project(key.clone(), None, None).await?;
        let reserved = self.draft_key(&id).ok_or(ClientError::Cancelled)?;
        let endpoint = self
            .runtime
            .client(&key.environment)
            .and_then(|c| c.session())
            .ok_or(ClientError::Disconnected)?;
        let input = serde_json::from_value(
            json!({"cwd":project.project.workspace_root,"reference":pr.url(),"mode":"worktree","threadId":reserved.thread}),
        )?;
        if !endpoint.available::<WsGitPreparePullRequestThreadRpc>(&input) {
            return Err(ClientError::PermissionDenied {
                method: "git.preparePullRequestThread".into(),
            });
        }
        let prepared = SourceControl::default()
            .prepare_pull_request_thread(key.environment.clone(), &endpoint, &self.platform, input)
            .await
            .map_err(crate::rpc::RpcCallError::into_client_error)?;
        self.project(&key)?;
        let pointed = self.open_project(key.clone(), None, None).await?;
        let now = crate::time::iso_from_epoch_ms(self.platform.now_epoch_ms());
        let draft_id = pointed.clone();
        self.drafts.update(Box::new(move|doc| {
            web_sessions::update_context(doc,web_sessions::Target::Draft(&draft_id),None,&json!({"branch":prepared.branch,"worktreePath":prepared.worktree_path,"envMode":if prepared.worktree_path.is_some(){"worktree"}else{"local"},"checkoutPullRequest":{"url":prepared.pull_request.url,"branch":prepared.branch}}),&now);Ok(())
        }))?;
        self.place_link_prompt(
            search,
            link,
            pointed.clone(),
            Destination::Draft(pointed),
            vec![],
            false,
        )
        .await
    }
}
