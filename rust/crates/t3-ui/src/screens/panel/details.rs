//! Port of apps/web/src/components/chat/ThreadDetailsPanel.tsx ThreadDetailsPanel.

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::store::Scope;
use t3_client::types::ThreadKey;
use t3_contracts::environment::EnvironmentMachineKind;
use t3_contracts::server::ServerConfig;

use super::{Action, PanelHost, panel_target};
use crate::i18n::{Text, Value, t, tr};
use crate::runtime::{Shared, rt};
use crate::theme::tw;
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::popover::dropdown_glass_background;
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// GIT-2 supplies `branch`; GIT-1 supplies `git_actions` (publish, then changes).
#[derive(Default)]
pub struct VersionControlSlots {
    pub branch: Option<AnyElement>,
    pub git_actions: Option<AnyElement>,
}

/// Mount in the canvas's details placement, separately from the workspace-tab registry.
pub fn thread_details_target(
    host: PanelHost,
    key: ThreadKey,
    inline: Prop<bool>,
    max_height: Prop<f32>,
    version_control: VersionControlSlots,
) -> impl IntoElement {
    let content = thread_details_panel(key.clone(), max_height, version_control);
    let key = stored(key);
    // Canvas placement changes outside the panel store, so entering inline clears its popup flag.
    effect(move || {
        if inline.get() && host.thread_panel_open(&key.get(), false) {
            host.apply(
                &key.get(),
                Action::SetThreadPanel {
                    inline: false,
                    open: false,
                },
            );
        }
    });
    panel_target()
        .open(move || host.thread_panel_open(&key.get(), inline.get()))
        .child(content)
}

/// ThreadDetailsCard's anchored popup. TL-HEADER supplies its actual details-toggle anchor.
pub fn thread_details_popover(
    host: PanelHost,
    key: ThreadKey,
    anchor: impl IntoElement,
    max_height: Prop<f32>,
    version_control: VersionControlSlots,
) -> impl IntoElement {
    let content =
        thread_details_target(host, key.clone(), false.into(), max_height, version_control);
    let key = stored(key);
    // NSH owns outside dismissal/Escape/focus return; DOM focus-out cancellation is not exposed.
    nsh_base::popover()
        .open(move || Some(host.thread_panel_open(&key.get(), false)))
        .on_open_change(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(open, _) = event {
                host.apply(
                    &key.get(),
                    Action::SetThreadPanel {
                        inline: false,
                        open: *open,
                    },
                );
            }
        })
        .placement(Placement::Bottom)
        .align(Align::End)
        .trigger(anchor)
        .child(
            view()
                .w(move || {
                    px(tokens()
                        .number("thread-details-panel-width")
                        .unwrap_or(280.0))
                })
                .max_w(move || px((window().width.0 - 24.0).max(0.0)))
                .child(content),
        )
}

/// The content tree reads the client projection itself; callers supply only other owners' rows.
pub fn thread_details_panel(
    key: ThreadKey,
    max_height: Prop<f32>,
    version_control: VersionControlSlots,
) -> impl IntoElement {
    let runtime = rt();
    let config = environment_config(&key);
    let environment = key.environment.clone();
    let label = computed(move || {
        config.with(|config| {
            config
                .as_ref()
                .map(|config| config.environment.label.0.clone())
                .or_else(|| {
                    runtime
                        .environments()
                        .into_iter()
                        .find(|entry| entry.id == environment)
                        .map(|entry| entry.label)
                })
        })
    });
    let shell = runtime.store.shell(key.environment);
    let context = computed(move || {
        shell.with(|shell| {
            let shell = shell.as_ref()?;
            let thread = shell
                .threads
                .iter()
                .find(|thread| thread.id == key.thread)?;
            let project = shell
                .projects
                .iter()
                .find(|project| project.id == thread.project_id)?;
            Some((
                thread.worktree_path.as_ref().map(|path| path.0.clone()),
                project.workspace_root.0.clone(),
            ))
        })
    });
    let workspace = dynamic(move || {
        let label = label.get()?;
        let (worktree, workspace_root) = context.get()?;
        Some(untracked(|| {
            thread_details_section(
                Text::from("Workspace"),
                false.into(),
                view()
                    .app_class(tw("flex flex-col"))
                    .child(run_context_selector(
                        label,
                        worktree,
                        workspace_root,
                        config,
                    )),
            )
        }))
    });
    let has_version_control =
        version_control.branch.is_some() || version_control.git_actions.is_some();
    let version_control = has_version_control.then(|| {
        thread_details_section(
            Text::from("Version Control"),
            Prop::bind(move || label.get().is_some() && context.get().is_some()),
            view()
                .app_class(tw("flex flex-col"))
                .child(version_control.branch)
                .child(version_control.git_actions),
        )
    });
    // Editor/script services and details-item preferences are requested in client-seam-requests.md.
    thread_details_card(
        max_height,
        view()
            .app_class(tw("group/details-items"))
            .child(workspace)
            .child(version_control),
    )
}

