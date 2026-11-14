//! Port of apps/web/src/components/sidebar/SidebarChrome.tsx and SidebarThreadHeader.tsx.

use std::rc::Rc;

use nsh_core::{element::ImageSrc, prelude::*};
use t3_client::services::assets::ProjectFaviconKey;
use t3_client::view_logic::shell::{
    navigation::{Direction, HistoryAvailability},
    sidebar::{ScopeMenuAction, ScopeMenuState, filter_project_scope_items},
};
use t3_client::workspace::projects::{LogicalProjectKey, ProjectGroup, pickable_project_groups};
use t3_contracts::settings::EnvironmentIdentificationMode;

use crate::i18n::{Value, t, tr};
use crate::state::{
    layout::{Pointer, layout_class, pointer},
    nav::{Route, SettingsPage, navigator},
    panels::panels,
};
use crate::theme::{Scope, cn, palette, tw, tw_with};
use crate::ui::{
    avatar::{ProjectFaviconProject, project_favicon},
    icon::{Icon, icon},
    menu::{self, ItemOptions, MenuOptions},
    tooltip::{TooltipOptions, tooltip, tooltip_create_handle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageBackdropVariant {
    Dev,
    Nightly,
}

/// Port of SidebarStageBackdrop.tsx resolveSidebarStageBackdropVariant.
pub fn resolve_sidebar_stage_backdrop_variant(
    label: &str,
    mode: &EnvironmentIdentificationMode,
) -> Option<StageBackdropVariant> {
    match mode {
        EnvironmentIdentificationMode::DevArtwork => Some(StageBackdropVariant::Dev),
        EnvironmentIdentificationMode::NightlyArtwork => Some(StageBackdropVariant::Nightly),
        EnvironmentIdentificationMode::Artwork => match label.trim().to_lowercase().as_str() {
            "dev" => Some(StageBackdropVariant::Dev),
            "nightly" => Some(StageBackdropVariant::Nightly),
            _ => None,
        },
        _ => None,
    }
}

/// Port of SidebarStageBackdrop.tsx resolveEnvironmentIdentificationPillLabel.
pub fn resolve_environment_identification_pill_label(label: &str) -> Option<&'static str> {
    match label.trim().to_lowercase().as_str() {
        "dev" => Some("Dev"),
        "nightly" => Some("Nightly"),
        _ => None,
    }
}

#[derive(Clone)]
pub struct HistoryNavigation {
    pub availability: Rc<dyn Fn() -> HistoryAvailability>,
    pub back_shortcut: Prop<Option<String>>,
    pub forward_shortcut: Prop<Option<String>>,
    pub navigate: Rc<dyn Fn(Direction)>,
}

#[derive(Clone)]
pub struct ChromeHeaderProps {
    pub stage_label: Prop<String>,
    pub identification_mode: Prop<EnvironmentIdentificationMode>,
    /// Supplied when the host exposes history, rather than guessed from its OS.
    pub history: Option<HistoryNavigation>,
}

impl Default for ChromeHeaderProps {
    fn default() -> Self {
        Self {
            stage_label: String::new().into(),
            identification_mode: EnvironmentIdentificationMode::Artwork.into(),
            history: None,
        }
    }
}

pub fn sidebar_chrome_header(props: ChromeHeaderProps) -> impl IntoElement {
    let nav = navigator();
    let panels = panels();
    let backdrop = computed(move || {
        resolve_sidebar_stage_backdrop_variant(
            &props.stage_label.get(),
            &props.identification_mode.get(),
        )
    });
    let pill = computed(move || {
        (props.identification_mode.get() == EnvironmentIdentificationMode::Pill)
            .then(|| resolve_environment_identification_pill_label(&props.stage_label.get()))
            .flatten()
    });
    view().app_class(tw("relative flex h-[var(--workspace-topbar-height)] shrink-0 flex-row items-center gap-2 pr-3"))
        .test_id("sidebar-chrome-header")
        .child(dynamic(move || backdrop.get().map(sidebar_stage_backdrop)))
        // The frozen shell has no floating SidebarTrigger; keep its collapse control here.
        .child(view().app_class(tw("relative z-10 ml-3 flex shrink-0 items-center"))
            .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || if layout_class().is_compact() { t("The thread list is the root page at this width.") } else { t("Toggle main sidebar") })))
            .child(nsh_base::button().accessibility_label(t("Toggle main sidebar"))
                .disabled(move || layout_class().is_compact())
                .on_press(move || { if !layout_class().is_compact() { panels.sidebar_open.update(|open| *open = !*open); } })
                .app_class(move || tw(&cn([
                    Some("relative flex size-7 items-center justify-center rounded-md outline-hidden focus-visible:ring-2 focus-visible:ring-inset"),
                    Some(if backdrop.get().is_some() { "text-white/90 hover:bg-white/10 hover:text-white focus-visible:ring-white" } else { "text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring" }),
                ])))
                .hit_slop(touch_slop)
                .child(icon(Icon::PanelLeft, 16.0, move || if backdrop.get().is_some() { AdaptiveColor::rgb(0xffffff, 0xffffff).opacity(0.9) } else { palette().muted_foreground }))))
        .child(view().app_class(tw("relative z-10 flex h-8 min-w-0 flex-1 flex-wrap content-start items-center gap-x-2 overflow-hidden py-0.5"))
            .child(view().app_class(tw("flex shrink-0 items-center gap-1"))
                .child(sidebar_brand(move || backdrop.get().is_some(), move || nav.open(Route::Home)))
                .child(props.history.map(move |history| view().app_class(tw("hidden items-center md:flex"))
                    .child(history_navigation_button(Direction::Back, history.clone(), move || backdrop.get().is_some()))
                    .child(history_navigation_button(Direction::Forward, history, move || backdrop.get().is_some())))))
            .child(when(move || pill.get().is_some(), move || {
                // UI-BADGE is a stub; these are Badge secondary/sm's classes.
                view().app_class(tw("ml-1 flex h-7 items-center"))
                    .child(text(move || t(pill.get().unwrap_or_default())).app_class(tw("relative inline-flex shrink-0 items-center justify-center gap-1 whitespace-nowrap border border-transparent font-medium outline-none h-5 min-w-5 rounded-[.25rem] px-[3px] text-xs leading-none sm:h-4 sm:min-w-4 sm:text-[.625rem] bg-secondary text-secondary-foreground")))
            })))
}

