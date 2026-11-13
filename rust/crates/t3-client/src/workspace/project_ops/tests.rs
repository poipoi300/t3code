use super::super::{ProjectKey, projects::*, sidebar::*, vcs::*};
use super::{
    actions::*, editor::*, icons::*, paths::*, scripts::*, source_control::SourceControl,
    subscriptions::LiveVcs, wsl::parse_wsl_unc_path, *,
};
use crate::{
    ClientError,
    rpc::{RpcEndpoint, RpcMethod},
    testing::FakePlatform,
    types::{EnvironmentId, Generation, ProjectId, ThreadId, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
    executor::block_on,
    future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
use t3_contracts::{editor::EditorId, git::*, project::*, rpc::*};

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn env(value: &str) -> EnvironmentId {
    EnvironmentId(value.into())
}
fn target(environment: &str) -> CheckoutKey {
    CheckoutKey {
        environment: env(environment),
        cwd: "/repo".into(),
    }
}
fn thread(environment: &str, id: &str) -> ThreadKey {
    ThreadKey {
        environment: env(environment),
        thread: ThreadId(id.into()),
    }
}
fn refs_input() -> VcsListRefsInput {
    decode(json!({"cwd":"/repo", "limit":100}))
}
fn refs(label: &str) -> VcsListRefsResult {
    decode(
        json!({"refs":[{"name":label,"current":true,"worktreePath":null,"isDefault":false}],"isRepo":true,"hasPrimaryRemote":false,"nextCursor":null,"totalCount":1}),
    )
}
fn project(id: &str, path: &str) -> EnvironmentProject {
    EnvironmentProject {
        environment: env("local"),
        is_scratch: false,
        project: decode(
            json!({"id":id,"title":id,"workspaceRoot":path,"defaultModelSelection":null,"scripts":[],"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}),
        ),
    }
}

struct Request {
    method: &'static str,
    input: Value,
    reply: oneshot::Sender<Result<Value, ClientError>>,
}
struct StreamRequest {
    method: &'static str,
    input: Value,
    reply: mpsc::UnboundedSender<Result<Value, ClientError>>,
}
#[derive(Default)]
struct ControlledRpc {
    environment: Option<EnvironmentId>,
    requests: RefCell<VecDeque<Request>>,
    streams: RefCell<VecDeque<StreamRequest>>,
}
impl ControlledRpc {
    fn new(environment: &str) -> Self {
        Self {
            environment: Some(env(environment)),
            ..Self::default()
        }
    }
    fn request(&self) -> Request {
        self.requests.borrow_mut().pop_front().unwrap()
    }
    fn stream(&self) -> StreamRequest {
        self.streams.borrow_mut().pop_front().unwrap()
    }
}
impl RpcEndpoint for ControlledRpc {
    fn destination(&self) -> Option<EnvironmentId> {
        self.environment.clone()
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        let (reply, response) = oneshot::channel();
        self.requests.borrow_mut().push_back(Request {
            method: M::NAME,
            input: serde_json::to_value(input).unwrap(),
            reply,
        });
        async {
            serde_json::from_value(response.await.map_err(|_| ClientError::Disconnected)??)
                .map_err(Into::into)
        }
        .boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        let (reply, response) = mpsc::unbounded();
        self.streams.borrow_mut().push_back(StreamRequest {
            method: M::NAME,
            input: serde_json::to_value(input).unwrap(),
            reply,
        });
        response
            .map(|value| serde_json::from_value(value?).map_err(Into::into))
            .boxed_local()
    }
}

#[test]
fn browse_helpers_resolve_relative_paths_and_preserve_filesystem_roots() {
    for (path, cwd, expected) in [
        ("../src", "/repo/app", "/repo/src"),
        ("../../..", "/repo", "/"),
        (".\\src", "C:\\repo", "C:\\repo\\src"),
        ("../src", "\\\\host\\share\\app", "\\\\host\\share\\src"),
        ("./src", "relative", "./src"),
        ("./src", "/repo/a\\b", "/repo/a\\b/src"),
    ] {
        assert_eq!(resolve_project_path_for_dispatch(path, Some(cwd)), expected);
    }
    for (path, parent) in [
        ("/", None),
        ("C:\\", None),
        ("\\\\host\\share\\", None),
        ("/repo/", Some("/")),
        ("/repo/src/", Some("/repo/")),
        ("C:\\repo\\", Some("C:\\")),
        ("\\\\host\\share\\repo", Some("\\\\host\\share\\")),
        ("relative", None),
        ("rel/path", Some("rel/")),
    ] {
        assert_eq!(get_browse_parent_path(path).as_deref(), parent);
    }
    assert_eq!(append_browse_path_segment("/repo/fi", "src"), "/repo/src/");
    assert_eq!(
        append_browse_path_segment("C:\\repo\\fi", "src"),
        "C:\\repo\\src\\"
    );
    assert_eq!(get_browse_leaf_path_segment("/repo/a\\b"), "a\\b");
    assert_eq!(get_browse_directory_path("/repo/foo"), "/repo/");
    assert_eq!(ensure_browse_directory_path(" /repo "), "/repo/");
    assert_eq!(infer_project_title_from_path("/repo/a\\b/"), "a\\b");
    assert_eq!(infer_project_title_from_path("C:\\"), "C:\\");
    assert!(can_navigate_up("/repo/"));
    assert!(!can_navigate_up("/repo"));
    for path in ["./src", "../", ".\\src", "/src", "~/src"] {
        assert!(is_filesystem_browse_query(path, "linux"));
    }
    assert!(!is_filesystem_browse_query("C:\\repo", "linux"));
    assert!(is_filesystem_browse_query("C:\\repo", "Windows"));
    assert!(is_unsupported_windows_project_path(
        "\\\\server\\share",
        "darwin"
    ));
    assert!(
        find_project_by_path(&[project("p", "C:\\Repo")], "c:/repo/", |p| &p
            .project
            .workspace_root
            .0)
        .is_some()
    );
    assert!(
        find_project_by_path(&[project("p", "/Repo")], "/repo", |p| &p
            .project
            .workspace_root
            .0)
        .is_none()
    );
}
#[test]
fn cleanup_only_removes_unshared_checkout_in_destination_environment() {
    for (a, b) in [
        ("/repo/wt", "/repo/wt/"),
        ("/repo/wt", "/repo/wt//"),
        ("C:\\repo\\wt", "c:/repo/wt/"),
    ] {
        let threads = vec![
            (thread("local", "a"), Some(a.into())),
            (thread("local", "b"), Some(b.into())),
        ];
        assert_eq!(
            get_orphaned_worktree_path_for_thread(&threads, &thread("local", "a")),
            None
        );
        assert_eq!(
            get_orphaned_worktree_path_for_thread(&threads, &thread("local", "b")),
            None
        );
    }
    let threads = vec![
        (thread("local", "a"), Some(" /repo/Feature ".into())),
        (thread("local", "b"), Some("/repo/feature".into())),
        (thread("remote", "a"), Some("/repo/Feature".into())),
    ];
    assert_eq!(
        get_orphaned_worktree_path_for_thread(&threads, &thread("local", "a")),
        Some("/repo/Feature")
    );
    assert_eq!(
        get_orphaned_worktree_path_for_thread(&threads, &thread("local", "missing")),
        None
    );
    assert_eq!(
        format_worktree_path_for_display(" C:\\repo\\feature\\ "),
        "feature"
    );
    assert_eq!(format_worktree_path_for_display("  "), "  ");
}
#[test]
fn diff_paths_are_relative_to_nested_projects_and_cannot_escape() {
    for (path, workspace, repository, expected) in [
        (
            "frontend/Dockerfile",
            "/repo/frontend",
            "/repo",
            "Dockerfile",
        ),
        (
            "frontend/Dockerfile",
            "/worktrees/feature",
            "/repo",
            "frontend/Dockerfile",
        ),
        (
            "Frontend/src\\index.ts",
            "C:\\repo\\frontend",
            "C:\\repo",
            "src/index.ts",
        ),
        ("frontend/index.ts", "/frontend", "/", "index.ts"),
        ("frontend/index.ts", "C:\\frontend", "C:\\", "index.ts"),
        (
            "/repo/frontend/src/index.ts",
            "/repo/frontend",
            "/repo/frontend",
            "src/index.ts",
        ),
        (
            "c:\\Repo\\src\\index.ts",
            "C:\\repo",
            "C:\\repo",
            "src/index.ts",
        ),
    ] {
        assert_eq!(
            resolve_diff_path_for_workspace(path, Some(workspace), Some(repository)).as_deref(),
            Some(expected)
        );
    }
    for path in [
        "backend/server.ts",
        "frontend2/app.ts",
        "frontend/../secret.ts",
        "C:secret.ts",
        "/repo/backend/server.ts",
        "/repo/frontend/../backend/server.ts",
        "/repo/frontend",
        "/etc/passwd",
    ] {
        assert_eq!(
            open_diff_file_primary_action(
                Some(&thread("local", "a")),
                path,
                Some("/repo/frontend"),
                Some("/repo")
            ),
            None
        );
    }
    assert_eq!(
        open_diff_file_primary_action(
            Some(&thread("local", "a")),
            "frontend/Dockerfile",
            Some("/repo/frontend"),
            Some("/repo")
        ),
        Some(DiffFileAction::ThreadFile {
            thread: thread("local", "a"),
            path: "Dockerfile".into()
        })
    );
    assert_eq!(
        open_diff_file_primary_action(None, "file.ts", Some("/repo"), None),
        Some(DiffFileAction::Editor("/repo/file.ts".into()))
    );
}
#[test]
fn file_display_preserves_positions_and_windows_case() {
    for (path, root, expected) in [
        (
            "/repo/src/main.rs:12:4",
            Some("/repo"),
            "repo/src/main.rs:12:4",
        ),
        ("/C:/REPO/src.ts:3", Some("C:\\Repo"), "Repo/src.ts:3"),
        ("./src.ts", Some("/repo"), "repo/src.ts"),
        ("repo/src.ts", Some("/repo"), "repo/src.ts"),
        ("/outside/src.ts", Some("/repo"), "/outside/src.ts"),
        ("/repo", Some("/repo"), "repo"),
        ("/repo/a:2-4", Some("/repo"), "repo/a:2"),
        ("a:0", None, "a"),
        // filePathDisplay.ts strips one `./` run, and compares the label case-insensitively
        // under a Windows root.
        ("././src.ts", Some("/repo"), "repo/./src.ts"),
        ("repo/src.ts", Some("C:\\Repo"), "repo/src.ts"),
    ] {
        assert_eq!(format_workspace_relative_path(path, root), expected);
    }
    for (path, expected) in [
        ("/repo/file:2-L4", "repo/file:2"),
        ("/repo/file:+2", "repo/file:+2"),
        ("/repo/file:2:+4", "repo/file:2:+4"),
    ] {
        assert_eq!(
            format_workspace_relative_path(path, Some("/repo")),
            expected
        );
    }
}
fn script(id: &str, setup: bool, settle: bool) -> ProjectScript {
    decode(
        json!({"id":id,"name":id,"command":"true","icon":"play","runOnWorktreeCreate":setup,"runOnSettle":settle}),
    )
}
fn script_input() -> ProjectScriptInput {
    ProjectScriptInput {
        name: decode(json!("Next")),
        command: decode(json!("true")),
        icon: ProjectScriptIcon::Play,
        run_on_worktree_create: false,
        wait_for_setup: false,
        run_on_settle: false,
        stop_on_settle: false,
        preview_url: None,
        auto_open_preview: false,
    }
}
#[test]
fn script_commands_preserve_validation_slug_limits_and_role_priority() {
    for id in [
        "install-javascript-dependencies",
        "A",
        "a.b",
        "a b",
        "-a",
        "",
        &"a".repeat(25),
    ] {
        assert!(command_for_project_script(id).is_none());
    }
    assert_eq!(
        command_for_project_script("lint").unwrap().0,
        "script.lint.run"
    );
    assert_eq!(
        project_script_id_from_command(" script.lint.run "),
        Some("lint")
    );
    assert_eq!(next_project_script_id("Run Tests", [], 0), "run-tests");
    assert_eq!(
        next_project_script_id("Run Tests", ["run-tests"], 0),
        "run-tests-2"
    );
    assert_eq!(next_project_script_id("!!!", [], 0), "script");
    assert_eq!(
        next_project_script_id(&"a".repeat(30), ["aaaaaaaaaaaaaaaaaaaaaaaa"], 0),
        "aaaaaaaaaaaaaaaaaaaaaa-2"
    );
    let scripts = [
        script("setup", true, false),
        script("settle", false, true),
        script("test", false, false),
    ];
    assert_eq!(primary_project_script(&scripts).unwrap().id.0, "test");
    assert_eq!(setup_project_script(&scripts).unwrap().id.0, "setup");
    assert_eq!(settle_project_script(&scripts).unwrap().id.0, "settle");
    assert!(primary_project_script(&scripts[1..2]).is_none());
    assert_eq!(primary_project_script(&scripts[..2]).unwrap().id.0, "setup");
    let holder = script("holder", true, true);
    let mut input = script_input();
    input.run_on_settle = true;
    let next = release_claimed_roles(&holder, &input);
    assert!(next.run_on_worktree_create);
    assert_eq!(next.run_on_settle, Some(Some(false)));
    input.run_on_settle = false;
    input.run_on_worktree_create = true;
    let next = release_claimed_roles(&holder, &input);
    assert!(!next.run_on_worktree_create);
    assert_eq!(next.run_on_settle, Some(Some(true)));
    input.run_on_worktree_create = false;
    assert_eq!(release_claimed_roles(&holder, &input), holder);
    input.run_on_worktree_create = true;
    input.wait_for_setup = true;
    input.stop_on_settle = true;
    let built = build_project_script(decode(json!("next")), input);
    assert_eq!(built.r#async, Some(Some(false)));
    assert_eq!(built.stop_on_settle, Some(Some(true)));
    assert!(built.preview_url.is_none());
    assert!(built.auto_open_preview.is_none());
}
#[test]
fn icons_preserve_offered_names_legacy_spellings_and_complete_graphemes() {
    for name in filter_project_icon_names("") {
        assert!(is_lucide_icon_name(name), "{name}");
    }
    for name in ["layers-3", "arrow-down-01", "arrow-down-0-1", "alarm-check"] {
        assert!(is_lucide_icon_name(name));
    }
    assert!(!is_lucide_icon_name("create-lucide-icon"));
    for (value, expected) in [
        ("  👩🏽‍💻 hello", Some("👩🏽‍💻")),
        ("🇺🇸 project", Some("🇺🇸")),
        ("1️⃣ project", Some("1️⃣")),
        ("plain text", None),
        ("", None),
        ("🇺", None),
    ] {
        assert_eq!(first_emoji(value), expected);
    }
    assert!(filter_project_icon_names("arrow").len() <= 60);
    assert_eq!(
        filter_project_icon_names("arrow down"),
        filter_project_icon_names("arrow-down")
    );
    assert_eq!(PROJECT_ICON_COLORS.len(), 18);
    assert_eq!(
        project_icon_color_class_name(&ProjectIconColor::Blue),
        "text-blue-600 dark:text-blue-400"
    );
}
#[test]
fn project_identity_uses_nfkc_unicode_words_numbers_and_stable_color() {
    for (name, monogram) in [
        ("", "PR"),
        ("!!!", "PR"),
        ("project", "PT"),
        ("My Project", "MP"),
        ("t3code", "T3"),
        ("Ａｌｐｈａ ３", "A３"),
        ("Ａ３", "A3"),
        ("école", "ÉE"),
        ("ß", "SS"),
        ("项目", "项目"),
    ] {
        let actual = derive_project_identity(name);
        let expected = if name == "Ａｌｐｈａ ３" {
            "A3"
        } else {
            monogram
        };
        assert_eq!(actual.monogram, expected, "{name}");
    }
    assert_eq!(
        derive_project_identity("  Ａｌｐｈａ  "),
        derive_project_identity("Alpha")
    );
    assert_eq!(
        derive_project_identity("e\u{301}cole"),
        derive_project_identity("école")
    );
}
#[test]
fn wsl_paths_route_only_to_the_distro_of_the_running_backend() {
    assert_eq!(
        parse_wsl_unc_path(" \\\\wsl.localhost\\Ubuntu\\home\\me ").unwrap(),
        wsl::WslUncPath {
            distro: "Ubuntu".into(),
            linux_path: "/home/me".into()
        }
    );
    assert_eq!(parse_wsl_unc_path("//WSL$/Ubuntu").unwrap().linux_path, "/");
    for input in [
        "C:\\repo",
        "\\\\server\\share",
        "\\\\wsl$\\Ubuntu Bad\\repo",
        "\\\\wsl$\\",
    ] {
        assert!(parse_wsl_unc_path(input).is_none());
    }
    let candidates = vec![wsl::WslEnvironmentCandidate {
        environment: env("wsl"),
        backend_id: "wsl:default".into(),
        running_distro: Some("Ubuntu".into()),
    }];
    assert_eq!(
        wsl::resolve_wsl_project_selection("\\\\wsl$\\ubuntu\\repo", &candidates)
            .unwrap()
            .environment,
        env("wsl")
    );
    assert!(wsl::resolve_wsl_project_selection("\\\\wsl$\\Debian\\repo", &candidates).is_none());
    let config = wsl::WslEnvironmentConfiguration {
        enabled: true,
        wsl_only: true,
        distro: Some("debian".into()),
        distros: vec![("Debian".into(), true)],
    };
    assert_eq!(
        wsl::apply_wsl_environment_configuration(
            &candidates,
            Some(&env("wsl")),
            Some(&config),
            None
        ),
        candidates
    );
    let added =
        wsl::apply_wsl_environment_configuration(&[], Some(&env("primary")), Some(&config), None);
    assert_eq!(added[0].backend_id, "wsl:Debian");
    assert_eq!(added[0].running_distro.as_deref(), Some("Debian"));
    assert_eq!(
        wsl::resolve_project_picker_target(
            Some(&env("primary")),
            Some(&env("primary")),
            None,
            Some(&config)
        )
        .as_deref(),
        Some("wsl:Debian")
    );
    assert_eq!(
        wsl::resolve_project_picker_target(None, None, Some("desktop"), Some(&config)).as_deref(),
        Some("desktop")
    );
    let default = wsl::WslEnvironmentConfiguration {
        distro: None,
        ..config.clone()
    };
    assert_eq!(
        wsl::resolve_project_picker_target(
            Some(&env("primary")),
            Some(&env("primary")),
            None,
            Some(&default)
        )
        .as_deref(),
        Some("wsl:default")
    );
    let added = wsl::apply_wsl_environment_configuration(
        &[],
        Some(&env("primary")),
        Some(&default),
        Some("Ubuntu"),
    );
    assert_eq!(added[0].running_distro.as_deref(), Some("Ubuntu"));
    let combo = wsl::WslEnvironmentConfiguration {
        wsl_only: false,
        ..config
    };
    assert!(
        wsl::resolve_project_picker_target(
            Some(&env("primary")),
            Some(&env("primary")),
            None,
            Some(&combo)
        )
        .is_none()
    );
    assert!(
        wsl::apply_wsl_environment_configuration(&[], Some(&env("primary")), Some(&combo), None)
            .is_empty()
    );
}

fn install_projects(
    store: &crate::ClientStore,
    environment: &EnvironmentId,
    generation: Generation,
    projects: Vec<EnvironmentProject>,
) {
    store.update_shell(environment,generation,|shell| { shell.install(decode(json!({"schemaVersion":1,"snapshotSequence":1,"threads":[],"archivedThreads":[],"projects":projects.into_iter().map(|p| p.project).collect::<Vec<_>>()}))); true });
}
#[test]
fn project_entities_react_to_configs_without_changing_unrelated_rows_or_sources() {
    block_on(async {
        let store = crate::ClientStore::default();
        let generation = store.begin_session(env("local"));
        store.begin_session(env("remote"));
        install_projects(
            &store,
            &env("local"),
            generation,
            vec![project("scratch", "/scratch"), project("ordinary", "/repo")],
        );
        let entities = ProjectEntities::default();
        let mut configs = vec![
            ProjectEnvironmentConfig {
                environment: env("local"),
                enabled: true,
                loaded: false,
                scratch_workspace_root: None,
            },
            ProjectEnvironmentConfig {
                environment: env("remote"),
                enabled: true,
                loaded: false,
                scratch_workspace_root: None,
            },
        ];
        entities.refresh(&store, true, &configs, true);
        let initial = entities.snapshot();
        let source = store.shared_shell(&env("local")).unwrap();
        assert!(!initial.configs_ready);
        let mut notifications = entities.observe();
        notifications.next().await;
        configs[0].loaded = true;
        configs[0].scratch_workspace_root = Some("/scratch/".into());
        entities.refresh(&store, true, &configs, true);
        let next = entities.snapshot();
        assert!(next.projects[0].is_scratch);
        assert!(!Rc::ptr_eq(&initial.projects[0], &next.projects[0]));
        assert!(Rc::ptr_eq(&initial.projects[1], &next.projects[1]));
        assert!(Rc::ptr_eq(&initial.refs, &next.refs));
        assert!(Rc::ptr_eq(
            &source,
            &store.shared_shell(&env("local")).unwrap()
        ));
        assert!(notifications.next().now_or_never().is_some());
        entities.refresh(&store, true, &configs, true);
        assert!(Rc::ptr_eq(&next, &entities.snapshot()));
        assert!(notifications.next().now_or_never().is_none());
        configs[1].enabled = false;
        entities.refresh(&store, true, &configs, true);
        assert!(entities.snapshot().configs_ready);
        configs[1].enabled = true;
        entities.refresh(&store, true, &configs, true);
        assert!(!entities.snapshot().configs_ready);
        configs[1].loaded = true;
        entities.refresh(&store, true, &configs, true);
        assert!(entities.snapshot().configs_ready);
        entities.refresh(&store, false, &configs, true);
        assert!(!entities.snapshot().configs_ready);
        configs[0].scratch_workspace_root = Some("/repo".into());
        entities.refresh(&store, true, &configs, true);
        assert!(!entities.snapshot().projects[0].is_scratch);
        assert!(entities.snapshot().projects[1].is_scratch);
        configs[0].scratch_workspace_root = None;
        entities.refresh(&store, true, &configs, true);
        assert!(entities.snapshot().projects.iter().all(|p| !p.is_scratch));
        assert!(
            source
                .projects
                .iter()
                .all(|p| p.id.0 == "scratch" || p.id.0 == "ordinary")
        );
        let retained = entities.environment_snapshot(&env("local"));
        configs[0].enabled = false;
        entities.refresh(&store, true, &configs, true);
        assert!(entities.snapshot().projects.is_empty());
        assert!(Rc::ptr_eq(
            &retained,
            &entities.environment_snapshot(&env("local"))
        ));
        assert!(
            entities
                .project(&ProjectKey {
                    environment: env("local"),
                    project: ProjectId("ordinary".into())
                })
                .is_some()
        );
        entities.refresh(&store, true, &configs[1..], true);
        assert!(
            entities
                .environment_snapshot(&env("local"))
                .projects
                .is_empty()
        );
    });
}
#[test]
fn project_entities_follow_catalog_and_config_changes_without_a_store_write() {
    block_on(async {
        let store = crate::ClientStore::default();
        let generation = store.begin_session(env("local"));
        install_projects(
            &store,
            &env("local"),
            generation,
            vec![project("p", "/scratch")],
        );
        let entities = ProjectEntities::default();
        let (sender, changes) = mpsc::unbounded();
        let mut driver = Box::pin(entities.follow_inputs(&store, changes));
        assert!(futures::poll!(&mut driver).is_pending());
        let mut input = ProjectEntityInputs {
            catalog_ready: true,
            configs: vec![ProjectEnvironmentConfig {
                environment: env("local"),
                enabled: true,
                loaded: false,
                scratch_workspace_root: None,
            }],
            require_config: true,
        };
        sender.unbounded_send(input.clone()).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(!entities.snapshot().configs_ready);
        assert!(!entities.snapshot().projects[0].is_scratch);
        input.configs[0].loaded = true;
        input.configs[0].scratch_workspace_root = Some("/scratch".into());
        sender.unbounded_send(input).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(entities.snapshot().configs_ready);
        assert!(entities.snapshot().projects[0].is_scratch);
    });
}
#[test]
fn open_scratch_waits_for_store_and_has_injected_timeout_and_session_guard() {
    block_on(async {
        let store = crate::ClientStore::default();
        let generation = store.begin_session(env("local"));
        let host = FakePlatform::default();
        let endpoint = ControlledRpc::new("local");
        let commands = ProjectCommands::default();
        let mut opening =
            Box::pin(
                commands.open_scratch(env("local"), &endpoint, &host, &store, || {
                    Some("/scratch".into())
                }),
            );
        assert!(futures::poll!(&mut opening).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "projects.ensureScratch");
        request
            .reply
            .send(Ok(json!({"projectId":"scratch"})))
            .unwrap();
        assert!(futures::poll!(&mut opening).is_pending());
        install_projects(
            &store,
            &env("local"),
            generation,
            vec![project("scratch", "/scratch")],
        );
        let result = opening.await.unwrap();
        assert!(result.is_scratch);
        assert_eq!(result.project.id.0, "scratch");
        let mut opening =
            Box::pin(commands.open_scratch(env("local"), &endpoint, &host, &store, || None));
        assert!(futures::poll!(&mut opening).is_pending());
        endpoint
            .request()
            .reply
            .send(Ok(json!({"projectId":"missing"})))
            .unwrap();
        assert!(futures::poll!(&mut opening).is_pending());
        host.advance(9999);
        assert!(futures::poll!(&mut opening).is_pending());
        host.advance(1);
        assert!(
            matches!(opening.await,Err(ProjectCommandError::ScratchProjectNotLoaded{project}) if project.0=="missing")
        );
        let mut opening =
            Box::pin(commands.open_scratch(env("local"), &endpoint, &host, &store, || None));
        assert!(futures::poll!(&mut opening).is_pending());
        endpoint
            .request()
            .reply
            .send(Ok(json!({"projectId":"missing"})))
            .unwrap();
        assert!(futures::poll!(&mut opening).is_pending());
        store.begin_session(env("local"));
        assert!(matches!(
            opening.await,
            Err(ProjectCommandError::Client(ClientError::Cancelled))
        ));
    });
}
#[test]
fn file_reads_do_not_seed_missing_files_or_restore_metadata_after_writes() {
    block_on(async {
        let host = FakePlatform::default();
        let endpoint = ControlledRpc::new("local");
        let commands = ProjectCommands::default();
        let environment = env("local");
        commands.begin_session(environment.clone(), Generation(1));
        let mut read = Box::pin(commands.read_file(
            &environment,
            &endpoint,
            &host,
            decode(json!({"cwd":"/repo","relativePath":"foo","allowMissing":true})),
        ));
        assert!(futures::poll!(&mut read).is_pending());
        endpoint.request().reply.send(Ok(json!({"relativePath":"foo","byteLength":0,"contents":"","truncated":false,"exists":false}))).unwrap();
        assert_eq!(read.await.unwrap().exists, Some(Some(false)));
        let metadata = commands
            .metadata
            .enqueue(env("local"), "/repo/foo", None, 0);
        assert!(metadata.snapshot().value.is_none());
        let alias = commands
            .metadata
            .enqueue(env("local"), "/repo/./foo", None, 0);
        let batch = commands.metadata.next_batch(&env("local")).unwrap();
        commands.metadata.complete(
            batch,
            vec![Some(decode(json!({"kind":"file","byteLength":5}))); 2],
            0,
        );
        let mut read = Box::pin(commands.read_file(
            &environment,
            &endpoint,
            &host,
            decode(json!({"cwd":"/repo","relativePath":"foo"})),
        ));
        assert!(futures::poll!(&mut read).is_pending());
        let old = endpoint.request();
        let mut write = Box::pin(commands.write_file(
            env("local"),
            &endpoint,
            &host,
            decode(json!({"cwd":"/repo","relativePath":"./foo","contents":"new"})),
        ));
        assert!(futures::poll!(&mut write).is_pending());
        endpoint
            .request()
            .reply
            .send(Ok(json!({"relativePath":"foo"})))
            .unwrap();
        assert!(futures::poll!(&mut write).is_pending());
        let metadata_request = endpoint.request();
        assert_eq!(metadata_request.method, "filesystem.getMetadata");
        let count = metadata_request.input["paths"].as_array().unwrap().len();
        metadata_request
            .reply
            .send(Ok(
                json!({"entries":vec![json!({"kind":"file","byteLength":10});count]}),
            ))
            .unwrap();
        assert_eq!(write.await.unwrap().relative_path, "foo");
        old.reply
            .send(Ok(
                json!({"relativePath":"foo","byteLength":5,"contents":"old","truncated":false}),
            ))
            .unwrap();
        assert_eq!(read.await.unwrap().byte_length, 5);
        let current = commands
            .metadata
            .enqueue(env("local"), "/repo/foo", None, 0);
        assert_eq!(
            current
                .snapshot()
                .value
                .unwrap()
                .as_ref()
                .as_ref()
                .unwrap()
                .byte_length,
            Some(10)
        );
        assert_eq!(
            alias
                .snapshot()
                .value
                .unwrap()
                .as_ref()
                .as_ref()
                .unwrap()
                .byte_length,
            Some(10)
        );
    });
}
#[test]
fn editor_checks_destination_permissions_before_preference_io_and_on_each_invocation() {
    block_on(async {
        let host = FakePlatform::default();
        let endpoint = ControlledRpc::new("secondary");
        let allowed = Cell::new(false);
        let open = || {
            open_in_preferred_editor(
                endpoint.environment.as_ref(),
                &[EditorId::Vscode],
                None,
                "/work/readme.md:5",
                &endpoint,
                &host,
                |environment| allowed.get() && environment == &env("secondary"),
            )
        };
        assert!(matches!(
            open().await,
            Err(PreferredEditorError::Client(
                ClientError::PermissionDenied { .. }
            ))
        ));
        assert!(endpoint.requests.borrow().is_empty());
        assert_eq!(host.storage_write_count(), 0);
        allowed.set(true);
        let mut opening = Box::pin(open());
        assert!(futures::poll!(&mut opening).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "shell.openInEditor");
        assert_eq!(
            request.input,
            json!({"cwd":"/work/readme.md:5","editor":"vscode"})
        );
        request.reply.send(Ok(Value::Null)).unwrap();
        assert_eq!(opening.await.unwrap(), EditorId::Vscode);
        assert_eq!(host.storage_write_count(), 1);
        allowed.set(false);
        assert!(open().await.is_err());
        assert!(endpoint.requests.borrow().is_empty());
        assert_eq!(host.storage_write_count(), 1);
        assert_eq!(
            preferred_editor(&[EditorId::Vscode, EditorId::Cursor], None),
            Some(EditorId::Cursor)
        );
        assert_eq!(
            preferred_editor(
                &[EditorId::Vscode, EditorId::Cursor],
                Some(EditorId::Vscode)
            ),
            Some(EditorId::Vscode)
        );
    });
}

fn progress(kind: &str, id: &str) -> GitActionProgressEvent {
    let mut value = json!({"kind":kind,"actionId":id,"cwd":"/repo","action":"commit_push"});
    match kind {
        "action_started" => value["phases"] = json!(["commit", "push"]),
        "phase_started" => {
            value["phase"] = json!("commit");
            value["label"] = json!("Committing...");
        }
        "hook_started" => value["hookName"] = json!("post-commit"),
        "hook_output" => {
            value["hookName"] = json!("post-commit");
            value["stream"] = json!("stdout");
            value["text"] = json!("hook output");
        }
        "hook_finished" => {
            value["hookName"] = json!("post-commit");
            value["exitCode"] = json!(0);
            value["durationMs"] = json!(12);
        }
        "action_failed" => {
            value["phase"] = json!("push");
            value["message"] = json!("credential=do-not-log");
        }
        "action_finished" => value["result"] = serde_json::to_value(action_result()).unwrap(),
        _ => unreachable!(),
    }
    decode(value)
}
fn action_result() -> GitRunStackedActionResult {
    decode(
        json!({"action":"commit_push","branch":{"status":"skipped_not_requested"},"commit":{"status":"created"},"push":{"status":"pushed"},"pr":{"status":"skipped_not_requested"},"toast":{"title":"Done","cta":{"kind":"none"}}}),
    )
}
#[test]
fn action_progress_preserves_phase_hook_timing_and_newer_ownership() {
    let initial = begin_vcs_action_state(
        VcsActionOperation::RunChangeRequest,
        "Running source control action",
        "local".into(),
        10,
    );
    let phase = apply_vcs_action_progress_event(&initial, &progress("phase_started", "local"), 20);
    assert_eq!(phase.current_phase_label.as_deref(), Some("Committing..."));
    assert_eq!(phase.phase_started_at_ms, Some(20));
    let hook = apply_vcs_action_progress_event(&phase, &progress("hook_started", "local"), 30);
    assert_eq!(
        hook.current_label.as_deref(),
        Some("Running post-commit...")
    );
    assert_eq!(hook.hook_started_at_ms, Some(30));
    assert_eq!(hook.phase_started_at_ms, Some(20));
    let output = apply_vcs_action_progress_event(&hook, &progress("hook_output", "local"), 40);
    assert_eq!(output.last_output_line.as_deref(), Some("hook output"));
    let finished =
        apply_vcs_action_progress_event(&output, &progress("hook_finished", "local"), 50);
    assert_eq!(finished.current_label, phase.current_label);
    assert!(finished.hook_name.is_none());
    assert!(finished.last_output_line.is_none());
    assert!(finished.is_running);
    let newer = begin_vcs_action_state(VcsActionOperation::Pull, "Pulling", "newer".into(), 60);
    assert_eq!(
        apply_vcs_action_progress_event(&newer, &progress("phase_started", "local"), 70),
        newer
    );
    let finished =
        apply_vcs_action_progress_event(&finished, &progress("action_finished", "local"), 60);
    assert!(finished.is_running);
}
#[test]
fn progress_transport_ids_isolate_environments_and_report_redacted_failures() {
    block_on(async {
        let target = target("local");
        let id = create_vcs_action_transport_id(&target, "action").unwrap();
        let other =
            create_vcs_action_transport_id(&super::tests::target("remote"), "action").unwrap();
        assert_ne!(id, other);
        assert!(
            normalize_vcs_action_progress_event(
                &target,
                &id,
                "action",
                progress("phase_started", &other)
            )
            .is_none()
        );
        let mut seen = Vec::new();
        let events = vec![
            Ok(progress("phase_started", &other)),
            Ok(progress("phase_started", &id)),
            Ok(progress("action_finished", &id)),
        ];
        let result = consume_vcs_action_progress(
            futures::stream::iter(events).boxed_local(),
            &target,
            &id,
            "action",
            &GitStackedAction::CommitPush,
            |event| seen.push(serde_json::to_value(event).unwrap()["actionId"].clone()),
        )
        .await
        .unwrap();
        assert_eq!(result, action_result());
        assert_eq!(seen, vec![json!("action"), json!("action")]);
        let error = consume_vcs_action_progress(
            futures::stream::iter(vec![Ok(progress("action_failed", &id))]).boxed_local(),
            &target,
            &id,
            "action",
            &GitStackedAction::CommitPush,
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            VcsActionError::RemoteFailure {
                remote_message_length: 21,
                ..
            }
        ));
        assert_eq!(
            error.to_string(),
            "Source control action 'commit_push' failed during push."
        );
        assert!(!format!("{error:?}").contains("credential="));
        let error = consume_vcs_action_progress(
            futures::stream::empty().boxed_local(),
            &target,
            &id,
            "action",
            &GitStackedAction::CommitPush,
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, VcsActionError::MissingTerminalEvent { .. }));
        let malformed = "not-json-with-credential=do-not-log";
        let error = parse_vcs_action_target_key(malformed).unwrap_err();
        assert_eq!(error.key_length, malformed.len());
        assert!(!format!("{error:?}").contains(malformed));
        assert_eq!(
            parse_vcs_action_target_key("[\"local\",\"/repo\"]").unwrap(),
            target
        );
    });
}
#[test]
fn tracked_actions_do_not_clear_newer_state_and_cancellation_releases_ownership() {
    block_on(async {
        let manager = VcsActionManager::default();
        let host = FakePlatform::default();
        let target = target("local");
        let (finish, finished) = oneshot::channel::<Result<(), ClientError>>();
        let mut first =
            Box::pin(
                manager.track(&target, VcsActionOperation::Pull, "Pulling", &host, async {
                    finished.await.unwrap()
                }),
            );
        assert!(futures::poll!(&mut first).is_pending());
        let first_id = manager.state(Some(&target)).action_id.clone();
        let (fail, failed) = oneshot::channel::<Result<(), ClientError>>();
        let mut second = Box::pin(manager.track(
            &target,
            VcsActionOperation::SwitchRef,
            "Switching",
            &host,
            async { failed.await.unwrap() },
        ));
        assert!(futures::poll!(&mut second).is_pending());
        let second_id = manager.state(Some(&target)).action_id.clone();
        assert_ne!(first_id, second_id);
        finish.send(Ok(())).unwrap();
        first.await.unwrap();
        assert_eq!(manager.state(Some(&target)).action_id, second_id);
        fail.send(Err(ClientError::InvalidPayload("switch failed".into())))
            .unwrap();
        assert!(second.await.is_err());
        assert!(!manager.state(Some(&target)).is_running);
        assert!(
            manager
                .state(Some(&target))
                .error
                .as_ref()
                .unwrap()
                .contains("switch failed")
        );
        manager.reset_error(&target, VcsActionOperation::Pull);
        assert!(manager.state(Some(&target)).error.is_some());
        manager.reset_error(&target, VcsActionOperation::SwitchRef);
        assert_eq!(*manager.state(Some(&target)), VcsActionState::default());
        let mut cancelled = Box::pin(manager.track::<(), ClientError>(
            &target,
            VcsActionOperation::Pull,
            "Pulling",
            &host,
            futures::future::pending(),
        ));
        assert!(futures::poll!(&mut cancelled).is_pending());
        drop(cancelled);
        assert_eq!(*manager.state(Some(&target)), VcsActionState::default());
        let interrupted = manager
            .track(&target, VcsActionOperation::Pull, "Pulling", &host, async {
                Err::<(), _>(ClientError::Cancelled)
            })
            .await;
        assert!(matches!(interrupted, Err(ClientError::Cancelled)));
        assert_eq!(*manager.state(Some(&target)), VcsActionState::default());
    });
}
#[test]
fn stacked_actions_share_checkout_queue_invalidate_on_all_outcomes_and_keep_ids() {
    block_on(async {
        let source = SourceControl::default();
        let manager = VcsActionManager::default();
        let host = FakePlatform::default();
        let endpoint = ControlledRpc::new("local");
        let target = target("local");
        for kind in ["action_finished", "action_failed", "cancel"] {
            let input = decode(
                json!({"actionId":"action","cwd":"/repo","action":"commit_push","threadId":"t","projectId":"p"}),
            );
            let before = source.refs.state(&env("local")).revision;
            let mut running = Box::pin(manager.run_stacked_action(
                &source,
                &target,
                &endpoint,
                &host,
                input,
                |_| Err(ClientError::Cancelled),
            ));
            assert!(futures::poll!(&mut running).is_pending());
            let stream = endpoint.stream();
            assert_eq!(stream.method, "git.runStackedAction");
            assert_eq!(stream.input["threadId"], "t");
            assert_eq!(stream.input["projectId"], "p");
            let transport = stream.input["actionId"].as_str().unwrap();
            if kind == "cancel" {
                drop(running);
                assert!(stream.reply.is_closed());
                assert!(!source.refs.state(&env("local")).persisted_cache_readable);
            } else {
                stream
                    .reply
                    .unbounded_send(Ok(serde_json::to_value(progress(kind, transport)).unwrap()))
                    .unwrap();
                drop(stream);
                let result = running.await;
                assert_eq!(result.is_ok(), kind == "action_finished");
            }
            assert_eq!(source.refs.state(&env("local")).revision, before + 1);
            assert!(!manager.state(Some(&target)).is_running);
        }
    });
}

#[test]
fn live_refs_emit_cache_before_live_refresh_and_reconnect_without_stale_results() {
    block_on(async {
        let host = FakePlatform::default();
        let cache = VcsRefsCache::default();
        let target = target("local");
        let input = refs_input();
        let cached = refs("cached");
        cache.commit_refresh(&host, &target, &cached, 0, true).await;
        let live = LiveVcs::<ControlledRpc>::new(cache.clone());
        let resource = live.list_refs(env("local"), &input).unwrap();
        let mut observer = resource.observe(0);
        observer.next().await;
        let mut driver = Box::pin(live.follow_refs(env("local"), &host, input.clone()));
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(resource.snapshot().value.as_deref(), Some(&cached));
        let first = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(1), Some(first.clone()))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        let old = first.request();
        assert_eq!(old.input, json!({"cwd":"/repo","limit":100}));
        let second = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(2), Some(second.clone()))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(old.reply.is_canceled());
        let request = second.request();
        let fresh = refs("fresh");
        request
            .reply
            .send(Ok(serde_json::to_value(&fresh).unwrap()))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(resource.snapshot().value.as_deref(), Some(&fresh));
        assert!(Rc::ptr_eq(
            &resource.snapshot().value.unwrap(),
            &live
                .list_refs(env("local"), &input)
                .unwrap()
                .snapshot()
                .value
                .unwrap()
        ));
        cache.invalidate_cached(&host, &env("local")).await;
        assert!(futures::poll!(&mut driver).is_pending());
        let request = second.request();
        assert_eq!(request.input["refresh"], true);
        live.set_session(env("local"), Generation(3), None).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(request.reply.is_canceled());
        assert_eq!(resource.snapshot().value.as_deref(), Some(&fresh));
        drop(driver);
        assert_eq!(host.pending_timers(), 0);
    });
}
#[test]
fn refs_cache_reads_and_writes_the_shared_persistence_envelope() {
    block_on(async {
        let host = FakePlatform::default();
        let persistence = crate::persistence::EnvironmentCacheStore::new(host.clone());
        let cache = VcsRefsCache::default();
        let target = target("local");
        let cached = refs("imported");
        persistence
            .save_vcs_refs(&crate::persistence::StoredVcsRefs {
                schema_version: 1,
                environment_id: env("local"),
                cwd: "/repo".into(),
                refs: cached.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            cache.load(&host, &target, &refs_input()).await,
            Some(cached)
        );
        let fresh = refs("fresh");
        assert!(cache.commit_refresh(&host, &target, &fresh, 0, true).await);
        assert_eq!(
            persistence
                .load_vcs_refs(&env("local"), "/repo")
                .await
                .unwrap()
                .unwrap()
                .refs,
            fresh
        );
        assert!(cache.invalidate_cached(&host, &env("local")).await);
        assert!(
            persistence
                .load_vcs_refs(&env("local"), "/repo")
                .await
                .unwrap()
                .is_none()
        );
    });
}
#[test]
fn live_ref_retries_use_injected_time_and_cancel_when_session_disappears() {
    block_on(async {
        let host = FakePlatform::default();
        let live = LiveVcs::<ControlledRpc>::new(VcsRefsCache::default());
        let endpoint = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(1), Some(endpoint.clone()))
            .unwrap();
        let input = refs_input();
        let resource = live.list_refs(env("local"), &input).unwrap();
        let _observer = resource.observe(0);
        let mut driver = Box::pin(live.follow_refs(env("local"), &host, input));
        assert!(futures::poll!(&mut driver).is_pending());
        endpoint
            .request()
            .reply
            .send(Err(ClientError::InvalidPayload("Git failed".into())))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(host.pending_timers(), 1);
        host.advance(999);
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(endpoint.requests.borrow().is_empty());
        host.advance(1);
        assert!(futures::poll!(&mut driver).is_pending());
        endpoint
            .request()
            .reply
            .send(Err(ClientError::Disconnected))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(host.pending_timers(), 1);
        live.set_session(env("local"), Generation(2), None).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(host.pending_timers(), 0);
        host.advance(30_000);
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(endpoint.requests.borrow().is_empty());
    });
}
#[test]
fn filtered_refs_stay_live_only_and_share_server_freshness_per_checkout() {
    block_on(async {
        let host = FakePlatform::default();
        let cache = VcsRefsCache::default();
        cache
            .commit_refresh(&host, &target("local"), &refs("cached"), 0, true)
            .await;
        cache.invalidate(&env("local"), None);
        let live = LiveVcs::<ControlledRpc>::new(cache.clone());
        let endpoint = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(1), Some(endpoint.clone()))
            .unwrap();
        let mut input = refs_input();
        input.query = Some(Some("feature".into()));
        let resource = live.list_refs(env("local"), &input).unwrap();
        let _observer = resource.observe(0);
        let mut driver = Box::pin(live.follow_refs(env("local"), &host, input));
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(resource.snapshot().value.is_none());
        let request = endpoint.request();
        assert_eq!(request.input["refresh"], true);
        request
            .reply
            .send(Ok(serde_json::to_value(refs("filtered")).unwrap()))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(
            cache
                .load(&host, &target("local"), &refs_input())
                .await
                .unwrap()
                .refs[0]
                .name
                .0,
            "cached"
        );
        assert!(!cache.bypass_server_snapshot(&target("local"), 1));
    });
}
#[test]
fn status_and_branch_streams_replace_sessions_and_idle_disposal_stops_streams() {
    block_on(async {
        let host = FakePlatform::default();
        let live = LiveVcs::<ControlledRpc>::new(VcsRefsCache::default());
        let endpoint = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(1), Some(endpoint.clone()))
            .unwrap();
        let input = decode(json!({"cwd":"/repo"}));
        let resource = live.branch(env("local"), &input).unwrap();
        let mut observer = resource.observe(0);
        let mut driver = Box::pin(live.follow_branch(env("local"), &host, input));
        assert!(futures::poll!(&mut driver).is_pending());
        let stream = endpoint.stream();
        assert_eq!(stream.method, "subscribeVcsBranch");
        assert_eq!(stream.input, json!({"cwd":"/repo"}));
        live.set_session(env("local"), Generation(1), Some(endpoint.clone()))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(!stream.reply.is_closed());
        assert!(endpoint.streams.borrow().is_empty());
        stream
            .reply
            .unbounded_send(Ok(json!({"refName":"main","isRepo":true})))
            .unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(
            resource.snapshot().value.unwrap().ref_name.as_deref(),
            Some("main")
        );
        observer.tick(0);
        drop(observer);
        live.sweep(STATUS_IDLE_TTL_MS - 1);
        assert!(!stream.reply.is_closed());
        live.sweep(STATUS_IDLE_TTL_MS);
        assert!(matches!(
            futures::poll!(&mut driver),
            std::task::Poll::Ready(Err(ClientError::Cancelled))
        ));
        assert!(stream.reply.is_closed());
        let input = decode(json!({"cwd":"/repo"}));
        let resource = live.status(env("local"), &input).unwrap();
        let _observer = resource.observe(0);
        let mut driver = Box::pin(live.follow_status(env("local"), &host, input));
        assert!(futures::poll!(&mut driver).is_pending());
        let stream = endpoint.stream();
        assert_eq!(stream.method, "subscribeVcsStatus");
        stream.reply.unbounded_send(Ok(json!({"_tag":"remoteUpdated","remote":{"hasUpstream":true,"aheadCount":2,"behindCount":1,"pr":null}}))).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert_eq!(resource.snapshot().value.unwrap().ahead_count, 2);
        live.set_session(env("local"), Generation(2), None).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(stream.reply.is_closed());
    });
}
#[test]
fn picker_status_refresh_is_unscheduled_and_mutations_invalidate_even_on_failure() {
    block_on(async {
        let source = SourceControl::default();
        let endpoint = ControlledRpc::new("local");
        let host = FakePlatform::default();
        let input = decode(json!({"cwd":"/repo","localOnly":true}));
        let mut refresh =
            Box::pin(source.refresh_local_status_for_picker(env("local"), &endpoint, &host, input));
        assert!(futures::poll!(&mut refresh).is_pending());
        let read = endpoint.request();
        assert_eq!(read.method, "vcs.refreshStatus");
        let mut switch = Box::pin(source.switch_ref(
            env("local"),
            &endpoint,
            &host,
            decode(json!({"cwd":"/repo","refName":"feature"})),
        ));
        assert!(futures::poll!(&mut switch).is_pending());
        let checkout = endpoint.request();
        assert_eq!(checkout.method, "vcs.switchRef");
        checkout
            .reply
            .send(Err(ClientError::InvalidPayload("switch failed".into())))
            .unwrap();
        assert!(switch.await.is_err());
        assert_eq!(source.refs.state(&env("local")).revision, 1);
        read.reply.send(Err(ClientError::Disconnected)).unwrap();
        assert!(refresh.await.is_err());
        assert_eq!(source.refs.state(&env("local")).revision, 2);
    });
}

