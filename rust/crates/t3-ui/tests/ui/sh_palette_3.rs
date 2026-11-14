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
