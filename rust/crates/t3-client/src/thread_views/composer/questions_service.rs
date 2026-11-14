//! Pending question state is shared across mounts and commands revalidate request/question ids.
use super::{answer_context, send::Changes, target::ComposerTarget};
use crate::{
    ClientError,
    composition::{ClientRuntime, Spawner},
    platform::Platform,
    rpc::{RpcEndpoint, SessionEndpoint},
    thread_views::{
        questions::{self, Answer, DraftAnswer, Question},
        requests,
    },
    types::{SyncStatus, ThreadKey},
    view_logic::session::questions::{
        pending_user_input_progress, question_attachment_draft_id,
        return_unsent_answers_to_thread_draft,
    },
};
use futures::{FutureExt, StreamExt, stream::LocalBoxStream};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    base_schemas::RuntimeRequestId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    rpc::WsOrchestrationV2DispatchCommandRpc,
};

#[derive(Clone, Debug, PartialEq)]
pub struct PendingUserInputView {
    pub request_id: RuntimeRequestId,
    pub questions: Rc<Vec<OrchestrationV2UserInputQuestion>>,
    pub question_index: usize,
    pub selected_option_values: Vec<String>,
    pub using_custom_answer: bool,
    pub disabled: bool,
    pub response_disabled: bool,
    pub is_responding: bool,
    pub dismissible: bool,
    pub question_draft_key: String,
    pub can_previous: bool,
    pub is_last_question: bool,
    pub is_complete: bool,
    pub error: Option<String>,
}
#[derive(Default)]
struct Progress {
    answers: BTreeMap<String, DraftAnswer>,
    order: Vec<String>,
    displaced: Vec<(String, String)>,
    index: i64,
    responding: bool,
    submitted: bool,
    error: Option<String>,
}
#[derive(Default)]
pub struct PendingUserInputState {
    progress: RefCell<BTreeMap<(ThreadKey, RuntimeRequestId), Progress>>,
    views: RefCell<BTreeMap<ThreadKey, Rc<PendingUserInputView>>>,
    changes: Changes,
}
#[derive(Clone)]
pub struct PendingUserInputSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    key: ThreadKey,
    state: Rc<PendingUserInputState>,
    spawn: Spawner,
}
fn question(raw: &OrchestrationV2UserInputQuestion) -> Question {
    Question {
        id: raw.id.0.clone(),
        options: raw
            .options
            .iter()
            .map(|option| questions::QuestionOption {
                label: option.label.0.clone(),
                value: option.value.clone().flatten(),
            })
            .collect(),
        multi_select: raw.multi_select == Some(Some(true)),
        allow_custom_answer: raw.allow_custom_answer != Some(Some(false)),
        initial_answer: raw.initial_answer.clone().flatten(),
    }
}
impl<P: Platform + Clone + 'static> PendingUserInputSession<P> {
    pub fn new(
        runtime: ClientRuntime<P>,
        platform: P,
        key: ThreadKey,
        state: Rc<PendingUserInputState>,
        spawn: Spawner,
    ) -> Self {
        Self {
            runtime,
            platform,
            key,
            state,
            spawn,
        }
    }
    fn endpoint(&self) -> Result<SessionEndpoint, ClientError> {
        self.runtime
            .client(&self.key.environment)
            .and_then(|client| client.session())
            .ok_or(ClientError::Disconnected)
    }
    fn command(
        &self,
        request: &RuntimeRequestId,
        dismiss: bool,
        answers: Option<Value>,
    ) -> Result<OrchestrationV2Command, ClientError> {
        let mut raw = json!({"type":if dismiss{"thread.user-input.dismiss"}else{"runtime-request.respond"},"commandId":crate::operations::allocate_command_id(&self.platform,None)?,"threadId":self.key.thread,"requestId":request});
        if let Some(answers) = answers {
            raw["answers"] = answers;
        }
        Ok(serde_json::from_value(raw)?)
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let this = self.clone();
        futures::stream::select_all(vec![
            self.runtime
                .store()
                .subscribe_environment(&self.key.environment)
                .map(|_| ())
                .boxed_local(),
            self.runtime.composer_drafts().changes(),
            self.state.changes.subscribe(),
        ])
        .map(move |_| {
            this.view();
        })
        .boxed_local()
    }
    pub fn view(&self) -> Option<Rc<PendingUserInputView>> {
        let store = self.runtime.store().snapshot();
        let thread = store
            .environments
            .get(&self.key.environment)
            .and_then(|environment| environment.threads.get(&self.key.thread));
        let Some(projection) = thread.and_then(|thread| thread.projection.as_deref()) else {
            return self
                .state
                .views
                .borrow()
                .get(&self.key)
                .cloned()
                .map(|view| {
                    let mut next = (*view).clone();
                    next.disabled = true;
                    next.response_disabled = true;
                    Rc::new(next)
                });
        };
        let pending =
            requests::derive_pending(&projection.runtime_requests, &projection.turn_items);
        let ids = pending
            .user_inputs
            .iter()
            .filter_map(|input| match &input.request.id {
                IdOrIndex::Id(id) => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if thread.is_some_and(|thread| thread.status == SyncStatus::Live) {
            let answered = projection
                .runtime_requests
                .iter()
                .filter(|request| request.answers.as_ref().is_some_and(Option::is_some))
                .filter_map(|request| match &request.id {
                    IdOrIndex::Id(id) => Some(id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            self.return_resolved(&ids, &answered);
        }
        let Some(input) = pending.user_inputs.first() else {
            self.runtime
                .composer_drafts()
                .set_editor_target(&self.key, None);
            self.state.views.borrow_mut().remove(&self.key);
            return None;
        };
        let IdOrIndex::Id(request) = &input.request.id else {
            return None;
        };
        let canonical = input
            .item
            .questions
            .iter()
            .map(question)
            .collect::<Vec<_>>();
        let drafts = self.runtime.composer_drafts();
        let mut states = self.state.progress.borrow_mut();
        let progress = states
            .entry((self.key.clone(), request.clone()))
            .or_default();
        requests::seed_answers(&canonical, &mut progress.answers);
        progress.order = canonical
            .iter()
            .map(|question| question.id.clone())
            .collect();
        let supports_attachments = store
            .environments
            .get(&self.key.environment)
            .and_then(|environment| environment.config.as_ref())
            .is_some_and(|config| {
                config.environment.capabilities.question_attachments == Some(true)
            });
        for question in &canonical {
            let draft_id = question_attachment_draft_id(&self.key, &request.0, &question.id);
            let saved = drafts.view_key(&draft_id);
            let answer = progress.answers.entry(question.id.clone()).or_default();
            if saved.revision == 0
                && saved.prompt.is_empty()
                && let Some(initial) = &question.initial_answer
            {
                let _ = drafts.set_prompt_key(
                    &draft_id,
                    initial.clone(),
                    initial.encode_utf16().count(),
                );
            }
            let text = drafts.view_key(&draft_id).prompt.to_string();
            if answer.custom_answer.as_deref() != Some(text.as_str()) {
                *answer = questions::set_custom_answer(question, answer, text.clone());
            }
            if let Some(index) = progress.displaced.iter().rposition(|(id, displaced)| {
                id == &question.id
                    && crate::view_logic::JsTrim::js_trim(displaced.as_str())
                        == crate::view_logic::JsTrim::js_trim(text.as_str())
            }) {
                let (_, displaced) = progress.displaced.remove(index);
                let later = progress.displaced[index..]
                    .iter()
                    .map(|(_, text)| text.clone())
                    .collect::<Vec<_>>();
                let normal = drafts.thread_draft_key(&self.key);
                let prior = drafts.view_key(&normal).prompt.to_string();
                let prompt =
                    crate::view_logic::session::questions::withdraw_displaced_custom_answer(
                        &prior, &displaced, &later,
                    );
                let _ =
                    drafts.set_prompt_key(&normal, prompt.clone(), prompt.encode_utf16().count());
            }
            let target = ComposerTarget::Draft {
                draft_id,
                environment_id: Some(self.key.environment.clone()),
            };
            if let Ok(attachments) = self.runtime.composer_attachments(&target) {
                let items = attachments.items();
                answer.attachment_count = items.len();
                answer.attachments_blocked = !supports_attachments && !items.is_empty()
                    || attachments.block_reason().is_some()
                    || items.iter().any(|item| {
                        item.needs_reattach
                            || matches!(
                                item.upload,
                                super::attachment_intake::UploadState::Preparing
                                    | super::attachment_intake::UploadState::Uploading
                                    | super::attachment_intake::UploadState::Failed(_)
                            )
                    });
            }
        }
        let view = pending_user_input_progress(&canonical, &progress.answers, progress.index);
        progress.index = view.question_index as i64;
        let question_draft_key = question_attachment_draft_id(
            &self.key,
            &request.0,
            view.active_question
                .map(|question| question.id.as_str())
                .unwrap_or_default(),
        );
        drafts.set_editor_target(&self.key, Some(question_draft_key.clone()));
        let can_operate = self.runtime.approval_services().can_operate(&self.key);
        let disabled = !can_operate
            || thread.is_none_or(|thread| thread.status != SyncStatus::Live)
            || progress.responding
            || matches!(
                input.request.response_capability,
                RuntimeRequestResponseCapability::NotResumable(_)
            );
        let next = PendingUserInputView {
            request_id: request.clone(),
            questions: Rc::new(input.item.questions.clone()),
            question_index: view.question_index,
            selected_option_values: view.selected_option_values.clone(),
            using_custom_answer: view.using_custom_answer,
            disabled,
            response_disabled: disabled
                || !view.can_advance
                || view.is_last_question && !view.is_complete,
            is_responding: progress.responding,
            dismissible: input.dismissible,
            question_draft_key,
            can_previous: view.question_index > 0,
            is_last_question: view.is_last_question,
            is_complete: view.is_complete,
            error: progress.error.clone(),
        };
        drop(states);
        let mut views = self.state.views.borrow_mut();
        if let Some(prior) = views.get(&self.key).filter(|prior| ***prior == next) {
            return Some(prior.clone());
        }
        let next = Rc::new(next);
        views.insert(self.key.clone(), next.clone());
        Some(next)
    }
    fn return_resolved(&self, pending: &[RuntimeRequestId], answered: &[RuntimeRequestId]) {
        let removed = {
            let mut states = self.state.progress.borrow_mut();
            let keys = states
                .keys()
                .filter(|(key, request)| key == &self.key && !pending.contains(request))
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| states.remove(&key).map(|progress| (key.1, progress)))
                .collect::<Vec<_>>()
        };
        let drafts = self.runtime.composer_drafts();
        let normal = drafts.thread_draft_key(&self.key);
        for (request, progress) in removed {
            if !progress.submitted && !answered.contains(&request) {
                let prompt = drafts.view_key(&normal).prompt.to_string();
                let raw = drafts.document();
                let raw = raw.borrow();
                let entries = progress
                    .order
                    .iter()
                    .map(|id| {
                        let key = question_attachment_draft_id(&self.key, &request.0, id);
                        let draft = &raw["draftsByThreadKey"][key];
                        let text = draft["prompt"].as_str().map(str::to_owned).or_else(|| {
                            progress
                                .answers
                                .get(id)
                                .and_then(|answer| answer.custom_answer.clone())
                        });
                        let contexts: Vec<t3_contracts::composer_context::ThreadContextRecord> =
                            draft["threadContexts"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|record| serde_json::from_value(record.clone()).ok())
                                .collect();
                        (text, contexts)
                    })
                    .collect::<Vec<_>>();
                let returned = return_unsent_answers_to_thread_draft(
                    &prompt,
                    entries
                        .iter()
                        .map(|(text, records)| (text.as_deref(), records.as_slice())),
                );
                drop(raw);
                let _ = drafts.set_prompt_key(
                    &normal,
                    returned.prompt.clone(),
                    returned.prompt.encode_utf16().count(),
                );
                let _ = drafts.update(|document| {
                    let records = returned
                        .thread_contexts
                        .iter()
                        .map(serde_json::to_value)
                        .collect::<Result<Vec<_>, _>>()?;
                    super::web_context::add(
                        document,
                        &normal,
                        super::web_context::Kind::Thread,
                        &records,
                        &self.key.thread.0,
                        super::web_context::Placement {
                            append_reference: false,
                            ..Default::default()
                        },
                    );
                    Ok(())
                });
            }
            let prefix =
                crate::view_logic::session::questions::question_attachment_draft_prefix(&self.key);
            let encoded = crate::persistence::encode_uri_component(
                &serde_json::to_string(&request.0).expect("request id"),
            );
            let _ = drafts.update(|document| {
                let keys = document["draftsByThreadKey"]
                    .as_object()
                    .into_iter()
                    .flat_map(|drafts| drafts.keys())
                    .filter(|key| key.starts_with(&prefix) && key.contains(&encoded))
                    .cloned()
                    .collect::<Vec<_>>();
                for key in keys {
                    super::web_models::clear_content(document, &key);
                }
                Ok(())
            });
        }
    }
    fn guard(
        &self,
        request: &RuntimeRequestId,
        question: Option<&str>,
    ) -> Result<Rc<PendingUserInputView>, ClientError> {
        let view = self
            .view()
            .filter(|view| {
                &view.request_id == request
                    && question.is_none_or(|id| {
                        view.questions
                            .get(view.question_index)
                            .is_some_and(|question| question.id.0 == id)
                    })
            })
            .ok_or(ClientError::Cancelled)?;
        if view.disabled {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        Ok(view)
    }
    pub fn toggle_option(
        &self,
        request: &RuntimeRequestId,
        id: &str,
        value: &str,
    ) -> Result<(), ClientError> {
        let view = self.guard(request, Some(id))?;
        let question = question(&view.questions[view.question_index]);
        let mut states = self.state.progress.borrow_mut();
        let progress = states
            .get_mut(&(self.key.clone(), request.clone()))
            .ok_or(ClientError::Cancelled)?;
        let prior = progress.answers.get(id).cloned().unwrap_or_default();
        let next = questions::toggle_option(&question, &prior, value);
        let displaced = prior.custom_answer.filter(|text| !text.trim().is_empty());
        if let Some(text) = &displaced {
            progress.displaced.push((id.into(), text.clone()));
        }
        progress.answers.insert(id.into(), next);
        drop(states);
        if let Some(displaced) = displaced {
            let drafts = self.runtime.composer_drafts();
            let key = drafts.thread_draft_key(&self.key);
            let prior = drafts.view_key(&key).prompt.to_string();
            let prompt = crate::view_logic::session::questions::carry_displaced_custom_answer(
                &prior,
                Some(&displaced),
            );
            drafts.set_prompt_key(&key, prompt.clone(), prompt.encode_utf16().count())?;
            let raw = drafts.document();
            let raw = raw.borrow();
            let records: Vec<t3_contracts::composer_context::ThreadContextRecord> =
                raw["draftsByThreadKey"][&view.question_draft_key]["threadContexts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|record| serde_json::from_value(record.clone()).ok())
                    .collect();
            let referenced =
                crate::view_logic::session::questions::thread_contexts_referenced_by_answer(
                    &displaced, &records,
                )
                .into_iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?;
            drop(raw);
            drafts.update(|document| {
                super::web_context::add(
                    document,
                    &key,
                    super::web_context::Kind::Thread,
                    &referenced,
                    &self.key.thread.0,
                    super::web_context::Placement {
                        append_reference: false,
                        ..Default::default()
                    },
                );
                Ok(())
            })?;
        }
        self.runtime.composer_drafts().set_prompt_key(
            &view.question_draft_key,
            String::new(),
            0,
        )?;
        self.state.changes.notify();
        Ok(())
    }
    pub fn previous(&self, request: &RuntimeRequestId, id: &str) -> Result<(), ClientError> {
        self.guard(request, Some(id))?;
        if let Some(progress) = self
            .state
            .progress
            .borrow_mut()
            .get_mut(&(self.key.clone(), request.clone()))
        {
            progress.index = progress.index.saturating_sub(1);
        }
        self.state.changes.notify();
        Ok(())
    }
    pub fn advance(&self, request: &RuntimeRequestId, id: &str) -> Result<(), ClientError> {
        let view = self.guard(request, Some(id))?;
        if view.response_disabled {
            return Err(ClientError::Cancelled);
        }
        if !view.is_last_question {
            if let Some(progress) = self
                .state
                .progress
                .borrow_mut()
                .get_mut(&(self.key.clone(), request.clone()))
            {
                progress.index += 1;
            }
            self.state.changes.notify();
            return Ok(());
        }
        self.respond(request, false)
    }
    pub fn dismiss(&self, request: &RuntimeRequestId) -> Result<(), ClientError> {
        let view = self.guard(request, None)?;
        if !view.dismissible {
            return Err(ClientError::Cancelled);
        }
        self.respond(request, true)
    }
    fn respond(&self, request: &RuntimeRequestId, dismiss: bool) -> Result<(), ClientError> {
        let view = self.guard(request, None)?;
        let questions = view.questions.iter().map(question).collect::<Vec<_>>();
        let answers = {
            let states = self.state.progress.borrow();
            let progress = states
                .get(&(self.key.clone(), request.clone()))
                .ok_or(ClientError::Cancelled)?;
            questions::build_answers(&questions, &progress.answers)
        };
        let drafts = self.runtime.composer_drafts();
        let document = drafts.document();
        let raw = document.borrow();
        let thread_draft = &raw["draftsByThreadKey"][drafts.thread_draft_key(&self.key)];
        let mut projected = serde_json::Map::new();
        let mut attachments_by_question = serde_json::Map::new();
        if !dismiss {
            for (id, answer) in answers.ok_or(ClientError::Cancelled)? {
                let answer = match answer {
                    Answer::Text(text) => json!(text),
                    Answer::Choices(values) => json!(values),
                };
                let question_key = question_attachment_draft_id(&self.key, &request.0, &id);
                let question_draft = &raw["draftsByThreadKey"][&question_key];
                let mut attachments = vec![];
                for attachment in ["images", "files"]
                    .into_iter()
                    .flat_map(|field| question_draft[field].as_array().into_iter().flatten())
                {
                    if attachment["uploadEnvironmentId"] != self.key.environment.0
                        || !attachment["uploadedAttachmentId"].is_string()
                    {
                        return Err(ClientError::InvalidPayload(
                            "Wait for attachments to finish uploading, or remove failed uploads."
                                .into(),
                        ));
                    }
                    attachments.push(super::attachment_intake::uploaded_attachment(attachment)?);
                }
                if !attachments.is_empty() {
                    attachments_by_question.insert(id.clone(), json!(attachments));
                }
                projected.insert(
                    id,
                    answer_context::project_answer(
                        &answer,
                        Some(question_draft),
                        Some(thread_draft),
                    )?,
                );
            }
        }
        let mut command = self.command(
            request,
            dismiss,
            (!dismiss).then(|| Value::Object(projected)),
        )?;
        if let OrchestrationV2Command::RuntimeRequestRespond(command) = &mut command
            && !attachments_by_question.is_empty()
        {
            command.attachments_by_question_id = Some(Some(serde_json::from_value(
                Value::Object(attachments_by_question),
            )?));
        }
        drop(raw);
        let endpoint = self.endpoint()?;
        if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&command) {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        let identity = (self.key.clone(), request.clone());
        if let Some(progress) = self.state.progress.borrow_mut().get_mut(&identity) {
            if progress.responding {
                return Err(ClientError::Cancelled);
            }
            progress.responding = true;
            progress.error = None;
        }
        self.state.changes.notify();
        let this = self.clone();
        (self.spawn)(
            async move {
                let current = this
                    .view()
                    .is_some_and(|view| view.request_id == identity.1);
                let result = (if current {
                    this.endpoint()
                } else {
                    Err(ClientError::Cancelled)
                })
                .map(|endpoint| async move {
                    endpoint
                        .command::<WsOrchestrationV2DispatchCommandRpc>(command)
                        .await
                        .map(|_| ())
                        .map_err(crate::rpc::RpcCallError::into_client_error)
                });
                let result = match result {
                    Ok(result) => result.await,
                    Err(error) => Err(error),
                };
                if let Some(progress) = this.state.progress.borrow_mut().get_mut(&identity) {
                    progress.responding = false;
                    match result {
                        Ok(()) => progress.submitted = !dismiss,
                        Err(error) => progress.error = Some(error.to_string()),
                    }
                }
                this.state.changes.notify();
            }
            .boxed_local(),
        );
        Ok(())
    }
}
