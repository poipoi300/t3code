//! Port of apps/web/src/components/CommandPalette.tsx add-project and useNewProject.ts.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;

use super::new_thread::Destination;
use crate::i18n::{Text, Value, t, tr};
use crate::state::nav::{Route, navigator};
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{PopupOptions, dialog, dialog_popup};
use crate::ui::icon::{Icon, icon};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Environments,
    Sources,
    LocalFolder,
    NewProject,
    Repository,
    CloneDestination,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectItem {
    pub id: String,
    pub title: Text,
    pub description: Option<Text>,
    pub icon: Icon,
    pub disabled: bool,
    pub setup_required: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectGroup {
    pub label: Text,
    pub items: Vec<ProjectItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Summary {
    New {
        name: String,
        path: String,
        root: String,
        environment: Option<String>,
    },
    Repository {
        title: String,
        url: String,
        icon: Icon,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddProjectView {
    pub revision: u64,
    pub step: Step,
    pub query: String,
    pub placeholder: Text,
    pub groups: Vec<ProjectGroup>,
    pub summary: Option<Summary>,
    pub can_submit: bool,
    pub submit_label: Text,
    pub submit_accessibility: Text,
    pub can_go_back: bool,
    pub folder_picker_label: Option<Text>,
    pub can_pick_folder: bool,
    pub loading_folders: bool,
    pub error: Option<Text>,
    pub empty_message: Option<Text>,
}

/// Adapter for client-owned creation, path resolution, permissions, projection waits and drafts.
pub trait AddProjectClient {
    /// Emits current state first; dropping the stream releases this flow's demand.
    fn watch(&self) -> LocalBoxStream<'static, Rc<AddProjectView>>;
    fn set_query(&self, query: String) -> Result<(), Text>;
    fn select_item(&self, id: &str) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>>;
    fn back(&self) -> Result<(), Text>;
    fn submit(&self, revision: u64) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>>;
    fn pick_folder(
        &self,
        revision: u64,
    ) -> LocalBoxFuture<'static, Result<Option<Destination>, Text>>;
}

#[derive(Clone)]
pub struct AddProjectRuntime(pub Rc<dyn AddProjectClient>);

pub fn screen() -> AnyElement {
    let Some(client) = use_context::<AddProjectRuntime>() else {
        // Frozen Backend has no creation service; no unsupported message is invented here.
        return view().flex_1().into_element();
    };
    command_palette_add_project(client.0)
}

pub fn command_palette_add_project(client: Rc<dyn AddProjectClient>) -> AnyElement {
    let mut changes = client.watch();
    let current = signal(None::<Rc<AddProjectView>>);
    spawn(async move {
        while let Some(next) = changes.next().await {
            current.set(Some(next));
        }
    });
    let client = stored(client);
    let busy = signal(false);
    let error = signal(None::<Text>);
    let open = signal(true);
    let nav = navigator();
    let origin = nav.top();
    let complete = Rc::new(move |result: Result<Option<Destination>, Text>| {
        busy.set(false);
        match result {
            Ok(Some(destination)) if nav.top() == origin => {
                open.set(false);
                nav.open(destination.route());
            }
            Ok(_) => {}
            Err(message) => error.set(Some(message)),
        }
    });
    let submit = {
        let complete = complete.clone();
        Rc::new(move || {
            let Some(value) = current.get_untracked() else {
                return;
            };
            if busy.get_untracked() || !value.can_submit {
                return;
            }
            busy.set(true);
            error.set(None);
            let operation = client.get().submit(value.revision);
            let complete = complete.clone();
            spawn(async move {
                complete(operation.await);
            });
        })
    };
    let input_ref = element_ref();
    dialog(open, move |state| {
        state.on_open_change(move |next| {
            open.set(next);
            if !next && nav.top() == Route::AddProject { nav.back(); }
        });
        dialog_popup(state, PopupOptions { show_close_button: false, initial_focus: Some(input_ref), ..Default::default() }, move || {
            let input_submit = submit.clone();
            let button_submit = submit.clone();
            let completed = complete.clone();
            view().label(t("Command palette"))
                .app_class(tw("flex min-h-0 w-full flex-col overflow-hidden"))
                .child(view().app_class(tw("relative flex items-center gap-2 px-4 py-3"))
                    .child(dynamic(move || current.get().filter(|value| value.can_go_back).map(|_| {
                        button(ButtonContent::Icon { icon: Icon::ArrowLeft, label: t("Back") }, ButtonVariant::Ghost, ButtonSize::IconSm,
                            move || busy.get(), move || { if !busy.get_untracked() && let Err(message) = client.get().back() { error.set(Some(message)); } })
                    })))
                    .child(view().app_class(tw("min-w-0 flex-1"))
                        .child(nsh_base::input().element_ref(&input_ref)
                            .label(t("Command palette"))
                            .value(move || current.get().map_or_else(String::new, |value| value.query.clone()))
                            .placeholder(move || current.get().map_or_else(String::new, |value| value.placeholder.resolve().to_string()))
                            .on_change(move |query: &str| {
                                if !busy.get_untracked() && let Err(message) = client.get().set_query(query.into()) { error.set(Some(message)); }
                            })
                            .on_submit(move |_: &str| input_submit())
                            .submit_behavior(SubmitBehavior::Submit)
                            .text_color(move || palette().foreground)
                            .app_class(tw("w-full bg-transparent text-base sm:text-sm outline-none"))))
                    .child(dynamic(move || {
                        let submit = button_submit.clone();
                        current.get().filter(|value| matches!(value.step, Step::LocalFolder | Step::NewProject | Step::Repository | Step::CloneDestination)).map(|value| {
                            command_palette_submit_button(value.submit_label.clone(), value.submit_accessibility.clone(),
                                move || busy.get() || current.get().is_none_or(|value| !value.can_submit), move || submit())
                        })
                    })))
                .child(dynamic(move || current.get().and_then(|value| value.summary.clone()).map(|summary| match summary {
                    Summary::New { name, path, root, environment } => new_project_summary(&name, &path, &root, environment.as_deref()),
                    Summary::Repository { title, url, icon } => remote_repository_summary(&title, &url, icon),
                })))
                .child(dynamic(move || {
                    let value = current.get();
                    let message = error.get().or_else(|| value.as_ref().and_then(|value| value.error.clone()));
                    message.map(|message| text(message.resolve()).role(AccessibilityRole::Alert)
                        .app_class(tw("px-4 py-3 text-sm text-muted-foreground")))
                }))
                .child(dynamic(move || current.get().filter(|value| value.loading_folders).map(|_| {
                    text(t("Loading folders...")).app_class(tw("px-4 py-3 text-sm text-muted-foreground"))
                })))
                .child(scroll().app_class(tw("min-h-0 max-h-80"))
                    .child(dynamic(move || {
                        let mut groups = view().app_class(tw("px-2 py-2"));
                        let Some(value) = current.get() else { return groups; };
                        for group in &value.groups {
                            groups = groups.child(text(group.label.resolve()).app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs")));
                            for item in &group.items {
                                let id = item.id.clone();
                                let complete = completed.clone();
                                let id_for_disabled = id.clone();
                                let row = nsh_base::button().accessibility_label(Some(item.title.resolve()))
                                    .disabled(move || busy.get() || !item_available(current.get().as_deref(), &id_for_disabled))
                                    .on_press(move || {
                                        if busy.get_untracked() || !item_available(current.get_untracked().as_deref(), &id) { return; }
                                        busy.set(true); error.set(None);
                                        let operation = client.get().select_item(&id);
                                        let complete = complete.clone();
                                        spawn(async move { complete(operation.await); });
                                    })
                                    .app_class(tw("flex min-h-8 cursor-default select-none items-center gap-2 rounded-sm px-2 py-1 text-base outline-none hover:bg-accent disabled:opacity-64 sm:min-h-7 sm:text-sm"))
                                    .child(view().app_class(tw("flex h-5 shrink-0 items-center self-start pt-0.5"))
                                        .child(icon(item.icon, 16.0, || palette().muted_foreground)))
                                    .child(view().app_class(tw("flex min-w-0 flex-1 flex-col"))
                                        .child(text(item.title.resolve()).app_class(tw("flex min-w-0 items-center gap-1.5 text-sm text-foreground")))
                                        .children(item.description.clone().map(|description| text(description.resolve()).app_class(tw("min-w-0 text-muted-foreground/70 text-xs")))));
                                groups = groups.child(row);
                            }
                        }
                        if value.groups.iter().all(|group| group.items.is_empty()) && let Some(message) = &value.empty_message {
                            groups = groups.child(text(message.resolve()).app_class(tw("py-10 text-center text-sm text-muted-foreground")));
                        }
                        groups
                    })))
                .child(dynamic(move || {
                    let complete = complete.clone();
                    current.get().and_then(|value| value.folder_picker_label.clone()).map(|label| {
                        button(ButtonContent::Label(label.resolve().to_string()), ButtonVariant::GhostMuted, ButtonSize::Xs,
                            move || busy.get() || current.get().is_none_or(|value| !value.can_pick_folder), move || {
                                let Some(value) = current.get_untracked().filter(|value| value.can_pick_folder) else { return; };
                                if busy.get_untracked() { return; }
                                busy.set(true); error.set(None);
                                let operation = client.get().pick_folder(value.revision);
                                let complete = complete.clone();
                                spawn(async move { complete(operation.await); });
                            })
                    })
                })).into_element()
        })
    }).into_element()
}

fn item_available(value: Option<&AddProjectView>, id: &str) -> bool {
    value.is_some_and(|value| {
        value
            .groups
            .iter()
            .flat_map(|group| &group.items)
            .any(|item| item.id == id && !item.disabled)
    })
}

/// Port of CommandPaletteAddProject.tsx CommandPaletteSubmitButton.
fn command_palette_submit_button(
    label: Text,
    aria_label: Text,
    disabled: impl Fn() -> bool + 'static,
    submit: impl Fn() + 'static,
) -> impl IntoElement {
    // Button has no child/keycap slot; the Enter keycap awaits the UI-BUTTON content seam.
    let _ = aria_label;
    button(
        ButtonContent::Label(label.resolve().to_string()),
        ButtonVariant::Outline,
        ButtonSize::Xs,
        disabled,
        submit,
    )
}

/// Port of CommandPaletteAddProject.tsx NewProjectSummary.
fn new_project_summary(
    name: &str,
    path: &str,
    root: &str,
    environment: Option<&str>,
) -> AnyElement {
    let title = if name.is_empty() {
        Text::from("New project")
    } else {
        Text::content(name.to_owned())
    };
    let description = match (name.is_empty(), environment) {
        (false, None) => tr("Creates {path}", &[("path", Value::from(path))]),
        (false, Some(environment)) => tr(
            "Creates {path} on {environment}",
            &[
                ("path", Value::from(path)),
                ("environment", Value::from(environment)),
            ],
        ),
        (true, None) => tr("Goes in {path}", &[("path", Value::from(root))]),
        (true, Some(environment)) => tr(
            "Goes in {path} on {environment}",
            &[
                ("path", Value::from(root)),
                ("environment", Value::from(environment)),
            ],
        ),
    };
    view()
        .app_class(tw("p-2 pb-0"))
        .child(summary_row(
            title,
            Text::content(description),
            Icon::FolderGit2,
        ))
        .into_element()
}

/// Port of CommandPaletteAddProject.tsx RemoteRepositorySummary.
fn remote_repository_summary(title: &str, url: &str, glyph: Icon) -> AnyElement {
    view()
        .app_class(tw("shrink-0 p-2 pb-0"))
        .child(
            text(t("Repository"))
                .app_class(tw("px-2 py-1.5 font-medium text-muted-foreground text-xs")),
        )
        .child(summary_row(
            Text::content(title.to_owned()),
            Text::content(url.to_owned()),
            glyph,
        ))
        .into_element()
}

fn summary_row(title: Text, description: Text, glyph: Icon) -> AnyElement {
    view()
        .app_class(tw("flex min-h-8 items-center gap-2 rounded-sm px-2 py-1.5"))
        .child(
            view()
                .app_class(tw("flex h-5 shrink-0 items-center self-start pt-0.5"))
                .child(icon(glyph, 16.0, || palette().muted_foreground)),
        )
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-1 flex-col"))
                .child(text(title.resolve()).app_class(tw("truncate text-foreground text-sm")))
                .child(
                    text(description.resolve())
                        .app_class(tw("truncate text-muted-foreground/85 text-xs")),
                ),
        )
        .into_element()
}
