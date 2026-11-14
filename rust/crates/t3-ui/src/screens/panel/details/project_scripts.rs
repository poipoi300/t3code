//! Port of apps/web/src/components/ProjectScriptsControl.tsx ProjectScriptsControl with
//! displayMode="panel", and projectScriptEditor.tsx ProjectScriptEditorDialog.

use std::rc::Rc;

use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::view_logic::shell::keybindings::shortcut_label_for_command;
use t3_client::workspace::git_panel::scripts::{
    NewProjectScriptInput, ProjectScriptsSnapshot, import_input, script_keybinding_value,
};
use t3_client::workspace::project_ops::scripts::{ProjectScriptInput, primary_project_script};
use t3_contracts::base_schemas::TrimmedNonEmptyString;
use t3_contracts::project::{ProjectScript, ProjectScriptIcon};
use t3_contracts::t3_project_file::T3ProjectFileScript;

use crate::i18n::{Value, t, tr};
use crate::screens::panel::git_actions::thread_details_control;
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{self, ActionVariant, FooterVariant, PopupOptions};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{self, ItemOptions, MenuOptions};
use crate::ui::popover::{PopoverOptions, popover, popover_create_handle};
use crate::ui::switch::{ControlState, Size as SwitchSize, switch};
use crate::ui::toast::{self, ToastOptions};
use crate::ui::tooltip::{Variant as TooltipVariant, tooltip_popup};

/// The client's `ProjectScripts`, bound by delegation in `runtime/services/git_panel.rs`.
pub trait ProjectScriptsBackend {
    /// The current snapshot first, then after every change.
    fn watch(&self) -> LocalBoxStream<'static, Result<ProjectScriptsSnapshot, ClientError>>;
    fn add(&self, input: NewProjectScriptInput) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn update(
        &self,
        id: String,
        input: NewProjectScriptInput,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn delete(&self, id: String) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn import(&self, script: T3ProjectFileScript)
    -> LocalBoxFuture<'static, Result<(), ClientError>>;
    /// Opens a terminal running the script and answers its terminal id.
    fn run(&self, id: String) -> LocalBoxFuture<'static, Result<String, ClientError>>;
}

type Backend = Stored<Rc<dyn ProjectScriptsBackend>>;
type Snapshot = Signal<Option<Rc<ProjectScriptsSnapshot>>>;

/// The editor form's fields (`NewProjectScriptInput` in projectScriptEditor.tsx).
#[derive(Clone, Debug, PartialEq)]
struct ScriptForm {
    name: String,
    command: String,
    icon: ProjectScriptIcon,
    run_on_worktree_create: bool,
    wait_for_setup: bool,
    run_on_settle: bool,
    stop_on_settle: bool,
    keybinding: Option<String>,
    preview_url: Option<String>,
    auto_open_preview: bool,
}

impl Default for ScriptForm {
    /// `EMPTY_PROJECT_SCRIPT_INPUT`.
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            icon: ProjectScriptIcon::Play,
            run_on_worktree_create: false,
            wait_for_setup: false,
            run_on_settle: false,
            stop_on_settle: false,
            keybinding: None,
            preview_url: None,
            auto_open_preview: false,
        }
    }
}

/// `ProjectScriptEditorRequest`.
#[derive(Clone, Debug, PartialEq)]
struct EditorRequest {
    script_id: Option<String>,
    initial: ScriptForm,
    error: Option<String>,
}

/// `editorRequestForScript`.
fn editor_request_for_script(
    script: &ProjectScript,
    snapshot: &ProjectScriptsSnapshot,
) -> EditorRequest {
    EditorRequest {
        script_id: Some(script.id.0.clone()),
        initial: ScriptForm {
            name: script.name.0.clone(),
            command: script.command.0.clone(),
            icon: script.icon.clone(),
            run_on_worktree_create: script.run_on_worktree_create,
            wait_for_setup: script.run_on_worktree_create && script.r#async.flatten() == Some(false),
            run_on_settle: script.run_on_settle.flatten().unwrap_or(false),
            stop_on_settle: script.stop_on_settle.flatten().unwrap_or(false),
            keybinding: script_keybinding_value(&snapshot.keybindings, &script.id.0),
            preview_url: script.preview_url.clone().flatten().map(|u| u.0),
            auto_open_preview: script.auto_open_preview.flatten().unwrap_or(false),
        },
        error: None,
    }
}

