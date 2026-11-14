//! Port of apps/web/src/components/ui/command.tsx Command and its component slots.
//!
//! Create one [`CommandState`] per mounted command, with a reactive item catalogue and a
//! selection callback. [`command`] builds its children once. Compose [`command_input`],
//! [`command_panel`], [`command_list`] with [`command_collection`] (or
//! [`command_list_virtualized`]), and [`command_footer`] in that order. Row renderers receive
//! a memo, including when recycled. `active: Some(bool)` on [`command_item`] overrides only
//! appearance; keyboard selection still follows the highlight, as on the web.
//!
//! [`CommandOptions`] defaults to `autoHighlight="always"`, `keepHighlight=true`, inline/open.
//! Set `filter=false` for results already ranked by the client. Query, selected and disabled
//! are caller-controlled signals. Empty/loading/error/offline content belongs in the caller's
//! panel/list slots; `loading` marks either list as busy. Disabled, hidden and unknown items
//! never dispatch a selection. Disabled rows remain keyboard-navigable, as in Base UI.
//! `command_group(state, name, follows_group, || children)` scopes
//! its collection to that group and hides the entire group when it has no filtered results.
//! There are no Command `Variant` or `Size` enums in the TSX. Input is always `lg`; footer
//! actions are `ghost-muted` / `xs`. Elements are opaque and cannot be restyled.
//!
//! [`command_dialog`] retains its native modal through the source's 200 ms exit. Put
//! [`command_dialog_popup`] inside it. The modal traps/restores focus; input autofocus takes
//! priority over a preceding Back addon. Input's cancel callback owns closing or going back.
//! The backdrop dismisses the dialog itself; its callback only observes the outside press.
//! NSH has no decomposed autocomplete, so slots share UI state over native controls.
//! Item values must be unique within the catalogue; labels may repeat.

use std::rc::Rc;

use nsh_core::animation::{Easing, MotionChannel};
use nsh_core::{
    AccessibilityState,
    element::{IntoElements, Timing, desktop::title_bar},
    keymap::When,
    prelude::*,
};

use crate::i18n::Text;
use crate::state::layout::{Pointer, layout_class, pointer};
use crate::theme::{Scope, cn, metrics, palette, tw, tw_with};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;

use crate::runtime::tasks::watch;

/// Values are stable, untranslated keys. Labels are `i18n::Text`: a literal is a key, client
/// data (thread titles, project names, paths) is content. Keywords provide additional filter
/// text. `group` names the `command_group` the item belongs to (its label's source text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub value: SharedString,
    pub label: Text,
    pub group: Option<SharedString>,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
}