#[test]
fn clone_actions_serialize_per_environment_while_sign_in_remains_independent() {
    block_on(async {
        let source = SourceControl::default();
        let endpoint = ControlledRpc::new("local");
        let remote = ControlledRpc::new("remote");
        let environment = env("local");
        let input = decode(
            json!({"projectId":"p","title":"Repo","createdAt":"2026-01-01T00:00:00Z","remoteUrl":"https://github.com/org/repo","destinationPath":"/repo"}),
        );
        let mut start = Box::pin(source.start_project_clone(env("local"), &endpoint, input));
        assert!(futures::poll!(&mut start).is_pending());
        let started = endpoint.request();
        assert_eq!(started.method, "projectClone.start");
        let mut cancel = Box::pin(source.cancel_project_clone(
            env("local"),
            &endpoint,
            decode(json!({"projectId":"p"})),
        ));
        let mut retry = Box::pin(source.retry_project_clone(
            env("local"),
            &endpoint,
            decode(json!({"projectId":"p"})),
        ));
        assert!(futures::poll!(&mut cancel).is_pending());
        assert!(futures::poll!(&mut retry).is_pending());
        assert!(endpoint.requests.borrow().is_empty());
        let mut wait = Box::pin(source.wait_for_sign_in(
            &environment,
            &endpoint,
            decode(json!({"provider":"github","host":"github.com"})),
        ));
        assert!(futures::poll!(&mut wait).is_pending());
        let sign_in = endpoint.request();
        assert_eq!(sign_in.method, "sourceControl.signInWait");
        let mut other = Box::pin(source.cancel_project_clone(
            env("remote"),
            &remote,
            decode(json!({"projectId":"p"})),
        ));
        assert!(futures::poll!(&mut other).is_pending());
        remote
            .request()
            .reply
            .send(Ok(json!({"applied":true})))
            .unwrap();
        assert!(other.await.unwrap().applied);
        started.reply.send(Ok(json!({"projectId":"p","cwd":"/repo","remoteUrl":"https://github.com/org/repo","repository":null}))).unwrap();
        start.await.unwrap();
        assert!(futures::poll!(&mut retry).is_pending());
        assert!(endpoint.requests.borrow().is_empty());
        assert!(futures::poll!(&mut cancel).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "projectClone.cancel");
        request.reply.send(Ok(json!({"applied":true}))).unwrap();
        cancel.await.unwrap();
        assert!(futures::poll!(&mut retry).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "projectClone.retry");
        request.reply.send(Ok(json!({"applied":true}))).unwrap();
        retry.await.unwrap();
        sign_in
            .reply
            .send(Ok(json!({"account":"octocat"})))
            .unwrap();
        assert_eq!(wait.await.unwrap().account.0, "octocat");
    });
}

