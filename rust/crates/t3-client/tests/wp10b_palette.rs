#![allow(clippy::unwrap_used)]
use t3_client::{
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::shell::{palette::*, search::ThreadSubject},
};
fn group(value: &str, items: Vec<Item>) -> Group {
    Group {
        value: value.into(),
        label: value.into(),
        items,
    }
}
fn thread(value: &str, title: &str, project: Option<&str>, time: f64) -> Item {
    let mut i = Item::action(value, title);
    i.thread_search = Some(ThreadSubject {
        title: title.into(),
        project_title: project.map(str::to_owned),
        activity_at: Some(time),
        ..Default::default()
    });
    i
}
fn values(groups: &[Group]) -> Vec<&str> {
    groups
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.value.as_str()))
        .collect()
}
fn filter<'a>(active: &'a [Group], query: &'a str, threads: &'a [Item]) -> FilterInput<'a> {
    FilterInput {
        active,
        query,
        submenu: false,
        root: None,
        nested: &[],
        projects: &[],
        settings: &[],
        threads,
        messages: &[],
        now: 100.0,
    }
}
#[test]
fn environment_lookups_keep_catalog_labels_and_local_machine_presentations_distinct() {
    use t3_contracts::environment::EnvironmentMachineKind as M;
    let ids = [
        EnvironmentId("primary".into()),
        EnvironmentId("desktop".into()),
        EnvironmentId("remote".into()),
    ];
    let (labels, locations) = environment_lookups(&[
        EnvironmentLookupInput {
            id: &ids[0],
            label: "Workstation",
            primary: true,
            desktop_local: false,
            machine: M::Desktop,
        },
        EnvironmentLookupInput {
            id: &ids[1],
            label: "Sandbox",
            primary: false,
            desktop_local: true,
            machine: M::Laptop,
        },
        EnvironmentLookupInput {
            id: &ids[2],
            label: "VPS",
            primary: false,
            desktop_local: false,
            machine: M::Linux,
        },
    ]);
    assert_eq!(labels[&ids[0]], "Workstation");
    assert_eq!(locations[&ids[0]].label, "Local");
    assert_eq!(locations[&ids[1]].label, "Sandbox (Local)");
    assert_eq!(locations[&ids[2]].label, "VPS");
    assert!(!locations[&ids[2]].local);
    assert_eq!(locations[&ids[2]].machine, M::Linux);
}
#[test]
fn ui_modes_intents_parent_search_and_mount_sessions() {
    let mut s = UiState::default();
    s.apply(UiAction::ToggleMode(OverlayMode::Files));
    assert!(s.open);
    assert_eq!(s.session, 0);
    s.apply(UiAction::OpenIntent(Intent::Search {
        query: "#42".into(),
        linked_threads: None,
    }));
    assert_eq!(s.mode, OverlayMode::Command);
    assert_eq!(s.session, 1);
    let context = ReturnContext {
        query: "parent".into(),
        linked_search: s.intent.clone(),
    };
    s.apply(UiAction::OpenChild {
        mode: OverlayMode::Content,
        context: context.clone(),
    });
    s.apply(UiAction::BackToCommand);
    assert_eq!(s.restored_search, Some(context));
    assert_eq!(s.session, 1);
    s.apply(UiAction::ToggleMode(OverlayMode::Files));
    s.apply(UiAction::SetOpen(false));
    s.apply(UiAction::SetOpen(true));
    assert_eq!(s.parent_search, None);
    assert_eq!(s.restored_search, None);
    assert_eq!(s.mode, OverlayMode::Command);
    s.apply(UiAction::OpenIntent(Intent::AddProject));
    assert_eq!(s.session, 2);
    let mut mount = MountState {
        state: s,
        ..Default::default()
    };
    mount.mount();
    mount.mount();
    mount.unmount();
    assert!(mount.state.open);
    mount.unmount();
    assert!(!mount.state.open);
    assert_eq!(mount.state.session, 3);
    assert_eq!(
        return_focus(true, true, true, true, false),
        ReturnFocus::Opener
    );
    assert_eq!(
        return_focus(true, false, true, true, false),
        ReturnFocus::Destination
    );
    assert_eq!(
        return_focus(true, true, true, false, false),
        ReturnFocus::Default
    );
}
#[test]
fn nested_picker_queries_backtrack_and_new_intent_clears_old_views() {
    let mut s = SearchState {
        query: "root".into(),
        ..Default::default()
    };
    s.apply(SearchAction::Push(View {
        groups: vec![],
        initial_query: Some("one".into()),
    }));
    s.apply(SearchAction::Push(View {
        groups: vec![],
        initial_query: None,
    }));
    assert_eq!(s.query, "");
    s.apply(SearchAction::Back);
    assert_eq!(s.query, "one");
    s.apply(SearchAction::Back);
    assert_eq!(s.query, "root");
    s.apply(SearchAction::Reset(Some("new".into())));
    s.apply(SearchAction::Back);
    assert_eq!(s.query, "new");
}
#[test]
fn linked_archived_relations_keep_explicit_environment_and_untitled_fallback() {
    let items = linked_thread_items(
        &EnvironmentId("remote".into()),
        &[("t", "".into(), true)],
        "#42",
    );
    assert_eq!(items[0].title, "Untitled thread");
    assert_eq!(items[0].description.as_deref(), Some("Archived thread"));
    assert_eq!(
        items[0].thread,
        Some(ThreadKey {
            environment: EnvironmentId("remote".into()),
            thread: ThreadId("t".into())
        })
    );
    assert_eq!(items[0].search_terms, vec!["#42", ""]);
}
#[test]
fn ranking_commands_threads_context_and_message_groups() {
    let actions = [group("actions", vec![Item::action("new", "New thread")])];
    let mut current = thread("current", "New thread", None, 99.0);
    current.secondary = true;
    let threads = [
        current,
        thread("other", "New thread", None, 10.0),
        thread("context", "Provider setup", Some("New thread"), 100.0),
    ];
    let messages = [
        Item::action("other", "New thread"),
        Item::action("message", "message result"),
    ];
    let result = filter_groups(&FilterInput {
        messages: &messages,
        ..filter(&actions, "New thread", &threads)
    });
    assert_eq!(result[0].value, "actions");
    let thread_group = result.iter().find(|g| g.value == "threads-search").unwrap();
    assert_eq!(thread_group.items.first().unwrap().value, "other");
    assert_eq!(thread_group.items.last().unwrap().value, "current");
    assert_eq!(result.last().unwrap().value, "threads-messages");
    assert_eq!(result.last().unwrap().items[0].value, "message");
    let actions = [group("actions", vec![Item::action("weak", "New thread")])];
    let threads = [thread("strong", "Workspace models", None, 1.0)];
    let result = filter_groups(&filter(&actions, "workspace models", &threads));
    assert_eq!(result[0].value, "threads-search");
}
#[test]
fn action_prefix_submenu_fallback_and_nested_options() {
    let root = [
        group("actions", vec![Item::action("a", "New thread")]),
        group("recent-threads", vec![thread("t", "Recent", None, 0.0)]),
    ];
    assert_eq!(filter_groups(&filter(&root, ">", &[])).len(), 1);
    assert_eq!(filter_groups(&filter(&root, " ", &[])).len(), 2);
    assert!(filter_groups(&filter(&root, ">Recent", &[])).is_empty());
    let mut parent = Item::action("themes", "Change theme...");
    parent.groups = Some(vec![group("themes", vec![Item::action("dark", "Dracula")])]);
    let nested = nested_items(std::slice::from_ref(&parent), "Dracula");
    assert_eq!(nested[0].title, "Change theme Dracula");
    assert!(nested_items(&[parent], "change theme").is_empty());
    let submenu = [group("child", vec![Item::action("child", "different")])];
    let result = filter_groups(&FilterInput {
        active: &submenu,
        query: "new thread",
        submenu: true,
        root: Some(&root),
        nested: &nested,
        projects: &[],
        settings: &[],
        threads: &[],
        messages: &[],
        now: 0.0,
    });
    assert_eq!(values(&result), vec!["a"]);
}
#[test]
fn styled_titles_typos_case_and_recency() {
    let mut item = Item::action("s", "styled");
    item.styled_title = true;
    item.search_title = Some("Context window".into());
    item.search_terms = vec!["irrelevant".into()];
    let groups = [group("settings", vec![item])];
    assert_eq!(
        values(&filter_groups(&filter(&groups, "context window", &[]))),
        vec!["s"]
    );
    assert_eq!(
        values(&filter_groups(&filter(&groups, "contxt window", &[]))),
        vec!["s"]
    );
    let threads = [
        thread("old", "Fix Istanbul", None, 1.0),
        thread("new", "Fix Istanbul", None, 90.0),
    ];
    let result = filter_groups(&filter(&[], "ISTANBUL", &threads));
    assert_eq!(values(&result), vec!["new", "old"]);
}
#[test]
fn rows_skip_disabled_and_jump_labels_stop_at_nine() {
    let mut items = (0..11)
        .map(|i| Item::action(i.to_string(), i.to_string()))
        .collect::<Vec<_>>();
    enumerate(&mut items);
    assert_eq!(items[8].shortcut.as_deref(), Some("thread.jump.9"));
    assert_eq!(items[9].shortcut, None);
    items[1].disabled = true;
    let groups = [group("x", items)];
    let rows = build_rows(&groups);
    assert_eq!(rows.item_values.len(), 10);
    assert_eq!(rows.row_index_by_item_index[1], 3);
    assert!(highlighted(&groups, Some("1")).is_none());
    assert_eq!(highlighted(&groups, Some("2")).unwrap().value, "2");
}
#[test]
fn no_project_slot_and_add_project_environment_decisions() {
    assert_eq!(
        no_project_index(&[
            (false, true, false),
            (false, false, false),
            (false, false, false)
        ]),
        2
    );
    assert_eq!(
        no_project_index(&[
            (false, false, false),
            (true, true, false),
            (false, false, false)
        ]),
        1
    );
    assert_eq!(
        add_project_entry(0, None),
        AddProjectEntry::ConnectEnvironment
    );
    assert_eq!(
        add_project_entry(1, None),
        AddProjectEntry::ChooseEnvironment
    );
    assert_eq!(
        add_project_entry(1, Some(&EnvironmentId("e".into()))),
        AddProjectEntry::ChooseSource(EnvironmentId("e".into()))
    );
    assert_eq!(
        overlay_mode(Some("filePicker.toggle")),
        Some(OverlayMode::Files)
    );
}