impl Item {
    pub fn new(value: impl Into<SharedString>, label: impl Into<Text>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            group: None,
            keywords: Vec::new(),
            disabled: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoHighlight {
    #[default]
    Always,
    OnInput,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandOptions {
    pub auto_highlight: AutoHighlight,
    pub keep_highlight: bool,
    pub loop_focus: bool,
    pub filter: bool,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            auto_highlight: AutoHighlight::Always,
            keep_highlight: true,
            loop_focus: true,
            filter: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Highlight {
    query: String,
    index: Option<usize>,
}

type SelectHandler = Rc<dyn Fn(&str)>;

/// UI-only state, constructed under the owner mounting the command.
#[derive(Clone, Copy)]
pub struct CommandState {
    pub query: Signal<String>,
    pub selected: Signal<Option<SharedString>>,
    pub disabled: Signal<bool>,
    pub loading: Signal<bool>,
    pub items: Memo<Vec<Item>>,
    pub highlighted: Memo<Option<SharedString>>,
    options: CommandOptions,
    requested: Signal<Option<Highlight>>,
    select: Stored<SelectHandler>,
    input: ElementRef,
    list: ElementRef,
    active_row: Signal<Option<(SharedString, ElementRef)>>,
    dialog_open: Option<Signal<bool>>,
    reveal_inset: Signal<f32>,
}

impl CommandState {
    pub fn new(
        items: impl Fn() -> Vec<Item> + 'static,
        options: CommandOptions,
        on_select: impl Fn(&str) + 'static,
    ) -> Self {
        let query = signal(String::new());
        let catalogue = computed(items);
        let index = computed(move || {
            catalogue.with(|items| {
                items
                    .iter()
                    .map(|item| (item.clone(), search_text(item)))
                    .collect::<Vec<_>>()
            })
        });
        let filtered = computed(move || {
            if !options.filter {
                return catalogue.get();
            }
            let query = query.get().trim().to_lowercase();
            index.with(|items| {
                items
                    .iter()
                    .filter(|(_, text)| matches_query(text, &query))
                    .map(|(item, _)| item.clone())
                    .collect::<Vec<_>>()
            })
        });
        let requested = signal(None::<Highlight>);
        let highlighted = computed(move || {
            let query = query.get();
            filtered.with(|items| {
                resolve_highlight(
                    items,
                    &query,
                    requested.get().as_ref(),
                    options.auto_highlight,
                )
            })
        });
        // Reconcile the native selection when async results arrive or its query changes.
        effect(move || {
            let query = query.get();
            let items = filtered.get();
            untracked(|| {
                let value = resolve_highlight(
                    &items,
                    &query,
                    requested.get().as_ref(),
                    options.auto_highlight,
                );
                let index = value.and_then(|value| items.iter().position(|i| i.value == value));
                requested.set(Some(Highlight { query, index }));
            });
        });
        Self {
            query,
            selected: signal(None),
            disabled: signal(false),
            loading: signal(false),
            items: filtered,
            highlighted,
            options,
            requested,
            select: stored(Rc::new(on_select)),
            input: element_ref(),
            list: element_ref(),
            active_row: signal(None),
            dialog_open: use_context::<DialogScope>().map(|dialog| dialog.open),
            reveal_inset: signal(0.0),
        }
    }

    /// Validate at activation, since a mounted row can become disabled or leave the results.
    pub fn choose(self, value: &str) {
        if untracked(|| self.is_disabled())
            || !self
                .items
                .with_untracked(|items| items.iter().any(|i| i.value == value && !i.disabled))
        {
            return;
        }
        // Autocomplete uses selectionMode=none: choosing does not set data-selected.
        self.select.get()(value);
    }

    pub fn accept(self) {
        if let Some(value) = self.highlighted.get_untracked() {
            self.choose(&value);
        }
    }

    pub fn highlight(self, value: Option<SharedString>) {
        if untracked(|| self.is_disabled())
            || value.as_ref().is_some_and(|value| {
                !self
                    .items
                    .with_untracked(|items| items.iter().any(|i| &i.value == value))
            })
        {
            return;
        }
        self.requested.set(Some(Highlight {
            query: self.query.get_untracked(),
            index: value.and_then(|value| {
                self.items
                    .with_untracked(|items| items.iter().position(|i| i.value == value))
            }),
        }));
    }

    /// Arrows wrap options; only autoHighlight=false permits a loop through the input.
    pub fn move_highlight(self, forward: bool) {
        let next = self.items.with_untracked(|items| {
            step(
                items,
                self.highlighted.get_untracked().as_deref(),
                forward,
                self.options.loop_focus,
                self.options.auto_highlight == AutoHighlight::Never,
            )
        });
        self.highlight(next);
    }

    pub fn focus(self) {
        self.input.focus();
    }

    fn is_disabled(self) -> bool {
        self.disabled.get() || self.dialog_open.is_some_and(|open| !open.get())
    }
}

fn search_text(item: &Item) -> Vec<String> {
    // Exact Intl collation needs a t3-i18n contains helper; client-ranked results use filter=false.
    std::iter::once(item.label.resolve().to_lowercase())
        .chain(item.keywords.iter().map(|k| k.to_lowercase()))
        .collect()
}

fn matches_query(text: &[String], query: &str) -> bool {
    text.iter().any(|text| text.contains(query))
}

fn resolve_highlight(
    items: &[Item],
    query: &str,
    requested: Option<&Highlight>,
    auto: AutoHighlight,
) -> Option<SharedString> {
    if let Some(requested) = requested.filter(|h| h.query == query) {
        match requested.index {
            None if auto != AutoHighlight::Always => return None,
            Some(index) if index < items.len() => {
                return Some(items[index].value.clone());
            }
            _ => {}
        }
    }
    (auto == AutoHighlight::Always || (auto == AutoHighlight::OnInput && !query.trim().is_empty()))
        .then(|| items.first().map(|i| i.value.clone()))
        .flatten()
}

fn step(
    items: &[Item],
    current: Option<&str>,
    forward: bool,
    looping: bool,
    allow_escape: bool,
) -> Option<SharedString> {
    // Base UI keeps disabled options navigable, so their reason can be read, but not chosen.
    let rows = items;
    let at = current.and_then(|v| rows.iter().position(|i| i.value == v));
    let next = match (at, forward) {
        (None, true) => rows.first(),
        (None, false) => rows.last(),
        (Some(at), true) => rows.get(at + 1).or_else(|| {
            if !looping {
                rows.last()
            } else if allow_escape {
                None
            } else {
                rows.first()
            }
        }),
        (Some(at), false) => at.checked_sub(1).and_then(|at| rows.get(at)).or_else(|| {
            if !looping {
                rows.first()
            } else if allow_escape {
                None
            } else {
                rows.last()
            }
        }),
    };
    next.map(|i| i.value.clone())
}

/// Autocomplete.Root provides state with no element of its own.
pub fn command<E: IntoElements>(
    state: CommandState,
    children: impl FnOnce(CommandState) -> E + 'static,
) -> impl IntoElement {
    let children = stored(Some(children));
    // A transparent one-time region permits sibling slots without an extra layout element.
    dynamic(move || {
        untracked(|| {
            children
                .with(|children| children.take())
                .map(|children| children(state).into_elements())
                .unwrap_or_default()
        })
    })
}

#[derive(Clone, Copy)]
struct DialogScope {
    open: Signal<bool>,
    initial_focus: Signal<Option<ElementRef>>,
    popup_motion: Motion,
    backdrop_motion: Motion,
}

fn dialog_transition(motion: Motion, open: bool, popup: bool) -> Animation {
    let timing = Timing::new(
        if reduce_motion() { 0.0 } else { 200.0 },
        Easing::Bezier(0.4, 0.0, 0.2, 1.0),
    );
    let mut tracks = vec![Animation::timing(
        &motion.value(MotionChannel::Opacity),
        if open { 1.0 } else { 0.0 },
        timing,
    )];
    if popup {
        tracks.push(Animation::timing(
            &motion.value(MotionChannel::Scale),
            if open { 1.0 } else { 0.98 },
            timing,
        ));
    }
    Animation::parallel(tracks)
}

/// NSH owns the portal and focus scope; exit completion unmounts command-owned work.
pub fn command_dialog<E: IntoElement>(
    open: Signal<bool>,
    label: impl Into<Text>,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    let mounted = signal(false);
    let generation = stored(0u64);
    let popup_motion = motion();
    let backdrop_motion = motion();
    effect(move || {
        let open_now = open.get();
        generation.with(|generation| *generation += 1);
        let current = generation.get();
        if open_now {
            if !mounted.get_untracked() {
                popup_motion.value(MotionChannel::Opacity).set(0.0);
                popup_motion.value(MotionChannel::Scale).set(0.98);
                backdrop_motion.value(MotionChannel::Opacity).set(0.0);
                mounted.set(true);
            }
            drop(dialog_transition(popup_motion, true, true).start());
            drop(dialog_transition(backdrop_motion, true, false).start());
        } else if mounted.get_untracked() {
            if reduce_motion() {
                mounted.set(false);
            } else {
                let ended = dialog_transition(popup_motion, false, true).start();
                drop(dialog_transition(backdrop_motion, false, false).start());
                watch(async move {
                    let _ = ended.await;
                    if generation.get() == current && !open.get_untracked() {
                        mounted.set(false);
                    }
                });
            }
        }
    });
    when(mounted, move || {
        // Untracked: signals read while constructing slots must not remount the modal.
        untracked(|| {
            let initial_focus = signal(None::<ElementRef>);
            provide_context(DialogScope {
                open,
                initial_focus,
                popup_motion,
                backdrop_motion,
            });
            let label = label.clone();
            nsh_base::dialog()
                .label(move || label.resolve())
                // Keep the native layer alive until both explicit CSS-equivalent tracks end.
                .animation(ModalAnimation::None)
                .close_on_escape(false)
                .close_on_backdrop_press(false)
                .close_on_swipe(false)
                .on_cancel(move || open.set(false))
                // Native modal presentation focuses its first control after Input's autofocus.
                .on_open(move || {
                    if let Some(input) = initial_focus.get_untracked() {
                        input.focus();
                    }
                })
                .w_full()
                .h_full()
                .child(children())
        })
    })
}

pub fn command_dialog_trigger(
    open: Signal<bool>,
    label: impl Into<Text>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    nsh_base::button()
        .accessibility_label(move || Some(label.resolve()))
        .on_press(move || open.set(true))
        .child(children)
}

pub fn command_dialog_backdrop(on_pointer_down: impl Fn() + 'static) -> impl IntoElement {
    // index.css `.dialog-backdrop`; explicit native tracks supply starting/ending opacity.
    let dialog = use_context::<DialogScope>();
    let backdrop_motion = dialog.map_or_else(motion, |dialog| dialog.backdrop_motion);
    view()
        .test_id("command-dialog-backdrop")
        .app_class(move || tw(&cn([
            Some("fixed inset-0 z-50 transition-all duration-200 bg-background/60 dark:bg-background/64 backdrop-blur-[4px]"),
            dialog.is_some_and(|dialog| !dialog.open.get()).then_some("pointer-events-none"),
        ])))
        .bind_motion(&backdrop_motion)
        .on_press_down(move || {
            if dialog.is_some_and(|dialog| !dialog.open.get_untracked()) {
                return;
            }
            on_pointer_down();
            if let Some(dialog) = dialog {
                // The observer may remove the dialog's owner entirely.
                let _ = dialog.open.try_set(false);
            }
        })
        .accessible(false)
}

pub fn command_dialog_viewport(children: impl IntoElement) -> impl IntoElement {
    // CSS max(--spacing(), vh, titlebar) resolves in points from the shared window insets.
    view()
        .app_class(tw(
            "pointer-events-none fixed inset-0 z-50 flex flex-col items-center px-4",
        ))
        .pt(move || {
            px(viewport_padding(
                window().height.0,
                title_bar().top.0,
                layout_class().is_compact(),
            )
            .0)
        })
        .pb(move || {
            px(viewport_padding(
                window().height.0,
                title_bar().top.0,
                layout_class().is_compact(),
            )
            .1)
        })
        .child(children)
}

fn viewport_padding(height: f32, titlebar: f32, compact: bool) -> (f32, f32) {
    if compact {
        (
            (height * 0.04).max(16.0).max(titlebar + 8.0),
            (height * 0.04).max(16.0),
        )
    } else {
        ((height * 0.1).max(titlebar + 8.0), height * 0.1)
    }
}

pub fn command_dialog_popup(
    children: impl IntoElement,
    on_backdrop_pointer_down: impl Fn() + 'static,
) -> impl IntoElement {
    // index.css `.dialog-glass`; NSH exposes no shared nested-dialog count.
    // Glass opacity/saturation use the stock values: theme only exports length/colour tokens.
    // Explicit motion tracks replace CSS start/end styles; app-region/will-change are omitted.
    let dialog = use_context::<DialogScope>();
    let popup_motion = dialog.map_or_else(motion, |dialog| dialog.popup_motion);
    view().w_full().h_full().child(command_dialog_backdrop(on_backdrop_pointer_down))
        .child(command_dialog_viewport(view()
            .app_class(tw("relative flex min-h-0 w-full min-w-0 flex-col outline-none rounded-2xl border border-contrast-foreground/10 dark:border-white/8 bg-background/80 backdrop-blur-[12px] dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_24px_64px_-24px_rgb(0_0_0/65%)] dark:inset-shadow-[0_1px_rgb(255_255_255/4%)] dark:shadow-[0_24px_72px_-20px_rgb(0_0_0/90%)] pointer-events-auto max-h-105 max-w-xl text-foreground"))
            .bind_motion(&popup_motion)
            .pointer_events(move || if dialog.is_some_and(|dialog| !dialog.open.get()) { PointerEvents::None } else { PointerEvents::Auto })
            .child(children)))
}

/// Input is always lg. A start addon replaces Search (for example, a submenu Back button).
pub struct InputOptions {
    pub label: Text,
    pub placeholder: Text,
    pub auto_focus: bool,
    pub start_addon: Option<AnyElement>,
    /// None lets Escape close the enclosing CommandDialog; Some can navigate a submenu back.
    pub on_cancel: Option<Rc<dyn Fn()>>,
    /// Override Enter when the palette owns its active row; choose through CommandState for guards.
    pub on_submit: Option<Rc<dyn Fn()>>,
}

impl InputOptions {
    pub fn new(label: impl Into<Text>, placeholder: impl Into<Text>) -> Self {
        Self {
            label: label.into(),
            placeholder: placeholder.into(),
            auto_focus: true,
            start_addon: None,
            on_cancel: None,
            on_submit: None,
        }
    }
}

pub fn command_input(state: CommandState, options: InputOptions) -> impl IntoElement {
    let focused = signal(false);
    let dialog = use_context::<DialogScope>();
    if let Some(dialog) = dialog
        && dialog.initial_focus.get_untracked().is_none()
    {
        dialog.initial_focus.set(Some(state.input));
    }
    let cancel = options
        .on_cancel
        .or_else(|| dialog.map(|dialog| Rc::new(move || dialog.open.set(false)) as Rc<dyn Fn()>));
    // NSH keys follow the focused input; their registration is disposed with its owner.
    effect(move || {
        if !focused.get() || state.is_disabled() {
            return;
        }
        for (key, forward) in [("down", true), ("up", false)] {
            on_key_with(
                key,
                KeyOptions {
                    when: When::Always,
                    repeat: true,
                    for_focus: true,
                    ..Default::default()
                },
                move |_| state.move_highlight(forward),
            );
        }
        if let Some(cancel) = cancel.clone() {
            on_key_with(
                "escape",
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| cancel(),
            );
        }
    });
    let label = options.label;
    let placeholder = options.placeholder;
    let submit = options.on_submit;
    let addon = options.start_addon.unwrap_or_else(|| {
        // SVG selectors are applied to their target, with palette tint and responsive size.
        view()
            .app_class(tw("translate-x-0.5 -mx-0.5 size-4.5 sm:size-4"))
            .child(
                image(Icon::Search.asset())
                    .w(move || px(metrics().icon))
                    .h(move || px(metrics().icon))
                    .tint_color(move || palette().icon_muted)
                    .accessible(false),
            )
            .into_element()
    });
    view().app_class(tw("px-[var(--command-shell-inset)] py-1.5"))
        .child(
            // AutocompleteInput > Input control > native input, selectors on their targets.
            // CommandInput removes Input's before layer, border, shadow and focus ring.
            view().app_class(move || tw(&cn([
                Some("relative w-full text-foreground"),
                state.disabled.get().then_some("opacity-64"),
            ])))
                // The web hides the entire addon subtree, including custom Back controls.
                // NSH cannot evaluate calc(var(...)); preserve logical start padding in points.
                .child(view().accessibility_elements_hidden(true).app_class(move || {
                    let padding = format!("ps-[{}px]", tokens().number("command-shell-inset").unwrap_or(8.0) + 1.0);
                    tw(&cn([Some("absolute inset-y-0 start-px z-10 flex items-center opacity-80"), Some(&padding)]))
                }).child(addon))
                .child(view().app_class(tw("relative inline-flex w-full rounded-lg border border-transparent bg-transparent text-base text-foreground sm:text-sm shadow-none"))
                    .child(nsh_base::input().element_ref(&state.input)
                        .role(AccessibilityRole::Combobox)
                        .accessibility_state(move || AccessibilityState {
                            expanded: Some(true), has_popup: true,
                            disabled: state.is_disabled(), ..Default::default()
                        })
                        .accessibility_controls(Some(state.list))
                        .accessibility_active_descendant(move || state.active_row.get()
                            .filter(|(value, _)| state.highlighted.get().as_ref() == Some(value))
                            .map(|(_, row)| row))
                        .value(move || state.query.get())
                        .on_change(move |value: &str| { if !untracked(|| state.is_disabled()) { state.query.set(value.to_owned()); } })
                        .disabled(move || state.is_disabled())
                        .on_focus(move || focused.set(true)).on_blur(move || focused.set(false))
                        .auto_focus(options.auto_focus).on_submit(move |_: &str| {
                            if untracked(|| state.is_disabled()) { return; }
                            if let Some(submit) = &submit { submit(); } else { state.accept(); }
                        })
                        .submit_behavior(SubmitBehavior::Submit)
                        .label(move || label.resolve())
                        .placeholder(move || placeholder.resolve().to_string())
                        // The inherited radius is the input control's rounded-lg.
                        .app_class(move || {
                            let inset = if layout_class().is_compact() { 36.0 } else { tokens().number("command-shell-inset").unwrap_or(8.0) + 24.0 };
                            let padding = format!("ps-[{inset}px]");
                            tw(&cn([Some("h-8.5 w-full min-w-0 rounded-lg px-[11px] leading-8.5 outline-none placeholder:text-placeholder sm:h-7.5 sm:leading-7.5 h-9.5 leading-9.5 sm:h-8.5 sm:leading-8.5 text-base text-foreground sm:text-sm bg-transparent"), Some(&padding)]))
                        }))))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelLayout {
    #[default]
    Default,
    List,
    TallList,
    Fill,
}

pub fn command_panel(
    has_footer: impl IntoReactive<bool>,
    children: impl IntoElement,
) -> impl IntoElement {
    command_panel_with_layout(has_footer, PanelLayout::default(), children)
}

pub fn command_panel_with_layout(
    has_footer: impl IntoReactive<bool>,
    layout: PanelLayout,
    children: impl IntoElement,
) -> impl IntoElement {
    let has_footer = has_footer.into_reactive();
    // The footer sibling condition replaces not-has; touch-action and scrollbar selectors
    // are native scroll behavior, without a CSS scrollbar element to target.
    let panel = view().app_class(move || {
        tw(&cn([
            Some("relative min-h-0 overflow-hidden rounded-t-xl bg-transparent"),
            (!has_footer.get()).then_some("rounded-b-2xl"),
            Some(match layout {
                PanelLayout::Default | PanelLayout::TallList => "",
                PanelLayout::List => "flex min-h-0 flex-col",
                PanelLayout::Fill => "flex min-h-0 flex-1 flex-col",
            }),
        ]))
    });
    // NSH cannot parse min(rem, vh), and its views do not shrink like CSS flex items by default.
    let panel = match layout {
        PanelLayout::List => panel.flex_shrink(1.0).max_h(move || {
            px((28.0 * nsh_core::geometry::REM_SIZE.0).min(window().height.0 * 0.70))
        }),
        PanelLayout::TallList => panel.flex_shrink(1.0).max_h(move || {
            px((34.0 * nsh_core::geometry::REM_SIZE.0).min(window().height.0 * 0.76))
        }),
        // NSH's flex-1 uses a zero-point basis; the browser resolves this shorthand to zero percent.
        PanelLayout::Fill => panel.basis(Percentage(0.0)),
        PanelLayout::Default => panel,
    };
    panel.child(children)
}

pub fn command_list(
    state: CommandState,
    label: impl Into<Text>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    state.reveal_inset.set(8.0);
    // ScrollArea's size-full constrains native scrolling; not-empty follows the catalogue.
    scroll()
        .element_ref(&state.list)
        .role(AccessibilityRole::Listbox)
        .accessibility_state(move || AccessibilityState {
            busy: state.loading.get(),
            ..Default::default()
        })
        .label(move || label.resolve())
        .app_class(move || {
            tw(&cn([
                Some("relative size-full min-h-0 outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"),
                (!state.items.with(|i| i.is_empty())).then_some("p-2"),
            ]))
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            if !state.options.keep_highlight {
                state.highlight(None);
            }
        })
        .child(children)
}

pub fn command_list_virtualized<E: IntoElement>(
    state: CommandState,
    label: impl Into<Text>,
    render: impl Fn(Memo<Option<Item>>) -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    state.reveal_inset.set(0.0);
    let list = state.list;
    // NSH's virtual list owns scrolling; no enclosing scroll view is needed.
    effect(move || {
        let highlight = state.highlighted.get();
        let index = state.items.with(|items| {
            items
                .iter()
                .position(|i| Some(&i.value) == highlight.as_ref())
        });
        if let Some(index) = index {
            list.scroll_to_index(index as u32, Percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || state.items.with(|items| items.len()),
        move |at| {
            let item = computed(move || state.items.with(|items| items.get(at.index()).cloned()));
            render(item)
        },
    )
    .keys(move || {
        state
            .items
            .with(|items| items.iter().map(|i| i.value.clone()).collect::<Vec<_>>())
    })
    .element_ref(&list)
    .role(AccessibilityRole::Listbox)
    .accessibility_state(move || AccessibilityState {
        busy: state.loading.get(),
        ..Default::default()
    })
    .on_mouse_leave(move |_: &MouseEvent| {
        if !state.options.keep_highlight {
            state.highlight(None);
        }
    })
    .label(move || label.resolve())
    .estimated_item_size(move || {
        px(if layout_class().is_compact() {
            36.0
        } else {
            32.0
        })
    })
    // NSH has no max-h:inherit; the native list fills its constrained parent instead.
    .flex_1()
    .app_class(tw("size-full min-w-0 flex min-h-0 flex-col"))
}

pub fn command_collection<E: IntoElement>(
    state: CommandState,
    render: impl Fn(Memo<Option<Item>>) -> E + 'static,
) -> impl IntoElement {
    let group = use_context::<GroupScope>();
    each(
        move || {
            state.items.with(|items| {
                items
                    .iter()
                    .filter(|i| {
                        group
                            .as_ref()
                            .is_none_or(|group| i.group.as_ref() == Some(&group.0))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
        },
        |item| item.value.clone(),
        move |item, _| {
            let item = computed(move || Some(item.get()));
            render(item)
        },
    )
}

pub fn command_list_heading(label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    // NSH lacks role=presentation; this label is plain, non-interactive text.
    text(move || label.resolve())
        .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

#[derive(Clone)]
struct GroupScope(SharedString);

/// A group's identity: its label's source text, whether key or content.
fn text_source(label: &Text) -> SharedString {
    match label {
        Text::Key(source) | Text::Content(source) => source.clone(),
    }
}

pub fn command_group<E: IntoElement>(
    state: CommandState,
    label: impl Into<Text>,
    follows_group: impl IntoReactive<bool>,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    let follows_group = follows_group.into_reactive();
    let value = text_source(&label);
    // AutocompleteGroup's [[role=group]+&] becomes the caller's sibling condition.
    when(
        move || {
            state
                .items
                .with(|items| items.iter().any(|i| i.group.as_ref() == Some(&value)))
        },
        move || {
            untracked(|| {
                provide_context(GroupScope(text_source(&label)));
                let label = label.clone();
                let follows_group = follows_group.clone();
                view()
                    .role(AccessibilityRole::Group)
                    .label(move || label.resolve())
                    .app_class(move || tw(&cn([follows_group.get().then_some("mt-1.5")])))
                    .child(children())
            })
        },
    )
}

pub fn command_group_label(label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    text(move || label.resolve())
        .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

pub fn command_item(
    state: CommandState,
    item: Memo<Option<Item>>,
    active: impl Fn() -> Option<bool> + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let row = element_ref();
    let reveal_area = element_ref();
    let disabled =
        move || state.is_disabled() || item.with(|i| i.as_ref().is_none_or(|i| i.disabled));
    let selected = move || {
        item.with(|i| {
            i.as_ref()
                .is_some_and(|i| state.selected.get().as_ref() == Some(&i.value))
        })
    };
    let highlighted = computed(move || {
        item.with(|i| {
            i.as_ref()
                .is_some_and(|i| state.highlighted.get().as_ref() == Some(&i.value))
        })
    });
    effect(move || {
        if highlighted.get() {
            if let Some(item) = item.get_untracked() {
                state.active_row.set(Some((item.value, row)));
            }
            cleanup(move || {
                if state
                    .active_row
                    .get_untracked()
                    .is_some_and(|(_, active)| active == row)
                {
                    state.active_row.set(None);
                }
            });
            // NSH has no scroll-padding: reveal an unpainted row rectangle extended by scroll-py-2.
            reveal_area.reveal(
                RevealOptions::default()
                    .vertical(ScrollAlignment::Nearest)
                    .animated(false),
            );
        }
    });
    // Autocomplete.Item is a role=option div, outside the Tab order; input retains key focus.
    view()
        .relative()
        .on_mouse_move(move |_: &MouseEvent| {
            if let Some(item) = item.get_untracked().filter(|i| !i.disabled) {
                state.highlight(Some(item.value));
            }
        })
        .element_ref(&row)
        .role(AccessibilityRole::Option)
        .focusable(false)
        .optional_label(move || item.get().map(|i| i.label.resolve()))
        .disabled(disabled)
        .accessibility_state(move || AccessibilityState {
            selected: selected(),
            disabled: disabled(),
            ..Default::default()
        })
        .on_press(move || {
            if let Some(item) = item.get_untracked() {
                state.choose(&item.value);
            }
        })
        .hit_slop(move || {
            EdgeInsets::all(px(if pointer() == Pointer::Coarse {
                if layout_class().is_compact() {
                    4.0
                } else {
                    6.0
                }
            } else {
                0.0
            }))
        })
        .app_class(move || item_classes(selected(), highlighted.get(), disabled(), active()))
        .child(
            view()
                .element_ref(&reveal_area)
                .accessible(false)
                .app_class(tw("absolute inset-x-0 pointer-events-none"))
                .top(move || px(-state.reveal_inset.get()))
                .bottom(move || px(-state.reveal_inset.get())),
        )
        .child(children)
}

fn item_classes(
    selected: bool,
    highlighted: bool,
    disabled: bool,
    active: Option<bool>,
) -> SharedString {
    let attrs: Vec<_> = [
        ("data-selected", selected),
        ("data-highlighted", highlighted),
        ("data-disabled", disabled),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some((name, "")))
    .collect();
    // Combined [highlighted][selected] selectors repeat the highlighted declarations.
    // Icon descendants take size/tint at their call; active's important classes go last.
    tw_with(Scope::Root, &cn([
        Some("flex min-h-8 cursor-default select-none items-center gap-2 rounded-sm px-2 py-1 text-base outline-none hover:bg-accent data-disabled:pointer-events-none data-selected:bg-accent/50 data-selected:text-foreground data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:opacity-64 sm:min-h-7 sm:text-sm"),
        Some("py-1.5 data-selected:bg-foreground/[0.06] data-highlighted:bg-foreground/[0.09] data-highlighted:text-foreground"),
        active.is_some().then_some("cursor-pointer hover:bg-transparent hover:text-inherit data-highlighted:bg-transparent data-highlighted:text-inherit data-selected:bg-transparent data-selected:text-inherit"),
        (active == Some(true)).then_some("bg-accent! text-accent-foreground!"),
    ]), &attrs)
}

pub fn command_shortcut(label: impl Into<Text>) -> impl IntoElement {
    let label = label.into();
    text(move || label.resolve()).app_class(tw(
        "ms-auto font-medium font-sans text-secondary-label text-xs tracking-widest",
    ))
}

pub fn command_footer(children: impl IntoElement) -> impl IntoElement {
    // Descendant kbd selectors go on caller-owned keycaps; NSH has no kbd primitive yet.
    // calc(var(--radius-2xl)-1px) is computed from the exported length token.
    view().app_class(tw("relative flex items-center justify-between gap-2 border-t border-foreground/[0.07] bg-foreground/[0.025] px-[var(--command-content-inset)] py-2.5 font-medium text-sm text-muted-foreground"))
        .rounded_b(move || px((tokens().number("radius-2xl").unwrap_or(16.0) - 1.0).max(0.0)))
        .child(children)
}

pub fn command_footer_action(
    label: impl Into<Text>,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    // Frozen Button API lacks the TSX's h-auto/px-2/transparent-hover override.
    let dialog = use_context::<DialogScope>();
    button(
        ButtonContent::Label(label.into().resolve().to_string()),
        ButtonVariant::GhostMuted,
        ButtonSize::Xs,
        move || dialog.is_some_and(|dialog| !dialog.open.get()) || disabled(),
        on_press,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        let mut disabled = Item::new("disabled", "Unavailable");
        disabled.disabled = true;
        vec![
            disabled,
            Item::new("a", "Open settings"),
            Item::new("b", "Open project"),
        ]
    }

    #[test]
    fn filtering_trims_and_matches_labels_and_keywords_without_changing_values() {
        let mut item = Item::new("stable-key", "Open settings");
        item.keywords.push("Preferences".into());
        for query in ["", "open", "SETTINGS", "sett", "preferences", " settings "] {
            assert!(
                matches_query(&search_text(&item), &query.trim().to_lowercase()),
                "{query}"
            );
        }
        for query in ["stable-key", "missing"] {
            assert!(
                !matches_query(&search_text(&item), &query.to_lowercase()),
                "{query}"
            );
        }
    }

    #[test]
    fn auto_highlight_always_handles_disabled_and_empty_results() {
        assert_eq!(
            resolve_highlight(&items(), "", None, AutoHighlight::Always).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            resolve_highlight(&[], "", None, AutoHighlight::Always),
            None
        );
        assert_eq!(
            resolve_highlight(&items()[..1], "", None, AutoHighlight::Always).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            resolve_highlight(&items(), "", None, AutoHighlight::OnInput),
            None
        );
        assert_eq!(
            resolve_highlight(&items()[1..], "open", None, AutoHighlight::OnInput).as_deref(),
            Some("a")
        );
        assert_eq!(
            resolve_highlight(&items(), "open", None, AutoHighlight::Never),
            None
        );
    }

    #[test]
    fn highlight_tracks_the_position_when_results_reorder_even_if_the_option_is_disabled() {
        let requested = Highlight {
            query: "open".into(),
            index: Some(2),
        };
        let mut rows = items();
        assert_eq!(
            resolve_highlight(&rows, "open", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("b")
        );
        rows.reverse();
        assert_eq!(
            resolve_highlight(&rows, "open", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            resolve_highlight(&rows[1..], "open", Some(&requested), AutoHighlight::Always)
                .as_deref(),
            Some("a")
        );
    }

    #[test]
    fn query_changes_reset_highlight_and_auto_highlight_controls_clearing() {
        let requested = Highlight {
            query: "old".into(),
            index: Some(2),
        };
        assert_eq!(
            resolve_highlight(&items(), "new", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("disabled")
        );
        let input = Highlight {
            query: "new".into(),
            index: None,
        };
        assert_eq!(
            resolve_highlight(&items(), "new", Some(&input), AutoHighlight::Never),
            None
        );
        assert_eq!(
            resolve_highlight(&items(), "new", Some(&input), AutoHighlight::Always).as_deref(),
            Some("disabled")
        );
    }

    #[test]
    fn arrows_include_disabled_options_and_escape_only_without_auto_highlight() {
        let rows = items();
        for (from, forward, expected) in [
            (None, true, Some("disabled")),
            (None, false, Some("b")),
            (Some("disabled"), true, Some("a")),
            (Some("a"), true, Some("b")),
            (Some("b"), false, Some("a")),
            (Some("disabled"), false, None),
            (Some("b"), true, None),
            (Some("missing"), true, Some("disabled")),
        ] {
            assert_eq!(step(&rows, from, forward, true, true).as_deref(), expected);
        }
        assert_eq!(
            step(&rows, Some("disabled"), false, true, false).as_deref(),
            Some("b")
        );
        assert_eq!(
            step(&rows, Some("b"), true, true, false).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            step(&rows, Some("disabled"), false, false, false).as_deref(),
            Some("disabled")
        );
        assert_eq!(
            step(&rows, Some("b"), true, false, false).as_deref(),
            Some("b")
        );
        assert_eq!(step(&[], None, true, true, false), None);
        assert_eq!(
            step(&rows[..1], None, false, true, false).as_deref(),
            Some("disabled")
        );
    }

    #[test]
    fn viewport_keeps_search_clear_of_titlebar_even_in_short_windows() {
        assert_eq!(viewport_padding(800.0, 0.0, false), (80.0, 80.0));
        assert_eq!(viewport_padding(844.0, 0.0, true), (33.76, 33.76));
        assert_eq!(viewport_padding(200.0, 40.0, false), (48.0, 20.0));
        assert_eq!(viewport_padding(200.0, 40.0, true), (48.0, 16.0));
    }
}