#[test]
fn optimistic_files_are_observable_scoped_and_compare_confirmation_identity() {
    block_on(async {
        let commands = ProjectCommands::default();
        let target = ProjectFileTarget {
            environment: env("local"),
            cwd: "/repo".into(),
            relative_path: "file".into(),
        };
        let remote = ProjectFileTarget {
            environment: env("remote"),
            ..target.clone()
        };
        let resource = commands.optimistic_file(&target);
        let mut observer = resource.observe(0);
        observer.next().await;
        let data: ProjectReadFileResult = decode(
            json!({"relativePath":"file","byteLength":3,"contents":"new","truncated":false}),
        );
        let first = OptimisticProjectFile {
            data: data.clone(),
            confirmed_against: Some(Rc::new(data.clone())),
        };
        let second = OptimisticProjectFile {
            data: data.clone(),
            confirmed_against: Some(Rc::new(data)),
        };
        assert_ne!(first, second);
        commands.set_optimistic_file(&target, Some(first.clone()));
        assert!(observer.next().now_or_never().is_some());
        assert_eq!(
            resource.snapshot().value.as_deref(),
            Some(&Some(first.clone()))
        );
        commands.set_optimistic_file(&remote, Some(first));
        commands.remove_environment(&env("local"));
        assert_eq!(resource.snapshot().value.as_deref(), Some(&None));
        assert!(
            commands
                .optimistic_file(&remote)
                .snapshot()
                .value
                .unwrap()
                .is_some()
        );
    });
}

