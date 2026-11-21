//! File/content search interactions from client projections, without fixture-supplied rows.

use crate::support::{DESKTOP, PHONE};
use futures::{
    StreamExt,
    channel::mpsc,
    stream::{self, LocalBoxStream},
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use t3_client::{
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::shell::palette_files::{
        ContentSearchView, FileSearchView, PaletteSearchClient, SearchOptions, SearchTarget,
    },
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    project::{ProjectContentMatch, ProjectEntry, ProjectEntryKind},
};
use t3_ui::{
    screens::palette::files::{
        PaletteSearchRuntime, project_content_search_dialog, project_file_picker,
    },
    theme::{self, ThemeChoice},
};

#[derive(Default)]
struct Queries {
    files: RefCell<Vec<(String, mpsc::UnboundedSender<FileSearchView>)>>,
    allowed: Cell<bool>,
    latest_options: RefCell<SearchOptions>,
}

fn target() -> SearchTarget {
    SearchTarget {
        thread: ThreadKey {
            environment: EnvironmentId("remote".into()),
            thread: ThreadId("thread".into()),
        },
        cwd: "/repo".into(),
        project_name: "Settings".into(),
    }
}

impl PaletteSearchClient for Queries {
    fn watch_target(&self) -> LocalBoxStream<'static, Option<SearchTarget>> {
        stream::once(async { Some(target()) })
            .chain(stream::pending())
            .boxed_local()
    }
    fn files(&self, _: SearchTarget, query: String) -> LocalBoxStream<'static, FileSearchView> {
        let (send, receive) = mpsc::unbounded();
        self.files.borrow_mut().push((query, send));
        receive.boxed_local()
    }
    fn contents(
        &self,
        _: SearchTarget,
        _: String,
        options: SearchOptions,
    ) -> LocalBoxStream<'static, ContentSearchView> {
        *self.latest_options.borrow_mut() = options;
        stream::once(async {
            ContentSearchView {
                can_read_files: true,
                has_query: true,
                matches: Rc::new(
                    ["a.ts", "b.ts", "a.ts"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, path)| ProjectContentMatch {
                            path: TrimmedNonEmptyString(path.into()),
                            line_number: index as u64 + 1,
                            line_content: format!("literal 計画 {index}"),
                            match_ranges: vec![],
                        })
                        .collect(),
                ),
                ..Default::default()
            }
        })
        .chain(stream::pending())
        .boxed_local()
    }
    fn can_open(&self, _: &SearchTarget) -> bool {
        self.allowed.get()
    }
}

#[derive(Clone, PartialEq, Props)]
struct SearchFixtureProps {
    content: bool,
}