pub fn sidebar_brand_width_probe(
    history_supported: bool,
    on_width_change: impl Fn(f32) + 'static,
) -> impl IntoElement {
    // Native measurement replaces ResizeObserver; safe-area/titlebar offsets belong to SH-TITLEBAR.
    view().accessible(false).app_class(tw("pointer-events-none invisible fixed top-0 left-0 flex w-max border-r border-transparent pr-3"))
        .on_layout(move |bounds: &Bounds<Points>| on_width_change(bounds.size.width.0))
        .child(view().app_class(tw("flex gap-1")).child(sidebar_brand_mark(|| false))
            .child(history_supported.then(|| view().app_class(tw("w-14")))))
}

pub fn history_navigation_button(
    direction: Direction,
    history: HistoryNavigation,
    on_backdrop: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    let availability = history.availability.clone();
    let enabled = Prop::bind(move || availability().can_navigate(direction));
    let label = if direction == Direction::Back {
        "Go back"
    } else {
        "Go forward"
    };
    let hint = move || {
        let shortcut = match direction {
            Direction::Back => history.back_shortcut.get(),
            Direction::Forward => history.forward_shortcut.get(),
        };
        shortcut.map_or_else(
            || t(label),
            |shortcut| {
                tr(
                    "{label} ({shortcut})",
                    &[
                        ("label", Value::Text(t(label))),
                        ("shortcut", Value::Text(shortcut)),
                    ],
                )
            },
        )
    };
    let handle = tooltip_create_handle();
    view().app_class(tw("flex shrink-0"))
        .tooltip(tooltip(handle, TooltipOptions { side: Placement::Bottom, ..Default::default() }, text(hint)))
        .child(nsh_base::button().accessibility_label(t(label)).disabled(move || !enabled.get())
        .on_press(move || { if enabled.get_untracked() { (history.navigate)(direction); } })
        .app_class(move || tw(&cn([
            Some("relative flex size-8 items-center justify-center rounded-md outline-hidden focus-visible:ring-2 focus-visible:ring-inset disabled:opacity-64 sm:size-7"),
            Some(if on_backdrop() { "text-white/90 hover:bg-white/10 hover:text-white focus-visible:ring-white" } else { "text-foreground hover:bg-accent focus-visible:ring-ring" }),
        ])))
        .hit_slop(touch_slop)
        .child(svg(if direction == Direction::Back { Icon::ArrowLeft } else { Icon::ArrowRight }.asset()).accessible(false).app_class(tw("size-3.5"))))
}

fn sidebar_brand(
    on_backdrop: impl Fn() -> bool + Copy + 'static,
    on_home: impl Fn() + 'static,
) -> impl IntoElement {
    // The local trigger occupies workspace-titlebar-content-left; do not apply that offset twice.
    nsh_base::button().role(AccessibilityRole::Link).accessibility_label(t("T3 Code home")).on_press(on_home)
        .app_class(move || tw(&cn([
            Some("relative z-10 hidden h-7 w-fit min-w-0 shrink-0 items-center overflow-hidden rounded-md outline-hidden ring-ring focus-visible:ring-2 focus-visible:ring-inset md:flex"),
            Some(if on_backdrop() { "text-white" } else { "text-foreground" }),
        ]))).child(sidebar_brand_mark(on_backdrop))
}

pub fn sidebar_brand_mark(on_backdrop: impl Fn() -> bool + Copy + 'static) -> impl IntoElement {
    // NSH has no cap unit or text-box trim; Inter's 14px cap height is 10.2px.
    view()
        .app_class(tw(
            "inline-flex min-w-0 items-baseline gap-1 text-sm font-medium tracking-tight",
        ))
        .child(
            svg(ImageSrc::svg_markup(WORDMARK))
                .accessible(false)
                .w(px(16.9))
                .h(px(10.2)),
        )
        .child(text(t("Code")).app_class(move || {
            tw(&cn([
                Some("truncate"),
                Some(if on_backdrop() {
                    "text-white/70"
                } else {
                    "text-muted-foreground"
                }),
            ]))
        }))
}

/// Port of SidebarStageBackdrop.tsx SidebarStageBackdrop and index.css sidebar-stage-backdrop.
pub fn sidebar_stage_backdrop(variant: StageBackdropVariant) -> impl IntoElement {
    view()
        .accessible(false)
        .app_class(tw(
            "pointer-events-none absolute inset-x-0 top-0 z-0 h-20 select-none overflow-hidden",
        ))
        // NSH has no CSS mask-image; the SVG carries the same mask and ::after gradient.
        .child(
            svg(move || ImageSrc::svg_markup(&stage_art_markup(variant)))
                .accessible(false)
                .app_class(tw("h-full w-full")),
        )
}

fn svg_color(color: AdaptiveColor) -> String {
    let Rgba { r, g, b, a } = nsh_core::element::color(color).into();
    format!(
        "rgba({},{},{},{a})",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8
    )
}

