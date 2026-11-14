//! GitActionsControl exercised as PNL-DETAILS will mount it, from client resources.

use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    ClientError,
    services::resources::{Resource, ResourcePolicy},
    types::{EnvironmentId, Generation, ThreadId, ThreadKey},
};
use t3_contracts::{git::*, source_control::*};
use t3_ui::{i18n::t, screens::panel::git_actions::*, ui::toast};

fn scope() -> GitActionsScope {
    GitActionsScope {
        thread: ThreadKey {
            environment: EnvironmentId("first".into()),
            thread: ThreadId("thread".into()),
        },
        cwd: "/project".into(),
        draft_id: None,
    }
}

fn status(extra: serde_json::Value) -> VcsStatusResult {
    let mut value = json!({"isRepo":true,"hasPrimaryRemote":false,"isDefaultRef":true,"refName":"main","hasWorkingTreeChanges":false,
        "workingTree":{"rootPath":"/repo-root","files":[],"insertions":0,"deletions":0},"hasUpstream":false,"aheadCount":0,"behindCount":0,"pr":null});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("overrides").clone());
    serde_json::from_value(value).expect("canonical VCS projection")
}

fn snapshot() -> GitActionsSnapshot {
    GitActionsSnapshot {
        status: Some(Rc::new(status(json!({})))), can_write: true, can_operate: true, has_server_thread: true,
        publish_providers: vec![PublishProvider { kind: "github".into(), label: "GitHub".into(), host: "github.com".into(), path_placeholder: "owner/repo".into(),
            icon_svg: "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><path d=\"M2 2h20v20H2z\"/></svg>".into() }],
        discovery: Some(Rc::new(serde_json::from_value(json!({"versionControlSystems":[],"sourceControlProviders":[{
            "kind":"github","label":"GitHub","status":"available","version":{"_tag":"None"},"installHint":"Install GitHub CLI.","detail":{"_tag":"None"},
            "auth":{"status":"authenticated","account":{"_tag":"Some","value":"alice"},"host":{"_tag":"None"},"detail":{"_tag":"None"}}
        }]})).expect("discovery"))), ..Default::default()
    }
}

#[derive(Clone)]
struct FakeGit {
    resource: Resource<GitActionsSnapshot>,
    recorded: Signal<String>,
    fail: Signal<bool>,
}

impl FakeGit {
    fn change(&self, f: impl FnOnce(&mut GitActionsSnapshot)) {
        let mut value = self
            .resource
            .snapshot()
            .value
            .as_deref()
            .cloned()
            .expect("seeded");
        f(&mut value);
        let token = self.resource.begin();
        self.resource.finish(token, value, 0, false);
    }
    fn record(&self, value: String) {
        self.recorded.set(value);
    }
}