/// The importer's payload as an editor request, kept when the import fails.
fn request_from_input(input: NewProjectScriptInput, error: String) -> EditorRequest {
    let script = input.script;
    EditorRequest {
        script_id: None,
        initial: ScriptForm {
            name: script.name.0,
            command: script.command.0,
            icon: script.icon,
            run_on_worktree_create: script.run_on_worktree_create,
            wait_for_setup: script.wait_for_setup,
            run_on_settle: script.run_on_settle,
            stop_on_settle: script.stop_on_settle,
            keybinding: input.keybinding.flatten(),
            preview_url: script.preview_url.map(|u| u.0),
            auto_open_preview: script.auto_open_preview,
        },
        error: Some(error),
    }
}

/// The form's submit validation and payload (`ProjectScriptEditorForm.submit`).
fn submit_payload(
    request: &EditorRequest,
    form: &ScriptForm,
    can_edit: bool,
    can_change_keybinding: bool,
) -> Result<NewProjectScriptInput, String> {
    if !can_edit {
        return Err(t("This connection cannot change project actions."));
    }
    let trimmed = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let keybinding = trimmed(&form.keybinding);
    let changes_keybinding = keybinding != trimmed(&request.initial.keybinding);
    if changes_keybinding && !can_change_keybinding {
        return Err(t("This connection cannot change keyboard shortcuts."));
    }
    let name = form.name.trim();
    let command = form.command.trim();
    if name.is_empty() {
        return Err(t("Name is required."));
    }
    if command.is_empty() {
        return Err(t("Command is required."));
    }
    let preview_url = trimmed(&form.preview_url);
    Ok(NewProjectScriptInput {
        script: ProjectScriptInput {
            name: TrimmedNonEmptyString(name.to_owned()),
            command: TrimmedNonEmptyString(command.to_owned()),
            icon: form.icon.clone(),
            run_on_worktree_create: form.run_on_worktree_create,
            wait_for_setup: form.run_on_worktree_create && form.wait_for_setup,
            run_on_settle: form.run_on_settle,
            stop_on_settle: form.stop_on_settle,
            auto_open_preview: preview_url.is_some() && form.auto_open_preview,
            preview_url: preview_url.map(TrimmedNonEmptyString),
        },
        // Omitted keeps the current shortcut when the form did not edit it.
        keybinding: ((request.script_id.is_none() && can_change_keybinding) || changes_keybinding)
            .then_some(keybinding),
    })
}

/// `projectScriptMenuLabel`.
fn project_script_menu_label(script: &ProjectScript) -> String {
    let mut roles = Vec::new();
    if script.run_on_worktree_create {
        roles.push(t("setup"));
    }
    if script.run_on_settle.flatten() == Some(true) {
        roles.push(t("on settle"));
    }
    if roles.is_empty() {
        script.name.0.clone()
    } else {
        tr(
            "{name} ({roles})",
            &[
                ("name", Value::Text(script.name.0.clone())),
                ("roles", Value::Text(roles.join(", "))),
            ],
        )
    }
}

/// `primaryScript`: the last-run script, else the client's primary script.
fn primary_script(snapshot: &ProjectScriptsSnapshot) -> Option<ProjectScript> {
    snapshot
        .preferred_script_id
        .as_ref()
        .and_then(|id| snapshot.scripts.iter().find(|s| &s.id.0 == id))
        .or_else(|| primary_project_script(&snapshot.scripts))
        .cloned()
}

fn shortcut_label(snapshot: &ProjectScriptsSnapshot, script_id: &str) -> Option<String> {
    // The keybinding host's convention (keys/bindings.rs `shortcut_platform`).
    let platform = if apple_keys() { "MacIntel" } else { "Linux" };
    shortcut_label_for_command(
        &snapshot.keybindings.0,
        Some(&format!("script.{script_id}.run")),
        platform,
        &Default::default(),
    )
}

/// `ScriptIcon`.
fn script_icon(icon: ProjectScriptIcon, class: &'static str) -> impl IntoElement {
    icon_with_class(
        match icon {
            ProjectScriptIcon::Test => Icon::FlaskConical,
            ProjectScriptIcon::Lint => Icon::ListChecks,
            ProjectScriptIcon::Configure => Icon::Wrench,
            ProjectScriptIcon::Build => Icon::Hammer,
            ProjectScriptIcon::Debug => Icon::Bug,
            ProjectScriptIcon::Play => Icon::Play,
        },
        class,
    )
}