fn stage_art_markup(variant: StageBackdropVariant) -> String {
    let mut markup = match variant {
        StageBackdropVariant::Dev => DEV_ART,
        StageBackdropVariant::Nightly => NIGHTLY_ART,
    }
    .to_owned();
    while let Some(at) = markup.find("var(--") {
        let Some(end) = markup[at..].find(')') else {
            break;
        };
        let name = &markup[at + 6..at + end];
        let resolved = tokens().color(name).unwrap_or(AdaptiveColor::TRANSPARENT);
        markup.replace_range(at..=at + end, &svg_color(resolved));
    }
    let fade = svg_color(palette().sidebar);
    let defs = format!(
        r##"<linearGradient id="stage-mask" x2="0" y2="1"><stop stop-color="white"/><stop offset=".55" stop-color="white"/><stop offset=".92" stop-color="black"/></linearGradient><mask id="stage-fade"><rect width="100%" height="96" fill="url(#stage-mask)"/></mask><linearGradient id="stage-overlay" x2="0" y2="1">{}</linearGradient>"##,
        [
            (0.0, 0.0),
            (0.28, 0.0),
            (0.4, 0.12),
            (0.52, 0.30),
            (0.64, 0.58),
            (0.75, 0.82),
            (0.85, 0.96),
            (0.93, 1.0)
        ]
        .into_iter()
        .map(|(offset, opacity)| format!(
            r#"<stop offset="{offset}" stop-color="{fade}" stop-opacity="{opacity}"/>"#
        ))
        .collect::<String>()
    );
    markup = markup.replace(
        "</defs>",
        &format!("{defs}</defs><g mask=\"url(#stage-fade)\">"),
    );
    markup.replace(
        "</svg>",
        "<rect width=\"100%\" height=\"96\" fill=\"url(#stage-overlay)\"/></g></svg>",
    )
}

pub struct SidebarThreadHeaderProps {
    pub has_projects: Prop<bool>,
    pub new_thread_disabled: Prop<bool>,
    pub new_thread_shortcut: Prop<Option<String>>,
    pub new_thread_in_project_shortcut: Prop<Option<String>>,
    pub show_new_thread_in_project_hint: Prop<bool>,
    pub search_query: Signal<String>,
    pub is_searching: Prop<bool>,
    pub search_result_count: Prop<usize>,
    pub active_search_result_index: Prop<usize>,
    pub search_results_ref: Prop<Option<ElementRef>>,
    pub active_search_result_ref: Prop<Option<ElementRef>>,
    pub search_input_ref: ElementRef,
    pub on_search_query_change: Rc<dyn Fn(String)>,
    pub on_search_key_down: Rc<dyn Fn(&KeyEvent)>,
    pub on_clear_search: Rc<dyn Fn()>,
    pub on_new_project: Rc<dyn Fn()>,
    pub on_new_thread: Rc<dyn Fn()>,
}

impl SidebarThreadHeaderProps {
    /// The slice routes until palette/new-thread services land; the list owns search.
    pub fn new(search_query: Signal<String>, has_projects: Prop<bool>) -> Self {
        let nav = navigator();
        Self {
            has_projects,
            new_thread_disabled: Prop::bind(move || !has_projects.get()),
            new_thread_shortcut: None.into(),
            new_thread_in_project_shortcut: None.into(),
            show_new_thread_in_project_hint: false.into(),
            search_query,
            is_searching: Prop::bind(move || !search_query.get().trim().is_empty()),
            search_result_count: 0.into(),
            active_search_result_index: 0.into(),
            search_results_ref: None.into(),
            active_search_result_ref: None.into(),
            search_input_ref: element_ref(),
            on_search_query_change: Rc::new(move |value| search_query.set(value)),
            on_search_key_down: Rc::new(|_| {}),
            on_clear_search: Rc::new(move || search_query.set(String::new())),
            on_new_project: Rc::new(move || nav.open(Route::AddProject)),
            on_new_thread: Rc::new(move || nav.open(Route::NewThread)),
        }
    }
}

/// Render inside SidebarGroup; the caller owns results, keyboard navigation and scope.
pub fn sidebar_thread_header<E: IntoElement>(
    props: SidebarThreadHeaderProps,
    project_scope: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let disabled = props.new_thread_disabled;
    let hint = move || {
        let label = props.new_thread_shortcut.get().map_or_else(
            || t("New thread"),
            |shortcut| {
                tr(
                    "New thread ({shortcut})",
                    &[("shortcut", Value::Text(shortcut))],
                )
            },
        );
        view()
            .app_class(tw("flex flex-col gap-0.5"))
            .child(text(label))
            .child(when(
                move || props.show_new_thread_in_project_hint.get(),
                move || {
                    text(move || {
                        props.new_thread_in_project_shortcut.get().map_or_else(
                            || t("New thread in current project: Shift+click"),
                            |shortcut| {
                                tr(
                                    "New thread in current project: Shift+click ({shortcut})",
                                    &[("shortcut", Value::Text(shortcut))],
                                )
                            },
                        )
                    })
                    .app_class(tw("text-muted-foreground"))
                },
            ))
    };
    let clear = props.on_clear_search;
    let input = props.search_input_ref;
    let new_thread_hint = tooltip_create_handle();
    let results_visible = move || props.is_searching.get() && props.search_result_count.get() > 0;
    view().app_class(tw("flex items-center gap-1")).test_id("sidebar-thread-header")
        .child(view().app_class(tw_with(Scope::Sidebar,
            "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-sidebar-muted-foreground hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-within:bg-sidebar-row-hover focus-within:text-sidebar-foreground focus-within:ring-1 focus-within:ring-inset focus-within:ring-ring/70", &[]))
            .child(icon(Icon::Search, 16.0, move || palette().sidebar_icon))
            // The frozen Input has no unstyled variant; put SidebarInput's child classes on NSH's input.
            .child(nsh_base::input().element_ref(&input).value(move || props.search_query.get())
                .role(AccessibilityRole::Combobox)
                .accessibility_state(move || AccessibilityState { expanded: Some(results_visible()), ..Default::default() })
                .accessibility_controls(move || results_visible().then(|| props.search_results_ref.get()).flatten())
                .accessibility_active_descendant(move || (results_visible() && props.active_search_result_index.get() < props.search_result_count.get())
                    .then(|| props.active_search_result_ref.get()).flatten())
                .on_change(move |value: &str| (props.on_search_query_change)(value.to_owned()))
                .on_key_down(move |event: &KeyEvent| (props.on_search_key_down)(event))
                .placeholder(t("Search")).label(t("Search threads")).test_id("sidebar-thread-search")
                .app_class(tw_with(Scope::Sidebar, "h-auto min-w-0 flex-1 bg-transparent p-0 text-sm font-medium leading-normal text-sidebar-foreground placeholder:text-sidebar-muted-foreground", &[])))
            .child(when(move || props.is_searching.get(), move || {
                let clear = clear.clone();
                // UI-BUTTON lacks icon-micro; preserve its 20px target and ghost-muted classes here.
                nsh_base::button().accessibility_label(t("Clear thread search"))
                    .on_press(move || { clear(); input.focus(); })
                    .app_class(tw("flex size-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"))
                    .child(icon(Icon::X, 12.0, move || palette().muted_foreground))
            })))
        .child(view().app_class(tw("flex shrink-0 items-center"))
            .child(when(move || props.has_projects.get(), move || untracked(&project_scope)))
            .child(when(move || props.has_projects.get(), move || {
                let on_new_project = props.on_new_project.clone();
                sidebar_header_icon_button(t("Add project"), || false, move || on_new_project(), icon(Icon::FolderPlus, 16.0, move || palette().sidebar_icon))
            }))
            .child(view().app_class(tw("relative flex shrink-0"))
                .tooltip(tooltip(new_thread_hint, TooltipOptions::default(), dynamic(move || hint().into_element())))
                .child(header_button(t("New thread"), move || disabled.get(), move || (props.on_new_thread)())
                .child(icon(Icon::SquarePen, 16.0, move || palette().sidebar_icon)))))
}