#[test]
fn clone_and_worktree_streams_keep_contract_inputs_and_replace_sessions() {
    block_on(async {
        let host = FakePlatform::default();
        let live = LiveVcs::<ControlledRpc>::new(VcsRefsCache::default());
        let endpoint = Rc::new(ControlledRpc::new("local"));
        live.set_session(env("local"), Generation(1), Some(endpoint.clone()))
            .unwrap();
        let clones = live.project_clones(env("local")).unwrap();
        let _observer = clones.observe(0);
        let mut driver = Box::pin(live.follow_project_clones(env("local"), &host));
        assert!(futures::poll!(&mut driver).is_pending());
        let stream = endpoint.stream();
        assert_eq!(stream.method, "subscribeProjectClones");
        assert_eq!(stream.input, json!({}));
        stream.reply.unbounded_send(Ok(json!([]))).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(clones.snapshot().value.unwrap().is_empty());
        let input = decode(json!({"threadId":"t"}));
        let setup = live.worktree_setup(env("local"), &input).unwrap();
        let _observer = setup.observe(0);
        let mut setup_driver = Box::pin(live.follow_worktree_setup(env("local"), &host, input));
        assert!(futures::poll!(&mut setup_driver).is_pending());
        let setup_stream = endpoint.stream();
        assert_eq!(setup_stream.method, "subscribeWorktreeSetup");
        assert_eq!(setup_stream.input, json!({"threadId":"t"}));
        setup_stream.reply.unbounded_send(Ok(Value::Null)).unwrap();
        assert!(futures::poll!(&mut setup_driver).is_pending());
        assert_eq!(setup.snapshot().value.as_deref(), Some(&None));
        live.set_session(env("local"), Generation(2), None).unwrap();
        assert!(futures::poll!(&mut driver).is_pending());
        assert!(futures::poll!(&mut setup_driver).is_pending());
        assert!(stream.reply.is_closed());
        assert!(setup_stream.reply.is_closed());
        live.remove_environment(&env("local"));
        assert!(matches!(driver.await, Err(ClientError::Cancelled)));
        assert!(matches!(setup_driver.await, Err(ClientError::Cancelled)));
    });
}

