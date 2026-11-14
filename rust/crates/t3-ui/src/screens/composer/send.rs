//! Port of apps/web/src/components/chat/ComposerPrimaryActions.tsx ComposerPrimaryActions.

use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use nsh_core::prelude::*;
use t3_client::composition::{ClientRuntime, SendError};
use t3_client::operations::{ComposerDispatchMode, resolve_composer_dispatch_mode};
use t3_client::platform::Platform;
use t3_client::thread_views::composer::send::{PendingMessage, SendOptions, SendState};
use t3_client::thread_views::composer::submission::{
    PromptLengthValidation, get_composer_prompt_length_validation_message,
};
use t3_client::thread_views::outbox::DispatchMode;
use t3_client::types::ThreadKey;
use t3_contracts::base_schemas::MessageId;

use crate::i18n::{Text, Value, t, tr};
use crate::state::layout::layout_class;
use crate::theme::{cn, metrics, palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingActionState {
    pub question_index: usize,
    pub is_last_question: bool,
    pub can_advance: bool,
    pub is_responding: bool,
    pub is_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUpBehavior {
    Queue,
    Steer,
}

impl From<FollowUpBehavior> for ComposerDispatchMode {
    fn from(action: FollowUpBehavior) -> Self {
        match action {
            FollowUpBehavior::Queue => Self::Queue,
            FollowUpBehavior::Steer => Self::Steer,
        }
    }
}

/// Facts supplied by the client's composer projection, including its permission availability.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimaryActionState {
    pub compact: bool,
    pub can_operate_thread: bool,
    pub pending_action: Option<PendingActionState>,
    pub is_running: bool,
    pub can_interrupt: bool,
    pub run_id: Option<String>,
    pub follow_up_behavior: Option<FollowUpBehavior>,
    pub can_steer: bool,
    pub alternate_modifier: bool,
    pub voice_input_active: bool,
    pub is_send_busy: bool,
    pub send_disabled_reason: Option<Text>,
    pub is_environment_unavailable: bool,
    pub is_preparing_worktree: bool,
    pub has_sendable_content: bool,
    pub can_resume: bool,
    pub is_editing_queued_message: bool,
    /// The client's already-formatted context-window count, not a translation key.
    pub compact_tokens: Option<String>,
    pub hide_compact_toggle: bool,
    pub keep_full_history: bool,
}

#[derive(Clone)]
pub struct PrimaryActionsProps {
    pub state: Prop<PrimaryActionState>,
    /// The source's dictation slot, before the primary action in every branch.
    pub children: Option<Rc<dyn Fn() -> AnyElement>>,
    pub on_submit_message: Rc<dyn Fn(ComposerDispatchMode)>,
    pub on_interrupt: Option<Rc<dyn Fn()>>,
    pub on_resume: Option<Rc<dyn Fn()>>,
    pub on_previous_pending_question: Option<Rc<dyn Fn()>>,
    pub on_toggle_keep_full_history: Option<Rc<dyn Fn()>>,
    /// Retained source contract; pinned NSH Button cannot suppress pointer focus.
    pub preserve_composer_focus_on_pointer_down: bool,
    /// Measured by ComposerSurface; NSH has no named container queries.
    pub composer_surface_width: Prop<f32>,
    pub modifier_label: Prop<Option<String>>,
    pub alternate_shortcut_label: Prop<Option<String>>,
}

impl PrimaryActionsProps {
    pub fn new(
        state: impl Into<Prop<PrimaryActionState>>,
        on_submit_message: impl Fn(ComposerDispatchMode) + 'static,
    ) -> Self {
        Self {
            state: state.into(),
            children: None,
            on_submit_message: Rc::new(on_submit_message),
            on_interrupt: None,
            on_resume: None,
            on_previous_pending_question: None,
            on_toggle_keep_full_history: None,
            preserve_composer_focus_on_pointer_down: false,
            composer_surface_width: Prop::new(0.0),
            modifier_label: Prop::new(None),
            alternate_shortcut_label: Prop::new(None),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StopPhase {
    #[default]
    None,
    Pending,
    TimedOut,
    Landed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StopState {
    run_id: Option<String>,
    phase: StopPhase,
}

impl StopState {
    fn observe(&mut self, state: &PrimaryActionState) {
        if state.run_id.is_some() && self.run_id != state.run_id {
            self.run_id = state.run_id.clone();
            self.phase = StopPhase::None;
        } else if state.can_operate_thread {
            if matches!(self.phase, StopPhase::Pending | StopPhase::TimedOut)
                && !state.can_interrupt
            {
                self.phase = StopPhase::Landed;
            } else if self.phase == StopPhase::Landed && state.can_interrupt {
                self.phase = StopPhase::None;
            }
        }
    }
}

fn use_stop_phase(state: Prop<PrimaryActionState>) -> Signal<StopState> {
    let stop = signal(StopState {
        run_id: state.get().run_id,
        ..Default::default()
    });
    effect(move || {
        let state = state.get();
        let mut next = stop.get_untracked();
        next.observe(&state);
        if next != stop.get_untracked() {
            stop.set(next);
        }
    });
    // The two timers are visual click guards; the client owns interruption and acknowledgement.
    effect(move || {
        let phase = stop.get().phase;
        let (delay, next) = match phase {
            StopPhase::Pending => (10_000, StopPhase::TimedOut),
            StopPhase::Landed => (500, StopPhase::None),
            _ => return,
        };
        let timer = set_timeout(Duration::from_millis(delay), move || {
            if stop.get_untracked().phase == phase {
                stop.update(|state| state.phase = next);
            }
        });
        cleanup(move || timer.cancel());
    });
    stop
}

fn pending_primary_action_label(
    state: &PrimaryActionState,
    pending: &PendingActionState,
) -> String {
    if pending.is_responding {
        t("Submitting...")
    } else if state.compact {
        if pending.is_last_question {
            t("Submit")
        } else {
            t("Next")
        }
    } else if !pending.is_last_question {
        t("Next question")
    } else if pending.question_index > 0 {
        t("Submit answers")
    } else {
        t("Submit answer")
    }
}

fn dispatch_mode(state: &PrimaryActionState) -> ComposerDispatchMode {
    resolve_composer_dispatch_mode(
        state.is_running,
        state.alternate_modifier,
        state.follow_up_behavior.map(Into::into),
        state.can_steer,
    )
}

fn show_resume(state: &PrimaryActionState) -> bool {
    state.can_resume && !state.has_sendable_content && !state.is_editing_queued_message
}

fn compact_tokens(state: &PrimaryActionState) -> Option<&str> {
    (!show_resume(state) && !state.is_editing_queued_message)
        .then_some(state.compact_tokens.as_deref())
        .flatten()
}

fn submit_label(state: &PrimaryActionState) -> String {
    if show_resume(state) {
        t("Resume thread")
    } else if state.is_editing_queued_message {
        t("Update queued message")
    } else if compact_tokens(state).is_some() && !state.keep_full_history {
        t("Compact and send")
    } else if dispatch_mode(state) == ComposerDispatchMode::Queue {
        t("Queue message")
    } else if state.is_running {
        t("Steer message")
    } else {
        t("Submit message")
    }
}

fn submit_status(state: &PrimaryActionState) -> Option<SharedString> {
    if state.is_environment_unavailable {
        Some(t("Environment disconnected").into())
    } else if let Some(reason) = &state.send_disabled_reason {
        Some(reason.resolve())
    } else if state.is_preparing_worktree {
        Some(t("Preparing worktree").into())
    } else if state.is_send_busy {
        Some(
            if state.is_editing_queued_message {
                t("Updating queued message")
            } else {
                t("Submitting message")
            }
            .into(),
        )
    } else {
        None
    }
}

fn dispatch_action_label(mode: ComposerDispatchMode) -> Option<String> {
    match mode {
        ComposerDispatchMode::Queue => Some(t("queue")),
        ComposerDispatchMode::Steer => Some(t("steer")),
        ComposerDispatchMode::Auto
        | ComposerDispatchMode::Start
        | ComposerDispatchMode::Restart => None,
    }
}

fn submit_tooltip(
    state: &PrimaryActionState,
    modifier: Option<String>,
    shortcut: Option<String>,
) -> SharedString {
    if let Some(status) = submit_status(state) {
        return status;
    }
    if let Some(tokens) = compact_tokens(state).filter(|_| !state.keep_full_history) {
        return tr(
            "Summarize {tokens} tokens of history, then send",
            &[("tokens", Value::from(tokens))],
        )
        .into();
    }
    if state.is_running
        && !state.is_editing_queued_message
        && state.can_steer
        && let Some(modifier) = modifier
    {
        let Some(action) = dispatch_action_label(
            state
                .follow_up_behavior
                .unwrap_or(FollowUpBehavior::Steer)
                .into(),
        ) else {
            return submit_label(state).into();
        };
        let Some(alternate) = dispatch_action_label(resolve_composer_dispatch_mode(
            true,
            true,
            state.follow_up_behavior.map(Into::into),
            true,
        )) else {
            return submit_label(state).into();
        };
        return if let Some(shortcut) = shortcut {
            tr(
                "Click to {action}, {modifier}-click or {shortcut} to {alternate}",
                &[
                    ("action", Value::from(action)),
                    ("modifier", Value::from(modifier)),
                    ("shortcut", Value::from(shortcut)),
                    ("alternate", Value::from(alternate)),
                ],
            )
        } else {
            tr(
                "Click to {action}, {modifier}-click to {alternate}",
                &[
                    ("action", Value::from(action)),
                    ("modifier", Value::from(modifier)),
                    ("alternate", Value::from(alternate)),
                ],
            )
        }
        .into();
    }
    submit_label(state).into()
}

fn send_disabled(state: &PrimaryActionState) -> bool {
    !state.can_operate_thread
        || state.send_disabled_reason.is_some()
        || state.is_environment_unavailable
        || if let Some(pending) = &state.pending_action {
            pending.is_responding
                || (!state.voice_input_active
                    && if pending.is_last_question {
                        !pending.is_complete
                    } else {
                        !pending.can_advance
                    })
        } else {
            state.is_send_busy || (!state.has_sendable_content && !show_resume(state))
        }
}

fn stop_visible(state: &PrimaryActionState, phase: StopPhase) -> bool {
    state.can_interrupt || matches!(phase, StopPhase::Pending | StopPhase::TimedOut)
}

fn stop_generation_button(
    props: PrimaryActionsProps,
    stop: Signal<StopState>,
    inside_pending_action: bool,
) -> impl IntoElement {
    let enabled = props.clone();
    let action = props.clone();
    let hint = tooltip_create_handle();
    view()
        .app_class(tw("inline-flex"))
        .tooltip(tooltip(hint, TooltipOptions::default(), text(move || {
            if stop.get().phase == StopPhase::Pending { t("Stopping…") } else { t("Interrupt") }
        })))
        .child(nsh_base::button()
            .accessibility_label(move || Some(if stop.get().phase == StopPhase::Pending {
                t("Stopping")
            } else {
                t("Stop generation")
            }.into()))
            .disabled(move || !enabled.state.get().can_operate_thread
                || enabled.on_interrupt.is_none()
                || stop.get().phase == StopPhase::Pending)
            // NSH ButtonEvent::PressDown has no prevent-default or pointer-focus policy.
            .on_press(move || {
                if !action.state.get().can_operate_thread || stop.get_untracked().phase == StopPhase::Pending {
                    return;
                }
                if let Some(interrupt) = &action.on_interrupt {
                    stop.update(|state| state.phase = StopPhase::Pending);
                    interrupt();
                }
            })
            // index.css control inset shadows are expanded; the SVG selector moves to the glyph.
            .app_class(tw(&cn([
                Some("flex shrink-0 cursor-pointer items-center justify-center rounded-full bg-destructive/90 text-white shadow-xs shadow-destructive/24 inset-shadow-[0_1px_rgb(255_255_255/16%)] transition-colors duration-150 hover:bg-destructive active:inset-shadow-[0_1px_rgb(0_0_0/8%)] active:shadow-none disabled:cursor-default disabled:opacity-64 disabled:hover:bg-destructive/90"),
                Some(if inside_pending_action { "size-8 sm:size-7" } else { "size-8 sm:h-8 sm:w-8" }),
            ])))
            .hit_slop(move || {
                let height = if inside_pending_action && !layout_class().is_compact() { 28.0 } else { 32.0 };
                EdgeInsets::all(px(((metrics().hit_target - height) / 2.0).max(0.0)))
            })
            .child(svg("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Crect x='2' y='2' width='8' height='8' rx='1.5' fill='white'/%3E%3C/svg%3E")
                .size(px(12.0)).accessible(false).pointer_events(PointerEvents::None)))
}

/// The web's action group. Callback implementations are client operations, never RPC handlers.
pub fn composer_primary_actions(props: PrimaryActionsProps) -> impl IntoElement {
    let stop = use_stop_phase(props.state.clone());
    view()
        .app_class(tw("flex items-center justify-end gap-2"))
        .child(dynamic(move || {
            let state = props.state.get();
            let phase = stop.get().phase;
            let props = props.clone();
            untracked(move || {
                if let Some(pending) = &state.pending_action {
                    return pending_actions(props, stop, &state, pending).into_element();
                }
                if stop_visible(&state, phase)
                    && !state.has_sendable_content
                    && !state.is_editing_queued_message
                {
                    return view()
                        .app_class(tw("flex items-center gap-2"))
                        .child(props.children.as_ref().map(|render| render()))
                        .child(stop_generation_button(props, stop, false))
                        .into_element();
                }
                let mut actions = view().app_class(tw("flex items-center gap-2"));
                if compact_tokens(&state).is_some() && !state.hide_compact_toggle {
                    actions = actions.child(compact_toggle(props.clone()));
                }
                actions
                    .child(props.children.as_ref().map(|render| render()))
                    .child(send_button(props.clone(), stop))
                    .child(
                        matches!(phase, StopPhase::Pending | StopPhase::TimedOut)
                            .then(|| stop_generation_button(props, stop, false)),
                    )
                    .into_element()
            })
        }))
}

fn pending_actions(
    props: PrimaryActionsProps,
    stop: Signal<StopState>,
    state: &PrimaryActionState,
    pending: &PendingActionState,
) -> impl IntoElement + use<> {
    let previous = props.clone();
    let enabled = props.clone();
    let submit = props.clone();
    view().app_class(tw(&cn([
        Some("flex items-center justify-end"),
        Some(if state.compact { "gap-1.5" } else { "gap-2" }),
    ])))
        .child((pending.question_index > 0).then(|| button(
            if state.compact { ButtonContent::Icon { icon: Icon::ChevronLeft, label: t("Previous question") } }
                else { ButtonContent::Label(t("Previous")) },
            ButtonVariant::Outline,
            if state.compact { ButtonSize::IconSm } else { ButtonSize::Sm },
            move || enabled.on_previous_pending_question.is_none()
                || enabled.state.get().pending_action.as_ref().is_none_or(|p| p.is_responding),
            move || {
                if previous.state.get().pending_action.as_ref().is_some_and(|p| !p.is_responding)
                    && let Some(action) = &previous.on_previous_pending_question { action(); }
            },
        )))
        .child(props.children.as_ref().map(|render| render()))
        .child(nsh_base::button()
            .accessibility_label(Some(pending_primary_action_label(state, pending).into()))
            .app_class(tw(&cn([
                Some("inline-flex shrink-0 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap rounded-full bg-message-action font-medium text-base text-message-action-foreground shadow-xs shadow-message-action/24 outline-none hover:bg-message-action-hover focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 disabled:shadow-none sm:text-sm h-8 sm:h-7"),
                Some(if state.compact { "px-3" } else { "px-4" }),
            ])))
            .disabled({ let state = props.state.clone(); move || send_disabled(&state.get()) })
            .on_press(move || {
                let state = submit.state.get();
                if !send_disabled(&state) && stop.get_untracked().phase != StopPhase::Landed {
                    (submit.on_submit_message)(dispatch_mode(&state));
                }
            })
            .child(text(pending_primary_action_label(state, pending))))
        .child(stop_visible(state, stop.get_untracked().phase).then(|| stop_generation_button(props, stop, true)))
}

fn send_button(props: PrimaryActionsProps, stop: Signal<StopState>) -> impl IntoElement {
    let status = props.state.clone();
    let disabled = props.clone();
    let action = props.clone();
    let glyph = props.state.clone();
    let hint = tooltip_create_handle();
    view().app_class(tw("inline-flex"))
        .tooltip(tooltip(hint, TooltipOptions::default(), text({
            let state = props.state.clone();
            let modifier = props.modifier_label.clone();
            let shortcut = props.alternate_shortcut_label.clone();
            move || {
                let state = state.get();
                submit_tooltip(&state, modifier.get(), shortcut.get())
            }
        })))
        .child(nsh_base::button()
            .accessibility_label(move || {
                let state = status.get();
                Some(submit_status(&state).unwrap_or_else(|| submit_label(&state).into()))
            })
            .disabled(move || {
                let state = disabled.state.get();
                send_disabled(&state) || (show_resume(&state) && disabled.on_resume.is_none())
            })
            // NSH ButtonEvent::PressDown cannot prevent pointer focus like the DOM event.
            .on_press(move || {
                let state = action.state.get();
                if send_disabled(&state) || stop.get_untracked().phase == StopPhase::Landed { return; }
                if show_resume(&state) {
                    if let Some(resume) = &action.on_resume { resume(); }
                } else {
                    (action.on_submit_message)(dispatch_mode(&state));
                }
            })
            // `isolate` has no native meaning; index.css inset shadows expand here, selectors move to glyphs.
            .app_class(tw("relative flex h-9 w-9 items-center justify-center overflow-hidden rounded-full shadow-xs transition-all duration-150 enabled:cursor-pointer enabled:inset-shadow-[0_1px_rgb(255_255_255/16%)] hover:scale-105 active:inset-shadow-[0_1px_rgb(0_0_0/8%)] active:shadow-none disabled:pointer-events-none disabled:shadow-none disabled:hover:scale-100 sm:h-8 sm:w-8 bg-message-action text-message-action-foreground enabled:shadow-message-action/24 hover:bg-message-action-hover disabled:bg-foreground/12 disabled:text-muted-foreground"))
            .hit_slop(move || {
                let height = if layout_class().is_compact() { 36.0 } else { 32.0 };
                EdgeInsets::all(px(((metrics().hit_target - height) / 2.0).max(0.0)))
            })
            .child(dynamic(move || {
                let state = glyph.get();
                let is_disabled = send_disabled(&state);
                let color = move || if is_disabled {
                    palette().muted_foreground
                } else { palette().message_action_foreground };
                if !state.is_send_busy && !show_resume(&state) && !state.is_editing_queued_message && !state.is_running {
                    return svg("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='14' height='14' viewBox='0 0 14 14' fill='none'%3E%3Cpath d='M7 11.5V2.5M7 2.5L3 6.5M7 2.5L11 6.5' stroke='currentColor' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E")
                        .size(px(14.0)).text_color(color).accessible(false).pointer_events(PointerEvents::None).into_element();
                }
                let glyph = if state.is_send_busy { Icon::LoaderCircle }
                    else if show_resume(&state) { Icon::Play }
                    else if state.is_editing_queued_message { Icon::Check }
                    else if state.is_running && dispatch_mode(&state) == ComposerDispatchMode::Queue { Icon::ListPlus }
                    else { Icon::CornerUpRight };
                // A static loading glyph follows the shared no-continuous-animation rule.
                icon_with_class(glyph, if state.is_send_busy {
                    "size-3.5 pointer-events-none"
                } else if show_resume(&state) {
                    "size-4 fill-current pointer-events-none"
                } else {
                    "size-4 pointer-events-none"
                }).into_element()
            })))
}

fn compact_toggle(props: PrimaryActionsProps) -> impl IntoElement {
    let state = props.state.clone();
    let enabled = props.clone();
    let action = props.clone();
    let pressed = props.state.clone();
    let hint = tooltip_create_handle();
    let first_line = props.state.clone();
    let second_line = props.state.clone();
    view().app_class(tw("inline-flex"))
        .tooltip(tooltip(hint, TooltipOptions::default(), view()
            .child(text(move || {
                let state = first_line.get();
                let args = [("tokens", Value::from(state.compact_tokens.unwrap_or_default()))];
                if state.keep_full_history { tr("Next send keeps all {tokens} tokens.", &args) }
                else { tr("Next send compacts {tokens} tokens first.", &args) }
            }))
            .child(text(move || if second_line.get().keep_full_history {
                t("Click to compact first")
            } else { t("Click to keep full history") }))))
        .child(nsh_base::button()
        .accessibility_label({ let state = state.clone(); move || Some(tr(
            "Compact {tokens} tokens of history before sending",
            &[("tokens", Value::from(state.get().compact_tokens.unwrap_or_default()))],
        ).into()) })
        .disabled(move || !enabled.state.get().can_operate_thread || enabled.on_toggle_keep_full_history.is_none())
        .on_press(move || {
            if action.state.get().can_operate_thread
                && let Some(toggle) = &action.on_toggle_keep_full_history { toggle(); }
        })
        .app_class(move || Some(tw(&cn([
            Some("flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-(--control-radius) px-2 font-medium text-base text-foreground/80 tabular-nums outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-64 sm:text-sm"),
            (!pressed.get().keep_full_history).then_some("bg-accent/60 text-foreground"),
        ]))))
        .selected({ let state = state.clone(); move || !state.get().keep_full_history })
        .child(icon_with_class(Icon::Minimize2, "size-4 text-muted-foreground pointer-events-none"))
        // The source's named 480px container query becomes a binding to its measured surface.
        .child(dynamic({ let state = state.clone(); move || (props.composer_surface_width.get() > 480.0).then(||
            text(if state.get().keep_full_history { t("Keep") } else { t("Compact") })) }))
        .child(text(move || SharedString::from(state.get().compact_tokens.unwrap_or_default()))
            .app_class(tw("text-muted-foreground"))))
}

/// The collapsed composer's replacement for its ordinary send trigger.
pub fn collapsed_composer_stop_action(
    props: PrimaryActionsProps,
    children: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    let stop = use_stop_phase(props.state.clone());
    view()
        .app_class(tw("flex shrink-0"))
        .child(dynamic(move || {
            let state = props.state.get();
            if stop_visible(&state, stop.get().phase)
                && !state.has_sendable_content
                && !state.is_editing_queued_message
            {
                Some(untracked(|| {
                    stop_generation_button(props.clone(), stop, false).into_element()
                }))
            } else {
                Some(untracked(&children))
            }
        }))
}

/// U2's outbox binding. It subscribes before its first read and is disposed with the screen.
#[derive(Clone, Copy)]
pub struct PendingMessages {
    pub messages: Signal<Rc<Vec<PendingMessage>>>,
    pub restored_draft: Signal<Option<String>>,
    pub delivery_problem: Signal<Option<String>>,
}

impl PendingMessages {
    pub fn attach(client: Rc<dyn ComposerSendClient>, key: ThreadKey) -> Self {
        let mut changes = client.message_changes();
        let this = Self {
            messages: signal(Rc::new(client.pending_messages(&key))),
            restored_draft: signal(client.draft_text(&key)),
            delivery_problem: signal(client.delivery_problem(&key)),
        };
        spawn(async move {
            while changes.next().await.is_some() {
                this.messages.set(Rc::new(client.pending_messages(&key)));
                this.restored_draft.set(client.draft_text(&key));
                this.delivery_problem.set(client.delivery_problem(&key));
            }
        });
        this
    }
}

/// Object-safe adapter for the frozen Backend. Implementations delegate to client services.
pub trait ComposerSendClient {
    fn submit(
        &self,
        key: &ThreadKey,
        text: String,
        mode: ComposerDispatchMode,
        options: SendOptions,
    ) -> LocalBoxFuture<'static, Result<(), SendError>>;
    fn pending_messages(&self, key: &ThreadKey) -> Vec<PendingMessage>;
    fn message_changes(&self) -> LocalBoxStream<'static, ()>;
    fn draft_text(&self, key: &ThreadKey) -> Option<String>;
    fn delivery_problem(&self, key: &ThreadKey) -> Option<String>;
    fn retry_message(&self, key: &ThreadKey, id: &MessageId) -> Result<(), t3_client::ClientError>;
    fn dismiss_message(&self, key: &ThreadKey, id: &MessageId);
}

impl<P: Platform + Clone + 'static> ComposerSendClient for ClientRuntime<P> {
    fn submit(
        &self,
        key: &ThreadKey,
        text: String,
        mode: ComposerDispatchMode,
        options: SendOptions,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        send_message(self, key, text, mode, options)
    }
    fn pending_messages(&self, key: &ThreadKey) -> Vec<PendingMessage> {
        ClientRuntime::pending_messages(self, key)
    }
    fn message_changes(&self) -> LocalBoxStream<'static, ()> {
        ClientRuntime::message_changes(self)
    }
    fn draft_text(&self, key: &ThreadKey) -> Option<String> {
        ClientRuntime::draft_text(self, key)
    }
    fn delivery_problem(&self, key: &ThreadKey) -> Option<String> {
        ClientRuntime::delivery_problem(self, &key.environment)
    }
    fn retry_message(&self, key: &ThreadKey, id: &MessageId) -> Result<(), t3_client::ClientError> {
        self.messages(&key.environment)
            .ok_or(t3_client::ClientError::Disconnected)?
            .retry_message(id)
    }
    fn dismiss_message(&self, key: &ThreadKey, id: &MessageId) {
        ClientRuntime::dismiss_message(self, key, id);
    }
}

/// Port of apps/web/src/components/chat/useComposerSend.ts onSend's client submission boundary.
pub fn send_message<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    key: &ThreadKey,
    text: String,
    mode: ComposerDispatchMode,
    mut options: SendOptions,
) -> LocalBoxFuture<'static, Result<(), SendError>> {
    match get_composer_prompt_length_validation_message(&text) {
        Ok(Some(validation)) => {
            return Box::pin(async move {
                Err(SendError::Rejected(composer_submission_validation_message(
                    validation,
                )))
            });
        }
        Err(error) => return Box::pin(async move { Err(SendError::Rejected(error.to_string())) }),
        Ok(None) => {}
    }
    // The default text-only path is the composition root's own method.
    if mode == ComposerDispatchMode::Auto && options_is_text_only(&options) {
        return client.send_message(key, text);
    }
    options.dispatch_mode = match mode {
        ComposerDispatchMode::Auto => Some(DispatchMode::Auto),
        ComposerDispatchMode::Queue => Some(DispatchMode::Queue),
        ComposerDispatchMode::Steer => Some(DispatchMode::Steer),
        ComposerDispatchMode::Restart => Some(DispatchMode::Restart),
        ComposerDispatchMode::Start => None,
    };
    let service = client.messages(&key.environment);
    let key = key.clone();
    Box::pin(async move {
        let service = service.ok_or(SendError::Disconnected)?;
        service
            .submit(&key, text, options)
            .await
            .map(|_| ())
            .map_err(|error| {
                if matches!(error, t3_client::ClientError::Disconnected) {
                    SendError::Disconnected
                } else {
                    SendError::Rejected(error.to_string())
                }
            })
    })
}

pub fn composer_submission_validation_message(validation: PromptLengthValidation) -> String {
    tr(
        "Prompt is {excess, plural, one {# character} other {# characters}} over the {limit, number, integer}-character limit. Shorten or split it before sending.",
        &[
            ("excess", Value::from(validation.excess)),
            ("limit", Value::from(validation.limit)),
        ],
    )
}

fn options_is_text_only(options: &SendOptions) -> bool {
    options.attachments.is_empty()
        && options.context.is_none()
        && options.settings.is_none()
        && options.creation.is_none()
        && options.dispatch_mode.is_none()
        && !options.compact_before_send
        && !options.fresh_context
}

/// Port of apps/web/src/components/chat/QueuedRunsControl.tsx QueuedMessage, plus U2 feedback.
pub fn pending_message(
    client: Rc<dyn ComposerSendClient>,
    key: ThreadKey,
    pending: PendingMessage,
) -> impl IntoElement {
    let message = pending.message;
    let status = pending.state;
    let retrying = signal(false);
    let error = signal(None::<String>);
    let refused = matches!(status, SendState::Restored { .. });
    let retry = matches!(status, SendState::Retry { .. });
    let queued_turn = message.dispatch_mode == Some(DispatchMode::Queue);
    let copy_text = message.text.clone();
    let id = message.message_id.clone();
    let dismiss_client = client.clone();
    let dismiss_key = key.clone();
    let dismiss_id = id.clone();
    let reason = match &status {
        SendState::Retry { reason, .. } | SendState::Restored { reason, .. } => {
            Some(reason.clone())
        }
        _ => None,
    };
    view().app_class(tw("group flex flex-col items-end gap-1 pb-4"))
        .child(view().app_class(tw(&cn([
            Some(if queued_turn {
                "flex min-w-0 max-w-[80%] flex-col rounded-2xl border border-dashed text-sm border-muted-foreground/40 text-muted-foreground px-3 py-2"
            } else {
                "relative min-w-0 max-w-[80%] rounded-2xl text-message-foreground bg-message p-3"
            }),
        ])))
            // Native text preserves newlines and wraps; the TS whitespace/break hints are redundant.
            .child(text(SharedString::from(message.text.clone())).select_text()
                .app_class(tw("text-sm leading-relaxed"))))
        .child(view().app_class(tw("flex items-center gap-0.5 pe-1 text-xs text-muted-foreground"))
            .child(text(match status {
                SendState::Queued => t("Queued"),
                SendState::Preparing | SendState::Dispatching | SendState::Accepted { .. } => t("Submitting message"),
                SendState::Retry { .. } | SendState::Restored { .. } => t("Message not sent"),
                SendState::AwaitingConfirmation => t("Message may not have been sent"),
            }))
            .child(retry.then(|| button(ButtonContent::Label(t("Retry")), ButtonVariant::Ghost, ButtonSize::Xs,
                move || retrying.get(), move || {
                    if retrying.get_untracked() { return; }
                    retrying.set(true);
                    let result = client.retry_message(&key, &id);
                    error.set(result.err().map(|failure| failure.to_string()));
                    retrying.set(false);
                })))
            .child(refused.then(|| button(ButtonContent::Label(t("Copy message")), ButtonVariant::Ghost, ButtonSize::Xs,
                || false, move || set_clipboard(copy_text.clone()))))
            .child(refused.then(|| button(ButtonContent::Label(t("Dismiss")), ButtonVariant::Ghost, ButtonSize::Xs,
                || false, move || dismiss_client.dismiss_message(&dismiss_key, &dismiss_id)))))
        // U1's typed server detail is content, not an English catalogue key.
        .child(reason.map(|reason| text(SharedString::from(reason)).select_text()
            .app_class(tw("text-sm text-destructive"))))
        .child(dynamic(move || error.get().map(|error| text(SharedString::from(error))
            .app_class(tw("text-sm text-destructive")))))
}

/// Mount below the projected rows, inside their existing scroller; U2 already removes acknowledged ids.
pub fn pending_messages(
    client: Rc<dyn ComposerSendClient>,
    key: ThreadKey,
    pending: PendingMessages,
) -> impl IntoElement {
    view()
        .app_class(tw("flex flex-col"))
        .child(dynamic(move || {
            let messages = pending.messages.get();
            let client = client.clone();
            let key = key.clone();
            untracked(move || {
                messages.iter().fold(view(), |column, message| {
                    column.child(pending_message(
                        client.clone(),
                        key.clone(),
                        message.clone(),
                    ))
                })
            })
        }))
        .child(dynamic(move || {
            pending.delivery_problem.get().map(|reason| {
                text(SharedString::from(reason)).app_class(tw("text-sm text-destructive"))
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> PrimaryActionState {
        PrimaryActionState {
            can_operate_thread: true,
            is_running: true,
            can_interrupt: true,
            run_id: Some("first".into()),
            can_steer: true,
            ..Default::default()
        }
    }

    #[test]
    fn permission_loss_does_not_acknowledge_a_stop_and_a_cleared_id_does_not_reset_it() {
        let mut stop = StopState {
            run_id: Some("first".into()),
            phase: StopPhase::Pending,
        };
        stop.observe(&PrimaryActionState {
            can_operate_thread: false,
            can_interrupt: false,
            ..running()
        });
        assert_eq!(stop.phase, StopPhase::Pending);
        stop.observe(&PrimaryActionState {
            run_id: None,
            ..running()
        });
        assert_eq!(stop.phase, StopPhase::Pending);
        stop.observe(&PrimaryActionState {
            run_id: Some("second".into()),
            ..running()
        });
        assert_eq!(stop.phase, StopPhase::None);
    }

    #[test]
    fn late_stop_is_guarded_after_timeout_and_a_new_interruptible_turn_resets_it() {
        let mut stop = StopState {
            run_id: Some("first".into()),
            phase: StopPhase::TimedOut,
        };
        stop.observe(&PrimaryActionState {
            can_interrupt: false,
            ..running()
        });
        assert_eq!(stop.phase, StopPhase::Landed);
        stop.observe(&running());
        assert_eq!(stop.phase, StopPhase::None);
    }

    #[test]
    fn the_client_resolves_queue_steer_and_alternate_without_promising_unsupported_steering() {
        let mut state = PrimaryActionState {
            has_sendable_content: true,
            ..running()
        };
        assert_eq!(submit_label(&state), "Steer message");
        state.alternate_modifier = true;
        assert_eq!(dispatch_mode(&state), ComposerDispatchMode::Queue);
        assert_eq!(submit_label(&state), "Queue message");
        state.follow_up_behavior = Some(FollowUpBehavior::Queue);
        assert_eq!(dispatch_mode(&state), ComposerDispatchMode::Steer);
        state.can_steer = false;
        assert_eq!(dispatch_mode(&state), ComposerDispatchMode::Queue);
        state.is_running = false;
        assert_eq!(dispatch_mode(&state), ComposerDispatchMode::Auto);
    }

    #[test]
    fn pending_answers_have_their_own_busy_and_completion_guards() {
        let mut state = PrimaryActionState {
            can_operate_thread: true,
            is_send_busy: true,
            pending_action: Some(PendingActionState {
                question_index: 0,
                is_last_question: true,
                can_advance: true,
                is_responding: false,
                is_complete: true,
            }),
            ..Default::default()
        };
        assert!(!send_disabled(&state));
        state.pending_action.as_mut().expect("pending").is_complete = false;
        assert!(send_disabled(&state));
        state.voice_input_active = true;
        assert!(!send_disabled(&state));
        state.is_environment_unavailable = true;
        assert!(send_disabled(&state));
    }

    #[test]
    fn disconnected_denied_busy_empty_and_client_rejections_remain_inert() {
        let ready = PrimaryActionState {
            can_operate_thread: true,
            has_sendable_content: true,
            ..Default::default()
        };
        assert!(!send_disabled(&ready));
        for state in [
            PrimaryActionState {
                can_operate_thread: false,
                ..ready.clone()
            },
            PrimaryActionState {
                is_environment_unavailable: true,
                ..ready.clone()
            },
            PrimaryActionState {
                is_send_busy: true,
                ..ready.clone()
            },
            PrimaryActionState {
                has_sendable_content: false,
                ..ready.clone()
            },
            PrimaryActionState {
                send_disabled_reason: Some(Text::client_key("Messages loading")),
                ..ready
            },
        ] {
            assert!(send_disabled(&state));
        }
    }

    #[test]
    fn editing_and_resume_take_precedence_over_compaction_and_follow_up_actions() {
        let mut state = PrimaryActionState {
            compact_tokens: Some("12k".into()),
            has_sendable_content: true,
            ..running()
        };
        assert_eq!(submit_label(&state), "Compact and send");
        state.is_editing_queued_message = true;
        assert_eq!(submit_label(&state), "Update queued message");
        assert!(compact_tokens(&state).is_none());
        state.is_editing_queued_message = false;
        state.has_sendable_content = false;
        state.can_resume = true;
        assert_eq!(submit_label(&state), "Resume thread");
        assert!(compact_tokens(&state).is_none());
    }

    #[test]
    fn typed_prompt_limits_are_formatted_as_one_localisable_message() {
        for (excess, expected) in [(1, "1 character over"), (18, "18 characters over")] {
            let message = composer_submission_validation_message(PromptLengthValidation {
                excess,
                limit: 120_000,
            });
            assert!(message.contains(expected), "{message}");
            assert!(message.contains("120,000-character"), "{message}");
            assert!(
                !message.contains('{'),
                "ICU arguments must be formatted: {message}"
            );
        }
    }
}
