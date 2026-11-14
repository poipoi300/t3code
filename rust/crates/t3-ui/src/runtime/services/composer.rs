//! Thin bindings of the shared composer services.
use crate::{
    i18n::Text,
    screens::composer::{
        drafts::{ComposerDraftClient, ComposerDraftView},
        send::{FollowUpBehavior, PrimaryActionState},
    },
};
use futures::stream::LocalBoxStream;
use std::rc::Rc;
use std::{cell::RefCell, collections::BTreeMap};
use t3_client::{ClientError, composition::ClientRuntime, platform::Platform, types::ThreadKey};

use crate::runtime::tasks::watch;

pub fn citation_comments<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
) -> t3_client::thread_views::composer::citation_comments::ComposerCitationCommentContext<P> {
    runtime.composer_citation_comments(key)
}

pub fn composer_voice<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
) -> t3_client::thread_views::composer::voice_service::ComposerVoiceSession {
    runtime.composer_voice()
}

pub fn chip_data(
    view: &t3_client::thread_views::composer::chips_service::ComposerChipsView,
) -> crate::screens::composer::chips::ChipData {
    crate::screens::composer::chips::ChipData {
        records: view.records.clone(),
        skills: view.skills.clone(),
        providers: view.providers.clone(),
        previews: view.previews.clone(),
        video_attachments: view.video_attachments.clone(),
        draft_attachments: view.draft_attachments.clone(),
        ..Default::default()
    }
}

type QuestionViews = BTreeMap<
    ThreadKey,
    (
        Rc<t3_client::thread_views::composer::questions_service::PendingUserInputView>,
        Rc<crate::screens::composer::questions::PendingUserInputView>,
    ),
>;
struct QuestionsBinding<P: Platform + Clone + 'static>(ClientRuntime<P>, RefCell<QuestionViews>);
pub fn pending_user_input<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
) -> Rc<dyn crate::screens::composer::questions::PendingUserInputClient> {
    Rc::new(QuestionsBinding(runtime.clone(), RefCell::default()))
}
pub fn composer_questions<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: ThreadKey,
) -> crate::screens::composer::questions::ComposerQuestions {
    use futures::StreamExt;
    use nsh_core::prelude::*;
    let source = runtime.pending_user_input(&key);
    let mut changes = source.changes();
    let questions = crate::screens::composer::questions::ComposerQuestions::attach(
        pending_user_input(runtime),
        key,
    );
    let error = questions.error;
    watch(async move {
        while changes.next().await.is_some() {
            error.set(
                source
                    .view()
                    .and_then(|view| view.error.clone())
                    .map(crate::i18n::Text::content),
            );
        }
    });
    questions
}
impl<P: Platform + Clone + 'static> crate::screens::composer::questions::PendingUserInputClient
    for QuestionsBinding<P>
{
    fn changes(&self, key: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.0.pending_user_input(key).changes()
    }
    fn view(
        &self,
        key: &ThreadKey,
    ) -> Option<Rc<crate::screens::composer::questions::PendingUserInputView>> {
        self.0.pending_user_input(key).view().map(|view| {
            let mut cache = self.1.borrow_mut();
            if let Some((source, mapped)) = cache.get(key)
                && Rc::ptr_eq(source, &view)
            {
                return mapped.clone();
            }
            let mapped = Rc::new(question_view(&view));
            cache.insert(key.clone(), (view, mapped.clone()));
            mapped
        })
    }
    fn toggle_option(
        &self,
        key: &ThreadKey,
        request: &t3_contracts::base_schemas::RuntimeRequestId,
        question: &str,
        value: &str,
    ) -> Result<(), ClientError> {
        self.0
            .pending_user_input(key)
            .toggle_option(request, question, value)
    }
    fn advance(
        &self,
        key: &ThreadKey,
        request: &t3_contracts::base_schemas::RuntimeRequestId,
        question: &str,
    ) -> Result<(), ClientError> {
        self.0.pending_user_input(key).advance(request, question)
    }
    fn dismiss(
        &self,
        key: &ThreadKey,
        request: &t3_contracts::base_schemas::RuntimeRequestId,
    ) -> Result<(), ClientError> {
        self.0.pending_user_input(key).dismiss(request)
    }
}