#[test]
fn project_query_reads_seed_metadata_and_session_replacement_rejects_old_answers() {
    block_on(async {
        let host = FakePlatform::default();
        let endpoint = ControlledRpc::new("local");
        let commands = ProjectCommands::default();
        let environment = env("local");
        commands.begin_session(environment.clone(), Generation(1));
        let input: ProjectReadFileInput = decode(json!({"cwd":"/repo","relativePath":"file"}));
        let resource = commands
            .query::<WsProjectsReadFileRpc>(environment.clone(), &input)
            .unwrap();
        let _observer = resource.observe(0);
        let mut read = Box::pin(commands.refresh_read_file(
            environment.clone(),
            &endpoint,
            &host,
            input.clone(),
        ));
        assert!(futures::poll!(&mut read).is_pending());
        endpoint
            .request()
            .reply
            .send(Ok(
                json!({"relativePath":"file","byteLength":3,"contents":"new","truncated":false}),
            ))
            .unwrap();
        read.await.unwrap();
        assert_eq!(resource.snapshot().value.unwrap().contents, "new");
        let metadata = commands
            .metadata
            .enqueue(environment.clone(), "/repo/file", None, 0);
        assert_eq!(
            metadata
                .snapshot()
                .value
                .unwrap()
                .as_ref()
                .as_ref()
                .unwrap()
                .byte_length,
            Some(3)
        );
        let mut read =
            Box::pin(commands.refresh_read_file(environment.clone(), &endpoint, &host, input));
        assert!(futures::poll!(&mut read).is_pending());
        let old = endpoint.request();
        commands.begin_session(environment.clone(), Generation(2));
        assert!(matches!(read.await, Err(ClientError::Cancelled)));
        assert!(old.reply.is_canceled());
        assert_eq!(resource.snapshot().value.unwrap().contents, "new");
        let input = decode(json!({"cwd":"/repo"}));
        let entries = commands
            .query::<WsProjectsListEntriesRpc>(environment.clone(), &input)
            .unwrap();
        let mut read =
            Box::pin(commands.refresh_list_entries(environment.clone(), &endpoint, &host, input));
        assert!(futures::poll!(&mut read).is_pending());
        endpoint.request().reply.send(Ok(json!({"entries":[{"path":"file","kind":"file"},{"path":"src","kind":"directory"}],"truncated":false}))).unwrap();
        read.await.unwrap();
        assert_eq!(entries.snapshot().value.unwrap().entries.len(), 2);
        let mut invalidate = Box::pin(commands.invalidate_file_metadata(
            &environment,
            &endpoint,
            &host,
            vec!["/repo/file".into(), "/repo/file".into()],
        ));
        assert!(futures::poll!(&mut invalidate).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "filesystem.getMetadata");
        assert_eq!(request.input["paths"], json!(["/repo/file"]));
        request
            .reply
            .send(Ok(json!({"entries":[{"kind":"file","byteLength":10}]})))
            .unwrap();
        invalidate.await.unwrap();
        assert_eq!(
            metadata
                .snapshot()
                .value
                .unwrap()
                .as_ref()
                .as_ref()
                .unwrap()
                .byte_length,
            Some(10)
        );
    });
}

