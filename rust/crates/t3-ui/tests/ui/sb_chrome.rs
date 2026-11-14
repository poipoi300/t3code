//! SB-CHROME: the production components mounted directly while SB-LIST owns app composition.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::{
    types::EnvironmentId,
    view_logic::shell::navigation::{Direction, HistoryAvailability},
    workspace::projects::{
        EnvironmentProject, GroupingSettings, LogicalProjectKey, build_project_groups,
    },
};
use t3_contracts::settings::EnvironmentIdentificationMode;
use t3_ui::{
    i18n::t,
    screens::sidebar::chrome::*,
    state::{
        nav::{Navigator, Route},
        panels::Panels,
    },
    theme,
};

use crate::support::{DESKTOP, PHONE};

fn fixture(root: Root, window: (f32, f32)) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    let mut app = Headless::start(config).expect("chrome fixture starts");
    app.first_render().expect("first render");
    app
}

fn context() -> (Navigator, Panels) {
    theme::install(&theme::ThemeChoice::default());
    let nav = Navigator::new(Route::Home);
    let panels = Panels::new();
    provide_context(nav);
    provide_context(panels);
    (nav, panels)
}

#[component]
fn ThreadHeader() -> impl IntoElement {
    let (nav, _) = context();
    let query = signal(String::new());
    let projects = signal(true);
    let props = SidebarThreadHeaderProps::new(query, Prop::bind(move || projects.get()));
    view()
        .w_full()
        .h_full()
        .child(view().w(px(256.0)).child(sidebar_thread_header(props, || {
            sidebar_header_icon_button(
                t("Project scope fixture"),
                || false,
                || {},
                text(t("Projects")),
            )
        })))
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle projects"))
                .on_press(move || projects.update(|value| *value = !*value))
                .child(text(t("Toggle projects"))),
        )
        .child(text(move || format!("query:{}", query.get())))
        .child(text(move || format!("route:{}", nav.top().path())))
}

#[test]
fn search_clear_restores_focus_and_project_controls_return_after_empty_state() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(Root::of::<ThreadHeader>(), window);
        app.type_text("label:Search threads", "clamp")
            .expect("search");
        app.wait_for("text:query:clamp")
            .expect("query reaches caller");
        app.press("role:button[Clear thread search]")
            .expect("clear");
        assert!(
            !app.screen()
                .matches("text:query:clamp")
                .expect("valid selector")
        );
        assert!(app.press("role:button[Clear thread search]").is_err());
        app.press_key("c")
            .expect("typing resumes in the search field");
        app.wait_for("text:query:c").expect("input was refocused");
        app.press("role:button[Toggle projects]")
            .expect("remove projects");
        assert!(app.press("role:button[New thread]").is_err());
        assert!(
            !app.screen()
                .matches("role:button[Add project]")
                .expect("valid selector")
        );
        assert!(
            !app.screen()
                .matches("role:button[Project scope fixture]")
                .expect("valid selector")
        );
        assert!(
            app.screen()
                .matches("text:route:/")
                .expect("valid selector"),
            "disabled action did not navigate"
        );
        app.press("role:button[Toggle projects]")
            .expect("projects load again");
        app.press("role:button[Project scope fixture]")
            .expect("scope remounted");
        app.press("role:button[Add project]")
            .expect("add project route");
        app.wait_for("text:route:@add-project")
            .expect("add project destination");
        app.press("role:button[New thread]")
            .expect("new thread route");
        app.wait_for("text:route:/new")
            .expect("new thread destination");
    }
}

