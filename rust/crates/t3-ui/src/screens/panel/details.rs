//! Port of apps/web/src/components/chat/ThreadDetailsPanel.tsx ThreadDetailsPanel.

mod open_in_picker;
mod project_scripts;

use std::collections::BTreeMap;
use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::element::{SignalEquals, signal_with};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::types::ThreadKey;
use t3_client::workspace::git_panel::details::{
    DETAILS_ITEMS, DetailsDensity, is_default_thread_details_visibility,
    resolve_thread_details_density,
};
use t3_client::workspace::git_panel::editors::EditorPickerSnapshot;
use t3_client::workspace::git_panel::scripts::ProjectScriptsSnapshot;
use t3_contracts::environment::EnvironmentMachineKind;
use t3_contracts::server::ServerConfig;

pub use open_in_picker::EditorPickerBackend;
pub use project_scripts::ProjectScriptsBackend;

use super::{Action, PanelHost, panel_target};
use crate::i18n::{Text, Value, t, tr};
use crate::runtime::tasks::TaskOwner;
use crate::runtime::{Shared, rt};
use crate::theme::tw;
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::menu::{self, CheckboxVariant, ItemOptions, MenuOptions};
use crate::ui::popover::dropdown_glass_background;
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// GIT-2 supplies `branch`; GIT-1 supplies `git_actions` (publish, then changes).
#[derive(Default)]
pub struct VersionControlSlots {
    pub branch: Option<AnyElement>,
    pub git_actions: Option<AnyElement>,
}

