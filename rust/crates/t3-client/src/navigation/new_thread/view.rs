use super::*;
use crate::connection::Phase;

impl<P: Platform + Clone + 'static> NewThreadRuntime<P> {
    pub(super) fn draft_view(&self, id: &str) -> Result<NewThreadView, ClientError> {
        let document = self.drafts.document();
        let doc = document.borrow().clone();
        let session = &doc["draftThreadsByThreadKey"][id];
        let destination = session
            .get("promotedTo")
            .filter(|d| !d.is_null())
            .or_else(|| doc["promotedDraftRedirects"].get(id));
        if let Some(destination) = destination.and_then(policy::session_key)
            && self
                .shell(&destination)
                .as_ref()
                .is_some_and(|shell| web_drafts::promotion_ready(Some(shell), false))
        {
            return Ok(NewThreadView::Navigate(Destination::Thread(destination)));
        }
        let Some(key) = policy::session_key(session) else {
            return Ok(NewThreadView::StartFailed);
        };
        let project_key = ProjectKey {
            environment: key.environment.clone(),
            project: crate::types::ProjectId(
                session["projectId"].as_str().unwrap_or_default().into(),
            ),
        };
        let projects = self.projects();
        let project = projects.iter().find(|p| p.key() == project_key);
        let environments = self.runtime.environments();
        let environment = environments.iter().find(|e| e.id == key.environment);
        let unavailable = environment.is_none_or(|e| e.state.phase != Phase::Connected);
        let authority = self.drafts.view(id);
        let can_operate = self.can_operate(&key);
        let busy = self.state.busy.borrow().contains(id)
            || self.runtime.pending_messages(&key).iter().any(|m| {
                !matches!(
                    m.state,
                    SendState::Restored { .. } | SendState::Accepted { .. }
                )
            });
        let attachment_reason = self
            .attachments(id)
            .and_then(|session| session.block_reason());
        let reason = if !authority.writable {
            Some("The composer draft is not writable.".into())
        } else if project.is_none() {
            Some("The project is no longer available.".into())
        } else if unavailable {
            Some("This environment is unavailable.".into())
        } else if !can_operate {
            Some("You do not have permission to operate threads in this environment.".into())
        } else {
            attachment_reason
        };
        let view = DraftView {
            id: id.into(),
            key,
            prompt: authority.prompt.to_string(),
            cursor: authority.cursor,
            revision: authority.revision,
            project: project_key,
            project_title: project
                .map(|p| p.project.title.0.clone())
                .unwrap_or_default(),
            projects: projects
                .iter()
                .map(|p| ProjectChoice {
                    key: p.key(),
                    title: p.project.title.0.clone(),
                    environment_label: environments
                        .iter()
                        .find(|e| e.id == p.environment)
                        .map(|e| e.label.clone()),
                })
                .collect(),
            is_scratch: project.is_some_and(|p| p.is_scratch),
            can_start_scratch: self
                .scratch_environment(Some(&authority_environment(session)))
                .is_some(),
            editable: authority.writable && !busy,
            can_operate,
            busy,
            has_content: web_has_user_content(&doc["draftsByThreadKey"][id]),
            unavailable,
            disabled_reason: reason,
            banner: self
                .state
                .errors
                .borrow()
                .get(&Target::Draft(id.into()))
                .cloned(),
        };
        let mut cache = self.state.views.borrow_mut();
        let view = if let Some(current) = cache.get(id).filter(|current| current.as_ref() == &view)
        {
            current.clone()
        } else {
            let view = Rc::new(view);
            cache.insert(id.into(), view.clone());
            view
        };
        Ok(NewThreadView::Draft(view))
    }
}
fn authority_environment(session: &Value) -> EnvironmentId {
    EnvironmentId(session["environmentId"].as_str().unwrap_or_default().into())
}