/// The panel's `responseDisabled` is the web's `disabled || isResponding || !canRespond`
/// (ComposerPendingUserInputPanel.tsx); the client's `response_disabled` also closes while no
/// answer is chosen, which is the submit action's gate, not the options'.
pub fn question_view(
    view: &t3_client::thread_views::composer::questions_service::PendingUserInputView,
) -> crate::screens::composer::questions::PendingUserInputView {
    crate::screens::composer::questions::PendingUserInputView {
        request_id: view.request_id.clone(),
        questions: view.questions.clone(),
        question_index: view.question_index,
        selected_option_values: view.selected_option_values.clone(),
        using_custom_answer: view.using_custom_answer,
        disabled: view.disabled,
        // The client's `disabled` already includes a not-resumable request.
        response_disabled: view.disabled || view.is_responding,
        is_responding: view.is_responding,
        dismissible: view.dismissible,
    }
}

pub fn suggestion_request(
    request: &crate::screens::composer::suggestions::ComposerSuggestionRequest,
) -> t3_client::thread_views::composer::suggestions::SuggestionRequest {
    t3_client::thread_views::composer::suggestions::SuggestionRequest {
        trigger: request.trigger.clone(),
        generation: request.generation,
    }
}
pub fn suggestion_results(
    view: &t3_client::thread_views::composer::suggestions::SuggestionProjection,
) -> crate::screens::composer::suggestions::ComposerSuggestionResults {
    use crate::screens::composer::suggestions::{
        ComposerCommandItem, ComposerCommandItemKind, ComposerSuggestionRequest,
        ComposerSuggestionResults, SkillSourceKind,
    };
    use t3_client::thread_views::composer::suggestions::{SkillSource, SuggestionKind};
    ComposerSuggestionResults {
        request: ComposerSuggestionRequest {
            trigger: view.request.trigger.clone(),
            generation: view.request.generation,
        },
        items: view
            .items
            .iter()
            .map(|item| ComposerCommandItem {
                id: item.id.clone(),
                label: Text::content(item.label.clone()),
                description: Text::content(item.description.clone()),
                kind: match &item.kind {
                    SuggestionKind::Thread { .. } => ComposerCommandItemKind::Thread,
                    SuggestionKind::Path { directory } => ComposerCommandItemKind::Path {
                        directory: *directory,
                    },
                    SuggestionKind::Slash => ComposerCommandItemKind::SlashCommand,
                    SuggestionKind::ProviderSlash => ComposerCommandItemKind::ProviderSlashCommand,
                    SuggestionKind::Model {
                        driver,
                        provider_name,
                    } => ComposerCommandItemKind::Model {
                        driver: driver.clone(),
                        provider_name: provider_name.clone(),
                    },
                    SuggestionKind::Skill {
                        source,
                        display_name,
                    } => ComposerCommandItemKind::Skill {
                        source: match source {
                            SkillSource::App => SkillSourceKind::App,
                            SkillSource::Repo => SkillSourceKind::Repo,
                            SkillSource::Project => SkillSourceKind::Project,
                            SkillSource::Personal => SkillSourceKind::Personal,
                            SkillSource::System => SkillSourceKind::System,
                            SkillSource::Other => SkillSourceKind::Other,
                        },
                        display_name: display_name.clone(),
                    },
                },
            })
            .collect(),
        loading: view.loading,
        empty_state: Some(Text::client_key(view.empty_state.clone())),
    }
}

