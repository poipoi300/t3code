//! Port of apps/web/src/components/chat/ComposerPendingUserInputPanel.tsx ComposerPendingUserInputPanel.

use std::rc::Rc;
use std::time::Duration;

use futures::{StreamExt, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::text::{
    TextDirection, markdown_prose_direction, text_direction,
};
use t3_client::view_logic::shell::shortcuts::DispatchEvent;
use t3_contracts::base_schemas::RuntimeRequestId;
use t3_contracts::orchestration_v2::OrchestrationV2UserInputQuestion;

use crate::i18n::{Text as UiText, Value, tr, ts};
use crate::runtime::Shared;
use crate::screens::thread::markdown::base::chat_markdown_with_root_classes;
use crate::theme::{cn, tw};
use crate::ui::alert::composer_banner as banner;
use crate::ui::badge::Child;
use crate::ui::collapsible::{CollapsibleHandle, collapsible_panel};
use crate::ui::kbd::{Variant as KbdVariant, kbd};
use crate::ui::switch::{Variant as CheckboxVariant, checkbox_mark};

use crate::runtime::tasks::watch;

/// Presentation of the first pending request, projected by the client's question session.
/// Selection, custom-answer precedence and the normalized index come from pending_user_input_progress.
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
}

/// The client retains scoped answers/position, returns displaced text, projects context and sends.
/// Commands revalidate the current destination and request; stale ids must never affect a new question.
pub trait PendingUserInputClient {
    fn changes(&self, thread: &ThreadKey) -> LocalBoxStream<'static, ()>;
    fn view(&self, thread: &ThreadKey) -> Option<Rc<PendingUserInputView>>;
    fn toggle_option(
        &self,
        thread: &ThreadKey,
        request: &RuntimeRequestId,
        question: &str,
        value: &str,
    ) -> Result<(), ClientError>;
    fn advance(
        &self,
        thread: &ThreadKey,
        request: &RuntimeRequestId,
        question: &str,
    ) -> Result<(), ClientError>;
    fn dismiss(&self, thread: &ThreadKey, request: &RuntimeRequestId) -> Result<(), ClientError>;
}

#[derive(Clone, Copy)]
pub struct ComposerQuestions {
    client: Stored<Rc<dyn PendingUserInputClient>>,
    thread: Stored<ThreadKey>,
    pub view: Signal<Option<Shared<PendingUserInputView>>>,
    pub error: Signal<Option<UiText>>,
    interaction: Signal<Option<Interaction>>,
}

impl ComposerQuestions {
    pub fn attach(client: Rc<dyn PendingUserInputClient>, thread: ThreadKey) -> Self {
        let mut changes = client.changes(&thread);
        let view = signal(client.view(&thread).map(Shared));
        let interaction = signal(None);
        let source = client.clone();
        let key = thread.clone();
        watch(async move {
            while changes.next().await.is_some() {
                let next = source.view(&key).map(Shared);
                let replaced = view.with(|current| {
                    current.as_ref().map(|view| &view.request_id)
                        != next.as_ref().map(|view| &view.request_id)
                });
                if replaced {
                    interaction.set(None);
                }
                view.set(next);
            }
        });
        Self {
            client: stored(client),
            thread: stored(thread),
            view,
            error: signal(None),
            interaction,
        }
    }

    fn report(self, result: Result<(), ClientError>) -> bool {
        let success = result.is_ok();
        self.error.set(result.err().map(|error| match error {
            ClientError::Rejected { message, detail } => UiText::content(
                t3_client::error::rejection_text(&message, detail.as_deref()),
            ),
            ClientError::InvalidPayload(message) => UiText::client_key(message),
            ClientError::Disconnected => "RPC disconnected".into(),
            ClientError::EnvironmentMismatch => "unexpected environment identity".into(),
            ClientError::Cancelled => "cancelled".into(),
            other => UiText::content(other.to_string()),
        }));
        success
    }