#[component]
fn SearchFixture(props: SearchFixtureProps) -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    let queries = Rc::new(Queries::default());
    queries.allowed.set(true);
    let opened = signal(String::new());
    let opens = signal(0u32);
    provide_context(PaletteSearchRuntime {
        client: queries.clone(),
        open_file: Rc::new(move |target, path, line| {
            opens.update(|count| *count += 1);
            opened.set(format!(
                "opened:{}:{path}:{line:?}",
                target.thread.environment.0
            ))
        }),
    });
    let complete = queries.clone();
    let deny = queries.clone();
    let old = queries.clone();
    let error = queries.clone();
    let released = signal(false);
    view()
        .w_full()
        .h_full()
        .child(text(move || opened.get()))
        .child(text(move || format!("opens:{}", opens.get())))
        .child(text(move || format!("released:{}", released.get())))
        .child(text(move || {
            format!("case:{}", queries.latest_options.borrow().case_sensitive)
        }))
        .child(
            nsh_base::button()
                .accessibility_label(Some("Complete current query".into()))
                .on_press(move || {
                    if let Some((query, send)) = complete.files.borrow().last() {
                        send.unbounded_send(FileSearchView {
                            entries: Rc::new(vec![ProjectEntry {
                                path: TrimmedNonEmptyString("src/TestFlags.tsx".into()),
                                kind: ProjectEntryKind::File,
                                ignored: None,
                            }]),
                            matched_query: query.clone(),
                            ..Default::default()
                        })
                        .expect("current subscriber");
                    }
                })
                .child(text(String::from("Complete current query"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Complete old query".into()))
                .on_press(move || {
                    let files = old.files.borrow();
                    if let Some((_, send)) = files.first() {
                        released.set(send.unbounded_send(FileSearchView::default()).is_err());
                    }
                })
                .child(text(String::from("Complete old query"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Deny access".into()))
                .on_press(move || deny.allowed.set(false))
                .child(text(String::from("Deny access"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Fail query".into()))
                .on_press(move || {
                    if let Some((_, send)) = error.files.borrow().last() {
                        send.unbounded_send(FileSearchView {
                            error: Some("server detail: /repo".into()),
                            ..Default::default()
                        })
                        .expect("current subscriber");
                    }
                })
                .child(text(String::from("Fail query"))),
        )
        .child(view().h(px(350.0)).child(if props.content {
            project_content_search_dialog().into_element()
        } else {
            project_file_picker().into_element()
        }))
}

fn start(size_: (f32, f32), content: bool) -> Headless {
    let mut fixture = Registry::new();
    fixture.register::<SearchFixture>();
    let mut sets = t3_ui::registries();
    sets.push(fixture);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(size_.0), px(size_.1)),
        ..EngineConfig::new(Root::new::<SearchFixture>(move || SearchFixtureProps {
            content,
        }))
    })
    .expect("search fixture")
}

#[test]
fn replacing_a_file_query_drops_the_old_subscription_and_preserves_failure_details() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = start(size_, false);
        app.wait_for("Indexing workspace files…")
            .expect("empty query pending");
        app.type_text("role:combobox[File picker]", "tsfl")
            .expect("new query");
        app.wait_for("Searching workspace files…")
            .expect("search pending");
        app.press("role:button[Complete old query]")
            .expect("late old reply");
        app.wait_for("text:released:true")
            .expect("old subscription cancelled");
        app.press("role:button[Complete current query]")
            .expect("server results");
        app.wait_for("src/TestFlags.tsx")
            .expect("file path is literal content");
        app.press("role:button[Fail query]")
            .expect("server failure");
        app.wait_for("text:server detail: /repo")
            .expect("exact server detail");
        app.type_text("role:combobox[File picker]", "")
            .expect("refocus the input after the fixture publishes its error");
        app.press_key("enter").expect("no stale execution");
        assert!(app.screen().shows("opens:0"));
    }
}

#[test]
fn file_activation_rechecks_access_and_dispatches_the_scoped_target() {
    for denied in [false, true] {
        let mut app = start(DESKTOP, false);
        app.wait_for("Indexing workspace files…")
            .expect("query subscribed");
        app.press("role:button[Complete current query]")
            .expect("result");
        app.wait_for("src/TestFlags.tsx").expect("file");
        if denied {
            app.press("role:button[Deny access]")
                .expect("revoke after rendering");
        }
        app.press("role:option[TestFlags.tsx]")
            .expect("activate file");
        if denied {
            assert!(app.screen().shows("opens:0"));
        } else {
            app.wait_for("text:opened:remote:src/TestFlags.tsx:None")
                .expect("file opened in remote thread");
        }
    }
}

#[test]
fn content_search_keeps_file_groups_and_opens_the_matching_line() {
    for size_ in [PHONE, DESKTOP] {
        let mut app = start(size_, true);
        app.wait_for("3 results in 2 files")
            .expect("client results");
        app.type_text("role:combobox[Search project contents]", "")
            .expect("focus the search input");
        app.press_key("down").expect("navigate to the second match");
        app.wait_for("role:button[b.ts:2]")
            .expect("list reveals the selected match");
        let active = app.screen().id("role:button[b.ts:2]").expect("match");
        let nodes = app.screen().layout_nodes();
        let viewport_node = nodes
            .iter()
            .find(|node| node.scroll.is_some())
            .expect("content viewport");
        let row_node = nodes
            .iter()
            .find(|node| node.id == active)
            .expect("selected match bounds");
        let mut content_node = row_node;
        while content_node.parent != viewport_node.id {
            content_node = nodes
                .iter()
                .find(|node| node.id == content_node.parent)
                .expect("match is inside the scrolling content");
        }
        let content = content_node.bounds;
        let viewport = viewport_node.bounds;
        assert!(
            viewport.size.height.0 > content.size.height.0,
            "Fill expands beyond short search results"
        );
        let row = row_node.bounds;
        assert!(row.origin.y.0 >= viewport.origin.y.0);
        assert!(row.origin.y.0 + row.size.height.0 <= viewport.origin.y.0 + viewport.size.height.0);
        app.press("role:button[b.ts:2]")
            .expect("select a concrete result");
        app.wait_for("text:opened:remote:b.ts:Some(2)")
            .expect("line and environment preserved");
        app.press("role:button[Deny access]").expect("revoke scope");
        app.press("role:button[b.ts:2]")
            .expect("attempt another result");
        assert!(app.screen().shows("opens:1"));
    }
}

#[component]
fn ParityWorkspace() -> impl IntoElement {
    use futures::FutureExt;
    use nsh_core::keymap::When;
    use serde_json::json;
    use t3_client::{ClientStore, composition::ClientRuntime, testing::environment::*};
    use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
    use t3_ui::{
        app::{app_tree, provide_client_services},
        runtime::{Rt, host::Choice},
        screens::panel::{SurfaceContent, workspace_panel_host},
        state::nav::{Route, navigator},
    };

    let owner = Owner::current();
    let spawner: t3_client::composition::Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("remote", spawner.clone());
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "filesystem:read",
    ]);
    server.set_shell(shell(&[
        ("thread", "First parity thread"),
        ("second", "Second parity thread"),
    ]));
    server.set_thread(
        "thread",
        projection("thread", "First parity thread", json!([]), json!([])),
    );
    server.set_thread(
        "second",
        projection("second", "Second parity thread", json!([]), json!([])),
    );
    server.reply(
        "projects.searchEntries",
        Ok(json!({
            "entries": [{"path": "src/PickerParity.rs", "kind": "file"}], "truncated": false
        })),
    );
    server.reply(
        "projects.searchContents",
        Ok(json!({
            "matches": [{"path": "src/ContentParity.rs", "lineNumber": 17,
                         "lineContent": "let parity_needle = 1;", "matchRanges": []}],
            "truncated": false
        })),
    );
    let platform = server.platform();
    let clock = platform.fake.clone();
    on_key_with(
        "f10",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| clock.advance(120),
    );
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            pairing.await.expect("pair fixture environment");
        }
        .boxed_local(),
    );
    let key = target().thread;
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    let probe = stored(None);
    let app = app_tree(runtime, Route::Thread(key.clone()), move || {
        provide_client_services(&client);
        let panel = workspace_panel_host().panel(key);
        let nav = navigator();
        let status = computed(move || {
            let Route::Thread(key) = nav.top() else {
                return String::new();
            };
            panel.with(
                |panel| match panel.active().map(|surface| &surface.content) {
                    Some(SurfaceContent::File {
                        relative_path,
                        reveal_line,
                        ..
                    }) => {
                        format!("Active file {relative_path} line {reveal_line:?}")
                    }
                    _ => format!("Active thread {}", key.thread.0),
                },
            )
        });
        probe.with(|slot| *slot = Some(status));
    });
    let status = probe.with(|slot| slot.expect("workspace probe"));
    view()
        .w_full()
        .h_full()
        .child(app)
        .child(text(move || status.get()))
}