const SCRIPT_ICONS: [(ProjectScriptIcon, &str); 6] = [
    (ProjectScriptIcon::Play, "Play"),
    (ProjectScriptIcon::Test, "Test"),
    (ProjectScriptIcon::Lint, "Lint"),
    (ProjectScriptIcon::Configure, "Configure"),
    (ProjectScriptIcon::Build, "Build"),
    (ProjectScriptIcon::Debug, "Debug"),
];

fn show_error(title: String, error: ClientError) {
    if matches!(error, ClientError::Cancelled) {
        return;
    }
    toast::toast_manager().add(toast::stacked_thread_toast(ToastOptions {
        variant: toast::Variant::Error,
        title,
        description: Some(error.to_string()),
        ..Default::default()
    }));
}

fn run_script(backend: Backend, snapshot: Snapshot, script: ProjectScript) {
    if !snapshot.with_untracked(|s| s.as_ref().is_some_and(|s| s.can_run)) {
        return;
    }
    let backend = backend.get();
    spawn(async move {
        if let Err(error) = backend.run(script.id.0.clone()).await {
            // The source reports this on the thread's error banner (useChatTerminalActions.ts).
            show_error(
                tr("Failed to run script \"{name}\".", &[("name", Value::Text(script.name.0))]),
                error,
            );
        }
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Primary,
    Actions,
    Add,
}

pub(super) fn project_scripts_control(backend: Backend, snapshot: Snapshot) -> impl IntoElement {
    let request = signal::<Option<EditorRequest>>(None);
    let scripts_open = signal(false);
    let mode = computed(move || {
        snapshot.with(|s| match s.as_deref() {
            Some(s) if primary_script(s).is_some() => Mode::Primary,
            Some(s) if !s.scripts.is_empty() || !s.importable_scripts.is_empty() => Mode::Actions,
            _ => Mode::Add,
        })
    });
    view()
        .child(dynamic(move || {
            let mode = mode.get();
            Some(untracked(|| match mode {
                Mode::Primary => primary_group(backend, snapshot, request, scripts_open).into_element(),
                Mode::Actions => actions_group(backend, snapshot, request, scripts_open).into_element(),
                Mode::Add => add_row(request).into_element(),
            }))
        }))
        .child(project_script_editor_dialog(backend, snapshot, request))
}

fn split_group(open: Signal<bool>, label: String) -> View {
    view()
        .role(AccessibilityRole::Group)
        .label(label)
        .app_class(move || {
            Some(tw(if open.get() {
                "group/details-split flex w-full items-center rounded-lg hover:!bg-black/[0.035] dark:hover:!bg-white/[0.08] bg-black/[0.035] dark:bg-white/[0.08]"
            } else {
                "group/details-split flex w-full items-center rounded-lg hover:!bg-black/[0.035] dark:hover:!bg-white/[0.08]"
            }))
        })
}

fn split_separator(open: Signal<bool>) -> impl IntoElement {
    view().app_class(move || {
        Some(tw(if open.get() {
            "h-4 w-px shrink-0 bg-border/65"
        } else {
            "h-4 w-px shrink-0 bg-transparent group-hover/details-split:bg-border/65"
        }))
    })
}

fn scripts_menu(
    backend: Backend,
    snapshot: Snapshot,
    request: Signal<Option<EditorRequest>>,
    open: Signal<bool>,
    label: &'static str,
) -> impl IntoElement {
    menu::menu(
        thread_details_control(
            "secondary",
            t(label).into(),
            false.into(),
            false,
            icon_with_class(Icon::ChevronDown, "size-4 shrink-0 text-muted-foreground"),
            || {},
        ),
        MenuOptions {
            open: Some(open),
            align: Align::End,
            ..Default::default()
        },
        menu::menu_popup_with(move || script_items(backend, snapshot, request, open)),
    )
}

fn primary_group(
    backend: Backend,
    snapshot: Snapshot,
    request: Signal<Option<EditorRequest>>,
    open: Signal<bool>,
) -> impl IntoElement {
    let primary = computed(move || snapshot.with(|s| s.as_deref().and_then(primary_script)));
    let can_run = computed(move || snapshot.with(|s| s.as_ref().is_some_and(|s| s.can_run)));
    let name = move || primary.with(|p| p.as_ref().map(|p| p.name.0.clone()).unwrap_or_default());
    split_group(open, t("Project scripts"))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .tooltip(
                    tooltip_popup(
                        TooltipVariant::Default,
                        text(move || {
                            if can_run.get() {
                                tr("Run {name}", &[("name", Value::Text(name()))])
                            } else {
                                t("Pair this client again with permission to run terminal commands.")
                            }
                        }),
                    )
                    .into_element(),
                )
                // aria-disabled keeps the control focusable for its tooltip; the press is inert.
                .child(thread_details_control(
                    "primary",
                    Prop::bind(move || tr("Run {name}", &[("name", Value::Text(name()))]).into()),
                    false.into(),
                    false,
                    view()
                        .app_class(move || Some(tw(if can_run.get() { "flex min-w-0 items-center gap-2.5" } else { "flex min-w-0 items-center gap-2.5 opacity-64" })))
                        .child(dynamic(move || {
                            primary.with(|p| p.as_ref().map(|p| p.icon.clone())).map(|icon| script_icon(icon, "size-4 shrink-0 text-muted-foreground"))
                        }))
                        .child(text(move || name()).app_class(tw("ml-0 truncate"))),
                    move || {
                        if let Some(script) = primary.get_untracked() {
                            run_script(backend, snapshot, script);
                        }
                    },
                )),
        )
        .child(split_separator(open))
        .child(
            view()
                .tooltip(tooltip_popup(TooltipVariant::Default, text(t("Script actions"))).into_element())
                .child(scripts_menu(backend, snapshot, request, open, "Script actions")),
        )
}

fn actions_group(
    backend: Backend,
    snapshot: Snapshot,
    request: Signal<Option<EditorRequest>>,
    open: Signal<bool>,
) -> impl IntoElement {
    split_group(open, t("Project actions"))
        .child(thread_details_control(
            "primary",
            t("Project actions").into(),
            false.into(),
            false,
            view()
                .app_class(tw("flex min-w-0 items-center gap-2.5"))
                .child(icon_with_class(Icon::Wrench, "size-4 shrink-0 text-muted-foreground"))
                .child(text(t("Actions")).app_class(tw("min-w-0 truncate"))),
            move || open.set(true),
        ))
        .child(split_separator(open))
        .child(scripts_menu(backend, snapshot, request, open, "Choose project action"))
}

fn add_row(request: Signal<Option<EditorRequest>>) -> impl IntoElement {
    view()
        .tooltip(tooltip_popup(TooltipVariant::Default, text(t("Add project script"))).into_element())
        .child(thread_details_control(
            "row",
            t("Add project script").into(),
            false.into(),
            false,
            view()
                .app_class(tw("flex min-w-0 items-center gap-2.5"))
                .child(icon_with_class(Icon::Plus, "size-3.5"))
                .child(text(t("Add project script")).app_class(tw("ml-0.5"))),
            move || request.set(Some(EditorRequest { script_id: None, initial: ScriptForm::default(), error: None })),
        ))
}

/// `scriptItems`, including the t3.json imports and the add item.
fn script_items(
    backend: Backend,
    snapshot: Snapshot,
    request: Signal<Option<EditorRequest>>,
    open: Signal<bool>,
) -> AnyElement {
    let Some(current) = snapshot.get_untracked() else {
        return view().into_element();
    };
    let mut items: Vec<AnyElement> = Vec::new();
    for script in &current.scripts {
        let label = project_script_menu_label(script);
        let shortcut = shortcut_label(&current, &script.id.0);
        let run = script.clone();
        let edit = script.clone();
        let edit_snapshot = current.clone();
        items.push(
            menu::menu_item(
                label.clone(),
                ItemOptions {
                    disabled: (!current.can_run).into(),
                    ..Default::default()
                },
                view()
                    .app_class(tw("flex w-full items-center gap-2"))
                    .child(script_icon(script.icon.clone(), "size-4"))
                    .child(menu::menu_item_label(label))
                    .child(
                        view()
                            .app_class(tw("relative ms-auto flex h-6 min-w-6 items-center justify-end gap-1"))
                            .child(shortcut.map(menu::menu_shortcut))
                            // Hover-only reveal needs the row's hover state; the button stays shown.
                            .child(button(
                                ButtonContent::Icon {
                                    icon: Icon::Settings,
                                    label: tr("Edit {name}", &[("name", Value::Text(script.name.0.clone()))]),
                                },
                                ButtonVariant::Ghost,
                                ButtonSize::IconXs,
                                || false,
                                move || {
                                    open.set(false);
                                    request.set(Some(editor_request_for_script(&edit, &edit_snapshot)));
                                },
                            )),
                    ),
                move || run_script(backend, snapshot, run.clone()),
            )
            .into_element(),
        );
    }
    if !current.importable_scripts.is_empty() {
        if !current.scripts.is_empty() {
            items.push(menu::menu_separator().into_element());
        }
        let mut imports: Vec<AnyElement> = vec![menu::menu_group_label(t("From t3.json"), false).into_element()];
        for file in &current.importable_scripts {
            let script = file.clone();
            let can_edit_keybindings = current.can_edit_keybindings;
            imports.push(
                menu::menu_item(
                    file.name.0.clone(),
                    ItemOptions::default(),
                    view()
                        .app_class(tw("flex w-full items-center gap-2"))
                        .child(script_icon(file.icon.clone().unwrap_or(ProjectScriptIcon::Play), "size-4"))
                        .child(menu::menu_item_label(file.name.0.clone()))
                        .child(
                            view()
                                .app_class(tw("ms-auto"))
                                .label(t("Import"))
                                .child(icon_with_class(Icon::Download, "size-3.5")),
                        ),
                    move || {
                        let backend = backend.get();
                        let script = script.clone();
                        spawn(async move {
                            let payload = import_input(script.clone(), can_edit_keybindings);
                            match backend.import(script).await {
                                Ok(()) | Err(ClientError::Cancelled) => {}
                                Err(error) => {
                                    request.set(Some(request_from_input(payload, error.to_string())));
                                }
                            }
                        });
                    },
                )
                .into_element(),
            );
        }
        items.push(menu::menu_group(t("From t3.json"), view().child(imports)).into_element());
    }
    items.push(
        menu::menu_item(
            t("Add project script"),
            ItemOptions::default(),
            view()
                .app_class(tw("flex w-full items-center gap-2"))
                .child(icon_with_class(Icon::Plus, "size-4"))
                .child(menu::menu_item_label(t("Add project script"))),
            move || request.set(Some(EditorRequest { script_id: None, initial: ScriptForm::default(), error: None })),
        )
        .into_element(),
    );
    view().child(items).into_element()
}

/// Port of projectScriptEditor.tsx ProjectScriptEditorDialog.
fn project_script_editor_dialog(
    backend: Backend,
    snapshot: Snapshot,
    request: Signal<Option<EditorRequest>>,
) -> impl IntoElement {
    let open = signal(false);
    let delete_open = signal(false);
    let form = signal(ScriptForm::default());
    let error = signal::<Option<String>>(None);
    let saving = signal(false);
    // Each request mounts a fresh form; a late save from an older request is dropped.
    let generation = signal(0_u64);
    let can_edit = computed(move || snapshot.with(|s| s.as_ref().is_some_and(|s| s.can_edit)));
    let can_edit_keybindings =
        computed(move || snapshot.with(|s| s.as_ref().is_some_and(|s| s.can_edit_keybindings)));
    effect(move || {
        let next = request.get();
        untracked(|| {
            generation.update(|g| *g += 1);
            saving.set(false);
            match next {
                Some(next) => {
                    form.set(next.initial.clone());
                    error.set(next.error.clone());
                    open.set(true);
                }
                None => open.set(false),
            }
        });
    });
    let close = move || {
        if request.get_untracked().is_some() {
            request.set(None);
        }
    };
    let editing = move || request.with(|r| r.as_ref().is_some_and(|r| r.script_id.is_some()));
    let submit = move || {
        if saving.get_untracked() {
            return;
        }
        let Some(current) = request.get_untracked() else {
            return;
        };
        let payload = match submit_payload(
            &current,
            &form.get_untracked(),
            can_edit.get_untracked(),
            can_edit_keybindings.get_untracked(),
        ) {
            Ok(payload) => payload,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        error.set(None);
        saving.set(true);
        let mine = generation.get_untracked();
        let backend = backend.get();
        spawn(async move {
            let result = match current.script_id {
                Some(id) => backend.update(id, payload).await,
                None => backend.add(payload).await,
            };
            if generation.get_untracked() != mine {
                return;
            }
            saving.set(false);
            match result {
                Ok(()) => request.set(None),
                Err(ClientError::Cancelled) => {}
                Err(failure) => error.set(Some(failure.to_string())),
            }
        });
    };
    let fields_disabled = move || saving.get() || !can_edit.get();
    view()
        .child(dialog::dialog(open, move |state| {
            state.on_open_change(move |next| {
                if !next {
                    close();
                }
            });
            dialog::dialog_popup(state, PopupOptions::default(), move || {
                view()
                    .child(dialog::dialog_header(state, view()
                        .child(dialog::dialog_title(state, move || (if editing() { "Edit Action" } else { "Add Action" }).into()))
                        .child(dialog::dialog_description(state, || "Actions are project-scoped commands you can run from the top bar or keybindings.".into()))))
                    .child(dialog::dialog_panel(state, view().app_class(tw("space-y-4"))
                        .child(field(t("Name"), view().app_class(tw("flex items-center gap-2"))
                            .child(icon_picker(form, Prop::bind(fields_disabled)))
                            .child(text_input("script-name", t("Name"), t("Test"), Prop::bind(fields_disabled), false,
                                move || form.with(|f| f.name.clone()), move |value| form.update(|f| f.name = value)))))
                        .child(field(t("Keybinding"), view()
                            // The source captures a pressed shortcut; this input takes its text form.
                            .child(text_input("script-keybinding", t("Keybinding"), t("Press shortcut"), Prop::bind(move || fields_disabled() || !can_edit_keybindings.get()), false,
                                move || form.with(|f| f.keybinding.clone().unwrap_or_default()), move |value| form.update(|f| f.keybinding = Some(value))))
                            .child(text(t("Press a shortcut. Use Backspace to clear. Shortcuts are environment-wide. Projects using the same action share its shortcut.")).app_class(tw("text-xs text-muted-foreground")))))
                        .child(field(t("Command"), text_input("script-command", t("Command"), t("bun test"), Prop::bind(fields_disabled), true,
                            move || form.with(|f| f.command.clone()), move |value| form.update(|f| f.command = value))))
                        .child(field(t("Preview URL (optional)"), view()
                            .child(text_input("script-preview-url", t("Preview URL (optional)"), t("http://localhost:5173"), Prop::bind(fields_disabled), false,
                                move || form.with(|f| f.preview_url.clone().unwrap_or_default()), move |value| form.update(|f| f.preview_url = Some(value))))
                            .child(text(t("Open this URL in the in-app preview when this action runs.")).app_class(tw("text-xs text-muted-foreground")))))
                        .child(switch_row("Run automatically on worktree creation", Prop::bind(move || form.with(|f| f.run_on_worktree_create)), Prop::bind(fields_disabled), false.into(), move |v| form.update(|f| f.run_on_worktree_create = v)))
                        .child(switch_row("Wait for it to finish before the agent starts", Prop::bind(move || form.with(|f| f.wait_for_setup)), Prop::bind(move || fields_disabled() || !form.with(|f| f.run_on_worktree_create)), Prop::bind(move || !form.with(|f| f.run_on_worktree_create)), move |v| form.update(|f| f.wait_for_setup = v)))
                        .child(switch_row("Run in the thread's worktree when the thread settles", Prop::bind(move || form.with(|f| f.run_on_settle)), Prop::bind(fields_disabled), false.into(), move |v| form.update(|f| f.run_on_settle = v)))
                        .child(switch_row("Stop this action when the thread settles", Prop::bind(move || form.with(|f| f.stop_on_settle)), Prop::bind(fields_disabled), false.into(), move |v| form.update(|f| f.stop_on_settle = v)))
                        .child(switch_row("Open preview automatically when this action runs", Prop::bind(move || form.with(|f| f.auto_open_preview)), Prop::bind(move || fields_disabled() || form.with(|f| f.preview_url.as_deref().is_none_or(|u| u.trim().is_empty()))), Prop::bind(move || form.with(|f| f.preview_url.as_deref().is_none_or(|u| u.trim().is_empty()))), move |v| form.update(|f| f.auto_open_preview = v)))
                        .child(dynamic(move || error.get().map(|message| text(message).app_class(tw("text-sm text-destructive")).into_element())))))
                    .child(dialog::dialog_footer(state, FooterVariant::Bare, view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end"))
                        .child(when(editing, move || view().app_class(tw("mr-auto")).child(button(ButtonContent::Label(t("Delete")), ButtonVariant::DestructiveOutline, ButtonSize::Default,
                            move || saving.get() || !can_edit.get(), move || delete_open.set(true)))))
                        .child(button(ButtonContent::Label(t("Cancel")), ButtonVariant::Outline, ButtonSize::Default, || false, close))
                        .child(dialog::dialog_action(state, ButtonContent::Custom {
                                label: t(if editing() { "Save changes" } else { "Save action" }),
                                child: text(move || t(if saving.get() { "Saving…" } else if editing() { "Save changes" } else { "Save action" })).into_element(),
                            },
                            ActionVariant::Default, ButtonSize::Default, move || saving.get() || !can_edit.get(), submit))))
            })
        }))
        .child(delete_confirmation(backend, request, form, error, delete_open, can_edit))
}

fn delete_confirmation(
    backend: Backend,
    request: Signal<Option<EditorRequest>>,
    form: Signal<ScriptForm>,
    error: Signal<Option<String>>,
    open: Signal<bool>,
    can_edit: Memo<bool>,
) -> impl IntoElement {
    // useComposerMenuState closes the confirmation when the grant goes away.
    effect(move || {
        if !can_edit.get() {
            open.set(false);
        }
    });
    dialog::alert_dialog(open, move |state| {
        dialog::alert_dialog_popup(state, Default::default(), move || {
            view()
                .child(dialog::alert_dialog_header(state, view()
                    .child(dialog::alert_dialog_title(state, move || tr("Delete action \"{name}\"?", &[("name", Value::Text(form.with(|f| f.name.clone())))])))
                    .child(dialog::dialog_description(state, || "This action cannot be undone.".into()))))
                .child(dialog::alert_dialog_footer(state, FooterVariant::Default, view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end"))
                    .child(dialog::dialog_close(state, ButtonContent::Label(t("Cancel")), ButtonVariant::Outline, ButtonSize::Default, || false))
                    .child(button(ButtonContent::Label(t("Delete action")), ButtonVariant::Destructive, ButtonSize::Default, move || !can_edit.get(), move || {
                        let Some(id) = request.with_untracked(|r| r.as_ref().and_then(|r| r.script_id.clone())) else {
                            return;
                        };
                        if !can_edit.get_untracked() {
                            error.set(Some(t("This connection cannot change project actions.")));
                            return;
                        }
                        open.set(false);
                        request.set(None);
                        let backend = backend.get();
                        spawn(async move {
                            if let Err(failure) = backend.delete(id).await {
                                show_error(t("Failed to delete action."), failure);
                            }
                        });
                    }))))
        })
    })
}

fn field(label: String, control: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("space-y-1.5"))
        .child(text(label).app_class(tw("text-sm font-medium")))
        .child(control)
}

fn text_input(
    id: &'static str,
    label: String,
    placeholder: String,
    disabled: Prop<bool>,
    multiline: bool,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    // UI-FORM has no Textarea yet; the NSH multiline input keeps its contents.
    nsh_base::input()
        .test_id(id)
        .label(label)
        .placeholder(placeholder)
        .multiline(multiline)
        .disabled(disabled)
        .value(value)
        .on_change(move |next: &str| on_change(next.to_owned()))
        .app_class(tw(if multiline {
            "min-h-16 w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground"
        } else {
            "h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground"
        }))
}

fn switch_row(
    label: &'static str,
    checked: Prop<bool>,
    disabled: Prop<bool>,
    dimmed: Prop<bool>,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    view()
        .app_class(move || {
            Some(tw(if dimmed.get() {
                "flex items-center justify-between gap-3 rounded-md border border-border/70 px-3 py-2 text-sm dark:border-transparent dark:bg-white/[0.035] opacity-60"
            } else {
                "flex items-center justify-between gap-3 rounded-md border border-border/70 px-3 py-2 text-sm dark:border-transparent dark:bg-white/[0.035]"
            }))
        })
        .child(text(t(label)))
        .child(switch(
            label,
            ControlState {
                checked,
                disabled,
                ..Default::default()
            },
            SwitchSize::Default,
            on_change,
        ))
}

fn icon_picker(form: Signal<ScriptForm>, disabled: Prop<bool>) -> impl IntoElement {
    let handle = popover_create_handle();
    // useComposerMenuState closes the picker when the form becomes read-only.
    effect(move || {
        if disabled.get() {
            handle.set_open(false);
        }
    });
    popover(
        handle,
        PopoverOptions {
            align: Align::Start,
            ..Default::default()
        },
        button(
            ButtonContent::Custom {
                label: t("Choose icon"),
                child: dynamic(move || Some(script_icon(form.with(|f| f.icon.clone()), "size-4.5")))
                    .into_element(),
            },
            ButtonVariant::Outline,
            ButtonSize::Icon,
            move || disabled.get(),
            || {},
        ),
        move || {
            view()
                .app_class(tw("grid grid-cols-3 gap-2"))
                .child(SCRIPT_ICONS.map(|(icon, label)| {
                    let chosen = icon.clone();
                    let compared = icon.clone();
                    nsh_base::button()
                        .accessibility_label(t(label))
                        .on_press(move || {
                            form.update(|f| f.icon = chosen.clone());
                            handle.set_open(false);
                        })
                        .app_class(move || {
                            Some(tw(if form.with(|f| f.icon == compared) {
                                "relative flex flex-col items-center gap-2 rounded-md border px-2 py-2 text-xs dark:border-transparent border-primary/70 bg-primary/10 ring-inset dark:ring-1 dark:ring-primary/30"
                            } else {
                                "relative flex flex-col items-center gap-2 rounded-md border px-2 py-2 text-xs dark:border-transparent border-border/70 hover:bg-accent/60 dark:bg-white/[0.035]"
                            }))
                        })
                        .child(script_icon(icon, "size-4"))
                        .child(text(t(label)))
                        .into_element()
                }))
                .into_element()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(script_id: Option<&str>, keybinding: Option<&str>) -> EditorRequest {
        EditorRequest {
            script_id: script_id.map(str::to_owned),
            initial: ScriptForm {
                keybinding: keybinding.map(str::to_owned),
                ..Default::default()
            },
            error: None,
        }
    }

    fn form(name: &str, command: &str, keybinding: Option<&str>) -> ScriptForm {
        ScriptForm {
            name: name.into(),
            command: command.into(),
            keybinding: keybinding.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn submit_validates_in_the_source_order() {
        let new = request(None, None);
        assert_eq!(
            submit_payload(&new, &form("Test", "bun test", None), false, true).err(),
            Some(t("This connection cannot change project actions."))
        );
        assert_eq!(
            submit_payload(&new, &form("Test", "bun test", Some("mod+t")), true, false).err(),
            Some(t("This connection cannot change keyboard shortcuts."))
        );
        assert_eq!(
            submit_payload(&new, &form("  ", "bun test", None), true, true).err(),
            Some(t("Name is required."))
        );
        assert_eq!(
            submit_payload(&new, &form("Test", " ", None), true, true).err(),
            Some(t("Command is required."))
        );
    }

    #[test]
    fn the_keybinding_is_sent_only_when_new_or_changed() {
        let new = submit_payload(&request(None, None), &form(" Test ", " bun test ", None), true, true)
            .expect("valid");
        assert_eq!(new.script.name.0, "Test");
        assert_eq!(new.script.command.0, "bun test");
        assert_eq!(new.keybinding, Some(None), "a new script clears any stale shortcut");
        let read_only_keys = submit_payload(&request(None, None), &form("Test", "x", None), true, false)
            .expect("valid");
        assert_eq!(read_only_keys.keybinding, None);
        let unchanged = submit_payload(&request(Some("test"), Some("mod+t")), &form("Test", "x", Some(" mod+t ")), true, false)
            .expect("unchanged shortcut needs no keybinding grant");
        assert_eq!(unchanged.keybinding, None);
        let cleared = submit_payload(&request(Some("test"), Some("mod+t")), &form("Test", "x", Some("")), true, true)
            .expect("valid");
        assert_eq!(cleared.keybinding, Some(None));
    }

    #[test]
    fn wait_for_setup_and_auto_preview_need_their_parent_option() {
        let mut fields = form("Dev", "bun dev", None);
        fields.wait_for_setup = true;
        fields.auto_open_preview = true;
        fields.preview_url = Some("  ".into());
        let payload = submit_payload(&request(None, None), &fields, true, true).expect("valid");
        assert!(!payload.script.wait_for_setup);
        assert!(!payload.script.auto_open_preview);
        assert_eq!(payload.script.preview_url, None);
    }

    #[test]
    fn menu_labels_name_the_script_roles() {
        let script: ProjectScript = serde_json::from_value(serde_json::json!({
            "id":"setup","name":"Setup","command":"bun i","icon":"configure","runOnWorktreeCreate":true,"runOnSettle":true
        }))
        .expect("script");
        assert_eq!(project_script_menu_label(&script), "Setup (setup, on settle)");
    }
}
