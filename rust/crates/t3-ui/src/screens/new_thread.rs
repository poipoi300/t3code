//! Port of apps/web/src/routes/_chat.index.tsx IndexDraftLanding and ChatView.tsx draft hero.

use std::rc::Rc;

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::operations::ComposerDispatchMode;
use t3_client::thread_views::composer::attachment_intake::AttachmentSession;
use t3_client::types::ThreadKey;
use t3_client::workspace::ProjectKey;

use crate::i18n::{Text, t};
use crate::screens::composer::attachments::{
    ComposerAttachments, composer_attach_button, composer_attachment_strip,
};
use crate::screens::composer::editor::{
    ComposerEditor, ComposerPromptEditorProps, ComposerResolvers, ComposerSubmitCallback,
    EditorMode, composer_prompt_editor_tiptap,
};
use crate::screens::composer::model_picker::{PickerProps, provider_model_picker};
use crate::screens::composer::send::{
    PrimaryActionState, PrimaryActionsProps, composer_primary_actions,
};
use crate::state::nav::{Route, navigator};
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};
use crate::ui::popover::{PopoverOptions, popover, popover_create_handle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Home,
    Draft(String),
    Link(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Draft(String),
    Thread(ThreadKey),
}

impl Destination {
    pub fn route(self) -> Route {
        match self {
            Self::Draft(id) => Route::Draft(id),
            Self::Thread(key) => Route::Thread(key),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectChoice {
    pub key: ProjectKey,
    pub title: String,
    pub environment_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftView {
    pub id: String,
    pub prompt: String,
    pub cursor: usize,
    pub revision: u64,
    pub project: Option<ProjectKey>,
    pub project_title: Option<String>,
    pub projects: Vec<ProjectChoice>,
    pub is_scratch: bool,
    pub can_start_scratch: bool,
    pub editable: bool,
    pub actions: PrimaryActionState,
    pub banner: Option<Text>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum NewThreadView {
    #[default]
    Loading,
    Empty {
        can_start_scratch: bool,
    },
    StartFailed,
    LinkFailed {
        title: Text,
        message: Text,
    },
    LinkOpened,
    LinkChoice {
        title: Text,
        message: Text,
        choices: Vec<LinkChoice>,
    },
    Draft(Rc<DraftView>),
    Navigate(Destination),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    CloneRepository,
    UseWithoutProject,
    CheckoutPullRequest,
    ReplaceDraft,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkChoice {
    pub action: LinkAction,
    pub label: Text,
    pub disabled_reason: Option<Text>,
}

/// Adapter for the client-owned draft service requested in client-seam-requests.md MISC-NEW.
pub trait NewThreadClient {
    /// Emits the current view first; dropping the stream releases the session demand.
    fn watch(&self, target: Target) -> LocalBoxStream<'static, Rc<NewThreadView>>;
    fn watch_link(
        &self,
        search: String,
        _entry: String,
    ) -> LocalBoxStream<'static, Rc<NewThreadView>> {
        self.watch(Target::Link(search))
    }
    fn retry(&self, target: Target) -> LocalBoxFuture<'static, Result<(), Text>>;
    fn start_scratch(&self, draft: Option<String>) -> LocalBoxFuture<'static, Result<(), Text>>;
    fn select_project(
        &self,
        draft: String,
        project: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<(), Text>>;
    fn set_prompt(&self, draft: &str, prompt: String, cursor: usize) -> Result<u64, Text>;
    fn submit(
        &self,
        draft: String,
        revision: u64,
        mode: ComposerDispatchMode,
    ) -> LocalBoxFuture<'static, Result<ThreadKey, Text>>;
    fn attachments(&self, draft: &str) -> Option<AttachmentSession>;
    fn model_picker(&self, draft: &str) -> Option<PickerProps>;
    fn link_action(
        &self,
        _raw_search: String,
        _action: LinkAction,
    ) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>> {
        futures::future::ready(Err(Text::client_key(
            t3_client::ClientError::Unsupported("new-thread link choices").to_string(),
        )))
        .boxed_local()
    }
}

#[derive(Clone)]
pub struct NewThreadRuntime(pub Rc<dyn NewThreadClient>);

pub fn screen(draft: Option<&str>) -> AnyElement {
    index_draft_landing(draft.map_or(Target::Home, |id| Target::Draft(id.into())))
}

/// Port of routes/_chat.new.tsx NewThreadLinkRoute; SH-ROUTES supplies the raw query.
pub fn new_thread_link_route(search: &str) -> AnyElement {
    index_draft_landing(Target::Link(search.into()))
}

pub fn index_draft_landing(target: Target) -> AnyElement {
    let Some(client) = use_context::<NewThreadRuntime>() else {
        // The demo has no draft service; an unobserved shell cannot prove emptiness.
        debug_assert!(use_context::<crate::runtime::Rt>().is_none_or(|rt| rt.backend().client().is_none()), "NewThreadRuntime is not provided: app::provide_client_services provides it with the client");
        return view().flex_1().into_element();
    };
    let current = signal(Rc::new(NewThreadView::Loading));
    let error = signal(None::<Text>);
    let link_busy = signal(false);
    let owner = Owner::current();
    let nav = navigator();
    let navigation = LinkNavigation {
        urls: use_context::<crate::state::url::UrlState>(),
        pending: signal(None),
    };
    let origin = nav.top();
    let client = stored(client.0);
    let target = stored(target);
    effect(move || {
        if nav.top() != origin {
            return;
        }
        let mut changes = match (target.get(), navigation.urls) {
            (Target::Link(search), Some(urls)) => {
                if urls.location.get_untracked().search != search {
                    return;
                }
                let Some(entry) = urls.entry_identity() else {
                    return;
                };
                client.get().watch_link(search, entry)
            }
            (target, _) => client.get().watch(target),
        };
        navigation.pending.set(None);
        let origin = origin.clone();
        let task = spawn(async move {
            while let Some(next) = changes.next().await {
                current.set(next.clone());
                if let NewThreadView::Navigate(destination) = next.as_ref()
                    && nav.top() == origin
                {
                    open_destination(nav, navigation, destination.clone());
                }
            }
        });
        cleanup(move || task.cancel());
    });
    // Prompt edits update bindings, never remount the editor and its undo history.
    let page = computed(move || match current.get().as_ref() {
        NewThreadView::Loading | NewThreadView::Navigate(_) => (0, None),
        NewThreadView::Empty { .. } => (1, None),
        NewThreadView::StartFailed => (2, None),
        NewThreadView::LinkFailed { .. } => (3, None),
        NewThreadView::Draft(draft) => (4, Some(draft.id.clone())),
        NewThreadView::LinkChoice { .. } => (5, None),
        NewThreadView::LinkOpened => (6, None),
    });
    view()
        .app_class(tw(
            "relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background",
        ))
        .child(dynamic(move || {
            let (kind, _) = page.get();
            untracked(move || match kind {
                1 => no_projects_hero(current, client, error),
                2 => draft_start_error(client, target, error),
                3 => view()
                    .flex_1()
                    .child(dynamic(move || {
                        if let NewThreadView::LinkFailed { title, message } = current.get().as_ref()
                        {
                            Some(empty(title.clone(), message.clone()))
                        } else {
                            None
                        }
                    }))
                    .into_element(),
                4 => draft_chat_view(current, client, error),
                5 => link_choices(current, client, target, error, link_busy, owner, navigation),
                6 => opened_link(client, target, error),
                _ => view().into_element(),
            })
        }))
        .child(dynamic(move || {
            error.get().map(|message| {
                text(message.resolve())
                    .role(AccessibilityRole::Alert)
                    .app_class(tw("px-4 py-3 text-sm text-muted-foreground"))
            })
        }))
        .into_element()
}

#[derive(Clone, Copy)]
struct LinkNavigation {
    urls: Option<crate::state::url::UrlState>,
    pending: Signal<Option<Route>>,
}
fn open_destination(
    nav: crate::state::nav::Navigator,
    navigation: LinkNavigation,
    destination: Destination,
) {
    let route = destination.route();
    if navigation.pending.get_untracked().as_ref() == Some(&route) {
        return;
    }
    navigation.pending.set(Some(route.clone()));
    if let Some(urls) = navigation.urls
        && let Some(path) = route.address()
    {
        urls.request(crate::state::url::UrlRequest::Replace(path));
    } else {
        nav.open(route);
    }
}

fn link_choices(
    current: Signal<Rc<NewThreadView>>,
    client: Stored<Rc<dyn NewThreadClient>>,
    target: Stored<Target>,
    error: Signal<Option<Text>>,
    busy: Signal<bool>,
    owner: Owner,
    navigation: LinkNavigation,
) -> AnyElement {
    let nav = navigator();
    let origin = nav.top();
    view().flex_1().child(dynamic(move|| {
        let value=current.get();let NewThreadView::LinkChoice{title,message,choices}=value.as_ref() else{return view().into_element();};
        let mut buttons=view().app_class(tw("mt-5 flex justify-center gap-2"));
        for choice in choices {
            let action=choice.action;
            let origin=origin.clone();
            buttons=buttons.child(button(ButtonContent::Label(choice.label.resolve().to_string()),if action==LinkAction::UseWithoutProject{ButtonVariant::Outline}else{ButtonVariant::Default},ButtonSize::Sm,
                move||busy.get()||!matches!(current.get().as_ref(),NewThreadView::LinkChoice{choices,..} if choices.iter().any(|c|c.action==action&&c.disabled_reason.is_none())),
                move|| {
                    if busy.get_untracked()||!matches!(current.get_untracked().as_ref(),NewThreadView::LinkChoice{choices,..} if choices.iter().any(|c|c.action==action&&c.disabled_reason.is_none())) {return;}
                    let Target::Link(search)=target.get() else{return;};
                    busy.set(true);error.set(None);let operation=client.get().link_action(search,action);
                    let origin=origin.clone();
                    owner.run(move||spawn(async move {
                        let result=operation.await;busy.set(false);
                        match result {Ok(Some(destination)) if nav.top()==origin=>open_destination(nav,navigation,destination),Ok(_)=>{},Err(message)=>error.set(Some(message))}
                    }));
                }));
        }
        view().app_class(tw("flex min-w-0 flex-1 flex-col items-center justify-center text-center gap-6 p-6 md:p-12"))
            .child(view().app_class(tw("flex max-w-md flex-col items-center text-center"))
                .child(text(title.resolve()).app_class(tw("font-semibold text-xl")))
                .child(text(message.resolve()).app_class(tw("mt-1 text-muted-foreground text-sm")))
                .child(buttons)).into_element()
    })).into_element()
}

fn opened_link(
    client: Stored<Rc<dyn NewThreadClient>>,
    target: Stored<Target>,
    error: Signal<Option<Text>>,
) -> AnyElement {
    let owner = Owner::current();
    view().app_class(tw("flex min-w-0 flex-1 flex-col items-center justify-center text-center gap-6 p-6 md:p-12"))
        .child(text(t("This link is open")).app_class(tw("font-semibold text-xl")))
        .child(button(ButtonContent::Label(t("Open it again")),ButtonVariant::Outline,ButtonSize::Sm,||false,move||{
            error.set(None);let operation=client.get().retry(target.get());
            owner.run(move||spawn(async move{if let Err(message)=operation.await {error.set(Some(message));}}));
        })).into_element()
}

fn empty(title: Text, message: Text) -> AnyElement {
    // UI-EMPTY is absent; these are the source Empty/Header/Title/Description classes.
    view().app_class(tw("flex min-w-0 flex-1 flex-col items-center justify-center text-center gap-6 p-6 md:p-12"))
        .child(view().app_class(tw("flex max-w-md flex-col items-center text-center"))
            .child(text(move || title.resolve()).app_class(tw("font-semibold text-xl")))
            .child(text(move || message.resolve()).app_class(tw("mt-1 text-muted-foreground text-sm"))))
        .into_element()
}

/// Port of NoProjectsHero.tsx NoProjectsHero.
fn no_projects_hero(
    current: Signal<Rc<NewThreadView>>,
    client: Stored<Rc<dyn NewThreadClient>>,
    error: Signal<Option<Text>>,
) -> AnyElement {
    let nav = navigator();
    view().app_class(tw("flex min-w-0 flex-1 flex-col items-center justify-center text-center gap-6 p-6 md:p-12"))
        .child(view().app_class(tw("w-full max-w-lg px-8 py-12"))
            .child(text(move || t("What should we work on?")).app_class(tw("font-semibold text-2xl sm:text-3xl")))
            .child(text(move || if matches!(current.get().as_ref(), NewThreadView::Empty { can_start_scratch: true }) {
                t("Add a project, or start without one.")
            } else { t("Add a project to start your first thread.") }).app_class(tw("mt-1 text-muted-foreground text-sm")))
            .child(view().app_class(tw("mt-6 flex justify-center gap-2"))
                .child(button(ButtonContent::IconLabel { icon: Icon::Plus, label: t("Add project") }, ButtonVariant::Default, ButtonSize::Sm,
                    || false, move || nav.open(Route::AddProject)))
                .child(dynamic(move || {
                    matches!(current.get().as_ref(), NewThreadView::Empty { can_start_scratch: true }).then(|| {
                        button(ButtonContent::IconLabel { icon: Icon::MessageSquareDashed, label: t("Start without a project") }, ButtonVariant::Outline, ButtonSize::Sm,
                            || false, move || {
                                let operation = client.get().start_scratch(None);
                                spawn(async move { if let Err(message) = operation.await { error.set(Some(message)); } });
                            })
                    })
                })))).into_element()
}

/// Port of routes/_chat.index.tsx DraftStartError.
fn draft_start_error(
    client: Stored<Rc<dyn NewThreadClient>>,
    target: Stored<Target>,
    error: Signal<Option<Text>>,
) -> AnyElement {
    view()
        .app_class(tw(
            "flex flex-1 flex-col items-center justify-center gap-6 p-6 md:p-12",
        ))
        .child(empty(
            "Couldn’t start a new thread".into(),
            "The project is still available. Try opening the draft again.".into(),
        ))
        .child(
            view()
                .app_class(tw("mt-5 flex justify-center"))
                .child(button(
                    ButtonContent::IconLabel {
                        icon: Icon::RefreshCw,
                        label: t("Try again"),
                    },
                    ButtonVariant::Default,
                    ButtonSize::Sm,
                    || false,
                    move || {
                        error.set(None);
                        let operation = client.get().retry(target.get());
                        spawn(async move {
                            if let Err(message) = operation.await {
                                error.set(Some(message));
                            }
                        });
                    },
                )),
        )
        .into_element()
}

fn draft(current: Signal<Rc<NewThreadView>>) -> Option<Rc<DraftView>> {
    match current.get().as_ref() {
        NewThreadView::Draft(draft) => Some(draft.clone()),
        _ => None,
    }
}

fn can_submit(value: &DraftView, busy: bool) -> bool {
    value.editable
        && !busy
        && value.actions.can_operate_thread
        && value.actions.has_sendable_content
        && !value.actions.is_environment_unavailable
        && value.actions.send_disabled_reason.is_none()
        && !value.actions.is_send_busy
        && !value.actions.is_preparing_worktree
}

fn draft_chat_view(
    current: Signal<Rc<NewThreadView>>,
    client: Stored<Rc<dyn NewThreadClient>>,
    error: Signal<Option<Text>>,
) -> AnyElement {
    let Some(initial) = draft(current) else {
        return view().into_element();
    };
    let id = stored(initial.id.clone());
    let editor = match ComposerEditor::new(
        &initial.prompt,
        initial.cursor,
        EditorMode::Plain,
        ComposerResolvers::default(),
    ) {
        Ok(editor) => editor,
        Err(failure) => {
            error.set(Some(Text::client_key(failure.to_string())));
            return view().into_element();
        }
    };
    let syncing = editor.clone();
    let accepted_revision = signal(initial.revision);
    // Client revisions restore saved drafts or insert attachment references from outside the editor.
    effect(move || {
        if let Some(value) = draft(current)
            && value.revision >= accepted_revision.get_untracked()
            && let Err(failure) = syncing.sync_draft(&value.id, &value.prompt, value.cursor)
        {
            error.set(Some(Text::client_key(failure.to_string())));
        }
    });
    let disabled = signal(!initial.editable);
    effect(move || disabled.set(draft(current).is_none_or(|value| !value.editable)));
    let attachments = client
        .get()
        .attachments(&initial.id)
        .map(|session| ComposerAttachments::new(session, disabled));
    let busy = signal(false);
    let submit_owner = Owner::current();
    let nav = navigator();
    let origin = nav.top();
    let submit = Rc::new(move |mode| {
        let Some(value) = draft(current) else {
            return;
        };
        if !can_submit(&value, busy.get_untracked())
            || attachments.is_some_and(|a| a.block_reason().is_some())
        {
            return;
        }
        busy.set(true);
        error.set(None);
        let operation = client.get().submit(value.id.clone(), value.revision, mode);
        let origin = origin.clone();
        submit_owner.run(move || {
            spawn(async move {
                let result = operation.await;
                busy.set(false);
                match result {
                    Ok(key) if nav.top() == origin => nav.open(Route::Thread(key)),
                    Ok(_) => {}
                    Err(message) => error.set(Some(message)),
                }
            })
        });
    });
    let key_submit = submit.clone();
    let editor_ui = composer_prompt_editor_tiptap(
        editor.clone(),
        ComposerPromptEditorProps {
            disabled: Prop::bind(move || disabled.get()),
            placeholder: Prop::bind(move || {
                t("Ask anything. @ files, $ skills, / commands").into()
            }),
            aria_label: Prop::bind(move || t("Message").into()),
            on_submit: Prop::new(Some(ComposerSubmitCallback::new(move |_| {
                key_submit(ComposerDispatchMode::Auto);
                true
            }))),
            ..Default::default()
        },
        move |snapshot| {
            if !disabled.get_untracked() {
                match client
                    .get()
                    .set_prompt(&id.get(), snapshot.value, snapshot.cursor)
                {
                    Ok(revision) => accepted_revision.set(revision),
                    Err(message) => error.set(Some(message)),
                }
            }
        },
        move |files| {
            if let Some(attachments) = attachments {
                attachments.paste_files(files);
            }
        },
    );
    let mut footer = view().app_class(tw(
        "flex min-w-0 flex-nowrap items-center overflow-visible px-2 pb-2",
    ));
    if let Some(attachments) = attachments {
        footer = footer.child(
            view()
                .app_class(tw("me-2"))
                .child(composer_attach_button(attachments, move || editor.focus())),
        );
    }
    if let Some(props) = client.get().model_picker(&initial.id) {
        footer = footer.child(provider_model_picker(props));
    }
    let actions = PrimaryActionsProps::new(
        Prop::bind(move || {
            let mut actions = draft(current)
                .map_or_else(PrimaryActionState::default, |value| value.actions.clone());
            actions.compact = crate::state::layout::layout_class().is_compact();
            actions.is_send_busy |= busy.get();
            if let Some(reason) = attachments.and_then(ComposerAttachments::block_reason) {
                actions.send_disabled_reason = Some(reason);
            }
            actions
        }),
        move |mode| submit(mode),
    );
    footer = footer.child(
        view()
            .app_class(tw(
                "flex min-w-0 flex-1 flex-nowrap items-center justify-end gap-2",
            ))
            .child(composer_primary_actions(actions)),
    );
    // ComposerSurface before glass becomes a layer, after outline the host border; isolate is dropped.
    let mut main = view().app_class(tw("rounded-2xl"));
    if let Some(attachments) = attachments {
        main = main.child(composer_attachment_strip(attachments));
    }
    let headline_mode = computed(move || {
        draft(current).map(|value| {
            (
                value.is_scratch,
                value.project_title.is_some(),
                value.projects.is_empty(),
                value.can_start_scratch,
            )
        })
    });
    let surface = view().app_class(tw("relative mx-auto w-full max-w-(--chat-content-max-width)"))
        .child(view().app_class(tw("pointer-events-none absolute inset-0 z-0 rounded-2xl bg-card/80 dark:bg-surface-raised/80")))
        .child(view().app_class(tw("relative z-10 w-full rounded-2xl shadow-[0_1px_2px_rgb(0_0_0/5%)] dark:shadow-none border border-black/8 dark:border-white/5"))
            .child(view().app_class(tw("group relative z-10 rounded-2xl p-px"))
                .child(main.child(view().app_class(tw("relative px-4 pb-2 pt-3.5 sm:pt-4")).child(editor_ui)).child(footer))));
    // ChatView's max-height media query reads window height; native scrolling replaces overscroll hints.
    view()
        .app_class(tw("relative flex min-h-0 flex-1 flex-col bg-background"))
        .child(
            view()
                .app_class(move || {
                    Some(tw(if window().height.0 < 544.0 {
                        "absolute inset-0 z-20 flex flex-col justify-end"
                    } else {
                        "absolute inset-0 z-20 flex flex-col justify-center"
                    }))
                })
                .child(
                    view()
                        .app_class(tw("w-full px-3 sm:px-12"))
                        .pb(move || px(safe_area().bottom.0.max(keyboard_height().0) + 16.0))
                        .child(
                            view()
                                .app_class(tw(
                                    "relative z-10 mx-auto w-full max-w-(--chat-content-max-width)",
                                ))
                                .child(
                                    view()
                                        .app_class(tw("absolute inset-x-0 bottom-full pb-4"))
                                        .child(dynamic(move || {
                                            let _ = headline_mode.get();
                                            untracked(|| {
                                                draft_hero_headline(current, client, error)
                                            })
                                        })),
                                )
                                .child(surface)
                                .child(dynamic(move || {
                                    let message = attachments
                                        .and_then(ComposerAttachments::error)
                                        .or_else(|| {
                                            draft(current).and_then(|value| value.banner.clone())
                                        });
                                    message.map(|message| {
                                        text(message.resolve())
                                            .app_class(tw("pt-4 text-sm text-muted-foreground"))
                                    })
                                })),
                        ),
                ),
        )
        .into_element()
}

/// Port of chat/DraftHeroHeadline.tsx DraftHeroHeadline; the client owns draft retargeting.
fn draft_hero_headline(
    current: Signal<Rc<NewThreadView>>,
    client: Stored<Rc<dyn NewThreadClient>>,
    error: Signal<Option<Text>>,
) -> AnyElement {
    let nav = navigator();
    let picker = popover_create_handle();
    let query = signal(String::new());
    let trigger = nsh_base::button().accessibility_label(Some(t("Choose a project").into()))
        .on_press(move || {
            if draft(current).is_some_and(|value| !value.projects.is_empty()) { query.set(String::new()); picker.set_open(!picker.is_open()); }
            else { nav.open(Route::AddProject); }
        }).on_layout(move |bounds: &Bounds<Points>| picker.measure_anchor(*bounds))
        .app_class(tw("pointer-events-auto min-w-0 max-w-full cursor-pointer border-muted-foreground/35 border-b border-dotted text-muted-foreground/60 transition-colors hover:border-muted-foreground/60 hover:text-muted-foreground/80 focus-visible:rounded-sm focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .child(text(move || draft(current).map_or_else(|| t("Add a project"), |value| {
            if value.is_scratch { t("No project") } else { value.project_title.clone().unwrap_or_else(|| t("Choose a project")) }
        })));
    let selector = popover(picker, PopoverOptions::default(), trigger, move || {
        view()
            .app_class(tw("w-72 overflow-hidden"))
            .child(crate::ui::input::text_field(
                query,
                t("Search projects..."),
                t("Search projects"),
                crate::ui::input::InputSize::Default,
                || {},
            ))
            .child(scroll().child(dynamic(move || {
                let mut rows = view();
                let Some(value) = draft(current) else {
                    return rows;
                };
                let search = query.get().trim().to_lowercase();
                let mut matched = false;
                for choice in &value.projects {
                    if !search.is_empty() && !choice.title.to_lowercase().contains(&search) {
                        continue;
                    }
                    matched = true;
                    let key = choice.key.clone();
                    rows = rows.child(
                        nsh_base::button()
                            .accessibility_label(Some(choice.title.clone().into()))
                            .app_class(tw("flex min-h-8 items-center gap-2 rounded-sm px-2 py-1.5"))
                            .on_press(move || {
                                picker.set_open(false);
                                let Some(value) = draft(current) else {
                                    return;
                                };
                                if value.project.as_ref() == Some(&key) {
                                    return;
                                }
                                let operation =
                                    client.get().select_project(value.id.clone(), key.clone());
                                spawn(async move {
                                    if let Err(message) = operation.await {
                                        error.set(Some(message));
                                    }
                                });
                            })
                            .child(icon(Icon::Folder, 16.0, || palette().muted_foreground))
                            .child(
                                text(choice.title.clone()).app_class(tw("min-w-0 flex-1 text-sm")),
                            ),
                    );
                }
                if search.is_empty() {
                    rows = rows.child(button(
                        ButtonContent::IconLabel {
                            icon: Icon::Plus,
                            label: t("Add project"),
                        },
                        ButtonVariant::Ghost,
                        ButtonSize::Sm,
                        || false,
                        move || {
                            picker.set_open(false);
                            nav.open(Route::AddProject);
                        },
                    ));
                } else if !matched {
                    rows = rows.child(text(t("No matching projects.")));
                }
                rows
            })))
            .into_element()
    });
    let resolved =
        draft(current).is_some_and(|value| value.project_title.is_some() && !value.is_scratch);
    let scratch = draft(current).is_some_and(|value| value.is_scratch);
    let heading = if resolved {
        draft_hero_project_heading(current, selector.into_element())
    } else if scratch {
        view().child(text(t("What should we work on?"))
            .app_class(tw("w-full text-center font-normal text-2xl text-foreground tracking-tight sm:text-3xl")))
            .child(view().app_class(tw("mt-2 flex h-6 w-full items-center justify-center text-sm")).child(selector)).into_element()
    } else if draft(current).is_some_and(|value| !value.projects.is_empty()) {
        view().app_class(tw("flex w-full items-baseline justify-center gap-2 text-center font-normal text-2xl text-foreground tracking-tight sm:text-3xl"))
            .child(selector).child(text(t("to start"))).into_element()
    } else {
        text(t("Add a project to start")).app_class(tw("w-full text-center font-normal text-2xl text-foreground tracking-tight sm:text-3xl")).into_element()
    };
    view().app_class(tw("mx-auto flex w-full max-w-5xl flex-col items-center"))
        .child(heading)
        .child(dynamic(move || draft(current).filter(|value| value.can_start_scratch && !value.is_scratch).map(|value| {
            let id = value.id.clone();
            nsh_base::button().accessibility_label(Some(t("or start without a project").into()))
                .app_class(tw("pointer-events-auto mt-2 flex h-6 w-full items-center justify-center text-sm"))
                .on_press(move || {
                    let operation = client.get().start_scratch(Some(id.clone()));
                    spawn(async move { if let Err(message) = operation.await { error.set(Some(message)); } });
                }).child(text(t("or start without a project")))
        }))).into_element()
}

/// Port of chat/DraftHeroProjectHeading.tsx DraftHeroProjectHeading.
fn draft_hero_project_heading(
    current: Signal<Rc<NewThreadView>>,
    selector: AnyElement,
) -> AnyElement {
    let available = signal(0.0f32);
    let inline_width = signal(f32::INFINITY);
    let inline = move || window().width.0 >= 1024.0 && inline_width.get() <= available.get();
    view().app_class(tw("relative w-full"))
        .on_layout(move |bounds: &Bounds<Points>| available.set(bounds.size.width.0))
        .child(view().accessible(false).app_class(tw("pointer-events-none invisible absolute inset-x-0 top-0 h-0 overflow-hidden"))
            .child(view().app_class(tw("flex w-max items-baseline gap-2 font-normal text-3xl tracking-tight"))
                .on_layout(move |bounds: &Bounds<Points>| inline_width.set(bounds.size.width.0))
                .child(text(t("What should we build in")))
                .child(text(move || draft(current).and_then(|value| value.project_title.clone()).unwrap_or_default())
                    .app_class(tw("font-medium")))
                .child(text(t("?")))))
        .child(view().app_class(move || Some(tw(if inline() {
            "flex w-full flex-row items-center justify-center gap-2 text-center font-normal text-3xl text-foreground tracking-tight"
        } else {
            "flex w-full flex-col items-center justify-center gap-2 text-center font-normal text-2xl text-foreground tracking-tight sm:text-3xl"
        })))
            .child(dynamic(move || (window().height.0 > 448.0).then(|| text(move || {
                if inline() { t("What should we build in") } else { t("What should we build?") }
            }).app_class(tw("max-w-full")))))
            .child(view().app_class(move || Some(tw(if inline() {
                "flex min-w-0 max-w-full items-baseline justify-center text-3xl tracking-tight"
            } else { "flex min-w-0 max-w-full items-baseline justify-center text-base tracking-normal" })))
                .child(selector)
                .child(dynamic(move || inline().then(|| text(t("?")))))))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submission_rechecks_permission_connectivity_and_busy_states() {
        let mut value = DraftView {
            id: "draft".into(),
            prompt: "keep me".into(),
            cursor: 7,
            revision: 2,
            project: None,
            project_title: None,
            projects: vec![],
            is_scratch: true,
            can_start_scratch: false,
            editable: true,
            actions: PrimaryActionState {
                can_operate_thread: true,
                has_sendable_content: true,
                ..Default::default()
            },
            banner: None,
        };
        assert!(can_submit(&value, false));
        assert!(!can_submit(&value, true));
        value.actions.can_operate_thread = false;
        assert!(!can_submit(&value, false));
        value.actions.can_operate_thread = true;
        value.actions.is_environment_unavailable = true;
        assert!(!can_submit(&value, false));
        value.actions.is_environment_unavailable = false;
        value.actions.send_disabled_reason = Some(Text::client_key("Enter a message."));
        assert!(!can_submit(&value, false));
        value.actions.send_disabled_reason = None;
        value.actions.is_preparing_worktree = true;
        assert!(!can_submit(&value, false));
    }
}