fn start_parity_workspace(window: (f32, f32)) -> Headless {
    let mut fixture = Registry::new();
    fixture.register::<ParityWorkspace>();
    let mut sets = t3_ui::registries();
    sets.push(fixture);
    let mut config = EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ParityWorkspace>())
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    let mut app = Headless::start(config).expect("workspace starts");
    app.wait_for("First parity thread")
        .expect("paired thread renders");
    app
}

fn select_thread_through_palette(window: (f32, f32)) {
    let mut app = start_parity_workspace(window);
    app.press_key("mod+k").expect("open command palette");
    app.type_text("role:combobox[Command palette]", "second parity thread")
        .expect("filter the catalogue");
    app.wait_for("role:option[Second parity thread]")
        .expect("matching thread is selectable");
    app.press("role:option[Second parity thread]")
        .expect("open thread from catalogue");
    app.wait_for("Active thread second")
        .expect("thread route changes");
}

fn pick_file_through_palette(window: (f32, f32)) {
    let mut app = start_parity_workspace(window);
    app.press_key("mod+p").expect("open file picker");
    app.wait_for("Indexing workspace files…")
        .expect("picker observes the active project");
    app.press_key("f10")
        .expect("advance the fake debounce clock");
    app.wait_for("role:option[PickerParity.rs]")
        .expect("file from the project search RPC");
    app.press("role:option[PickerParity.rs]")
        .expect("select file");
    app.wait_for("Active file src/PickerParity.rs line None")
        .expect("workspace opens the exact file");
    app.wait_for("id:file-preview-panel")
        .expect("the registered Files surface renders the selection");
}

fn search_contents_through_palette(window: (f32, f32)) {
    let mut app = start_parity_workspace(window);
    app.press_key("mod+shift+f")
        .expect("open project content search");
    app.wait_for("Type to search across your project.")
        .expect("content search observes the active project");
    app.type_text("role:combobox[Search project contents]", "parity_needle")
        .expect("search contents");
    app.press_key("f10")
        .expect("advance the fake debounce clock");
    app.wait_for("role:button[src/ContentParity.rs:17]")
        .expect("match from the content search RPC");
    app.press("role:button[src/ContentParity.rs:17]")
        .expect("open match");
    app.wait_for("Active file src/ContentParity.rs line Some(17)")
        .expect("workspace opens the match at its line");
    app.wait_for("id:file-preview-panel")
        .expect("the registered Files surface renders the selection");
}

#[test]
fn parity_catalogue_desktop() {
    select_thread_through_palette(DESKTOP);
}
#[test]
fn parity_catalogue_phone() {
    select_thread_through_palette(PHONE);
}
#[test]
fn parity_file_picker_desktop() {
    pick_file_through_palette(DESKTOP);
}
#[test]
fn parity_file_picker_phone() {
    pick_file_through_palette(PHONE);
}
#[test]
fn parity_content_search_desktop() {
    search_contents_through_palette(DESKTOP);
}
#[test]
fn parity_content_search_phone() {
    search_contents_through_palette(PHONE);
}
