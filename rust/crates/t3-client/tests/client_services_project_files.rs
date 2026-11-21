#![allow(clippy::unwrap_used)]
mod support {
    pub mod fake_server;
}
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use support::fake_server::*;
use t3_client::{
    ClientStore,
    composition::ClientRuntime,
    connection::PairingInput,
    permissions::SessionGrants,
    platform::{Storage, StorageArea, StorageKey},
    workspace::project_files::{read::FileReadView, tree::FileTreeNode},
};
use t3_contracts::{orchestration_v2::OrchestrationV2CreationSource, project::ProjectFileFailure};
use t3_rpc::{Exit, HandlerOutput};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

fn methods() -> Vec<(&'static str, t3_rpc::CallKind)> {
    ["projects.listEntries", "projects.searchEntries", "projects.readFile", "assets.createUrl"]
        .into_iter()
        .map(|tag| (tag, t3_rpc::CallKind::Unary))
        .collect()
}

fn grant(runtime: &ClientRuntime<TestPlatform>, scopes: &[&str], valid: bool) {
    runtime
        .client(&environment("env"))
        .unwrap()
        .session()
        .unwrap()
        .update_grants(SessionGrants {
            valid,
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        });
}

fn setup(h: &mut Harness) -> ClientRuntime<TestPlatform> {
    h.server().state.borrow_mut().extra_methods = methods();
    let runtime = ClientRuntime::new(
        h.platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        h.spawner(),
    );
    h.run(runtime.pair(PairingInput {
        pairing_url: Some(format!("{ORIGIN}/pair#token=pairing-token")),
        ..PairingInput::default()
    }))
    .unwrap();
    h.settle();
    grant(&runtime, &["orchestration:read", "filesystem:read"], true);
    runtime
}

/// A workspace: `src/` holding `main.rs` and `lib/` (holding `mod.rs`), `README.md` at the root.
fn serve_workspace(h: &Harness) -> Calls {
    let calls: Calls = Rc::default();
    let recorded = calls.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        let payload = request.payload.clone();
        let answer = match request.tag.as_str() {
            "projects.listEntries" => {
                let entries = match payload["directoryPath"].as_str().unwrap_or("") {
                    "" => json!([{"path":"src","kind":"directory"},{"path":"README.md","kind":"file"}]),
                    "src" => json!([{"path":"src/main.rs","kind":"file"},{"path":"src/lib","kind":"directory"}]),
                    "src/lib" => json!([{"path":"src/lib/mod.rs","kind":"file"}]),
                    _ => json!([]),
                };
                Ok(json!({"entries": entries, "truncated": false}))
            }
            "projects.searchEntries" => Ok(json!({
                "entries": [{"path":"src/lib/mod.rs","kind":"file"}],
                "truncated": true
            })),
            "projects.readFile" => match payload["relativePath"].as_str().unwrap_or("") {
                "logo.bin" => Err(json!({"_tag":"ProjectReadFileError","message":"binary","failure":"binary_file"})),
                "src" => Err(json!({"_tag":"ProjectReadFileError","message":"not a file","failure":"path_not_file"})),
                path => Ok(json!({"relativePath": path, "contents": "fn main() {}\n", "byteLength": 13, "truncated": false})),
            },
            _ => return None,
        };
        recorded.borrow_mut().push((request.tag.clone(), payload));
        Some(HandlerOutput::Unary(
            async move {
                match answer {
                    Ok(value) => Exit::success(value),
                    Err(error) => Exit::fail(error),
                }
            }
            .boxed_local(),
        ))
    }));
    calls
}

fn count(calls: &Calls, tag: &str) -> usize {
    calls.borrow().iter().filter(|(t, _)| t == tag).count()
}
fn labels(nodes: &[FileTreeNode]) -> Vec<String> {
    nodes.iter().map(|n| n.label.to_string()).collect()
}

#[test]
fn the_tree_lists_lazily_and_notifies_its_readers() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let files = runtime.project_files();
    let tree = files.tree(&environment("env"), "/repo", false);
    let mut changes = tree.changes();
    h.settle();
    let view = tree.view();
    assert!(view.ready && !view.pending);
    assert_eq!(labels(&view.nodes), ["src", "README.md"]);
    assert_eq!(count(&calls, "projects.listEntries"), 1, "only the root is read");
    assert_eq!(calls.borrow()[0].1, json!({"cwd": "/repo", "directoryPath": ""}));
    assert!(h.run(async move { changes.next().await }).is_some());

    tree.set_expanded(&["src"]);
    h.settle();
    let view = tree.view();
    assert_eq!(labels(view.nodes[0].children.as_ref().unwrap()), ["lib", "main.rs"]);
    assert_eq!(count(&calls, "projects.listEntries"), 2);

    let same = files.tree(&environment("env"), "/repo", false);
    h.settle();
    assert_eq!(count(&calls, "projects.listEntries"), 2, "readers share one tree");
    drop(same);
}