struct StashBinding<P: Platform + Clone + 'static>(
    t3_client::thread_views::composer::stash_service::PromptStashSession<P>,
);
pub fn prompt_stash<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    target: t3_client::thread_views::composer::target::ComposerTarget,
) -> Rc<dyn crate::screens::composer::stash::PromptStashClient> {
    Rc::new(StashBinding(runtime.prompt_stash(target)))
}
fn stash_error(error: ClientError) -> crate::screens::composer::stash::StashNotice {
    crate::screens::composer::stash::StashNotice {
        variant: crate::ui::toast::Variant::Error,
        title: Text::content(error.to_string()),
        description: None,
    }
}
fn stash_completion(
    result: Result<t3_client::thread_views::composer::stash_service::StashCompletion, ClientError>,
) -> crate::screens::composer::stash::StashResult {
    use t3_client::thread_views::composer::stash_service::StashMenu;
    result
        .map(|result| crate::screens::composer::stash::StashCompletion {
            notices: result
                .notices
                .into_iter()
                .map(|notice| crate::screens::composer::stash::StashNotice {
                    variant: if notice.warning {
                        crate::ui::toast::Variant::Warning
                    } else {
                        crate::ui::toast::Variant::Info
                    },
                    title: Text::client_key(notice.title),
                    description: notice.description.map(Text::content),
                })
                .collect(),
            menu: match result.menu {
                StashMenu::Keep => crate::screens::composer::stash::MenuRequest::Keep,
                StashMenu::Open => crate::screens::composer::stash::MenuRequest::Open,
                StashMenu::Close => crate::screens::composer::stash::MenuRequest::Close,
                StashMenu::Toggle => crate::screens::composer::stash::MenuRequest::Toggle,
            },
        })
        .map_err(stash_error)
}
impl<P: Platform + Clone + 'static> crate::screens::composer::stash::PromptStashClient
    for StashBinding<P>
{
    fn snapshot(&self) -> crate::screens::composer::stash::StashSnapshot {
        let view = self.0.snapshot();
        crate::screens::composer::stash::StashSnapshot {
            entries: view
                .entries
                .into_iter()
                .map(crate::runtime::Shared)
                .collect(),
            can_stash: view.can_stash,
            stash_revision: view.stash_revision,
            can_restore: view.can_restore,
            can_delete: view.can_delete,
        }
    }
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.0.changes()
    }
    fn stash_current(
        &self,
    ) -> futures::future::LocalBoxFuture<'static, crate::screens::composer::stash::StashResult>
    {
        use futures::FutureExt;
        self.0.stash_current().map(stash_completion).boxed_local()
    }
    fn restore(
        &self,
        id: String,
    ) -> Result<
        futures::future::LocalBoxFuture<'static, crate::screens::composer::stash::StashResult>,
        crate::screens::composer::stash::StashNotice,
    > {
        use futures::FutureExt;
        self.0
            .restore(id)
            .map(|future| future.map(stash_completion).boxed_local())
            .map_err(stash_error)
    }
    fn delete(
        &self,
        id: String,
    ) -> futures::future::LocalBoxFuture<'static, crate::screens::composer::stash::StashResult>
    {
        use futures::FutureExt;
        self.0.delete(id).map(stash_completion).boxed_local()
    }
}