    /// Register in the host's bubble key scope; the editor keeps all keys while it has focus.
    pub fn handle_key(
        self,
        event: &mut DispatchEvent,
        background_thread: bool,
        inside_side_chat: bool,
    ) -> bool {
        let key = &event.key;
        if event.default_prevented
            || !key.is_keydown()
            || key.editable_focus
            || background_thread
            || inside_side_chat
            || key.modifiers.meta
            || key.modifiers.ctrl
            || key.modifiers.alt
        {
            return false;
        }
        let [digit] = key.key.as_bytes() else {
            return false;
        };
        if !(b'1'..=b'9').contains(digit) {
            return false;
        }
        let index = usize::from(*digit - b'1');
        let Some(interaction) = self.interaction.get_untracked() else {
            return false;
        };
        let Some(view) = self
            .view
            .get_untracked()
            .filter(|view| !view.response_disabled && !view.disabled && !view.is_responding)
        else {
            return false;
        };
        let Some(question) = view.questions.get(view.question_index).filter(|question| {
            interaction.collapsed.get_untracked().as_deref() != Some(&question.id.0)
        }) else {
            return false;
        };
        let Some(option) = question.options.get(index) else {
            return false;
        };
        let value = option
            .value
            .as_ref()
            .and_then(Option::as_ref)
            .unwrap_or(&option.label.0)
            .clone();
        event.claim();
        Card {
            questions: self,
            collapsed: interaction.collapsed,
            optimistic: interaction.optimistic,
            timer: interaction.timer,
        }
        .choose(view.request_id.clone(), question.id.0.clone(), value);
        true
    }
}

#[derive(Clone, Copy)]
struct Interaction {
    collapsed: Signal<Option<String>>,
    optimistic: Signal<Option<(String, String)>>,
    timer: Stored<Option<Spawned<()>>>,
}

impl PartialEq for Interaction {
    fn eq(&self, other: &Self) -> bool {
        self.collapsed == other.collapsed
    }
}

#[derive(Clone, Copy)]
struct Card {
    questions: ComposerQuestions,
    collapsed: Signal<Option<String>>,
    optimistic: Signal<Option<(String, String)>>,
    timer: Stored<Option<Spawned<()>>>,
}

impl Card {
    fn cancel_timer(self) {
        if let Some(timer) = self.timer.with(Option::take) {
            timer.cancel();
        }
    }

    fn choose(self, request: RuntimeRequestId, question_id: String, value: String) {
        let current = self.questions.view.get_untracked();
        let Some(current) = current.filter(|current| {
            current.request_id == request && !current.disabled && !current.is_responding
        }) else {
            return;
        };
        let Some(question) = current
            .questions
            .get(current.question_index)
            .filter(|question| question.id.0 == question_id)
        else {
            return;
        };
        let single = question.multi_select != Some(Some(true));
        if single {
            self.optimistic
                .set(Some((question_id.clone(), value.clone())));
        }
        let result = self.questions.client.with(|client| {
            self.questions
                .thread
                .with(|thread| client.toggle_option(thread, &request, &question_id, &value))
        });
        if !self.questions.report(result) {
            self.optimistic.set(None);
            return;
        }
        if !single {
            return;
        }
        self.cancel_timer();
        self.timer.with(|timer| {
            *timer = Some(set_timeout(Duration::from_millis(200), move || {
                let current = self.questions.view.get_untracked();
                if current.as_ref().is_none_or(|current| {
                    current.request_id != request
                        || current.response_disabled
                        || current.is_responding
                        || current
                            .questions
                            .get(current.question_index)
                            .is_none_or(|question| question.id.0 != question_id)
                }) {
                    return;
                }
                let result = self.questions.client.with(|client| {
                    self.questions
                        .thread
                        .with(|thread| client.advance(thread, &request, &question_id))
                });
                self.questions.report(result);
            }))
        });
    }
}