#[test]
fn expanded_folders_are_persisted_and_restored_by_the_next_tree() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let files = runtime.project_files();
    let tree = files.tree(&environment("env"), "/repo", false);
    h.settle();
    tree.set_expanded(&["src"]);
    h.settle();
    drop(tree);
    h.settle();
    let stored = h
        .run({
            let platform = h.platform.clone();
            async move {
                platform
                    .get(
                        StorageArea::Ordinary,
                        StorageKey::new("local-preferences", None, "t3code.fileTreeExpandedDirectories"),
                    )
                    .await
            }
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&stored).unwrap(),
        json!([{"root": "env:/repo", "paths": ["src"]}])
    );
    calls.borrow_mut().clear();
    let tree = files.tree(&environment("env"), "/repo", false);
    h.settle();
    assert_eq!(count(&calls, "projects.listEntries"), 2, "the restored folder reopens once listed");
    assert!(tree.view().nodes[0].children.is_some());
}

#[test]
fn a_refused_read_shows_the_server_reason_and_a_new_session_retries() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls: Calls = Rc::default();
    let recorded = calls.clone();
    let fail = Rc::new(RefCell::new(true));
    let failing = fail.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "projects.listEntries" {
            return None;
        }
        recorded.borrow_mut().push((request.tag.clone(), request.payload.clone()));
        let failed = *failing.borrow();
        Some(HandlerOutput::Unary(
            async move {
                if failed {
                    Exit::fail(json!({"_tag":"ProjectListEntriesError","message":"Workspace root is gone."}))
                } else {
                    Exit::success(json!({"entries": [], "truncated": false}))
                }
            }
            .boxed_local(),
        ))
    }));
    let tree = runtime.project_files().tree(&environment("env"), "/repo", false);
    h.settle();
    let error = tree.view().error.unwrap();
    assert_eq!(error.text, "Workspace root is gone.");
    assert!(!error.owned, "server detail is content, not a translation key");
    *fail.borrow_mut() = false;
    h.server().hang_up();
    h.settle();
    h.advance(5_000);
    assert!(count(&calls, "projects.listEntries") >= 2, "the failed folder is read again");
    assert_eq!(tree.view().error, None);
    assert!(tree.view().ready);
}

#[test]
fn read_access_follows_the_session_grant() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let files = runtime.project_files();
    assert!(files.read_access(&environment("env")).allowed);
    grant(&runtime, &["orchestration:read"], true);
    let access = files.read_access(&environment("env"));
    assert!(!access.allowed && !access.pending);
    grant(&runtime, &[], false);
    assert!(files.read_access(&environment("env")).pending, "grants not known yet");
    assert!(!files.read_access(&environment("missing")).allowed);
}

#[test]
fn the_search_is_debounced_and_only_the_last_query_runs() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let tree = runtime.project_files().tree(&environment("env"), "/repo", false);
    h.settle();
    tree.set_query("m".into());
    tree.set_query("mo".into());
    tree.set_query("mod".into());
    h.settle();
    assert_eq!(count(&calls, "projects.searchEntries"), 0, "nothing before the debounce");
    assert!(tree.view().search.pending);
    h.advance(250);
    assert_eq!(count(&calls, "projects.searchEntries"), 1);
    let search = calls.borrow().iter().find(|(t, _)| t == "projects.searchEntries").unwrap().1.clone();
    assert_eq!(search, json!({"cwd": "/repo", "query": "mod", "limit": 200}));
    let view = tree.view();
    assert!(!view.search.pending && view.search.truncated && !view.search.no_matches);
    assert_eq!(labels(&view.nodes), ["src"]);
    tree.set_query(String::new());
    h.advance(250);
    assert_eq!(count(&calls, "projects.searchEntries"), 1, "a cleared search sends nothing");
}

#[test]
fn host_folders_never_search_the_server() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let tree = runtime.project_files().tree(&environment("env"), "/etc", true);
    h.settle();
    tree.set_query("x".into());
    h.advance(500);
    assert_eq!(count(&calls, "projects.searchEntries"), 0);
}

#[test]
fn dropping_the_last_reader_cancels_the_tree() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    struct Interrupted(Rc<RefCell<bool>>);
    impl Drop for Interrupted {
        fn drop(&mut self) {
            *self.0.borrow_mut() = true;
        }
    }
    let held: Calls = Rc::default();
    let recorded = held.clone();
    let interrupted = Rc::new(RefCell::new(false));
    let observed = interrupted.clone();
    h.server().state.borrow_mut().extra_rpc = Some(Rc::new(move |request| {
        if request.tag != "projects.listEntries" {
            return None;
        }
        recorded.borrow_mut().push((request.tag.clone(), request.payload.clone()));
        let guard = Interrupted(interrupted.clone());
        Some(HandlerOutput::Unary(
            async move {
                let _guard = guard;
                futures::future::pending::<()>().await;
                Exit::success(json!({}))
            }
            .boxed_local(),
        ))
    }));
    let tree = runtime.project_files().tree(&environment("env"), "/repo", false);
    h.settle();
    assert!(tree.view().pending);
    assert!(!*observed.borrow());
    drop(tree);
    h.settle();
    assert!(*observed.borrow(), "the server stops serving the cancelled read");
    let again = runtime.project_files().tree(&environment("env"), "/repo", false);
    h.settle();
    assert_eq!(held.borrow().len(), 2, "a new tree starts over; the old one is gone");
    assert_eq!(again.lists_started(), 1);
}