#[component]
fn ProjectScope() -> impl IntoElement {
    context();
    let projects = [
        ("local", "same-id", "t3code", "/repo/t3code", false),
        ("remote", "same-id", "nsh-spikes", "/repo/nsh", false),
        ("local", "archive", "Archived project", "/repo/old", true),
    ]
    .map(
        |(environment, id, title, root, archived)| EnvironmentProject {
            environment: EnvironmentId(environment.into()),
            is_scratch: false,
            project: serde_json::from_value(serde_json::json!({
                "id":id, "title":title, "workspaceRoot":root, "repositoryIdentity":null,
                "defaultModelSelection":null, "scripts":[],
                "archivedAt":if archived { Some("2026-11-13T12:00:00.000Z") } else { None },
                "createdAt":"2026-11-01T12:00:00.000Z", "updatedAt":"2026-11-13T12:00:00.000Z"
            }))
            .expect("project shell"),
        },
    );
    let groups =
        computed(move || build_project_groups(&projects, &GroupingSettings::default(), None, true));
    let selected = signal(Vec::<LogicalProjectKey>::new());
    view()
        .w_full()
        .h_full()
        .p(px(12.0))
        .child(sidebar_project_scope(ProjectScopeProps {
            groups,
            selected: computed(move || selected.get()),
            on_change: Rc::new(move |keys| selected.set(keys)),
            favicon_source: Rc::new(|_| None),
        }))
        .child(text(move || {
            let labels = groups.with(|groups| {
                groups
                    .iter()
                    .filter(|group| selected.with(|keys| keys.contains(&group.key)))
                    .map(|g| g.label.clone())
                    .collect::<Vec<_>>()
            });
            format!(
                "scope:{}",
                if labels.is_empty() {
                    "all".into()
                } else {
                    labels.join(",")
                }
            )
        }))
}

#[test]
fn scope_search_selects_scoped_identity_clears_to_all_and_resets_on_reopen() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(Root::of::<ProjectScope>(), window);
        app.press("role:button[Filter threads by project]")
            .expect("open scope");
        app.wait_for("role:menuitem[All projects]")
            .expect("all scope");
        assert!(
            !app.screen()
                .matches("role:menuitem[Archived project]")
                .expect("valid selector")
        );
        app.type_text("label:Search projects", "NSH")
            .expect("filter case insensitively");
        app.wait_for("role:menuitem[nsh-spikes]")
            .expect("matching project");
        assert!(
            !app.screen()
                .matches("role:menuitem[All projects]")
                .expect("valid selector")
        );
        assert!(
            !app.screen()
                .matches("role:menuitem[t3code]")
                .expect("valid selector")
        );
        app.press("role:menuitem[nsh-spikes]")
            .expect("select remote project");
        app.wait_for("text:scope:nsh-spikes")
            .expect("selected remote identity");
        assert!(
            !app.screen()
                .matches("role:menuitem[nsh-spikes]")
                .expect("valid selector"),
            "ordinary selection closes"
        );
        app.press("role:button[Filter threads by project: nsh-spikes]")
            .expect("reopen");
        app.wait_for("role:menuitem[t3code]").expect("query reset");
        app.press("role:menuitem[All projects]")
            .expect("clear scope");
        app.wait_for("text:scope:all")
            .expect("all projects restored");
        app.press("role:button[Filter threads by project]")
            .expect("open");
        app.type_text("label:Search projects", "missing-project")
            .expect("no matches");
        app.wait_for("No matching projects.")
            .expect("honest empty state");
        app.press_key("escape").expect("dismiss");
        app.press("role:button[Filter threads by project]")
            .expect("reopen after escape");
        app.wait_for("role:menuitem[All projects]")
            .expect("query reset after outside state change");
    }
}

#[test]
fn shift_enter_toggles_multiple_project_scopes_without_dismissing_the_menu() {
    let mut app = fixture(Root::of::<ProjectScope>(), DESKTOP);
    app.press("role:button[Filter threads by project]")
        .expect("open");
    app.press_key("down").expect("all projects");
    app.press_key("down").expect("first project");
    app.press_key("shift+enter").expect("toggle first scope");
    app.wait_for("text:scope:t3code")
        .expect("first scope chosen");
    assert_eq!(app.screen().matches("role:menuitem[nsh-spikes]"), Ok(true));
    app.press_key("down").expect("second project");
    app.press_key("shift+enter").expect("extend scope");
    app.wait_for("text:scope:t3code,nsh-spikes")
        .expect("both environments selected");
    app.press_key("shift+enter").expect("toggle second off");
    app.wait_for("text:scope:t3code")
        .expect("first scope remains");
    app.press_key("enter").expect("ordinary selection");
    app.wait_for("text:scope:nsh-spikes")
        .expect("scope replaced");
    assert_eq!(app.screen().matches("role:menuitem[nsh-spikes]"), Ok(false));
}