/// The client's details services (`GitPanel`'s details visibility, editor picker and project
/// scripts), bound by delegation in `runtime/services/git_panel.rs`.
pub trait ThreadDetailsBackend {
    /// The resolved item visibility first, then after every settings change.
    fn visibility(&self) -> LocalBoxStream<'static, BTreeMap<String, bool>>;
    fn set_item(
        &self,
        id: String,
        visible: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn reset_items(&self) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn editor_picker(&self, key: &ThreadKey) -> Rc<dyn EditorPickerBackend>;
    fn project_scripts(&self, key: &ThreadKey) -> Rc<dyn ProjectScriptsBackend>;
}

/// Provided once above the routes; `visibility` follows the client's settings for every card.
#[derive(Clone)]
pub struct ThreadDetailsRuntime {
    pub backend: Rc<dyn ThreadDetailsBackend>,
    pub visibility: Signal<BTreeMap<String, bool>>,
}

/// Build under a long-lived owner (the app root): its subscription lives as long as that owner.
pub fn thread_details_runtime(backend: Rc<dyn ThreadDetailsBackend>) -> ThreadDetailsRuntime {
    let visibility = signal(default_visibility());
    let mut updates = backend.visibility();
    crate::runtime::tasks::watch(async move {
        while let Some(next) = updates.next().await {
            visibility.set(next);
        }
    });
    ThreadDetailsRuntime {
        backend,
        visibility,
    }
}

fn default_visibility() -> BTreeMap<String, bool> {
    DETAILS_ITEMS
        .iter()
        .map(|(id, _, visible)| ((*id).to_owned(), *visible))
        .collect()
}

/// `visible[id]` from the runtime, or the defaults when no client is bound.
fn item_visible(runtime: Option<&ThreadDetailsRuntime>, id: &str) -> bool {
    match runtime {
        Some(runtime) => runtime
            .visibility
            .with(|v| v.get(id).copied().unwrap_or(false)),
        None => DETAILS_ITEMS
            .iter()
            .any(|(item, _, visible)| *item == id && *visible),
    }
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

/// Which rows render, from ThreadDetailsPanel's `show` flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Show {
    environment: bool,
    open_in: bool,
    scripts: bool,
    branch: bool,
    git: bool,
}

impl Show {
    fn workspace(self) -> bool {
        self.environment || self.open_in || self.scripts
    }
    fn any(self) -> bool {
        self.workspace() || self.branch || self.git
    }
}

/// The content tree reads the client projection itself; callers supply only other owners' rows.
pub fn thread_details_panel(
    key: ThreadKey,
    max_height: Prop<f32>,
    version_control: VersionControlSlots,
) -> impl IntoElement {
    let runtime = rt();
    let details = use_context::<ThreadDetailsRuntime>();
    // The items menus close, and "Choose what to show" goes, as their choices apply.
    let owner = TaskOwner::here();
    debug_assert!(
        details.is_some() || runtime.backend().client().is_none(),
        "ThreadDetailsRuntime is not provided: app::provide_client_services provides it with the client"
    );
    let config = runtime.store.environment_config(key.environment.clone());
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
    let shell = runtime.store.shell(key.environment.clone());
    let thread = key.thread.clone();
    let context = computed(move || {
        shell.with(|shell| {
            let shell = shell.as_ref()?;
            let thread = shell.threads.iter().find(|t| t.id == thread)?;
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

    let editor_snapshot = signal::<Option<Rc<EditorPickerSnapshot>>>(None);
    let scripts_snapshot =
        signal_with::<Option<Rc<ProjectScriptsSnapshot>>>(None, SignalEquals::Never);
    let editors = details
        .as_ref()
        .map(|d| stored(d.backend.editor_picker(&key)));
    let scripts = details
        .as_ref()
        .map(|d| stored(d.backend.project_scripts(&key)));
    if let Some(editors) = editors {
        let mut updates = editors.with(|e| e.watch());
        crate::runtime::tasks::watch(async move {
            while let Some(next) = updates.next().await {
                editor_snapshot.set(next.ok().map(Rc::new));
            }
        });
    }
    if let Some(scripts) = scripts {
        let mut updates = scripts.with(|s| s.watch());
        crate::runtime::tasks::watch(async move {
            while let Some(next) = updates.next().await {
                scripts_snapshot.set(next.ok().map(Rc::new));
            }
        });
    }

    let details = stored(details);
    let visible = move |id: &str| details.with(|d| item_visible(d.as_ref(), id));
    // ThreadDetailsCard measures each density's content once it renders, per set of items.
    let content_key = computed(move || {
        DETAILS_ITEMS
            .iter()
            .filter(|(id, _, _)| visible(id))
            .map(|(id, _, _)| *id)
            .collect::<Vec<_>>()
            .join(",")
    });
    let heights = signal((String::new(), 0.0_f64, 0.0_f64));
    let density = computed(move || {
        let key = content_key.get();
        let (full, compact) = heights.with(|(measured, full, compact)| {
            if *measured == key {
                (*full, *compact)
            } else {
                (0.0, 0.0)
            }
        });
        resolve_thread_details_density(f64::from(max_height.get().max(0.0)), full, compact)
    });
    let has_branch = version_control.branch.is_some();
    let has_git = version_control.git_actions.is_some();
    let located = computed(move || label.get().is_some() && context.get().is_some());
    let show = computed(move || {
        let density = density.get();
        Show {
            environment: density == DetailsDensity::Full && visible("environment") && located.get(),
            open_in: density != DetailsDensity::Essential
                && visible("openIn")
                && editor_snapshot.with(Option::is_some),
            scripts: visible("scripts") && scripts_snapshot.with(Option::is_some),
            // The branch row hides itself outside a git checkout that offers git.
            branch: has_branch && visible("branch") && located.get(),
            git: has_git && (visible("gitActions") || visible("changes")) && located.get(),
        }
    });

    let workspace = view()
        .app_class(move || {
            Some(tw(if show.with(|s| s.workspace()) {
                ""
            } else {
                "hidden"
            }))
        })
        .child(thread_details_section(
            Text::from("Workspace"),
            false.into(),
            view()
                .app_class(tw("flex flex-col"))
                .child(dynamic(move || {
                    if !show.with(|s| s.environment) {
                        return None;
                    }
                    let label = label.get()?;
                    let (worktree, workspace_root) = context.get()?;
                    Some(untracked(|| {
                        run_context_selector(label, worktree, workspace_root, config).into_element()
                    }))
                }))
                .child(editors.map(|editors| {
                    when(
                        move || show.with(|s| s.open_in),
                        move || open_in_picker::open_in_picker(editors, editor_snapshot),
                    )
                }))
                .child(scripts.map(|scripts| {
                    when(
                        move || show.with(|s| s.scripts),
                        move || project_scripts::project_scripts_control(scripts, scripts_snapshot),
                    )
                })),
        ));
    // Hidden slots keep their element (the slot is built once by its owner) but leave the layout.
    let version_control_section = (has_branch || has_git).then(|| {
        view()
            .app_class(move || {
                Some(tw(if show.with(|s| s.branch || s.git) {
                    ""
                } else {
                    "hidden"
                }))
            })
            .child(thread_details_section(
                Text::from("Version Control"),
                Prop::bind(move || {
                    density.get() == DetailsDensity::Full && show.with(|s| s.workspace())
                }),
                view()
                    .app_class(tw("flex flex-col"))
                    .child(version_control.branch.map(|branch| {
                        view()
                            .app_class(move || {
                                Some(tw(if show.with(|s| s.branch) {
                                    ""
                                } else {
                                    "hidden"
                                }))
                            })
                            .child(branch)
                    }))
                    .child(version_control.git_actions.map(|git| {
                        view()
                            .app_class(move || {
                                Some(tw(if show.with(|s| s.git) { "" } else { "hidden" }))
                            })
                            .child(git)
                    })),
            ))
    });
    let menu_open = signal(false);
    let context_menu_open = signal(false);
    let items_runtime = details.get();
    let content = view()
        .app_class(tw("group/details-items"))
        .on_layout(move |bounds: &Bounds<Points>| {
            // The card's border is the frame's offsetHeight − clientHeight in the source.
            let height = f64::from(bounds.size.height.0) + 2.0;
            let key = content_key.get_untracked();
            let density = density.get_untracked();
            heights.update(|(measured, full, compact)| {
                if *measured != key {
                    *measured = key;
                    *full = 0.0;
                    *compact = 0.0;
                }
                match density {
                    DetailsDensity::Full => *full = height,
                    DetailsDensity::Compact => *compact = height,
                    DetailsDensity::Essential => {}
                }
            });
        })
        .child(workspace)
        .child(version_control_section)
        // Shown only when no section has content, so an emptied card can be refilled.
        .child(items_runtime.clone().map(|runtime| {
            when(
                move || !show.with(|s| s.any()),
                move || choose_what_to_show(runtime.clone(), menu_open, owner),
            )
        }));
    let card = thread_details_card(max_height, content);
    match items_runtime {
        // The card's own context menu; NSH's trigger leaves fields and links their own menus.
        Some(runtime) => menu::context_menu(
            card,
            MenuOptions {
                open: Some(context_menu_open),
                ..Default::default()
            },
            || false,
            menu::menu_popup_with(move || thread_details_items_menu_items(runtime.clone(), owner)),
        )
        .into_element(),
        None => card.into_element(),
    }
}

/// The empty card's "Choose what to show" row.
fn choose_what_to_show(
    runtime: ThreadDetailsRuntime,
    open: Signal<bool>,
    owner: TaskOwner,
) -> impl IntoElement {
    view().app_class(tw("p-2")).child(menu::menu(
        nsh_base::button()
            .accessibility_label(t("Choose what to show"))
            .app_class(tw("relative inline-flex shrink-0 cursor-pointer items-center whitespace-nowrap border outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background h-8 min-w-0 rounded-lg border-transparent gap-2.5 px-2.5 text-left text-[13px] font-medium text-foreground/80 w-full justify-start bg-transparent shadow-none hover:!bg-black/[0.035] dark:hover:!bg-white/[0.08]"))
            .child(icon_with_class(Icon::SlidersHorizontal, "size-4 shrink-0 text-muted-foreground"))
            .child(text(t("Choose what to show")).app_class(tw("ml-0.5"))),
        MenuOptions {
            open: Some(open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup_with(move || thread_details_items_menu_items(runtime.clone(), owner)),
    ))
}

/// Port of chat/ThreadDetailsItemsMenu.tsx ThreadDetailsItemsMenuItems.
pub fn thread_details_items_menu_items(
    runtime: ThreadDetailsRuntime,
    owner: TaskOwner,
) -> impl IntoElement {
    let backend = stored(runtime.backend.clone());
    let visibility = runtime.visibility;
    view()
        .child(menu::menu_group(
            t("Show in thread details"),
            view()
                .child(menu::menu_group_label(t("Show in thread details"), false))
                .child(
                    DETAILS_ITEMS
                        .iter()
                        .map(|(id, label, _)| {
                            let id = *id;
                            menu::menu_checkbox_item(
                                t(label),
                                move || visibility.with(|v| v.get(id).copied().unwrap_or(false)),
                                CheckboxVariant::Default,
                                ItemOptions {
                                    close_on_select: false,
                                    ..Default::default()
                                },
                                text(t(label)),
                                move |checked| {
                                    let backend = backend.get();
                                    owner.spawn(async move {
                                        let _ = backend.set_item(id.to_owned(), checked).await;
                                    });
                                },
                            )
                            .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
        ))
        .child(menu::menu_separator())
        .child(menu::menu_item(
            t("Reset to default"),
            ItemOptions {
                disabled: Prop::bind(move || visibility.with(is_default_thread_details_visibility)),
                ..Default::default()
            },
            text(t("Reset to default")),
            move || {
                let backend = backend.get();
                owner.spawn(async move {
                    let _ = backend.reset_items().await;
                });
            },
        ))
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
    // Environment/worktree mutation is not composed by the client yet; keep the locked row.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_thread_details_items_without_a_client() {
        for (id, _, default) in DETAILS_ITEMS {
            assert_eq!(item_visible(None, id), *default, "{id}");
        }
        assert!(!item_visible(None, "unknown"));
    }

    #[test]
    fn the_choose_row_shows_only_when_no_rendered_section_has_content() {
        assert!(!Show::default().any());
        assert!(
            Show {
                scripts: true,
                ..Default::default()
            }
            .any()
        );
        assert!(
            Show {
                git: true,
                ..Default::default()
            }
            .any()
        );
        assert!(
            !Show {
                branch: true,
                ..Default::default()
            }
            .workspace()
        );
    }
}
