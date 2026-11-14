//! Port of apps/web/src/components/chat/ComposerStashMenu.tsx ComposerStashMenu.

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use std::rc::Rc;
use std::time::Duration;
use t3_client::services::config::timestamp_ms;
use t3_client::thread_views::composer::stash_presentation::{
    AttachmentKind, Snippet, missing_image_count, stash_entry_snippet,
};
use t3_client::view_logic::session::timestamps::{
    format_relative_elapsed, ms_until_relative_time_change, relative_time_elapsed_ms,
};
use t3_client::view_logic::shell::stash::Entry;

use crate::i18n::{Text as UiText, Value, t, tr};
use crate::runtime::Shared;
use crate::theme::{cn, palette, tw};
use crate::ui::alert::composer_banner as banner;
use crate::ui::icon::{Icon, icon};
use crate::ui::toast::{ToastOptions, Variant, toast_manager};

#[derive(Clone, Default, PartialEq)]
pub struct StashSnapshot {
    pub entries: Vec<Shared<Entry>>,
    pub can_stash: bool,
    pub stash_revision: u64,
    pub can_restore: bool,
    pub can_delete: bool,
}

pub struct StashNotice {
    pub variant: Variant,
    pub title: UiText,
    pub description: Option<UiText>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuRequest {
    #[default]
    Keep,
    Open,
    Close,
    Toggle,
}

pub struct StashCompletion {
    pub notices: Vec<StashNotice>,
    pub menu: MenuRequest,
}

pub type StashResult = Result<StashCompletion, StashNotice>;

/// Adapter to a composed client service; it owns persistence, draft edits, uploads and feedback.
pub trait PromptStashClient {
    fn snapshot(&self) -> StashSnapshot;
    fn changes(&self) -> LocalBoxStream<'static, ()>;
    fn stash_current(&self) -> LocalBoxFuture<'static, StashResult>;
    fn restore(
        &self,
        entry_id: String,
    ) -> Result<LocalBoxFuture<'static, StashResult>, StashNotice>;
    fn delete(&self, entry_id: String) -> LocalBoxFuture<'static, StashResult>;
}

#[derive(Clone, Copy)]
pub struct PromptStash {
    pub state: Signal<StashSnapshot>,
    pub menu_open: Signal<bool>,
    pulse: Signal<bool>,
    client: Stored<Rc<dyn PromptStashClient>>,
    owner: Owner,
    expand_composer: Stored<Rc<dyn Fn()>>,
}

impl PromptStash {
    pub fn attach(client: Rc<dyn PromptStashClient>, expand_composer: impl Fn() + 'static) -> Self {
        let mut changes = client.changes();
        let state = signal(client.snapshot());
        let source = client.clone();
        let pulse = signal(false);
        let revision = stored(state.with(|state| state.stash_revision));
        let pulse_task = stored(None::<Spawned<()>>);
        spawn(async move {
            while changes.next().await.is_some() {
                let next = source.snapshot();
                if revision.replace(next.stash_revision) != next.stash_revision {
                    if let Some(task) = pulse_task.replace(None) {
                        task.cancel();
                    }
                    pulse.set(true);
                    pulse_task.replace(Some(set_timeout(
                        std::time::Duration::from_millis(1200),
                        move || pulse.set(false),
                    )));
                }
                state.set(next);
            }
        });
        Self {
            state,
            menu_open: signal(false),
            pulse,
            client: stored(client),
            owner: Owner::current(),
            expand_composer: stored(Rc::new(expand_composer)),
        }
    }

    /// SH-KEYS dispatches composer.stash here after the client's shortcut guards.
    pub fn stash_current(self) {
        if !self.state.with(|state| state.can_stash) {
            return;
        }
        let client = self.client.get();
        self.owner.run(move || {
            spawn(async move {
                show_result(self, client.stash_current().await);
            })
        });
    }

    fn open_menu(self) {
        self.expand_composer.with(|expand| expand());
        self.menu_open.set(true);
    }

    fn toggle_menu(self) {
        if self.menu_open.get_untracked() {
            self.menu_open.set(false);
        } else {
            self.open_menu();
        }
    }