#[test]
fn project_mutations_share_a_project_queue_without_blocking_other_projects() {
    block_on(async {
        let commands = ProjectCommands::default();
        let endpoint = ControlledRpc::new("local");
        let input = |project: &str, command: &str| {
            decode(json!({"type":"project.delete","projectId":project,"commandId":command}))
        };
        let mut first = Box::pin(commands.delete(env("local"), &endpoint, input("p", "one")));
        assert!(futures::poll!(&mut first).is_pending());
        let first_request = endpoint.request();
        let mut second = Box::pin(commands.delete(env("local"), &endpoint, input("p", "two")));
        assert!(futures::poll!(&mut second).is_pending());
        assert!(endpoint.requests.borrow().is_empty());
        let mut independent =
            Box::pin(commands.delete(env("local"), &endpoint, input("q", "three")));
        assert!(futures::poll!(&mut independent).is_pending());
        let request = endpoint.request();
        assert_eq!(request.input["commandId"], "three");
        drop(first);
        assert!(first_request.reply.is_canceled());
        assert!(futures::poll!(&mut second).is_pending());
        let request = endpoint.request();
        assert_eq!(request.method, "projects.mutate");
        assert_eq!(request.input["commandId"], "two");
        drop(second);
        drop(independent);
        let remote = ControlledRpc::new("remote");
        assert!(matches!(
            commands
                .delete(env("local"), &remote, input("p", "four"))
                .await,
            Err(crate::rpc::RpcCallError::NotSent(
                ClientError::EnvironmentMismatch
            ))
        ));
        assert!(remote.requests.borrow().is_empty());
    });
}