impl GitActionsBackend for FakeGit {
    fn watch(
        &self,
        _: &GitActionsScope,
        _: GitActionsWatch,
    ) -> LocalBoxStream<'static, GitActionsSnapshot> {
        let resource = self.resource.clone();
        self.resource
            .observe(0)
            .filter_map(move |_| futures::future::ready(resource.snapshot().value.as_deref().cloned()))
            .boxed_local()
    }
    fn refresh(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        async { Ok(()) }.boxed_local()
    }
    fn run_action(
        &self,
        scope: &GitActionsScope,
        request: GitActionRequest,
    ) -> LocalBoxFuture<'static, Result<GitRunStackedActionResult, ClientError>> {
        let this = self.clone();
        let scope = scope.clone();
        async move {
            this.record(format!("ran:{:?};feature:{};files:{};message:{};env:{}", request.action, request.feature_branch,
                request.file_paths.as_ref().map_or("all".into(), |p| p.join(",")), request.commit_message.as_deref().unwrap_or("auto"), scope.thread.environment.0));
            if this.fail.get_untracked() { return Err(ClientError::Rejected { message: "Hook refused the commit".into(), detail: None }); }
            Ok(serde_json::from_value(json!({"action":request.action,"branch":{"status":"skipped_not_requested"},"commit":{"status":"created"},"push":{"status":"skipped_not_requested"},
                "pr":{"status":"skipped_not_requested"},"toast":{"title":"Committed","cta":{"kind":"none"}}})).expect("result"))
        }.boxed_local()
    }
    fn pull(
        &self,
        _: &GitActionsScope,
    ) -> LocalBoxFuture<'static, Result<VcsPullResult, ClientError>> {
        self.record("pulled".into());
        async {
            Ok(serde_json::from_value(
                json!({"status":"pulled","refName":"feature","upstreamRef":"origin/feature"}),
            )
            .expect("pull result"))
        }
        .boxed_local()
    }
    fn initialize(&self, _: &GitActionsScope) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record("initialized".into());
        self.change(|s| s.status = Some(Rc::new(status(json!({})))));
        async { Ok(()) }.boxed_local()
    }
    fn publish(
        &self,
        _: &GitActionsScope,
        input: SourceControlPublishRepositoryInput,
    ) -> LocalBoxFuture<'static, Result<SourceControlPublishRepositoryResult, ClientError>> {
        let this = self.clone();
        async move {
            this.record(format!("published:{};remote:{};visibility:{:?};protocol:{:?}", input.repository.0, input.remote_name.flatten().expect("remote").0, input.visibility, input.protocol.flatten().expect("protocol")));
            if this.fail.get_untracked() { return Err(ClientError::Unsupported("sourceControl.publishRepository")); }
            Ok(serde_json::from_value(json!({"repository":{"provider":"github","nameWithOwner":input.repository.0,"url":"https://github.com/alice/project","sshUrl":"git@github.com:alice/project.git"},
                "remoteName":"origin","remoteUrl":"git@github.com:alice/project.git","branch":"main","status":"pushed"})).expect("publish result"))
        }.boxed_local()
    }
    fn sync_branch(
        &self,
        _: &GitActionsScope,
        branch: Option<String>,
        _: Option<String>,
        _: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("synced:{}", branch.as_deref().unwrap_or("null")));
        self.change(|s| s.thread_branch = branch);
        async { Ok(()) }.boxed_local()
    }
    fn open_file(
        &self,
        _: &GitActionsScope,
        root: String,
        path: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.record(format!("editor:{root}/{path}"));
        async { Ok(()) }.boxed_local()
    }
}

fn action(label: &'static str, callback: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(t(label))
        .on_press(callback)
        .child(text(t(label)))
}

#[component]
fn GitActionsFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let nav = t3_ui::state::nav::Navigator::new(t3_ui::state::nav::Route::Thread(scope().thread));
    provide_context(nav);
    let resource = Resource::new(ResourcePolicy::default(), Generation(0));
    let token = resource.begin();
    resource.finish(token, snapshot(), 0, false);
    let fake = FakeGit {
        resource,
        recorded: signal("no mutation".into()),
        fail: signal(false),
    };
    provide_context(GitActionsRuntime(Rc::new(fake.clone())));
    let selected_scope = signal(Some(scope()));
    let opens = signal(0);
    let mut props = GitActionsControlProps::new(Prop::bind(move || selected_scope.get()));
    props.on_open_changes = Some(Rc::new(move || opens.update(|n| *n += 1)));
    let dirty = fake.clone();
    let permission = fake.clone();
    let failure = fake.clone();
    let clone = fake.clone();
    let done = fake.clone();
    let unsupported = fake.clone();
    let nonrepo = fake.clone();
    let preview = fake.clone();
    let revoke_key = fake.clone();
    on_key("ctrl+shift+r", move |_| {
        revoke_key.change(|s| s.can_write = false)
    });
    let refuse_key = fake.clone();
    on_key("ctrl+shift+f", move |_| refuse_key.fail.set(true));
    on_key("ctrl+shift+e", move |_| {
        let mut next = scope();
        next.thread.environment = EnvironmentId("second".into());
        selected_scope.set(Some(next));
    });
    let tools = view()
        .child(action("Make dirty", move || dirty.change(|s| s.status = Some(Rc::new(status(json!({"hasPrimaryRemote":true,"hasUpstream":true,"hasWorkingTreeChanges":true,"branchChanges":{"baseRef":"main","insertions":0,"deletions":0},
            "workingTree":{"rootPath":"/repo-root","files":[{"path":"a.ts","insertions":5,"deletions":1},{"path":"b.ts","insertions":2,"deletions":0}],"insertions":7,"deletions":1}})))))))
        .child(action("Revoke write", move || permission.change(|s| s.can_write = false)))
        .child(action("Refuse mutations", move || failure.fail.set(true)))
        .child(action("Clone in progress", move || clone.change(|s| { s.has_server_thread = false; s.is_local_draft = true; s.project_cloning = true; s.thread_branch = None; s.status = Some(Rc::new(status(json!({"refName":"unborn"})))); })))
        .child(action("Clone finished", move || done.change(|s| { s.project_cloning = false; s.status = Some(Rc::new(status(json!({"refName":"real-default"})))); })))
        .child(action("Unsupported status", move || unsupported.change(|s| { s.status = None; s.status_error = Some("unsupported client operation: subscribeVcsStatus".into()); })))
        .child(action("Not a repository", move || nonrepo.change(|s| s.status = Some(Rc::new(status(json!({"isRepo":false})))))))
        .child(action("Preview totals", move || preview.change(|s| s.branch_preview = Some(t3_client::workspace::git_actions::ChangesTotals { base_ref: Some("origin/main".into()), insertions: 31, deletions: 4 }))))
        .child(action("Switch checkout", move || { let mut next = scope(); next.thread.environment = EnvironmentId("second".into()); selected_scope.set(Some(next)); }))
        .child(text(move || fake.recorded.get()))
        .child(text(move || format!("changes opened:{}", opens.get())));
    toast::toast_provider(
        view().w_full().h_full().child(tools).child(
            view()
                .w(px(280.0))
                .app_class(t3_ui::theme::tw("max-w-full"))
                .child(git_actions_control(props)),
        ),
        toast::Position::TopRight,
    )
}

fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        environment: nsh_core::platform::Environment {
            os: nsh_core::platform::Os::Web,
            idiom: nsh_core::platform::Idiom::Desktop,
            ..Default::default()
        },
        ..EngineConfig::new(Root::of::<GitActionsFixture>())
    })
    .expect("start Git controls");
    app.wait_for("Publish repository")
        .expect("client projection rendered");
    app
}

fn open_commit(app: &mut Headless) {
    app.press("role:button[Git action options]").expect("menu");
    app.press("role:menuitem[Commit]").expect("commit item");
    app.wait_for("Commit changes").expect("commit dialog");
}

#[test]
fn publish_and_changes_flow_runs_from_the_projection_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.press("role:button[Changes]").expect("open diff");
        app.wait_for("changes opened:1").expect("diff action");
        app.press("role:button[Publish repository]")
            .expect("publish");
        app.wait_for("Pick where to host it, then point us at a repo to push to.")
            .expect("wizard");
        app.press("role:button[Next]").expect("provider ready");
        assert_eq!(
            app.screen().input_text("id:publish-repository-path"),
            Ok(Some("alice/"))
        );
        app.type_text("id:publish-repository-path", "alice/project")
            .expect("repository");
        app.press("role:button[Publish]").expect("publish");
        app.wait_for("Repository published")
            .expect("real result branch");
        app.wait_for("published:alice/project;remote:origin;visibility:Private;protocol:Ssh")
            .expect("canonical publish input");
        app.press("role:button[Done]").expect("done");
    }
}

#[test]
fn commit_exclusion_message_failure_and_editor_root_follow_the_component() {
    let mut app = start(DESKTOP);
    app.press("role:button[Make dirty]").expect("external edit");
    app.wait_for("+7")
        .expect("dirty totals win over zero branch totals");
    open_commit(&mut app);
    app.press("role:button[Open a.ts in editor]")
        .expect("open file");
    app.wait_for("editor:/repo-root/a.ts")
        .expect("repository root");
    app.press("role:button[Edit]").expect("edit selection");
    app.press("role:checkbox[b.ts]").expect("exclude b");
    app.type_text("id:git-commit-message", "Keep my typed message")
        .expect("message");
    app.press_key("tab").expect("leave text input");
    app.press_key("ctrl+shift+f")
        .expect("server refusal while dialog stays open");
    app.press("role:button[Commit]").expect("commit");
    app.wait_for("ran:Commit;feature:false;files:a.ts;message:Keep my typed message;env:first")
        .expect("selected files only");
    open_commit(&mut app);
    assert_eq!(
        app.screen().input_text("id:git-commit-message"),
        Ok(Some("Keep my typed message"))
    );
}

