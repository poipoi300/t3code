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

struct QuestionsBinding<P: Platform + Clone + 'static>(ClientRuntime<P>);
pub fn pending_user_input<P: Platform + Clone + 'static>(
    runtime: &ClientRuntime<P>,
) -> Rc<dyn crate::screens::composer::questions::PendingUserInputClient> {
    Rc::new(QuestionsBinding(runtime.clone()))
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
            Rc::new(crate::screens::composer::questions::PendingUserInputView {
                request_id: view.request_id.clone(),
                questions: view.questions.clone(),
                question_index: view.question_index,
                selected_option_values: view.selected_option_values.clone(),
                using_custom_answer: view.using_custom_answer,
                disabled: view.disabled,
                response_disabled: view.response_disabled,
                is_responding: view.is_responding,
                dismissible: view.dismissible,
            })
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
                icon: Icon::WifiOff,
                custom_icon: None,
                title: Text::client_key(notice.title.clone()).into(),
                description: notice
                    .description
                    .clone()
                    .map(Text::content)
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
