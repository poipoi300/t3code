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
//! never dispatch a selection. `command_group(state, name, follows_group, || children)` scopes
//! its collection to that group and hides the entire group when it has no filtered results.
//! There are no Command `Variant` or `Size` enums in the TSX. Input is always `lg`; footer
//! actions are `ghost-muted` / `xs`. Elements are opaque and cannot be restyled.
//!
//! [`command_dialog`] mounts a native modal while its open signal is true. Put
//! [`command_dialog_popup`] inside it. The modal traps/restores focus; input autofocus takes
//! priority over a preceding Back addon. Input's cancel callback owns closing or going back.
//! NSH has no decomposed autocomplete, so slots share UI state over native controls.

use std::rc::Rc;

use nsh_core::{AccessibilityState, element::desktop::title_bar, keymap::When, prelude::*};

use crate::i18n::t;
use crate::state::layout::{Pointer, layout_class, pointer};
use crate::theme::{Scope, cn, metrics, palette, tw, tw_with};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;

/// Values are stable, untranslated keys. Labels/group names are English translation keys;
/// user-authored names pass through unchanged. Keywords provide additional filter text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub value: SharedString,
    pub label: SharedString,
    pub group: Option<SharedString>,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
}

impl Item {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
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
    value: Option<SharedString>,
}

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
    select: Stored<Rc<dyn Fn(&str)>>,
    input: ElementRef,
    list: ElementRef,
    active_row: Signal<Option<(SharedString, ElementRef)>>,
}