    fn restore(self, id: String) {
        if !self
            .state
            .with(|state| state.can_restore && state.entries.iter().any(|entry| entry.id == id))
        {
            return;
        }
        let client = self.client.get();
        match client.restore(id) {
            Ok(restore) => {
                self.menu_open.set(false);
                self.owner.run(move || {
                    spawn(async move {
                        show_result(self, restore.await);
                    })
                });
            }
            Err(notice) => show_result(self, Err(notice)),
        }
    }

    fn delete(self, id: String) {
        if !self
            .state
            .with(|state| state.can_delete && state.entries.iter().any(|entry| entry.id == id))
        {
            return;
        }
        let client = self.client.get();
        self.owner.run(move || {
            spawn(async move {
                show_result(self, client.delete(id).await);
            })
        });
    }
}

fn show_result(stash: PromptStash, result: StashResult) {
    let notices = match result {
        Ok(completion) => {
            match completion.menu {
                MenuRequest::Keep => {}
                MenuRequest::Close => stash.menu_open.set(false),
                MenuRequest::Open => stash.open_menu(),
                MenuRequest::Toggle => stash.toggle_menu(),
            }
            completion.notices
        }
        Err(notice) => vec![notice],
    };
    for notice in notices {
        let title = notice.title;
        let description = notice.description;
        let spoken = title.resolve().to_string();
        toast_manager().add(ToastOptions {
            variant: notice.variant,
            title: spoken.into(),
            title_content: Some(Rc::new(move || {
                let title = title.clone();
                text(move || title.resolve()).into_element()
            })),
            description_content: description.map(|description| {
                Rc::new(move || {
                    let description = description.clone();
                    text(move || description.resolve()).into_element()
                }) as crate::ui::toast::Slot
            }),
            ..Default::default()
        });
    }
}

/// Port of apps/web/src/components/chat/ComposerStashBadge.tsx ComposerStashBadge.
pub fn composer_stash_badge(stash: PromptStash) -> impl IntoElement {
    let count = computed(move || stash.state.with(|state| state.entries.len()));
    let shown = computed(move || count.get() > 0);
    let hovered = signal(false);
    dynamic(move || {
        shown.get().then(|| untracked(|| {
        view().app_class(tw("ml-auto"))
            .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
            .pointer_events(move || if stash.menu_open.get() { PointerEvents::None } else { PointerEvents::Auto })
            .child(banner::root(banner::Props {density:banner::Density::Comfortable,width:banner::Width::Content,..Default::default()}, move |context| {
                banner::row_button(context,banner::Layout::Inline,
                    move || UiText::content(tr("Stashed prompts: {count}. Open stash.", &[("count",Value::Int(count.get() as i64))])),
                    || false,
                    move || stash.toggle_menu(),
                    move |row| banner::RowSlots {
                        icon: Some(banner::lucide_icon(row,Icon::Bookmark).into_element()),
                        content: Some(banner::content(row,text(move || crate::i18n::ts("Stash"))
                            .app_class(tw("transition-colors duration-200 motion-reduce:transition-none"))
                            .text_color(move || if stash.pulse.get() || stash.menu_open.get() || hovered.get() {palette().foreground} else {palette().muted_foreground})
                            .hover(|s|s.text_color(palette().foreground))).into_element()),
                        actions: Some(banner::actions(row,banner::count(text(move || SharedString::from(crate::i18n::formats().decimal(count.get() as f64)))
                            .text_color(move || if stash.pulse.get() {palette().primary} else {palette().muted_foreground}))).into_element()),
                    })
            })).into_element()
    }))
    })
}

#[derive(Clone, Copy)]
pub struct StashMenuProps {
    pub anchor: ElementRef,
    pub composer_focused: Signal<bool>,
    pub composer_menu_open: Signal<bool>,
    pub prompt_revision: Signal<u64>,
    pub shortcut_label: Signal<Option<SharedString>>,
    /// Composer main-surface width; the source layer removes its 1.375rem inset on each side.
    pub composer_width: Signal<f32>,
}

pub fn composer_stash_menu(stash: PromptStash, props: StashMenuProps) -> impl IntoElement {
    let prior_prompt = stored(props.prompt_revision.get_untracked());
    // Synchronizes the menu lifecycle with external editor/menu state, as useComposerStash does.
    effect(move || {
        let revision = props.prompt_revision.get();
        let other_menu = props.composer_menu_open.get();
        if prior_prompt.replace(revision) != revision || other_menu {
            stash.menu_open.set(false);
        }
    });
    dynamic(move || {
        (stash.menu_open.get() && !props.composer_menu_open.get()).then(|| untracked(move || {
        let highlighted = signal(None::<String>);
        let focused = signal(false);
        let delete_focused = signal(false);
        let buttons = stored(std::collections::BTreeMap::<String, ElementRef>::new());
        let current = computed(move || stash.state.with(|state| {
            let selected = highlighted.get();
            state.entries.iter().find(|entry| Some(&entry.id) == selected.as_ref()).or_else(|| state.entries.first()).map(|entry| entry.id.clone())
        }));
        // Parent refs outlive recycled rows; pruning here avoids reading disposed state in cleanup.
        effect(move || {
            let ids = stash.state.with(|state| state.entries.iter().map(|entry| entry.id.clone()).collect::<std::collections::BTreeSet<_>>());
            buttons.with(|buttons| buttons.retain(|id, _| ids.contains(id)));
        });
        let menu = element_ref();
        menu.on_focus_in(move || focused.set(true));
        menu.on_focus_out(move || focused.set(false));
        let keyboard = dynamic(move || (props.composer_focused.get() || focused.get()).then(|| untracked(move || {
            let keys = KeyOptions { when: nsh_core::keymap::When::Always, ..Default::default() };
            on_key_with("escape", keys, move |_| stash.menu_open.set(false));
            for (key, offset) in [("down", 1_isize), ("up", -1)] {
                on_key_with(key, KeyOptions { repeat:true, ..keys }, move |_| {
                    let ids = stash.state.with(|state| state.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
                    if let Some(next) = next_highlight(&ids, current.get().as_deref(), offset) {
                        highlighted.set(Some(next.clone()));
                        buttons.with(|buttons| if let Some(button) = buttons.get(&next) {
                            button.reveal(RevealOptions::default());
                            if focused.get_untracked() { button.focus(); }
                        });
                    }
                });
            }
            on_key_with("mod+backspace", keys, move |_| { if let Some(id) = current.get_untracked() { stash.delete(id); } });
            // The delete control retains its native Enter press rather than restoring the row.
            dynamic(move || (!delete_focused.get()).then(|| untracked(move || {
                on_key_with("enter", keys, move |_| { if let Some(id) = current.get_untracked() { stash.restore(id); } });
                view()
            })))
        })));
        overlay().anchor_content(&props.anchor).placement(Placement::Top).align(Align::Center).offset(px(-17.0))
            .app_class(tw("pointer-events-auto z-40 flex flex-col"))
            .w(move || px((props.composer_width.get() - 44.0).max(0.0)))
            .element_ref(&menu)
            .on_cancel(move || stash.menu_open.set(false))
            .on_press_outside(move |_: &Point<Points>| stash.menu_open.set(false))
            .child(keyboard)
            .child(banner::root(banner::Props::default(), move |context| {
                view().app_class(tw("flex min-h-0 flex-col"))
                    .child(banner::row_button(context, banner::Layout::Inline, || "Close stash".into(), || false, move || stash.menu_open.set(false), move |row| banner::RowSlots {
                        icon: Some(banner::lucide_icon(row, Icon::Bookmark).into_element()),
                        content: Some(banner::content(row, text(move || crate::i18n::ts("Stash")).app_class(tw("text-muted-foreground"))).into_element()),
                        actions: Some(banner::actions(row, view().app_class(tw("flex items-center gap-1"))
                            .child(banner::count(text(move || SharedString::from(crate::i18n::formats().decimal(stash.state.with(|state| state.entries.len()) as f64)))))
                            .child(banner::toggle_icon(|| true))).into_element()),
                    }))
                    .child(banner::scroll(banner::children(context, move |children_context| {
                        view().role(AccessibilityRole::List).label(crate::i18n::ts("Stashed prompts"))
                            .child(dynamic(move || stash.state.with(|state| state.entries.is_empty()).then(|| untracked(move || {
                                banner::row(children_context, banner::Layout::Inline, move |row| banner::RowSlots {
                                    icon: Some(banner::icon(row, view()).into_element()),
                                    content: Some(banner::content(row, text(move || {
                                        let shortcut = props.shortcut_label.get();
                                        SharedString::from(if let Some(shortcut) = shortcut {
                                            tr("Nothing stashed yet. Press {shortcut} with a prompt in the composer to stash it.", &[("shortcut", Value::Text(shortcut.to_string()))])
                                        } else { t("Nothing stashed yet.") })
                                    }).app_class(tw("text-muted-foreground"))).into_element()),
                                    ..Default::default()
                                })
                            }))))
                            .child(each(move || stash.state.with(|state| state.entries.clone()), |entry| entry.id.clone(), move |entry, _| {
                                stash_entry_row(stash, entry, children_context, current, highlighted, buttons, delete_focused)
                            }))
                    })))
            })).into_element()
    }))
    })
}

fn next_highlight(ids: &[String], current: Option<&str>, offset: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let index = ids
        .iter()
        .position(|id| Some(id.as_str()) == current)
        .map(|at| at as isize)
        .unwrap_or(if offset == 1 { -1 } else { 0 });
    Some(ids[(index + offset).rem_euclid(ids.len() as isize) as usize].clone())
}

fn snippet_text(snippet: Snippet) -> SharedString {
    match snippet {
        Snippet::Prompt(prompt) => prompt.into(),
        Snippet::Empty => crate::i18n::ts("(empty)"),
        Snippet::Attachments { count, kind } => {
            let args = [("count", Value::Int(count as i64))];
            match kind {
                AttachmentKind::Image => {
                    tr("({count, plural, one {# image} other {# images}})", &args)
                }
                AttachmentKind::File => {
                    tr("({count, plural, one {# file} other {# files}})", &args)
                }
                AttachmentKind::Attachment => tr(
                    "({count, plural, one {# attachment} other {# attachments}})",
                    &args,
                ),
            }
            .into()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stash_entry_row(
    stash: PromptStash,
    entry: ReadSignal<Shared<Entry>>,
    context: banner::Context,
    current: Memo<Option<String>>,
    highlighted: Signal<Option<String>>,
    buttons: Stored<std::collections::BTreeMap<String, ElementRef>>,
    delete_focused: Signal<bool>,
) -> impl IntoElement {
    let snippet = computed(move || entry.with(|entry| stash_entry_snippet(entry)));
    let restore = element_ref();
    let delete = element_ref();
    delete.on_focus_in(move || delete_focused.set(true));
    delete.on_focus_out(move || delete_focused.set(false));
    let id = entry.with(|entry| entry.id.clone());
    buttons.with(|buttons| {
        buttons.insert(id.clone(), restore);
    });

    let row_ref = element_ref();
    row_ref.on_focus_in(move || highlighted.set(Some(entry.with(|entry| entry.id.clone()))));
    view().element_ref(&row_ref).test_id(&entry.with(|entry| format!("stash-entry-{}", entry.id))).role(AccessibilityRole::Group)
        .app_class(move || Some(tw(&cn([Some("relative rounded-sm"),
            (entry.with(|entry| Some(&entry.id) == current.get().as_ref())).then_some("bg-accent text-accent-foreground")]))))
        .on_mouse_move(move |_: &MouseEvent| highlighted.set(Some(entry.with(|entry| entry.id.clone()))))
        .child(banner::row(context, banner::Layout::Inline, move |row| {
            banner::RowSlots {
                icon: Some(banner::lucide_icon(row, Icon::FileText).into_element()),
                content: Some(banner::content(row, nsh_base::button()
                    .element_ref(&restore)
                    .accessibility_label(move || Some(tr("Restore stashed prompt: {snippet}", &[("snippet", Value::Text(snippet_text(snippet.get()).to_string()))]).into()))
                    .disabled(move || !stash.state.with(|state| state.can_restore))
                    .on_press(move || stash.restore(entry.with(|entry| entry.id.clone())))
                    // The before: pseudo-element becomes the first absolute child.
                    .app_class(tw("min-w-0 flex-1 cursor-pointer truncate text-left text-foreground/80 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
                    .child(view().app_class(tw("absolute inset-0 rounded-sm")))
                    .child(text(move || snippet_text(snippet.get())).number_of_lines(1_u32))).into_element()),
                actions: Some(banner::actions(row, view().app_class(tw("flex items-center justify-end gap-1"))
                    .child(dynamic(move || {
                        let (pending, missing) = entry.with(|entry| (entry.pending_image_count.unwrap_or(0.0), missing_image_count(entry)));
                        if pending != 0.0 { Some(text(SharedString::from(tr("saving {count, plural, one {# image} other {# images}}…", &[("count", Value::Float(pending))]))).app_class(tw("shrink-0 text-muted-foreground")).into_element()) }
                        else if missing > 0 { Some(text(SharedString::from(tr("{count, plural, one {# image} other {# images}} dropped", &[("count", Value::Int(missing as i64))]))).app_class(tw("shrink-0 text-warning-foreground")).into_element()) }
                        else { None }
                    }))
                    .child(view().app_class(tw("flex shrink-0 items-center")).child(each(move || entry.with(|entry| entry.attachments.iter().take(3).cloned().collect::<Vec<_>>()), |image| image.id.clone(), |attachment, index| {
                        image(move || attachment.with(|image| SharedString::from(image.data_url.clone())))
                            .accessible(false).app_class(move || Some(tw(&cn([Some("size-4 rounded border border-border/70 object-cover"), (index.get() > 0).then_some("-ms-1.5")]))))
                    })))
                    .child(dynamic(move || {
                        let count = entry.with(|entry| entry.files.as_ref().map_or(0, Vec::len));
                        (count > 0).then(|| view().app_class(tw("flex shrink-0 items-center gap-1 text-muted-foreground"))
                            .child(icon(Icon::File, 12.0, move || palette().muted_foreground))
                            .child(text(SharedString::from(crate::i18n::formats().decimal(count as f64)))))
                    }))
                    .child(view().app_class(tw("shrink-0 text-muted-foreground tabular-nums max-sm:hidden")).child(relative_time(move || entry.with(|entry| entry.created_at.clone()))))
                    .child(view().element_ref(&delete).app_class(tw("z-10")).child(banner::dismiss("Delete stashed prompt".into(), move || !stash.state.with(|state| state.can_delete), move || stash.delete(entry.with(|entry| entry.id.clone()))))))
                    .into_element()),
            }
        }))
}

/// Port of apps/web/src/components/RelativeTime.tsx RelativeTime and its clock binding.
fn relative_time(timestamp: impl Fn() -> String + 'static) -> impl IntoElement {
    let timestamp = computed(timestamp);
    dynamic(move || {
        let date = timestamp_ms(&timestamp.get())?;
        Some(untracked(move || {
            let elapsed = signal(relative_time_elapsed_ms(
                date,
                nsh_core::time::unix_ms() as i64,
            ));
            spawn(async move {
                loop {
                    let now = nsh_core::time::unix_ms() as i64;
                    timer(Duration::from_millis(
                        ms_until_relative_time_change(date, now) as u64,
                    ))
                    .await;
                    elapsed.set(relative_time_elapsed_ms(
                        date,
                        nsh_core::time::unix_ms() as i64,
                    ));
                }
            });
            text(move || {
                let parts = format_relative_elapsed(elapsed.get());
                if parts.suffix.is_some() {
                    tr("{time} ago", &[("time", Value::Text(parts.value))]).into()
                } else {
                    UiText::client_key(parts.value).resolve()
                }
            })
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_navigation_wraps_and_recovers_when_the_selected_entry_disappears() {
        let ids = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(next_highlight(&ids, Some("a"), -1), Some("c".into()));
        assert_eq!(next_highlight(&ids, Some("c"), 1), Some("a".into()));
        assert_eq!(next_highlight(&ids, Some("missing"), 1), Some("a".into()));
        assert_eq!(next_highlight(&ids, None, -1), Some("c".into()));
        assert_eq!(next_highlight(&[], None, 1), None);
    }
}