type BannerViews = Vec<(
    Rc<t3_client::thread_views::composer::banners::ComposerNotice>,
    Rc<crate::screens::composer::banners::ComposerBannerStackItem>,
)>;
pub struct ComposerBannerBinding {
    invoke: Rc<dyn Fn(String, String)>,
    views: RefCell<BannerViews>,
}
impl ComposerBannerBinding {
    pub fn new(invoke: Rc<dyn Fn(String, String)>) -> Self {
        Self {
            invoke,
            views: RefCell::default(),
        }
    }
    pub fn snapshot(
        &self,
        notices: &[Rc<t3_client::thread_views::composer::banners::ComposerNotice>],
    ) -> Vec<Rc<crate::screens::composer::banners::ComposerBannerStackItem>> {
        let mut views = self.views.borrow_mut();
        let mapped = notices
            .iter()
            .map(|notice| {
                let title = crate::i18n::tr(
                    notice.title_template,
                    &notice
                        .title_arguments
                        .iter()
                        .map(|(key, value)| (key.as_str(), crate::i18n::Value::from(value.clone())))
                        .collect::<Vec<_>>(),
                );
                let rendered = views
                    .iter()
                    .find(|(source, rendered)| {
                        Rc::ptr_eq(source, notice)
                            && rendered.title.text.resolve().to_string() == title
                    })
                    .map(|(_, rendered)| rendered.clone())
                    .unwrap_or_else(|| {
                        composer_banner_items(std::slice::from_ref(notice), self.invoke.clone())
                            .remove(0)
                    });
                (notice.clone(), rendered)
            })
            .collect::<Vec<_>>();
        let result = mapped
            .iter()
            .map(|(_, rendered)| rendered.clone())
            .collect();
        *views = mapped;
        result
    }
}
pub fn composer_banner_items(
    notices: &[Rc<t3_client::thread_views::composer::banners::ComposerNotice>],
    invoke: Rc<dyn Fn(String, String)>,
) -> Vec<Rc<crate::screens::composer::banners::ComposerBannerStackItem>> {
    use crate::i18n::Text;
    use crate::screens::composer::banners::{ComposerBannerGroup, ComposerBannerStackItem};
    use crate::ui::{
        alert::composer_banner::Variant,
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        icon::Icon,
    };
    use nsh_core::prelude::*;
    use t3_client::thread_views::composer::banners::BannerVariant;
    notices
        .iter()
        .map(|notice| {
            let actions = notice.actions.clone();
            let id = notice.id.clone();
            let invoke = invoke.clone();
            let dismiss_invoke = invoke.clone();
            let dismiss_id = id.clone();
            Rc::new(ComposerBannerStackItem {
                id: notice.id.clone(),
                variant: match notice.variant {
                    BannerVariant::Default => Variant::Default,
                    BannerVariant::Warning => Variant::Warning,
                    BannerVariant::Error => Variant::Error,
                },
                group: ComposerBannerGroup::Environment,
                icon: match notice.icon {
                    t3_client::thread_views::composer::banners::BannerIcon::Disconnected => {
                        Icon::WifiOff
                    }
                    t3_client::thread_views::composer::banners::BannerIcon::UpdateRunning => {
                        Icon::Download
                    }
                    t3_client::thread_views::composer::banners::BannerIcon::UpdateFailed => {
                        Icon::CircleAlert
                    }
                },
                custom_icon: None,
                title: Text::content(crate::i18n::tr(
                    notice.title_template,
                    &notice
                        .title_arguments
                        .iter()
                        .map(|(key, value)| (key.as_str(), crate::i18n::Value::from(value.clone())))
                        .collect::<Vec<_>>(),
                ))
                .into(),
                description: notice
                    .description
                    .clone()
                    .map(|text| {
                        if notice.description_is_key {
                            Text::client_key(text)
                        } else {
                            Text::content(text)
                        }
                    })
                    .map(Into::into),
                children: None,
                actions: Some(Rc::new(move || {
                    let mut row = view().flex_row();
                    for action in &actions {
                        let id = id.clone();
                        let action_id = action.id.clone();
                        let invoke = invoke.clone();
                        row = row.child(button(
                            ButtonContent::Label(
                                Text::client_key(action.label.clone()).resolve().to_string(),
                            ),
                            ButtonVariant::Ghost,
                            ButtonSize::Xs,
                            || false,
                            move || invoke(id.clone(), action_id.clone()),
                        ));
                    }
                    row.into_element()
                })),
                dismiss_label: notice.dismiss_label.clone().map(Text::client_key),
                on_dismiss: notice.dismiss_label.as_ref().map(|_| {
                    Rc::new(move || dismiss_invoke(dismiss_id.clone(), "dismiss".into()))
                        as Rc<dyn Fn()>
                }),
            })
        })
        .collect()
}

type DraftViews = BTreeMap<
    ThreadKey,
    (
        Rc<t3_client::thread_views::composer::draft_service::ComposerDraftView>,
        Rc<ComposerDraftView>,
    ),
>;

pub fn composer_attachments<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    target: &t3_client::thread_views::composer::target::ComposerTarget,
) -> Result<t3_client::thread_views::composer::attachment_intake::AttachmentSession, ClientError> {
    runtime.composer_attachments(target)
}
struct DraftBinding<P: Platform + 'static>(
    Rc<t3_client::thread_views::composer::draft_service::ComposerDraftService<P>>,
    RefCell<DraftViews>,
);

pub fn composer_drafts<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
) -> Rc<dyn ComposerDraftClient> {
    Rc::new(DraftBinding(runtime.composer_drafts(), RefCell::default()))
}

impl<P: Platform + 'static> ComposerDraftClient for DraftBinding<P> {
    fn changes(&self, _: &ThreadKey) -> LocalBoxStream<'static, ()> {
        self.0.changes()
    }
    fn view(&self, key: &ThreadKey) -> Rc<ComposerDraftView> {
        let view = self.0.view(key);
        if let Some((_, mapped)) = self
            .1
            .borrow()
            .get(key)
            .filter(|(prior, _)| Rc::ptr_eq(prior, &view))
        {
            return mapped.clone();
        }
        let mapped = Rc::new(ComposerDraftView {
            draft_key: view.draft_key.clone(),
            revision: view.revision,
            prompt: view.prompt.clone(),
            cursor: view.cursor,
            loaded: view.loaded,
            writable: view.writable,
            error: view.error.as_ref().map(|e| Text::content(e.to_string())),
        });
        self.1
            .borrow_mut()
            .insert(key.clone(), (view, mapped.clone()));
        mapped
    }
    fn set_prompt(
        &self,
        key: &ThreadKey,
        prompt: String,
        cursor: usize,
    ) -> Result<(), ClientError> {
        self.0.set_prompt(key, prompt, cursor)
    }
}