#[component]
fn Footer() -> impl IntoElement {
    let (nav, _) = context();
    let supported = signal(true);
    view()
        .w_full()
        .h_full()
        .child(sidebar_chrome_footer(
            Prop::bind(move || supported.get()),
            Route::NewThread.into(),
            ChromeFooterSlots::default(),
        ))
        .child(
            nsh_base::button()
                .accessibility_label(t("Toggle pull requests"))
                .on_press(move || supported.update(|value| *value = !*value))
                .child(text(t("Toggle pull requests"))),
        )
        .child(text(move || format!("route:{}", nav.top().path())))
}

#[test]
fn utilities_navigate_and_back_returns_to_main_with_live_capability_changes() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(Root::of::<Footer>(), window);
        app.press("role:button[Settings]").expect("settings");
        app.wait_for("text:route:/settings/general")
            .expect("general settings route");
        assert!(
            !app.screen()
                .matches("role:button[Usage]")
                .expect("valid selector"),
            "Usage stays hidden until its screen exists"
        );
        app.press("role:button[Pull requests]")
            .expect("pull requests");
        app.wait_for("role:button[Back]")
            .expect("reverse utility state");
        assert!(
            !app.screen()
                .matches("role:button[Settings]")
                .expect("valid selector")
        );
        app.press("role:button[Back]").expect("return to main");
        app.wait_for("text:route:/new")
            .expect("remembered main route");
        app.press("role:button[Toggle pull requests]")
            .expect("capability removed");
        assert!(app.press("role:button[Pull requests]").is_err());
        assert!(
            app.screen()
                .matches("text:route:/new")
                .expect("valid selector")
        );
        app.press("role:button[Toggle pull requests]")
            .expect("capability returns");
        app.press("role:button[Pull requests]")
            .expect("available again");
        app.wait_for("text:route:/pull-requests")
            .expect("destination");
    }
}

#[component]
fn ChromeHeader() -> impl IntoElement {
    let (_, panels) = context();
    let available = signal(false);
    let chosen = signal(String::new());
    view()
        .w_full()
        .h_full()
        .child(sidebar_chrome_header(ChromeHeaderProps {
            stage_label: "Dev".to_owned().into(),
            identification_mode: EnvironmentIdentificationMode::Artwork.into(),
            history: Some(HistoryNavigation {
                availability: Rc::new(move || HistoryAvailability {
                    back: available.get(),
                    forward: false,
                }),
                back_shortcut: None.into(),
                forward_shortcut: None.into(),
                navigate: Rc::new(move |direction| {
                    chosen.set(
                        if direction == Direction::Back {
                            "back"
                        } else {
                            "forward"
                        }
                        .into(),
                    )
                }),
            }),
        }))
        .child(
            nsh_base::button()
                .accessibility_label(t("Enable history"))
                .on_press(move || available.set(true))
                .child(text(t("Enable history"))),
        )
        .child(text(move || {
            format!("sidebar:{}", panels.sidebar_open.get())
        }))
        .child(text(move || format!("history:{}", chosen.get())))
}

#[test]
fn collapse_and_history_respect_layout_and_availability() {
    let mut app = fixture(Root::of::<ChromeHeader>(), DESKTOP);
    assert!(app.press("role:button[Go back]").is_err());
    assert!(app.press("role:button[Go forward]").is_err());
    assert!(
        !app.screen()
            .matches("text:history:back")
            .expect("valid selector")
    );
    app.press("role:button[Enable history]")
        .expect("history becomes available");
    app.press("role:button[Go back]").expect("back");
    app.wait_for("text:history:back").expect("history callback");
    assert!(app.press("role:button[Go forward]").is_err());
    app.press("role:button[Toggle main sidebar]")
        .expect("collapse");
    app.wait_for("text:sidebar:false")
        .expect("panel state changed");
    app.press("role:button[Toggle main sidebar]")
        .expect("expand");
    app.wait_for("text:sidebar:true").expect("reverse state");

    let mut app = fixture(Root::of::<ChromeHeader>(), PHONE);
    assert!(app.press("role:button[Toggle main sidebar]").is_err());
    assert!(
        app.screen()
            .matches("text:sidebar:true")
            .expect("valid selector"),
        "the compact root page cannot disappear"
    );
}