fn read_view(h: &mut Harness, runtime: &ClientRuntime<TestPlatform>, path: &str) -> FileReadView {
    let read = runtime.project_files().read_file(&environment("env"), "/repo", path);
    h.settle();
    read.view()
}

#[test]
fn file_reads_carry_contents_and_typed_failures() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let view = read_view(&mut h, &runtime, "src/main.rs");
    assert_eq!(view.data.unwrap().contents, "fn main() {}\n");
    assert!(view.is_file && !view.pending && view.error.is_none());

    let binary = read_view(&mut h, &runtime, "logo.bin");
    assert_eq!(
        binary.read_error.unwrap().failure.clone().flatten(),
        Some(ProjectFileFailure::BinaryFile)
    );
    assert!(binary.is_file, "a binary file is still a file the document viewer opens");

    let folder = read_view(&mut h, &runtime, "src");
    assert!(folder.is_not_file && !folder.is_file);
    assert_eq!(count(&calls, "projects.readFile"), 3);
}

#[test]
fn reads_refresh_on_each_new_mutation_and_keep_contents_meanwhile() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    let read = runtime.project_files().read_file(&environment("env"), "/repo", "a.rs");
    h.settle();
    read.observe_mutation(Some("m1"));
    h.settle();
    assert_eq!(read.reads(), 1, "the first token is the one the read was made under");
    read.observe_mutation(Some("m2"));
    assert!(read.view().data.is_some(), "contents stay while the refresh runs");
    h.settle();
    assert_eq!(read.reads(), 2);
    read.observe_mutation(Some("m2"));
    h.settle();
    assert_eq!(count(&calls, "projects.readFile"), 2);
}

#[test]
fn reads_without_file_access_say_so_and_send_nothing() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let calls = serve_workspace(&h);
    grant(&runtime, &["orchestration:read"], true);
    let view = read_view(&mut h, &runtime, "a.rs");
    assert_eq!(view.error.unwrap().text, "This connection cannot read host files.");
    assert_eq!(count(&calls, "projects.readFile"), 0);
}

#[test]
fn unserved_methods_surface_as_errors_not_success() {
    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    h.server().state.borrow_mut().extra_methods = vec![];
    let tree = runtime.project_files().tree(&environment("env"), "/repo", false);
    let read = runtime.project_files().read_file(&environment("env"), "/repo", "a.rs");
    h.settle();
    assert!(tree.view().error.is_some() && !tree.view().ready);
    let view = read.view();
    assert!(view.error.is_some() && view.data.is_none() && !view.pending);
}

#[test]
fn preview_assets_stay_on_the_minting_environment_and_obey_the_byte_cap() {
    use t3_client::workspace::project_files::assets::AssetReadError;
    use t3_contracts::{assets::AssetResource, base_schemas::TrimmedNonEmptyString};

    let mut h = Harness::new("env");
    let runtime = setup(&mut h);
    let relative_url = Rc::new(RefCell::new("/test-assets/rewind-note"));
    {
        let relative_url = relative_url.clone();
        let mut state = h.server().state.borrow_mut();
        state.extra_rpc =
            Some(Rc::new(move |request| {
                assert_eq!(request.tag, "assets.createUrl");
                assert_eq!(
                    request.payload["resource"],
                    json!({"_tag":"workspace-file","threadId":"thread","path":"/repo/note"})
                );
                let url = *relative_url.borrow();
                Some(HandlerOutput::Unary(
                    async move {
                        Exit::success(json!({"relativeUrl":url,"expiresAt":4102444800000_i64}))
                    }
                    .boxed_local(),
                ))
            }));
    }
    let resource = AssetResource::WorkspaceFile {
        thread_id: t3_client::types::ThreadId("thread".into()),
        path: TrimmedNonEmptyString("/repo/note".into()),
    };
    let read = |limit| {
        runtime.project_files().asset_bytes(
            &environment("env"),
            resource.clone(),
            limit,
            Some("r &=".into()),
        )
    };
    assert_eq!(h.run(read(4)), Ok(b"note".to_vec()));
    assert_eq!(h.run(read(3)), Err(AssetReadError::TooLarge));

    let http_before = h.server().state.borrow().http.len();
    *relative_url.borrow_mut() = "https://unrelated.test/test-assets/rewind-note";
    assert_eq!(h.run(read(4)), Err(AssetReadError::Unavailable));
    assert_eq!(h.server().state.borrow().http.len(), http_before);
}