#[test]
fn removal_plan_counts_listed_and_unloaded_threads_but_deletes_all_scoped_members() {
    let member = ProjectRemovalMember {
        key: ProjectKey {
            environment: env("a:b"),
            project: ProjectId("c".into()),
        },
        title: "Repo".into(),
        workspace_root: "/repo".into(),
        environment_label: Some("Desktop".into()),
    };
    let make_thread = |id: &str, relationship: Value, side_chat: bool| {
        decode(
            json!({"id":id,"projectId":"c","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":relationship,"sideChat":side_chat},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}),
        )
    };
    let mut collision: t3_contracts::orchestration_v2::OrchestrationV2ThreadShell =
        make_thread("outside", Value::Null, false);
    collision.project_id = ProjectId("b:c".into());
    let threads = vec![
        (env("a:b"), make_thread("root", Value::Null, false)),
        (env("a:b"), make_thread("child", json!("subagent"), false)),
        (env("a:b"), make_thread("side", Value::Null, true)),
        (env("a"), collision),
    ];
    let plan = plan_project_removal(
        std::slice::from_ref(&member),
        1,
        false,
        "Group",
        &threads,
        2,
    );
    assert_eq!(
        plan.project_threads,
        vec![
            thread("a:b", "root"),
            thread("a:b", "child"),
            thread("a:b", "side")
        ]
    );
    assert_eq!(
        plan.confirmation_message,
        "Remove project \"Repo\" and delete its 3 threads?\nPath: /repo\nEnvironment: Desktop\nThose threads and any archived threads are deleted from T3 Code. Provider sessions are kept.\nThis removes only the project entries, not the files on disk.\nThis action cannot be undone."
    );
    let plan = plan_project_removal(&[member], 2, true, "Group", &[], 0);
    assert_eq!(plan.target_kind, "checkout");
    assert!(!plan.is_whole_group);
    assert!(
        plan.confirmation_message
            .contains("Other entries in this grouped project are unaffected.")
    );
}