#[test]
fn default_ref_confirmation_aborts_or_runs_the_same_action_on_a_feature_ref() {
    let mut app = start(PHONE);
    app.press("role:button[Make dirty]").expect("edit");
    app.press("role:button[Commit & push]")
        .expect("quick action");
    app.wait_for("Commit & push to default ref?")
        .expect("confirmation");
    assert!(app.screen().shows("no mutation"));
    app.press("role:button[Abort]").expect("abort");
    app.press("role:button[Commit & push]").expect("again");
    app.press("role:button[Check out feature branch & continue]")
        .expect("feature branch");
    app.wait_for("ran:CommitPush;feature:true;files:all;message:auto;env:first")
        .expect("same action on feature branch");
}

#[test]
fn open_menu_does_not_run_after_write_permission_is_revoked() {
    let mut app = start(DESKTOP);
    app.press("role:button[Make dirty]").expect("edit");
    app.press("role:button[Git action options]").expect("menu");
    app.press_key("ctrl+shift+r").expect("grant changed");
    let _ = app.press("role:menuitem[Commit]");
    assert!(app.screen().shows("no mutation"));
    assert!(!app.screen().shows("Commit changes"));
}

#[test]
fn clone_completion_uses_the_real_default_branch_and_not_the_unborn_head() {
    let mut app = start(DESKTOP);
    app.press("role:button[Clone in progress]")
        .expect("cloning");
    assert!(app.screen().shows("no mutation"));
    app.press("role:button[Clone finished]")
        .expect("clone complete");
    app.wait_for("synced:real-default")
        .expect("deferred automatic branch selection");
    assert!(!app.screen().shows("synced:unborn"));
}

#[test]
fn unsupported_status_keeps_the_web_menu_error_and_changes_preview_in_sync() {
    let mut app = start(DESKTOP);
    app.press("role:button[Unsupported status]")
        .expect("unsupported subscription");
    app.press("role:button[Git action options]")
        .expect("error notice enables menu");
    app.wait_for("unsupported client operation: subscribeVcsStatus")
        .expect("actual client error");
    app.press("role:button[Preview totals]")
        .expect("branch diff projection");
    app.wait_for("vs origin/main").expect("same base as Diff");
    app.wait_for("+31").expect("preview totals");
}

#[test]
fn a_checkout_change_disposes_the_open_commit_message_and_exclusions() {
    let mut app = start(DESKTOP);
    app.press("role:button[Make dirty]").expect("edit");
    open_commit(&mut app);
    app.type_text("id:git-commit-message", "first checkout only")
        .expect("message");
    app.press_key("tab").expect("leave text input");
    app.press_key("ctrl+shift+e").expect("switch environment");
    assert!(
        !app.screen().shows("Commit changes"),
        "the old scope's dialog is disposed"
    );
    open_commit(&mut app);
    assert_eq!(
        app.screen().input_text("id:git-commit-message"),
        Ok(Some(""))
    );
    app.press("role:button[Commit]").expect("commit");
    app.wait_for("ran:Commit;feature:false;files:all;message:auto;env:second")
        .expect("destination environment follows scope");
}

#[test]
fn unsupported_publish_stays_on_the_form_and_does_not_claim_success() {
    let mut app = start(DESKTOP);
    app.press("role:button[Refuse mutations]")
        .expect("unsupported mutation");
    app.press("role:button[Publish repository]")
        .expect("publish");
    app.press("role:button[Next]").expect("next");
    app.type_text("id:publish-repository-path", "alice/project")
        .expect("path");
    app.press("role:button[Publish]").expect("submit");
    app.wait_for("Publish failed").expect("error");
    assert!(!app.screen().shows("Repository published"));
    assert_eq!(
        app.screen().input_text("id:publish-repository-path"),
        Ok(Some("alice/project"))
    );
}
