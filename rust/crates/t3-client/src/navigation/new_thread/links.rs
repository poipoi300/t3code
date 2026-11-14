use super::*;
use crate::navigation::{
    link_targets,
    prompt_links::{self, NewThreadLink},
};

impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
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
        let (prompt, repository, provider, model) = match link {
            NewThreadLink::New {
                prompt,
                repository,
                provider,
                model,
            } => (prompt, repository, provider, model),
            NewThreadLink::PullRequest {
                pull_request,
                prompt,
            } => {
                if prompt.is_some() {
                    return Err(ClientError::Unsupported(
                        "pull request linked-prompt confirmation",
                    ));
                }
                let snapshot = self.runtime.store().snapshot();
                if let Some(key) = link_targets::find_pull_request_thread(
                    snapshot.environments.iter().flat_map(|(env, state)| {
                        state
                            .shell
                            .snapshot
                            .iter()
                            .flat_map(move |shell| shell.threads.iter().map(move |t| (env, t)))
                    }),
                    &pull_request,
                ) {
                    return Ok(NewThreadView::Navigate(Destination::Thread(key)));
                }
                return Err(ClientError::Unsupported("pull request prompt links"));
            }
        };
        let project = self.projects().into_iter().find(|project| {
            repository
                .as_ref()
                .is_none_or(|repository| link_targets::repository_matches(project, repository))
        });
        let Some(project) = project else {
            return Err(ClientError::Unsupported("prompt link repository launch"));
        };
        let id = self
            .open_project(project.key(), None, Some(prompt.text))
            .await?;
        let mut notes = vec!["Prompt added from a link. Read it before you send.".to_owned()];
        if prompt.hidden_characters > 0 {
            notes.push(format!("This prompt had hidden characters, it may be malicious. T3 Code removed {} hidden character{}. Read the prompt before you send it.",prompt.hidden_characters,if prompt.hidden_characters==1{""}else{"s"}));
        }
        if let Some(provider) = provider {
            let resolved = self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&project.environment)
                .and_then(|e| e.config.as_ref())
                .and_then(|c| {
                    link_targets::model_selection(&c.providers.0, &provider, model.as_deref())
                });
            match resolved {
                Some((selection,exact))=>{
                    let id=id.clone();self.drafts.update(Box::new(move|doc|{web_models::ModelPicks::default().set(doc,&id,Some(&selection),true,true);Ok(())}))?;
                    if !exact {notes.push(format!("The link asked for {}, which {provider} does not offer here, so its default model is used.",model.unwrap_or_default()));}
                },
                None=>notes.push(format!("The link asked for {provider}, which is not set up here, so your default model is used.")),
            }
        }
        self.state
            .errors
            .borrow_mut()
            .insert(Target::Draft(id.clone()), notes.join(" "));
        let destination = Destination::Draft(id);
        self.state
            .destinations
            .borrow_mut()
            .insert(Target::Link(search.into()), destination.clone());
        Ok(NewThreadView::Navigate(destination))
    }
}