pub fn composer_pending_user_input_panel(questions: ComposerQuestions) -> impl IntoElement {
    let request = computed(move || {
        questions.view.with(|view| {
            view.as_ref()
                .filter(|view| view.questions.get(view.question_index).is_some())
                .map(|view| view.request_id.clone())
        })
    });
    dynamic(move || {
        request
            .get()
            .map(|_| untracked(|| composer_pending_user_input_card(questions)))
    })
}

fn composer_pending_user_input_card(questions: ComposerQuestions) -> impl IntoElement {
    let card = Card {
        questions,
        collapsed: signal(None),
        optimistic: signal(None),
        timer: stored(None),
    };
    questions.interaction.set(Some(Interaction {
        collapsed: card.collapsed,
        optimistic: card.optimistic,
        timer: card.timer,
    }));
    let active = computed(move || {
        questions.view.with(|view| {
            view.as_ref().and_then(|view| {
                view.questions
                    .get(view.question_index)
                    .map(|question| question.id.0.clone())
            })
        })
    });
    let collapsed =
        computed(move || active.with(|active| active.is_some() && *active == card.collapsed.get()));
    let body_question = computed(move || {
        questions.view.with(|view| {
            view.as_ref()
                .map(|view| (Shared(view.questions.clone()), view.question_index))
        })
    });
    let heading_directions = computed(move || {
        body_question.with(|current| {
            current.as_ref().and_then(|(questions, index)| {
                questions.get(*index).map(|question| {
                    (
                        text_direction(&question.header.0),
                        markdown_prose_direction(&question.question.0),
                    )
                })
            })
        })
    });
    let enabled_question = computed(move || {
        questions.view.with(|view| {
            view.as_ref().and_then(|view| {
                (!view.response_disabled && !view.is_responding)
                    .then(|| (view.request_id.clone(), active.get()))
            })
        })
    });
    effect(move || {
        enabled_question.get();
        untracked(|| card.cancel_timer());
    });
    effect(move || {
        let current = questions.view.get();
        untracked(|| {
            if let Some((question, value)) = card.optimistic.get_untracked() {
                let acknowledged = current.as_ref().is_none_or(|current| {
                    current
                        .questions
                        .get(current.question_index)
                        .is_none_or(|active| active.id.0 != question)
                        || (!current.using_custom_answer
                            && current.selected_option_values.contains(&value))
                });
                if acknowledged {
                    card.optimistic.set(None);
                }
            }
        });
    });
    let question_set = computed(move || {
        questions
            .view
            .with(|view| view.as_ref().map(|view| Shared(view.questions.clone())))
    });
    let direction = computed(move || {
        question_set.with(|questions| {
            questions.as_ref().and_then(|questions| {
                let source = questions
                    .iter()
                    .flat_map(|question| {
                        std::iter::once(question.header.0.as_str())
                            .chain(std::iter::once(question.question.0.as_str()))
                            .chain(question.options.iter().flat_map(|option| {
                                [option.label.0.as_str(), option.description.0.as_str()]
                            }))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                markdown_prose_direction(&source)
            })
        })
    });
    let handle = CollapsibleHandle::controlled(
        move || !collapsed.get(),
        || false,
        move |open| {
            card.collapsed
                .set(if open { None } else { active.get_untracked() })
        },
    );
    view().direction(move || layout_direction(direction.get())).child(
        view().app_class(tw("flex w-full items-center pe-4"))
            // NSH press callbacks cannot stop propagation: dismissal is a sibling of the trigger.
            .child(nsh_base::button()
                .accessibility_label(move || Some(if collapsed.get() { ts("Show the question and its options") } else { ts("Hide the question and its options") }))
                .on_press(move || handle.toggle())
                .app_class(tw("group/banner-row flex min-h-7 sm:min-h-6 min-w-0 flex-1 items-center gap-x-1 text-start cursor-pointer rounded-xl hover:bg-foreground/5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring px-4 py-1.5 pe-0"))
                .child(view().app_class(tw("flex min-w-0 flex-1 items-center gap-1 ps-2 sm:ps-1.5"))
                    .child(text(move || questions.view.with(|view| view.as_ref().and_then(|view| view.questions.get(view.question_index)).map(|question| question.header.0.clone()).unwrap_or_default()))
                        .style_with(move |style| style.writing_direction(writing_direction(heading_directions.get().and_then(|directions| directions.0))))
                        .app_class(move || Some(tw(&cn([Some("min-w-0 truncate text-muted-foreground"), collapsed.get().then_some("max-w-1/2")])))))
                    .child(when(collapsed, move || view().app_class(tw("flex min-w-0 flex-1 items-center gap-1"))
                        .child(banner::separator(true))
                        .child(text(move || questions.view.with(|view| view.as_ref().and_then(|view| view.questions.get(view.question_index)).map(|question| question.question.0.clone()).unwrap_or_default()))
                            .style_with(move |style| style.writing_direction(writing_direction(heading_directions.get().and_then(|directions| directions.1))))
                            .app_class(move || Some(tw(&cn([Some("min-w-0 flex-1 truncate text-secondary-label"), Some(align_class(direction.get()))]))))))))
                .child(view().app_class(tw("flex flex-wrap items-center justify-end gap-0"))
                    .child(dynamic(move || questions.view.with(|view| view.as_ref().filter(|view| view.questions.len() > 1).map(|view| banner::count(text(tr("{index}/{count}", &[("index", Value::from(view.question_index + 1)), ("count", Value::from(view.questions.len()))])))))))
                    .child(banner::toggle_icon(move || !collapsed.get()))))
            .child(dynamic(move || questions.view.with(|view| view.as_ref().filter(|view| view.dismissible).map(|view| {
                let request = view.request_id.clone();
                banner::dismiss("Dismiss question without answering".into(), move || questions.view.with(|view| view.as_ref().is_none_or(|view| view.is_responding)), move || {
                    if questions.view.with(|view| view.as_ref().is_none_or(|view| !view.dismissible || view.is_responding || view.request_id != request)) { return; }
                    card.cancel_timer();
                    let result = questions.client.with(|client| questions.thread.with(|thread| client.dismiss(thread, &request)));
                    questions.report(result);
                })
            }))))
    ).child(collapsible_panel(handle, true, dynamic(move || {
        body_question.get()?;
        let id = active.get()?;
        Some(untracked(|| question_body(card, id, direction)))
    })))
}

fn question_body(
    card: Card,
    id: String,
    direction: Memo<Option<TextDirection>>,
) -> impl IntoElement {
    let current = card.questions.view.get_untracked();
    let question = current
        .as_ref()
        .and_then(|view| view.questions.get(view.question_index))
        .cloned();
    // wrap-anywhere is redundant: native text wraps and retains newlines.
    banner::scroll(view().app_class(tw("min-w-0 ps-6 pe-4 pb-1 sm:ps-5.5"))
        .child(question.map(|question| {
            let question_text = question.question.0;
            view().child(view().app_class(move || Some(tw(align_class(direction.get()))))
                // TextView has no soft-break-as-hard-break option; source newlines retain its Markdown semantics.
                .child(chat_markdown_with_root_classes(move || question_text.clone(), || false, "pt-0.5 text-foreground/85")))
                .child(question.options.into_iter().enumerate().fold(view().app_class(tw("-ms-2 mt-1.5 sm:-ms-1.5")), |column, (index, option)| {
                    let label = option.label.0;
                    let description = option.description.0;
                    let value = option.value.flatten().unwrap_or_else(|| label.clone());
                    let chosen = value.clone();
                    let selected_id = id.clone();
                    let selected = computed(move || card.optimistic.with(|optimistic| optimistic.as_ref().is_some_and(|(question, value)| *question == selected_id && *value == chosen)) || card.questions.view.with(|view| view.as_ref().is_some_and(|view| !view.using_custom_answer && view.selected_option_values.contains(&chosen))));
                    let request = current.as_ref().expect("question came from this request").request_id.clone();
                    let select_id = id.clone();
                    let disabled = move || card.questions.view.with(|view| view.as_ref().is_none_or(|view| view.disabled || view.is_responding));
                    let preview = option.preview.flatten();
                    column.child(view()
                        .child(nsh_base::button().accessibility_label(Some(SharedString::from(label.clone())))
                            .selected(move || selected.get()).disabled(disabled).on_press(move || card.choose(request.clone(), select_id.clone(), value.clone()))
                            .app_class(move || Some(tw(&cn([
                                Some(align_class(direction.get())),
                                Some("group flex w-full items-start gap-2.5 rounded-md bg-transparent py-1.5 ps-2 sm:ps-1.5 outline-none transition-colors duration-150 focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-primary/25"),
                                Some(if selected.get() { "text-foreground" } else { "text-foreground/70 hover:bg-muted/30" }),
                                Some(if disabled() { "opacity-50 cursor-not-allowed" } else { "cursor-pointer" }),
                            ]))))
                            .child(view().app_class(tw("pointer-events-none flex h-5 shrink-0 items-center"))
                                .child(if question.multi_select == Some(Some(true)) { checkbox_mark(Prop::bind(move || selected.get()), CheckboxVariant::Contrast).into_element() } else { option_radio(selected).into_element() }))
                            .child(view().app_class(tw("min-w-0 flex-1 flex flex-col gap-0.5"))
                                .child(text(SharedString::from(label.clone())).writing_direction(writing_direction(text_direction(&label))).app_class(tw("text-sm font-medium")))
                                .child((!description.is_empty() && description != label).then(|| text(SharedString::from(description.clone())).writing_direction(writing_direction(text_direction(&description))).app_class(tw("text-secondary-label text-2xs")))))
                            .child((index < 9).then(|| view().app_class(tw("flex h-5 w-7 shrink-0 items-center justify-center sm:w-6")).child(kbd(KbdVariant::Default, vec![Child::Text(UiText::content((index + 1).to_string()))])))))
                        .child(preview.map(|preview| view().app_class(tw("ps-9 pe-2 pb-1.5 sm:ps-8")).child(chat_markdown_with_root_classes(move || preview.clone(), || false, "text-xs text-foreground/70")))))
                }))
        })))
}

fn option_radio(checked: Memo<bool>) -> impl IntoElement {
    view().accessible(false)
        // not-dark:bg-clip-padding is a CSS paint hint; native borders already clip their fill.
        .app_class(move || Some(tw(&cn([Some("relative inline-flex size-4.5 shrink-0 items-center justify-center rounded-full border border-foreground/55 bg-background sm:size-4 dark:bg-input/32"), Some(if checked.get() { "shadow-none" } else { "shadow-xs/5" })]))))
        .child(when(checked, || view().app_class(tw("-inset-px absolute flex items-center justify-center rounded-full bg-primary"))
            .child(view().app_class(tw("size-2 rounded-full bg-primary-foreground sm:size-1.5")))))
}

fn align_class(direction: Option<TextDirection>) -> &'static str {
    if direction == Some(TextDirection::Rtl) {
        "text-right"
    } else {
        "text-left"
    }
}
fn layout_direction(direction: Option<TextDirection>) -> LayoutDirection {
    if direction == Some(TextDirection::Rtl) {
        LayoutDirection::Rtl
    } else {
        LayoutDirection::Ltr
    }
}
fn writing_direction(direction: Option<TextDirection>) -> WritingDirection {
    match direction {
        Some(TextDirection::Rtl) => WritingDirection::Rtl,
        Some(TextDirection::Ltr) => WritingDirection::Ltr,
        None => WritingDirection::Auto,
    }
}