/// Port of chat/ThreadDetailsCard.tsx's card; the canvas owns its inline/popover placement.
pub fn thread_details_card(max_height: Prop<f32>, children: impl IntoElement) -> impl IntoElement {
    // `isolate`/`contain-paint` are DOM hints; grid's one minmax row is a native flex column.
    view()
        .test_id("thread-details-card")
        .role(AccessibilityRole::Region)
        .label(move || t("Thread details"))
        .app_class(tw("flex flex-col max-h-full overflow-hidden rounded-2xl border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"))
        .max_h(move || px(max_height.get().max(0.0)))
        .bg(dropdown_glass_background)
        .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
        .child(
            scroll()
                .app_class(tw("min-h-0"))
                .child(view().child(children)),
        )
}

/// Port of chat/ThreadDetailsSection.tsx with `showHeading={false}`.
pub fn thread_details_section(
    title: Text,
    separated: Prop<bool>,
    children: impl IntoElement,
) -> impl IntoElement {
    // The caller omits empty sections; CSS's sibling/has selectors become this explicit flag.
    view()
        .role(AccessibilityRole::Region)
        .label(move || title.resolve())
        .app_class(move || {
            Some(tw(if separated.get() {
                "px-2 pt-2 pb-2.5 border-t border-border/65"
            } else {
                "px-2 pt-2 pb-2.5"
            }))
        })
        .child(children)
}

/// Port of BranchToolbar.tsx RunContextSelector's locked panel row.
fn run_context_selector(
    label: String,
    worktree: Option<String>,
    workspace_root: String,
    config: Memo<Option<Shared<ServerConfig>>>,
) -> impl IntoElement {
    let workspace = t3_client::workspace::branch::locked_workspace_label(
        worktree.as_deref(),
        t3_client::workspace::branch::EnvMode::Local,
    );
    let workspace = if worktree.is_none() {
        Text::client_key(workspace)
    } else {
        Text::content(workspace)
    };
    let environment_label = label.clone();
    let environment_hint = label.clone();
    let workspace_path = worktree.clone().unwrap_or(workspace_root);
    // Environment/worktree mutation is absent from Backend; keep the source's locked row.
    view()
        .test_id("thread-details-environment")
        .label(move || tr("Run context: {context}", &[("context", Value::Text(format!("{environment_label}, {}", workspace.resolve())))]))
        .app_class(tw("inline-flex h-7 min-w-0 max-w-[48%] flex-initial items-center justify-start gap-1 rounded-md border border-transparent px-1.75 font-normal text-foreground/70 text-sm sm:h-6 sm:text-xs dark:text-muted-foreground max-w-full h-8 w-full min-w-0 justify-start rounded-lg border border-transparent gap-2.5 px-2.5 text-left text-[13px] font-medium text-foreground/80 sm:h-8 sm:text-[13px]"))
        .child(view().app_class(tw("inline-flex shrink-0"))
            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(SharedString::from(environment_hint))))
            .child(dynamic(move || {
            let machine = config.with(|config| config.as_ref().and_then(|config| config.environment.platform.machine.clone()));
            let glyph = match machine {
                None | Some(EnvironmentMachineKind::Server) => Some(Icon::Server),
                Some(EnvironmentMachineKind::Cloud) => Some(Icon::Cloud),
                Some(EnvironmentMachineKind::Desktop) => Some(Icon::Monitor),
                Some(EnvironmentMachineKind::Laptop) => Some(Icon::Laptop),
                // UI-ICONS does not yet expose EnvironmentMachineIcon's three custom marks.
                Some(EnvironmentMachineKind::Linux | EnvironmentMachineKind::MacMini | EnvironmentMachineKind::MacStudio) => None,
            };
            glyph.map(|glyph| icon_with_class(glyph, "size-4 shrink-0 text-muted-foreground -mx-px size-4.5"))
        })))
        .child(composer_context_label(SharedString::from(label)))
        .child(view().app_class(tw("inline-flex shrink-0"))
            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(SharedString::from(workspace_path))))
            .child(icon_with_class(if worktree.is_some() { Icon::FolderGit2 } else { Icon::Folder }, "size-4 shrink-0 text-muted-foreground")))
}

/// Port of ComposerContextLabel.tsx ComposerContextLabel in panel mode.
fn composer_context_label(content: SharedString) -> impl IntoElement {
    view()
        .app_class(tw(
            "min-w-0 flex-1 overflow-x-clip overflow-y-visible text-left",
        ))
        // `text-box` has no NSH meaning; native text supplies its own font metrics.
        .child(text(content).app_class(tw(
            "block w-full min-w-0 truncate overflow-x-clip overflow-y-visible",
        )))
}

// Runtime has no config selector yet; subscribe before reading, outside render bindings.
fn environment_config(key: &ThreadKey) -> Memo<Option<Shared<ServerConfig>>> {
    let store = rt().store.store();
    let environment = key.environment.clone();
    let mut changes = store.subscribe_environment(&environment);
    let config = signal(None);
    spawn(async move {
        loop {
            config.set(
                store
                    .snapshot()
                    .environments
                    .get(&environment)
                    .and_then(|state| state.config.clone())
                    .map(Shared),
            );
            loop {
                let Some(change) = changes.next().await else {
                    return;
                };
                if change
                    .scopes
                    .iter()
                    .any(|scope| matches!(scope, Scope::All | Scope::Environment(_)))
                {
                    break;
                }
            }
        }
    });
    computed(move || config.get())
}