impl CommandState {
    pub fn new(
        items: impl Fn() -> Vec<Item> + 'static,
        options: CommandOptions,
        on_select: impl Fn(&str) + 'static,
    ) -> Self {
        let query = signal(String::new());
        let catalogue = computed(items);
        let filtered = computed(move || {
            let query = query.get();
            catalogue.with(|items| {
                items
                    .iter()
                    .filter(|item| !options.filter || matches_query(item, &query))
                    .cloned()
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
                requested.set(Some(Highlight { query, value }));
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
        }
    }

    /// Validate at activation, since a mounted row can become disabled or leave the results.
    pub fn choose(self, value: &str) {
        if self.disabled.get_untracked()
            || !self
                .items
                .with_untracked(|items| items.iter().any(|i| i.value == value && !i.disabled))
        {
            return;
        }
        self.selected.set(Some(value.to_owned().into()));
        self.select.with(|select| select(value));
    }

    pub fn accept(self) {
        if let Some(value) = self.highlighted.get_untracked() {
            self.choose(&value);
        }
    }

    pub fn highlight(self, value: Option<SharedString>) {
        if self.disabled.get_untracked()
            || value.as_ref().is_some_and(|value| {
                !self
                    .items
                    .with_untracked(|items| items.iter().any(|i| &i.value == value && !i.disabled))
            })
        {
            return;
        }
        self.requested.set(Some(Highlight {
            query: self.query.get_untracked(),
            value,
        }));
    }

    /// With loopFocus, arrows include the input (no highlighted row) past either end.
    pub fn move_highlight(self, forward: bool) {
        let next = self.items.with_untracked(|items| {
            step(
                items,
                self.highlighted.get_untracked().as_deref(),
                forward,
                self.options.loop_focus,
            )
        });
        self.highlight(next);
    }

    pub fn focus(self) {
        self.input.focus();
    }
}

fn matches_query(item: &Item, query: &str) -> bool {
    let query = query.to_lowercase();
    t(&item.label).to_lowercase().contains(&query)
        || item
            .keywords
            .iter()
            .any(|k| k.to_lowercase().contains(&query))
}

fn resolve_highlight(
    items: &[Item],
    query: &str,
    requested: Option<&Highlight>,
    auto: AutoHighlight,
) -> Option<SharedString> {
    if let Some(requested) = requested.filter(|h| h.query == query) {
        match &requested.value {
            None => return None,
            Some(value) if items.iter().any(|i| &i.value == value && !i.disabled) => {
                return Some(value.clone());
            }
            _ => {}
        }
    }
    (auto == AutoHighlight::Always || (auto == AutoHighlight::OnInput && !query.is_empty()))
        .then(|| items.iter().find(|i| !i.disabled).map(|i| i.value.clone()))
        .flatten()
}

fn step(
    items: &[Item],
    current: Option<&str>,
    forward: bool,
    looping: bool,
) -> Option<SharedString> {
    let enabled: Vec<_> = items.iter().filter(|i| !i.disabled).collect();
    let at = current.and_then(|v| enabled.iter().position(|i| i.value == v));
    let next = match (at, forward) {
        (None, true) => enabled.first(),
        (None, false) => enabled.last(),
        (Some(at), true) => enabled
            .get(at + 1)
            .or_else(|| (!looping).then(|| enabled.last()).flatten()),
        (Some(at), false) => at
            .checked_sub(1)
            .and_then(|at| enabled.get(at))
            .or_else(|| (!looping).then(|| enabled.first()).flatten()),
    };
    next.map(|i| i.value.clone())
}

/// Autocomplete.Root provides state with no element of its own.
pub fn command<E: IntoElement>(
    state: CommandState,
    children: impl FnOnce(CommandState) -> E,
) -> impl IntoElement {
    children(state)
}

#[derive(Clone, Copy)]
struct DialogScope(Signal<bool>);

/// NSH owns the portal and focus scope; closing unmounts all command-owned work.
pub fn command_dialog<E: IntoElement>(
    open: Signal<bool>,
    label: impl Into<String>,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    when(
        move || open.get(),
        move || {
            // Untracked: signals read while constructing slots must not remount the modal.
            untracked(|| {
                provide_context(DialogScope(open));
                let label = label.clone();
                nsh_base::dialog()
                    .label(move || SharedString::from(t(&label)))
                    .animation(move || {
                        if reduce_motion() {
                            ModalAnimation::None
                        } else {
                            ModalAnimation::Fade
                        }
                    })
                    .on_cancel(move || open.set(false))
                    .w_full()
                    .h_full()
                    .child(children())
            })
        },
    )
}

pub fn command_dialog_trigger(
    open: Signal<bool>,
    label: impl Into<String>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    nsh_base::button()
        .accessibility_label(move || Some(SharedString::from(t(&label))))
        .on_press(move || open.set(true))
        .child(children)
}

pub fn command_dialog_backdrop(on_pointer_down: impl Fn() + 'static) -> impl IntoElement {
    // index.css `.dialog-backdrop`; unmounting needs no ending-style pointer guard.
    view()
        .app_class(tw(
            "fixed inset-0 z-50 bg-background/60 dark:bg-background/64 backdrop-blur-[4px]",
        ))
        .on_press_down(on_pointer_down)
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
    // Native modal fade replaces CSS start/end transitions; app-region/will-change are omitted.
    view().w_full().h_full().child(command_dialog_backdrop(on_backdrop_pointer_down))
        .child(command_dialog_viewport(view()
            .app_class(tw("relative flex min-h-0 w-full min-w-0 flex-col outline-none rounded-2xl border border-contrast-foreground/10 dark:border-white/8 bg-background/80 backdrop-blur-[12px] dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_24px_64px_-24px_rgb(0_0_0/65%)] dark:inset-shadow-[0_1px_rgb(255_255_255/4%)] dark:shadow-[0_24px_72px_-20px_rgb(0_0_0/90%)] pointer-events-auto max-h-105 max-w-xl text-foreground"))
            .child(children)))
}

/// Input is always lg. A start addon replaces Search (for example, a submenu Back button).
pub struct InputOptions {
    pub label: String,
    pub placeholder: String,
    pub auto_focus: bool,
    pub start_addon: Option<AnyElement>,
    /// None lets Escape close the enclosing CommandDialog; Some can navigate a submenu back.
    pub on_cancel: Option<Rc<dyn Fn()>>,
}

impl InputOptions {
    pub fn new(label: impl Into<String>, placeholder: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            placeholder: placeholder.into(),
            auto_focus: true,
            start_addon: None,
            on_cancel: None,
        }
    }
}

pub fn command_input(state: CommandState, options: InputOptions) -> impl IntoElement {
    let focused = signal(false);
    let dialog = use_context::<DialogScope>();
    let cancel = options
        .on_cancel
        .or_else(|| dialog.map(|dialog| Rc::new(move || dialog.0.set(false)) as Rc<dyn Fn()>));
    // NSH keys follow the focused input; their registration is disposed with its owner.
    effect(move || {
        if !focused.get() || state.disabled.get() {
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
                // NSH cannot evaluate calc(var(...)); preserve logical start padding in points.
                .child(view().app_class(move || {
                    let padding = format!("ps-[{}px]", tokens().number("command-shell-inset").unwrap_or(8.0) + 1.0);
                    tw(&cn([Some("absolute inset-y-0 start-px z-10 flex items-center opacity-80"), Some(&padding)]))
                }).child(addon))
                .child(view().app_class(tw("relative inline-flex w-full rounded-lg border border-transparent bg-transparent text-base text-foreground sm:text-sm shadow-none"))
                    .child(nsh_base::input().element_ref(&state.input)
                        .accessibility_controls(Some(state.list))
                        .accessibility_active_descendant(move || state.active_row.get()
                            .filter(|(value, _)| state.highlighted.get().as_ref() == Some(value))
                            .map(|(_, row)| row))
                        .value(move || state.query.get())
                        .on_change(move |value: &str| { if !state.disabled.get_untracked() { state.query.set(value.to_owned()); } })
                        .disabled(move || state.disabled.get())
                        .on_focus(move || focused.set(true)).on_blur(move || focused.set(false))
                        .auto_focus(options.auto_focus).on_submit(move |_: &str| state.accept())
                        .submit_behavior(SubmitBehavior::Submit)
                        .label(move || SharedString::from(t(&label)))
                        .placeholder(move || t(&placeholder))
                        // The inherited radius is the input control's rounded-lg.
                        .app_class(move || {
                            let inset = if layout_class().is_compact() { 36.0 } else { tokens().number("command-shell-inset").unwrap_or(8.0) + 24.0 };
                            let padding = format!("ps-[{inset}px]");
                            tw(&cn([Some("h-8.5 w-full min-w-0 rounded-lg px-[11px] leading-8.5 outline-none placeholder:text-placeholder sm:h-7.5 sm:leading-7.5 h-9.5 leading-9.5 sm:h-8.5 sm:leading-8.5 text-base text-foreground sm:text-sm bg-transparent"), Some(&padding)]))
                        }))))
}

pub fn command_panel(
    has_footer: impl IntoReactive<bool>,
    children: impl IntoElement,
) -> impl IntoElement {
    let has_footer = has_footer.into_reactive();
    // The footer sibling condition replaces not-has; touch-action and scrollbar selectors
    // are native scroll behavior, without a CSS scrollbar element to target.
    view()
        .app_class(move || {
            tw(&cn([
                Some("relative min-h-0 overflow-hidden rounded-t-xl bg-transparent"),
                (!has_footer.get()).then_some("rounded-b-2xl"),
            ]))
        })
        .child(children)
}

pub fn command_list(
    state: CommandState,
    label: impl Into<String>,
    children: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    // overflow-auto becomes scroll(); not-empty is derived from the catalogue.
    scroll()
        .element_ref(&state.list)
        .role(AccessibilityRole::Listbox)
        .accessibility_state(move || AccessibilityState {
            busy: state.loading.get(),
            ..Default::default()
        })
        .label(move || SharedString::from(t(&label)))
        .app_class(move || {
            tw(&cn([
                Some("min-h-0"),
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
    label: impl Into<String>,
    render: impl Fn(Memo<Option<Item>>) -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
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
    .label(move || SharedString::from(t(&label)))
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

pub fn command_list_heading(label: impl Into<String>) -> impl IntoElement {
    let label = label.into();
    // NSH lacks role=presentation; this label is plain, non-interactive text.
    text(move || SharedString::from(t(&label)))
        .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

#[derive(Clone)]
struct GroupScope(SharedString);

pub fn command_group<E: IntoElement>(
    state: CommandState,
    label: impl Into<String>,
    follows_group: bool,
    children: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let label = label.into();
    let value = SharedString::from(label.clone());
    // AutocompleteGroup's [[role=group]+&] becomes the caller's sibling condition.
    when(
        move || {
            state
                .items
                .with(|items| items.iter().any(|i| i.group.as_ref() == Some(&value)))
        },
        move || {
            untracked(|| {
                provide_context(GroupScope(SharedString::from(label.clone())));
                let label = label.clone();
                view()
                    .role(AccessibilityRole::Group)
                    .label(move || SharedString::from(t(&label)))
                    .app_class(tw(&cn([follows_group.then_some("mt-1.5")])))
                    .child(children())
            })
        },
    )
}

pub fn command_group_label(label: impl Into<String>) -> impl IntoElement {
    let label = label.into();
    text(move || SharedString::from(t(&label)))
        .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs"))
}

pub fn command_item(
    state: CommandState,
    item: Memo<Option<Item>>,
    active: impl Fn() -> Option<bool> + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let active = Rc::new(active);
    let row = element_ref();
    let disabled =
        move || state.disabled.get() || item.with(|i| i.as_ref().is_none_or(|i| i.disabled));
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
            row.reveal(RevealOptions::default());
        }
    });
    // Autocomplete.Item is a role=option div, outside the Tab order; input retains key focus.
    view()
        .on_mouse_move(move |_: &MouseEvent| {
            if let Some(item) = item.get_untracked() {
                state.highlight(Some(item.value));
            }
        })
        .child(
            view()
                .element_ref(&row)
                .role(AccessibilityRole::Option)
                .focusable(false)
                .optional_label(move || item.get().map(|i| SharedString::from(t(&i.label))))
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
                        4.0
                    } else {
                        0.0
                    }))
                })
                .app_class(move || {
                    item_classes(selected(), highlighted.get(), disabled(), active())
                })
                .child(children),
        )
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

pub fn command_shortcut(label: impl Into<String>) -> impl IntoElement {
    let label = label.into();
    text(move || SharedString::from(t(&label))).app_class(tw(
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
    label: impl Into<String>,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    // Frozen Button API lacks the TSX's h-auto/px-2/transparent-hover override.
    button(
        ButtonContent::Label(t(&label.into())),
        ButtonVariant::GhostMuted,
        ButtonSize::Xs,
        disabled,
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
    fn filtering_preserves_case_insensitive_contains_and_keyword_matches() {
        let mut item = Item::new("stable-key", "Open settings");
        item.keywords.push("Preferences".into());
        for query in ["", "open", "SETTINGS", "sett", "preferences"] {
            assert!(matches_query(&item, query), "{query}");
        }
        for query in ["stable-key", "missing", " settings "] {
            assert!(!matches_query(&item, query), "{query}");
        }
    }

    #[test]
    fn auto_highlight_always_handles_empty_query_disabled_and_empty_results() {
        assert_eq!(
            resolve_highlight(&items(), "", None, AutoHighlight::Always).as_deref(),
            Some("a")
        );
        assert_eq!(
            resolve_highlight(&[], "", None, AutoHighlight::Always),
            None
        );
        assert_eq!(
            resolve_highlight(&items()[..1], "", None, AutoHighlight::Always),
            None
        );
        assert_eq!(
            resolve_highlight(&items(), "", None, AutoHighlight::OnInput),
            None
        );
        assert_eq!(
            resolve_highlight(&items(), "open", None, AutoHighlight::OnInput).as_deref(),
            Some("a")
        );
        assert_eq!(
            resolve_highlight(&items(), "open", None, AutoHighlight::Never),
            None
        );
    }

    #[test]
    fn kept_highlight_follows_the_value_when_async_results_reorder() {
        let requested = Highlight {
            query: "open".into(),
            value: Some("b".into()),
        };
        let mut rows = items();
        rows.reverse();
        assert_eq!(
            resolve_highlight(&rows, "open", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("b")
        );
        rows[0].disabled = true;
        assert_eq!(
            resolve_highlight(&rows, "open", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("a")
        );
        assert_eq!(
            resolve_highlight(&rows[1..], "open", Some(&requested), AutoHighlight::Always)
                .as_deref(),
            Some("a")
        );
    }

    #[test]
    fn query_changes_reset_highlight_and_an_explicit_input_highlight_is_retained() {
        let requested = Highlight {
            query: "old".into(),
            value: Some("b".into()),
        };
        assert_eq!(
            resolve_highlight(&items(), "new", Some(&requested), AutoHighlight::Always).as_deref(),
            Some("a")
        );
        let input = Highlight {
            query: "new".into(),
            value: None,
        };
        assert_eq!(
            resolve_highlight(&items(), "new", Some(&input), AutoHighlight::Always),
            None
        );
    }

    #[test]
    fn arrows_skip_disabled_rows_and_loop_through_the_input() {
        let rows = items();
        for (from, forward, expected) in [
            (None, true, Some("a")),
            (None, false, Some("b")),
            (Some("a"), true, Some("b")),
            (Some("b"), false, Some("a")),
            (Some("a"), false, None),
            (Some("b"), true, None),
            (Some("missing"), true, Some("a")),
        ] {
            assert_eq!(step(&rows, from, forward, true).as_deref(), expected);
        }
        assert_eq!(step(&rows, Some("a"), false, false).as_deref(), Some("a"));
        assert_eq!(step(&rows, Some("b"), true, false).as_deref(), Some("b"));
        assert_eq!(step(&[], None, true, true), None);
        assert_eq!(step(&rows[..1], None, false, true), None);
    }

    #[test]
    fn viewport_keeps_search_clear_of_titlebar_even_in_short_windows() {
        assert_eq!(viewport_padding(800.0, 0.0, false), (80.0, 80.0));
        assert_eq!(viewport_padding(844.0, 0.0, true), (33.76, 33.76));
        assert_eq!(viewport_padding(200.0, 40.0, false), (48.0, 20.0));
        assert_eq!(viewport_padding(200.0, 40.0, true), (48.0, 16.0));
    }
}