#[test]
fn sidebar_settings_and_scope_migration_preserve_physical_scratch_and_ordinary_repositories() {
    let identity = json!({"canonicalKey":"github.com/org/repo","rootPath":"/repo","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/org/repo.git"}});
    let mut local = project("local", "/scratch");
    local.is_scratch = true;
    let mut remote = project("remote", "/remote/scratch");
    remote.environment = env("remote");
    remote.is_scratch = true;
    let projects = vec![local.clone(), remote.clone()];
    let settings = GroupingSettings::default();
    let filters = build_sidebar_snapshots(
        &projects,
        &settings,
        true,
        Some(&env("local")),
        |_| None,
        |_| false,
        |_| false,
    );
    let physical = build_sidebar_snapshots(
        &projects,
        &settings,
        false,
        Some(&env("local")),
        |_| None,
        |_| false,
        |_| false,
    );
    assert_eq!(filters.len(), 1);
    assert_eq!(physical.len(), 2);
    assert_eq!(
        settings_key(&filters[0], &settings, &projects, None),
        physical[0].group.key
    );
    assert_eq!(
        settings_key(&filters[0], &settings, &projects, Some(&remote.key())),
        physical[1].group.key
    );
    for mode in [GroupingMode::Repository, GroupingMode::RepositoryPath] {
        let settings = GroupingSettings {
            mode,
            ..Default::default()
        };
        local.project.repository_identity = Some(Some(decode(identity.clone())));
        local.project.workspace_root.0 = "/repo/scratch".into();
        let old_key = logical_project_key(&local, &settings, false);
        let groups = build_sidebar_snapshots(
            &[local.clone()],
            &settings,
            true,
            None,
            |_| None,
            |_| false,
            |_| false,
        );
        assert_eq!(
            resolve_scope(&groups, Some(old_key.clone()), &settings, true),
            Some(LogicalProjectKey::Scratch)
        );
        let mut ordinary = project("ordinary", "/ordinary");
        ordinary.project.repository_identity = local.project.repository_identity.clone();
        let key = logical_project_key(&ordinary, &settings, false);
        let groups = build_sidebar_snapshots(
            &[local.clone(), ordinary],
            &settings,
            true,
            None,
            |_| None,
            |_| false,
            |_| false,
        );
        assert_eq!(
            resolve_scope(&groups, Some(key.clone()), &settings, true),
            Some(key)
        );
    }
}

#[test]
fn sidebar_picker_dedupes_stale_rows_retains_refs_and_prefers_the_current_environment() {
    let identity = json!({"canonicalKey":"github.com/org/repo","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/org/repo.git"}});
    let mut stale = project("stale", "/repo");
    let mut canonical = project("canonical", "/repo/");
    canonical.project.repository_identity = Some(Some(decode(identity.clone())));
    canonical.project.updated_at.0 = "2026-01-02T00:00:00Z".into();
    let mut remote = project("remote", "/repo");
    remote.environment = env("remote");
    remote.project.repository_identity = canonical.project.repository_identity.clone();
    let settings = GroupingSettings::default();
    for identified in [false, true] {
        stale.project.repository_identity = identified.then(|| Some(decode(identity.clone())));
        let groups = build_sidebar_snapshots(
            &[stale.clone(), canonical.clone(), remote.clone()],
            &settings,
            false,
            Some(&env("local")),
            |_| None,
            |_| false,
            |_| false,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].group.members,
            vec![canonical.clone(), remote.clone()]
        );
        assert_eq!(
            groups[0].group.member_project_refs,
            vec![stale.key(), canonical.key(), remote.key()]
        );
        let entries = picker_entries(&groups, Some(&stale.key()));
        assert!(entries[0].preferred);
        assert_eq!(entries[0].target, &canonical);
    }
    let mut destination = canonical.clone();
    destination.project.id = ProjectId("destination".into());
    destination.project.workspace_root.0 = "/destination".into();
    destination
        .project
        .repository_identity
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .canonical_key
        .0 = "github.com/org/destination".into();
    let mut destination_remote = destination.clone();
    destination_remote.environment = env("remote");
    destination_remote.project.id = ProjectId("destination-remote".into());
    let fallback = project("fallback", "/fallback");
    let mut pinned = project("pinned", "/pinned");
    pinned.project.pinned_at = Some(Some(decode(json!("2026-01-01T00:00:00Z"))));
    let groups = build_sidebar_snapshots(
        &[
            fallback.clone(),
            pinned.clone(),
            canonical,
            remote.clone(),
            destination.clone(),
            destination_remote.clone(),
        ],
        &settings,
        false,
        Some(&env("local")),
        |_| None,
        |_| false,
        |_| false,
    );
    let entries = picker_entries(&groups, Some(&remote.key()));
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.target.project.id == destination_remote.project.id)
            .unwrap()
            .target,
        &destination_remote
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.target.project.id == fallback.project.id)
            .unwrap()
            .target,
        &fallback
    );
    let mut pickable = groups
        .iter()
        .filter(|group| !group.group.archived && group.group.pinned)
        .chain(
            groups
                .iter()
                .filter(|group| !group.group.archived && !group.group.pinned),
        )
        .cloned()
        .collect::<Vec<_>>();
    pickable.retain(|group| {
        [fallback.key(), pinned.key(), remote.key()]
            .iter()
            .any(|key| group.group.member_project_refs.contains(key))
    });
    let entries = picker_entries(&pickable, Some(&remote.key()));
    assert!(entries[0].preferred);
    assert_eq!(entries[1].target, &pinned);
    assert_eq!(entries[2].target, &fallback);
}