pub fn primary_action_state(
    state: &t3_client::thread_views::composer::actions::PrimaryActionState,
) -> PrimaryActionState {
    PrimaryActionState {
        pending_action: state.pending_action.as_ref().map(|view| {
            crate::screens::composer::send::PendingActionState {
                question_index: view.question_index,
                is_last_question: view.is_last_question,
                can_advance: view.can_advance,
                is_responding: view.is_responding,
                is_complete: view.is_complete,
            }
        }),
        can_operate_thread: state.can_operate_thread,
        is_running: state.is_running,
        can_interrupt: state.can_interrupt,
        run_id: state.run_id.clone(),
        follow_up_behavior: state.follow_up_behavior.map(|value| match value {
            t3_client::thread_views::composer::actions::FollowUpBehavior::Queue => {
                FollowUpBehavior::Queue
            }
            t3_client::thread_views::composer::actions::FollowUpBehavior::Steer => {
                FollowUpBehavior::Steer
            }
        }),
        can_steer: state.can_steer,
        is_send_busy: state.is_send_busy,
        send_disabled_reason: state.send_disabled_reason.clone().map(Text::client_key),
        is_environment_unavailable: state.is_environment_unavailable,
        is_preparing_worktree: state.is_preparing_worktree,
        has_sendable_content: state.has_sendable_content,
        can_resume: state.can_resume,
        ..Default::default()
    }
}

pub fn model_picker_data(
    view: &t3_client::thread_views::composer::model_service::ComposerModelsView,
) -> Rc<crate::screens::composer::model_picker::PickerData> {
    use crate::screens::composer::model_picker::{ModelKey, PickerData, PickerProvider};
    Rc::new(PickerData {
        providers: view
            .providers
            .iter()
            .map(|provider| PickerProvider {
                snapshot: provider.snapshot.clone(),
                display_name: provider.display_name.clone(),
                enabled: provider.enabled,
                available: provider.available,
                ready: provider.ready,
                continuation_group: provider.continuation_group.clone(),
                accent_color: provider.accent.clone(),
                acp_agent_id: provider.acp_agent.clone(),
                acp_icon_url: provider.acp_icon.clone(),
                options: provider.options.clone(),
            })
            .collect(),
        selection: ModelKey::from(&view.selection),
        selected_models: None,
        favorites: view
            .favorites
            .iter()
            .map(|favorite| ModelKey {
                instance_id: favorite.provider.clone(),
                model: favorite.model.0.clone(),
            })
            .collect(),
        locked_provider: view.locked_provider.clone(),
        locked_continuation_group: view.locked_continuation_group.clone(),
        disabled_models: view
            .disabled_models
            .iter()
            .map(|((instance, model), reason)| {
                (
                    ModelKey {
                        instance_id: instance.clone(),
                        model: model.clone(),
                    },
                    reason.clone(),
                )
            })
            .collect(),
    })
}

pub fn model_catalog_state(
    state: &t3_client::thread_views::composer::model_service::ModelCatalogState,
) -> crate::screens::composer::model_picker::CatalogState {
    use crate::screens::composer::model_picker::CatalogState;
    use t3_client::thread_views::composer::model_service::ModelCatalogState as Client;
    match state {
        Client::Ready => CatalogState::Ready,
        Client::Loading => CatalogState::Loading,
        Client::Offline => CatalogState::Offline,
        Client::Denied(reason) => CatalogState::Denied(reason.clone()),
        Client::Unsupported(reason) => CatalogState::Unsupported(reason.clone()),
        Client::Error(reason) => CatalogState::Error(reason.clone()),
    }
}

pub fn model_traits(
    view: &t3_client::thread_views::composer::model_service::ComposerModelsView,
) -> crate::screens::composer::model_picker::TraitsView {
    use crate::screens::composer::model_picker::{ModelKey, TraitsView};
    TraitsView {
        model: Some(ModelKey::from(&view.selection)),
        reasoning: view.traits.reasoning.clone(),
        others: view.traits.others.clone(),
        speed: view.traits.speed.clone(),
        ultracode: view.traits.ultracode.clone(),
        summary: Some(view.traits.summary.clone()),
        disabled_reason: None,
        unavailable: view.traits.unavailable,
    }
}