pub fn sidebar_header_icon_button(
    label: String,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let handle = tooltip_create_handle();
    view()
        .app_class(tw("relative flex shrink-0"))
        .tooltip(tooltip(
            handle,
            TooltipOptions::default(),
            text(label.clone()),
        ))
        .child(header_button(label, disabled, on_press).child(children))
}

fn header_button(
    label: String,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> nsh_base::ButtonElement {
    let disabled = Prop::bind(disabled);
    nsh_base::button().accessibility_label(label).disabled(disabled)
        .on_press(move || { if !disabled.get_untracked() { on_press(); } }).hit_slop(touch_slop)
        // Selector classes live on the icon; peer and icon-collapse variants have no matching state here.
        .app_class(tw_with(Scope::Sidebar,
            "relative flex w-full cursor-pointer items-center gap-[var(--sidebar-control-gap)] overflow-hidden text-left outline-hidden ring-ring hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-inset active:bg-sidebar-row-active active:text-sidebar-foreground disabled:pointer-events-none disabled:opacity-64 font-medium text-sidebar-muted-foreground/80 size-7 justify-center rounded-[var(--control-radius)] p-0 shrink-0", &[]))
}

#[derive(Clone)]
pub struct ProjectScopeProps {
    pub groups: Memo<Vec<ProjectGroup>>,
    pub selected: Memo<Vec<LogicalProjectKey>>,
    /// The list persists scope, clears selection and scrolls to the top in this callback.
    pub on_change: Rc<dyn Fn(Vec<LogicalProjectKey>)>,
    pub favicon_source: FaviconSource,
}

/// Port of Sidebar.tsx's projectScope slot using UI-MENU until UI-COMBOBOX lands.
pub fn sidebar_project_scope(props: ProjectScopeProps) -> impl IntoElement {
    let open = signal(false);
    let query = signal(String::new());
    let menu_state = signal(ScopeMenuState::default());
    let highlighted = signal(None::<LogicalProjectKey>);
    let toggle_scope = props.on_change.clone();
    let selected = props.selected;
    effect(move || {
        if open.get() {
            let toggle_scope = toggle_scope.clone();
            on_key_with(
                "shift+enter",
                KeyOptions {
                    when: nsh_core::keymap::When::Always,
                    for_focus: true,
                    ..Default::default()
                },
                move |_| {
                    if let Some(key) = highlighted.get_untracked() {
                        let mut keys = selected.get_untracked();
                        if keys.contains(&key) {
                            keys.retain(|candidate| candidate != &key);
                        } else {
                            keys.push(key);
                        }
                        toggle_scope(keys);
                    }
                },
            );
        }
    });
    let held = stored(Vec::<ProjectGroup>::new());
    let visible = computed(move || {
        let groups = props.groups.get();
        let groups: Vec<_> = pickable_project_groups(&groups)
            .into_iter()
            .cloned()
            .collect();
        let ordered = if open.get() {
            held.with(|old| {
                let mut next = old
                    .iter()
                    .filter_map(|old| groups.iter().find(|g| g.key == old.key).cloned())
                    .collect::<Vec<_>>();
                next.extend(
                    groups
                        .iter()
                        .filter(|g| !old.iter().any(|old| old.key == g.key))
                        .cloned(),
                );
                next
            })
        } else {
            groups
        };
        held.replace(ordered.clone());
        ordered
    });
    // Outside/Escape dismissal is external state: the client's reducer resets search on each transition.
    effect(move || {
        menu_state.update(|state| state.apply(ScopeMenuAction::Open(open.get())));
        // NSH delivers Select after closing; retain its row until that callback has run.
        if open.get() {
            query.set(menu_state.get_untracked().query);
        }
    });
    let selected = props.selected;
    let on_change = props.on_change.clone();
    let trigger_favicon = props.favicon_source.clone();
    let row_favicon = props.favicon_source.clone();
    menu::menu(
        header_button(t("Filter threads by project"), || false, || {})
            .accessibility_label(move || {
                let labels = props.groups.with(|groups| {
                    groups
                        .iter()
                        .filter(|group| selected.with(|keys| keys.contains(&group.key)))
                        .map(|group| group.label.clone())
                        .collect::<Vec<_>>()
                });
                Some(SharedString::from(if labels.is_empty() {
                    t("Filter threads by project")
                } else {
                    tr(
                        "Filter threads by project: {projects}",
                        &[("projects", Value::Text(labels.join(", ")))],
                    )
                }))
            })
            .child(dynamic(move || {
                let groups = props.groups.with(|groups| {
                    groups
                        .iter()
                        .filter(|g| selected.with(|keys| keys.contains(&g.key)))
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                });
                untracked(|| {
                    if groups.is_empty() {
                        return svg(Icon::ListFilter.asset())
                            .accessible(false)
                            .app_class(tw("size-4 text-(--sidebar-icon-color)"))
                            .into_element();
                    }
                    let count = groups.len();
                    view()
                        .app_class(tw("flex shrink-0"))
                        .children(groups.into_iter().enumerate().map(|(index, group)| {
                            view()
                                .app_class(tw(if index == 0 {
                                    "flex"
                                } else {
                                    "-ml-1 flex bg-sidebar ring-2 ring-sidebar"
                                }))
                                // NSH does not parse rounded-[25%]; the favicon owns its corner radius.
                                .child(scope_favicon(
                                    group,
                                    trigger_favicon.clone(),
                                    if count > 1 { "size-3.5" } else { "size-4" },
                                ))
                                .into_element()
                        }))
                        .into_element()
                })
            })),
        MenuOptions {
            open: Some(open),
            align: Align::Start,
            ..Default::default()
        },
        menu::menu_popup_with(move || {
            let on_change = on_change.clone();
            let row_favicon = row_favicon.clone();
            view()
                .w(px(272.0))
                .max_w(move || px((window().width.0 - 40.0).max(0.0)))
                .child(menu::menu_search_input(
                    query,
                    t("Search projects..."),
                    t("Search projects"),
                    || false,
                ))
                .child(dynamic(move || {
                    let groups = visible.get();
                    let mut items = vec![(None, t("All projects"))];
                    items.extend(
                        groups
                            .iter()
                            .map(|group| (Some(group.key.clone()), t(&group.label))),
                    );
                    let matches = filter_project_scope_items(
                        &items,
                        &query.get(),
                        |item| if item.0.is_none() { "all" } else { "project" },
                        |item, query| item.1.to_lowercase().contains(&query.to_lowercase()),
                    );
                    if matches.is_empty() {
                        return view()
                            .app_class(tw("px-2 py-6 text-center text-sm text-muted-foreground"))
                            .child(text(t("No matching projects.")))
                            .into_element();
                    }
                    let rows = matches
                        .into_iter()
                        .map(|(key, label)| {
                            let key = key.clone();
                            let selected_key = key.clone();
                            let focus_key = key.clone();
                            let row = element_ref();
                            row.on_focus_in(move || highlighted.set(focus_key.clone()));
                            let action = on_change.clone();
                            let glyph = groups
                                .iter()
                                .find(|group| Some(&group.key) == key.as_ref())
                                .map_or_else(
                                    || {
                                        icon(Icon::Folder, 16.0, move || palette().muted_foreground)
                                            .into_element()
                                    },
                                    |group| {
                                        scope_favicon(
                                            group.clone(),
                                            row_favicon.clone(),
                                            "size-4 shrink-0",
                                        )
                                        .into_element()
                                    },
                                );
                            view()
                                .element_ref(&row)
                                .child(menu::menu_item(
                                    label.clone(),
                                    ItemOptions::default(),
                                    view()
                                        .app_class(tw("flex w-full min-w-0 items-center gap-2"))
                                        .child(project_scope_row_icon(
                                            move || {
                                                selected_key.as_ref().map_or_else(
                                                    || selected.with(Vec::is_empty),
                                                    |key| selected.with(|keys| keys.contains(key)),
                                                )
                                            },
                                            glyph,
                                        ))
                                        .child(
                                            text(label.clone())
                                                .app_class(tw("min-w-0 truncate text-sm")),
                                        )
                                        .child(
                                            view()
                                                .app_class(tw("ml-auto size-7 shrink-0 sm:size-6")),
                                        ),
                                    // UI-MENU's Select omits pointer modifiers; Shift+Enter handles multi-scope.
                                    move || action(key.clone().into_iter().collect()),
                                ))
                                .into_element()
                        })
                        .collect::<Vec<_>>();
                    view().children(rows).into_element()
                }))
        }),
    )
}

fn project_scope_row_icon(
    selected: impl Fn() -> bool + 'static,
    glyph: impl IntoElement,
) -> impl IntoElement {
    view().app_class(tw("relative flex shrink-0"))
        .child(glyph)
        .child(when(selected, || view().app_class(tw("pointer-events-none absolute -right-1 -bottom-1 grid size-3 place-items-center rounded-full bg-primary ring-2 ring-popover"))
            .child(chrome_glyph(CHECK).app_class(tw("size-2 text-primary-foreground")))))
}

pub type FaviconSource = Rc<dyn Fn(&ProjectFaviconKey) -> Option<String>>;

fn scope_favicon(group: ProjectGroup, source: FaviconSource, class_name: &str) -> impl IntoElement {
    let mut project = ProjectFaviconProject::from_shell(
        group.representative.environment,
        &group.representative.project,
    );
    project.title = group.label;
    project_favicon(
        move || project.clone(),
        move |key| source(key),
        class_name,
        None,
    )
}

#[derive(Default)]
pub struct ChromeFooterSlots {
    pub provider_update: Option<AnyElement>,
    pub architecture_warning: Option<AnyElement>,
    pub undo_notice: Option<AnyElement>,
    pub update: Option<AnyElement>,
}

pub fn sidebar_chrome_footer(
    pull_requests_supported: Prop<bool>,
    main_app_route: Prop<Route>,
    slots: ChromeFooterSlots,
) -> impl IntoElement {
    view().app_class(tw("flex shrink-0 flex-col gap-2 px-[var(--sidebar-content-inset)] pt-1 pb-[var(--sidebar-content-inset)]"))
        .test_id("sidebar-chrome-footer").child(slots.provider_update).child(slots.architecture_warning)
        .child(sidebar_utility_menu(pull_requests_supported, main_app_route, slots.undo_notice, slots.update))
}

pub fn sidebar_utility_menu(
    pull_requests_supported: Prop<bool>,
    main_app_route: Prop<Route>,
    undo_notice: Option<AnyElement>,
    update: Option<AnyElement>,
) -> impl IntoElement {
    let nav = navigator();
    let utility = computed(move || is_sidebar_utility_page(&nav.top()));
    view().app_class(tw("flex w-full min-w-0 flex-row items-center gap-1"))
        .child(when(move || utility.get(), move || {
            view().app_class(tw("relative min-w-0 flex-1"))
                .child(nsh_base::button().accessibility_label(t("Back")).on_press(move || nav.open(main_app_route.get_untracked()))
                    .app_class(tw_with(Scope::Sidebar, "flex w-full h-8 items-center gap-[var(--sidebar-control-gap)] rounded-[var(--control-radius)] px-[var(--sidebar-row-content-inset)] py-1.5 text-sm font-medium text-sidebar-muted-foreground/80 hover:bg-sidebar-row-hover hover:text-sidebar-foreground outline-hidden ring-ring focus-visible:ring-2 focus-visible:ring-inset", &[]))
                    .child(icon(Icon::ArrowLeft, 16.0, move || palette().sidebar_icon)).child(text(t("Back"))))
        }).otherwise(move || view().app_class(tw("flex items-center gap-1"))
            .child(sidebar_utility_item(t("Settings"), move || nav.open(Route::Settings(SettingsPage::General)), icon(Icon::Settings, 16.0, move || palette().sidebar_icon)))
            .child(when(move || pull_requests_supported.get(), move || sidebar_utility_item(t("Pull requests"), move || {
                if pull_requests_supported.get_untracked() { nav.open(Route::PullRequests); }
            }, svg(Icon::GitPullRequestArrow.asset()).accessible(false).app_class(tw("size-4 text-(--sidebar-icon-color)")))))
            .child(sidebar_utility_item(t("Usage"), move || nav.open(Route::Usage), svg(Icon::ChartNoAxesColumn.asset()).accessible(false).app_class(tw("size-4 text-(--sidebar-icon-color)"))))))
        .child(undo_notice).child(update)
}

/// Port of sidebar/mainAppLocation.ts isSidebarUtilityPage; settings and usage are modal pages.
pub fn is_sidebar_utility_page(route: &Route) -> bool {
    matches!(
        route,
        Route::ProjectLink(_) | Route::AddProject | Route::PullRequests
    )
}

pub fn sidebar_utility_item(
    label: String,
    on_press: impl Fn() + 'static,
    children: impl IntoElement,
) -> impl IntoElement {
    let handle = tooltip_create_handle();
    // NSH Button has no tooltip slot; the fitted noninteractive wrapper anchors the hint.
    view().app_class(tw("relative shrink-0"))
        .tooltip(tooltip(handle, TooltipOptions::default(), text(label.clone())))
        .child(nsh_base::button().accessibility_label(label.clone()).on_press(on_press).hit_slop(touch_slop)
            .app_class(tw_with(Scope::Sidebar, "flex size-8 items-center justify-center rounded-[var(--control-radius)] p-0 font-medium text-sidebar-muted-foreground/80 hover:bg-sidebar-row-hover hover:text-sidebar-foreground active:bg-sidebar-row-active active:text-sidebar-foreground outline-hidden ring-ring focus-visible:ring-2 focus-visible:ring-inset", &[]))
            .child(children))
}

fn touch_slop() -> EdgeInsets {
    EdgeInsets::all(px(if pointer() == Pointer::Coarse {
        10.0
    } else {
        0.0
    }))
}

fn chrome_glyph(path: &'static str) -> nsh_core::element::Svg {
    // The project-scope selection badge uses the web's heavier check stroke.
    svg(ImageSrc::svg_markup(&format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#))).accessible(false)
}

const CHECK: &str = r#"<path d="m6 12 4 4 8-8" stroke-width="4"/>"#;

const DEV_ART: &str = r##"<svg
      width="100%" height="100%"
      fill="none"
      preserveAspectRatio="xMinYMin slice"
      viewBox="0 0 8192 96"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient
          id="paperId"
          x1="60"
          y1="0"
          x2="220"
          y2="96"
          gradientUnits="userSpaceOnUse"
          spreadMethod="reflect"
        >
          <stop stop-color="var(--stage-art-bottom)" />
          <stop offset="0.5" stop-color="var(--stage-art-mid)" />
          <stop offset="1" stop-color="var(--stage-art-top)" />
        </linearGradient>
        <radialGradient
          id="glowId"
          cx="0"
          cy="0"
          r="1"
          gradientTransform="translate(216 14) rotate(137) scale(120 84)"
          gradientUnits="userSpaceOnUse"
        >
          <stop stop-color="var(--stage-art-highlight)" stop-opacity="0.4" />
          <stop
            offset="0.52"
            stop-color="var(--stage-art-secondary)"
            stop-opacity="0.16"
          />
          <stop offset="1" stop-color="var(--stage-art-bottom)" stop-opacity="0" />
        </radialGradient>
        <radialGradient
          id="celesteGlowId"
          cx="0"
          cy="0"
          r="1"
          gradientTransform="translate(474 44) rotate(166) scale(156 92)"
          gradientUnits="userSpaceOnUse"
        >
          <stop stop-color="var(--stage-art-celeste-highlight)" stop-opacity="0.34" />
          <stop
            offset="0.5"
            stop-color="var(--stage-art-celeste-secondary)"
            stop-opacity="0.18"
          />
          <stop offset="1" stop-color="var(--stage-art-bottom)" stop-opacity="0" />
        </radialGradient>
        <radialGradient
          id="violetGlowId"
          cx="0"
          cy="0"
          r="1"
          gradientTransform="translate(704 18) rotate(145) scale(132 88)"
          gradientUnits="userSpaceOnUse"
        >
          <stop stop-color="var(--stage-art-violet-highlight)" stop-opacity="0.3" />
          <stop
            offset="0.52"
            stop-color="var(--stage-art-tertiary)"
            stop-opacity="0.14"
          />
          <stop offset="1" stop-color="var(--stage-art-bottom)" stop-opacity="0" />
        </radialGradient>
        <pattern id="minorGridId" width="8" height="8" patternUnits="userSpaceOnUse">
          <path
            d="M8 0H0V8"
            stroke="var(--stage-art-grid-line)"
            stroke-opacity="0.14"
            stroke-width="0.5"
          />
        </pattern>
        <pattern id="majorGridId" width="32" height="32" patternUnits="userSpaceOnUse">
          <path
            d="M32 0H0V32"
            stroke="var(--stage-art-grid-line)"
            stroke-opacity="0.26"
            stroke-width="0.6"
          />
        </pattern>
        <pattern id="rulerId" width="32" height="6" patternUnits="userSpaceOnUse">
          <path
            d="M4 0V2.5M12 0V2.5M20 0V4M28 0V2.5"
            stroke="var(--stage-art-line)"
            stroke-opacity="0.5"
            stroke-width="0.5"
          />
        </pattern>
        <pattern id="glowsId" width="768" height="96" patternUnits="userSpaceOnUse">
          <rect width="768" height="96" fill="url(#glowId)" />
          <rect width="768" height="96" fill="url(#celesteGlowId)" />
          <rect width="768" height="96" fill="url(#violetGlowId)" />
        </pattern>
        <pattern id="annotationsId" width="768" height="96" patternUnits="userSpaceOnUse">
          <g
            stroke="var(--stage-art-line)"
            stroke-linecap="round"
            stroke-opacity="0.6"
            stroke-width="0.7"
          >
            <path d="M180 64H264" stroke-dasharray="5 4" />
            <path d="M180 61V67M264 61V67" />
            <path d="M276 10V44" stroke-dasharray="4 4" stroke-opacity="0.5" />
            <path d="M273 10H279M273 44H279" stroke-opacity="0.5" />
            <path d="M348 30H428" stroke-dasharray="3.5 5" stroke-opacity="0.5" />
            <path d="M348 27V33M428 27V33" stroke-opacity="0.5" />
            <path d="M512 48V80" stroke-dasharray="5 3" stroke-opacity="0.45" />
            <path d="M509 48H515M509 80H515" stroke-opacity="0.45" />
            <path d="M590 70H724" stroke-dasharray="7 4" stroke-opacity="0.55" />
            <path d="M590 67V73M724 67V73" stroke-opacity="0.55" />
          </g>

          <g
            stroke="var(--stage-art-line)"
            stroke-linecap="round"
            stroke-opacity="0.55"
            stroke-width="0.6"
          >
            <g>
              <path d="M34 60L38 64M38 60L34 64" />
            </g>
            <g>
              <path d="M228 26H234M231 23V29" />
            </g>
            <g>
              <path d="M143 51H149M146 48V54" />
            </g>
            <g>
              <path d="M316 16L322 22M322 16L316 22" />
            </g>
            <g>
              <path d="M468 70H476M472 66V74" />
            </g>
            <g>
              <path d="M558 28L564 34M564 28L558 34" />
            </g>
            <g>
              <path d="M742 44H750M746 40V48" />
            </g>
          </g>

          <g stroke="var(--stage-art-line)" stroke-opacity="0.35" stroke-width="0.6">
            <circle cx="196" cy="38" r="13" stroke-dasharray="3.5 4" />
            <path d="M196 33V43M191 38H201" stroke-opacity="0.6" stroke-width="0.4" />
            <circle cx="414" cy="64" r="10" stroke-dasharray="2.5 3.5" />
            <path d="M414 60V68M410 64H418" stroke-opacity="0.6" stroke-width="0.4" />
            <circle cx="648" cy="32" r="15" stroke-dasharray="4 5" />
            <path d="M648 26V38M642 32H654" stroke-opacity="0.6" stroke-width="0.4" />
          </g>
        </pattern>
      </defs>

      <rect width="100%" height="96" fill="url(#paperId)" />
      <rect width="100%" height="96" fill="url(#glowsId)" />
      <rect width="100%" height="96" fill="url(#minorGridId)" />
      <rect width="100%" height="96" fill="url(#majorGridId)" />
      <rect width="100%" height="6" fill="url(#rulerId)" />
      <rect width="100%" height="96" fill="url(#annotationsId)" />
    </svg>"##;

const NIGHTLY_ART: &str = r##"<svg
      width="100%" height="100%"
      fill="none"
      preserveAspectRatio="xMinYMin slice"
      viewBox="0 0 8192 96"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient
          id="skyId"
          x1="24"
          y1="0"
          x2="264"
          y2="96"
          gradientUnits="userSpaceOnUse"
          spreadMethod="reflect"
        >
          <stop stop-color="var(--stage-night-bottom)" />
          <stop offset="0.5" stop-color="var(--stage-night-mid)" />
          <stop offset="1" stop-color="var(--stage-night-top)" />
        </linearGradient>
        <radialGradient
          id="glowId"
          cx="0"
          cy="0"
          r="1"
          gradientTransform="translate(216 18) rotate(137) scale(120 84)"
          gradientUnits="userSpaceOnUse"
        >
          <stop stop-color="var(--stage-night-glow-highlight)" stop-opacity="0.4" />
          <stop
            offset="0.5"
            stop-color="var(--stage-night-glow-secondary)"
            stop-opacity="0.16"
          />
          <stop offset="1" stop-color="var(--stage-night-bottom)" stop-opacity="0" />
        </radialGradient>
        <linearGradient id="cloudId" x1="0" y1="60" x2="288" y2="96" gradientUnits="userSpaceOnUse">
          <stop stop-color="var(--stage-night-highlight)" stop-opacity="0.5" />
          <stop
            offset="0.52"
            stop-color="var(--stage-night-secondary)"
            stop-opacity="0.62"
          />
          <stop offset="1" stop-color="var(--stage-night-tertiary)" stop-opacity="0.5" />
        </linearGradient>
        <filter id="softId" x="-24" y="-24" width="336" height="144" filterUnits="userSpaceOnUse">
          <feGaussianBlur stdDeviation="4" />
        </filter>
        <pattern id="starsId" width="288" height="96" patternUnits="userSpaceOnUse">
          <g fill="var(--stage-night-line)">
            <circle cx="14" cy="10" r="0.6" fill-opacity="0.85"/><circle cx="38" cy="22" r="0.4" fill-opacity="0.55"/><circle cx="58" cy="8" r="0.5" fill-opacity="0.7"/><circle cx="84" cy="16" r="0.4" fill-opacity="0.5"/><circle cx="104" cy="7" r="0.6" fill-opacity="0.8"/><circle cx="126" cy="20" r="0.4" fill-opacity="0.55"/><circle cx="148" cy="11" r="0.5" fill-opacity="0.7"/><circle cx="170" cy="24" r="0.4" fill-opacity="0.5"/><circle cx="192" cy="9" r="0.6" fill-opacity="0.8"/><circle cx="214" cy="18" r="0.4" fill-opacity="0.55"/><circle cx="236" cy="8" r="0.5" fill-opacity="0.7"/><circle cx="258" cy="20" r="0.45" fill-opacity="0.6"/><circle cx="278" cy="11" r="0.55" fill-opacity="0.75"/><circle cx="26" cy="34" r="0.4" fill-opacity="0.45"/><circle cx="118" cy="34" r="0.4" fill-opacity="0.45"/><circle cx="202" cy="32" r="0.4" fill-opacity="0.5"/><circle cx="268" cy="34" r="0.4" fill-opacity="0.45"/>
          </g>
          <g
            stroke="var(--stage-night-sparkle)"
            stroke-linecap="round"
            stroke-opacity="0.7"
            stroke-width="0.6"
          >
            <path d="M68.5 28H71.5M70 26.5V29.5"/><path d="M158.5 36H161.5M160 34.5V37.5"/><path d="M244.5 26H247.5M246 24.5V27.5"/>
          </g>
        </pattern>
        <pattern id="glowsId" width="640" height="96" patternUnits="userSpaceOnUse">
          <rect width="640" height="96" fill="url(#glowId)" />
        </pattern>
      </defs>

      <rect width="100%" height="96" fill="url(#skyId)" />
      <rect width="100%" height="96" fill="url(#glowsId)" />
      <rect width="100%" height="96" fill="url(#starsId)" />

      <g filter="url(#softId)">
        <path
          d="M-12 88C-12 74 0 63 14 63C18 50 30 41 44 41C58 41 70 49 74 62C79 57 86 54 94 54C110 54 123 66 124 82C132 83 138 88 141 96H-12V88Z"
          fill="url(#cloudId)"
        />
      </g>
      <g filter="url(#softId)">
        <path
          d="M150 96C151 84 161 75 173 75C176 64 186 57 198 57C210 57 220 64 223 75C231 75 238 80 241 87C250 87 257 91 260 96H150Z"
          fill="url(#cloudId)"
          fill-opacity="0.8"
        />
      </g>
    </svg>"##;

const WORDMARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="15.5309 37 94.3941 56.96">
      <path
        d="M33.4509 93V47.56H15.5309V37H64.3309V47.56H46.4109V93H33.4509ZM86.7253 93.96C82.832 93.96 78.9653 93.4533 75.1253 92.44C71.2853 91.3733 68.032 89.88 65.3653 87.96L70.4053 78.04C72.5386 79.5867 75.0186 80.8133 77.8453 81.72C80.672 82.6267 83.5253 83.08 86.4053 83.08C89.6586 83.08 92.2186 82.44 94.0853 81.16C95.952 79.88 96.8853 78.12 96.8853 75.88C96.8853 73.7467 96.0586 72.0667 94.4053 70.84C92.752 69.6133 90.0853 69 86.4053 69H80.4853V60.44L96.0853 42.76L97.5253 47.4H68.1653V37H107.365V45.4L91.8453 63.08L85.2853 59.32H89.0453C95.9253 59.32 101.125 60.8667 104.645 63.96C108.165 67.0533 109.925 71.0267 109.925 75.88C109.925 79.0267 109.099 81.9867 107.445 84.76C105.792 87.48 103.259 89.6933 99.8453 91.4C96.432 93.1067 92.0586 93.96 86.7253 93.96Z"
        fill="currentColor"
      />
    </svg>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_artwork_and_pills_follow_the_web_modes_without_guessing_unknown_channels() {
        use EnvironmentIdentificationMode::*;
        use StageBackdropVariant::*;
        for (label, mode, expected) in [
            (" Dev ", Artwork, Some(Dev)),
            ("NIGHTLY", Artwork, Some(Nightly)),
            ("latest", Artwork, Option::None),
            ("Beta", Artwork, Option::None),
            ("unknown", DevArtwork, Some(Dev)),
            ("latest", NightlyArtwork, Some(Nightly)),
            ("Dev", Pill, Option::None),
            ("Nightly", None, Option::None),
        ] {
            assert_eq!(
                resolve_sidebar_stage_backdrop_variant(label, &mode),
                expected
            );
        }
        assert_eq!(
            resolve_environment_identification_pill_label(" dev "),
            Some("Dev")
        );
        assert_eq!(
            resolve_environment_identification_pill_label("NIGHTLY"),
            Some("Nightly")
        );
        assert_eq!(
            resolve_environment_identification_pill_label("latest"),
            Option::None
        );
        assert_eq!(
            resolve_environment_identification_pill_label("unknown"),
            Option::None
        );
    }

    #[test]
    fn settings_and_usage_keep_utilities_while_project_and_pr_pages_offer_back() {
        for route in [
            Route::PullRequests,
            Route::AddProject,
            Route::ProjectLink("key".into()),
        ] {
            assert!(is_sidebar_utility_page(&route));
        }
        for route in [
            Route::Home,
            Route::NewThread,
            Route::Usage,
            Route::Settings(SettingsPage::General),
        ] {
            assert!(!is_sidebar_utility_page(&route));
        }
    }

    #[test]
    fn every_stage_art_color_is_a_theme_token() {
        let tokens = crate::theme::tokens_for(&crate::theme::ThemeChoice::default());
        for source in [DEV_ART, NIGHTLY_ART] {
            for variable in source.split("var(--").skip(1) {
                let name = variable.split(')').next().expect("token name");
                assert!(tokens.color(name).is_some(), "missing stage token {name}");
            }
        }
    }
}