pub fn ranked_model_search() -> crate::screens::composer::model_picker::Search {
    use crate::screens::composer::model_picker::ModelKey;
    use t3_client::thread_views::composer::model_search::{
        PickerSearchItem, rank_model_picker_search,
    };
    Rc::new(|data, query| {
        let entries = data
            .providers
            .iter()
            .flat_map(|provider| {
                provider.options.iter().map(move |option| {
                    let key = ModelKey {
                        instance_id: provider.snapshot.instance_id.clone(),
                        model: option.slug.clone(),
                    };
                    let item = PickerSearchItem {
                        driver_kind: provider.snapshot.driver.0.clone(),
                        provider_display_name: provider.display_name.clone(),
                        name: option.name.clone(),
                        short_name: option.short_name.clone(),
                        sub_provider: option.sub_provider.clone(),
                        is_favorite: data.favorites.contains(&key),
                    };
                    (key, item)
                })
            })
            .collect::<Vec<_>>();
        let items = entries
            .iter()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>();
        rank_model_picker_search(&items, query)
            .into_iter()
            .map(|index| entries[index].0.clone())
            .collect()
    })
}

/// CMP-MODEL for a thread composer: the controlled picker over the thread's client model session.
/// `failure` receives operation errors for the composer's error line (server detail is content).
pub fn thread_model_picker<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
    key: &ThreadKey,
    handle: crate::screens::composer::model_picker::PickerHandle,
) -> crate::screens::composer::model_picker::PickerProps {
    use crate::screens::composer::model_picker::{CatalogState, PickerProps};
    use futures::StreamExt;
    use nsh_core::prelude::*;
    let session = runtime.composer_models(key);
    let mut changes = session.changes();
    let current = signal(session.snapshot());
    let failure = signal(None::<String>);
    let watching = session.clone();
    watch(async move {
        while changes.next().await.is_some() {
            current.set(watching.snapshot());
        }
    });
    // Picks come from rows the popover rebuilds; their tasks run under the picker's owner.
    let owner = Owner::current();
    let report =
        move |operation: futures::future::LocalBoxFuture<'static, Result<(), ClientError>>| {
            failure.set(None);
            owner.run(move || {
                watch(async move {
                    if let Err(error) = operation.await {
                        failure.set(Some(error.to_string()));
                    }
                });
            });
        };
    let picking = session.clone();
    let mut props = PickerProps::new(
        computed(move || model_picker_data(&current.get())).into(),
        move |model| {
            report(
                picking.set_model_selection(t3_contracts::model_selection::ModelSelection {
                    instance_id: model.instance_id,
                    model: model.model,
                    options: None,
                }),
            )
        },
    );
    props.catalog = computed(move || {
        failure.get().map_or_else(
            || model_catalog_state(&current.get().state),
            CatalogState::Error,
        )
    })
    .into();
    props.traits = computed(move || model_traits(&current.get())).into();
    props.search = Some(ranked_model_search());
    let favorites = session.clone();
    props.on_toggle_favorite = Some(Rc::new(move |model| {
        report(favorites.toggle_model_favorite(model.instance_id, model.model))
    }));
    let options = session.clone();
    props.on_option_pick = Some(Rc::new(move |model, selection| {
        report(options.change_model_trait(model.instance_id, model.model, selection))
    }));
    props.on_refresh = Some(Rc::new(move || report(session.refresh_model_catalog())));
    props.handle = Some(handle);
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_question_still_takes_options_and_digits() {
        let client = t3_client::thread_views::composer::questions_service::PendingUserInputView {
            request_id: t3_contracts::base_schemas::RuntimeRequestId("request".into()),
            questions: Rc::new(Vec::new()),
            question_index: 0,
            selected_option_values: Vec::new(),
            using_custom_answer: false,
            disabled: false,
            response_disabled: true,
            is_responding: false,
            dismissible: true,
            question_draft_key: String::new(),
            can_previous: false,
            is_last_question: true,
            is_complete: false,
            error: None,
        };
        assert!(!question_view(&client).response_disabled);
        let responding =
            t3_client::thread_views::composer::questions_service::PendingUserInputView {
                is_responding: true,
                ..client.clone()
            };
        assert!(question_view(&responding).response_disabled);
        let disabled = t3_client::thread_views::composer::questions_service::PendingUserInputView {
            disabled: true,
            ..client
        };
        assert!(question_view(&disabled).response_disabled);
    }
}
