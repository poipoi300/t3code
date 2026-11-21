//! File review and feedback through the production PR surface, HTTP and websocket boundaries.
use crate::{
    pr_1::{answer, entry},
    pr_2::{app_at, detail},
    support::{DESKTOP, PHONE},
};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::Root;
use nsh_testing::Headless;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{
        HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, Platform, PlatformError,
        PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey, StorageTransaction,
        StorageWrite, WebSocket,
    },
    testing::environment::FakeEnvironment,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    state::nav::Route,
};

// The shared environment fixture serves pairing and RPCs. Route only the diff HTTP
// request to its scripted transport so both transports retain their wire assertions.
#[derive(Clone)]
struct ReviewPlatform(t3_client::testing::environment::EnvironmentPlatform);
impl Storage for ReviewPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.0.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.0.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.0.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.0.transaction(transaction)
    }
}
impl Platform for ReviewPlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.0.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.monotonic_ms()
    }
    fn random_bytes(&self, len: usize) -> Result<Vec<u8>, PlatformError> {
        self.0.random_bytes(len)
    }
    fn sleep(&self, ms: u64) -> futures::future::LocalBoxFuture<'_, ()> {
        self.0.sleep(ms)
    }
    fn yield_to_host(&self) -> futures::future::LocalBoxFuture<'_, ()> {
        self.0.yield_to_host()
    }
    fn events(&self) -> futures::stream::LocalBoxStream<'static, PlatformEvent> {
        self.0.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        if request.method == "POST" && request.url.ends_with("/api/pull-requests/diff") {
            self.0.fake.http(request)
        } else {
            self.0.http(request)
        }
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.0.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.0.host(action)
    }
}

fn message(body: &str, id: &str) -> Value {
    json!({"id":id,"kind":"issue-comment","body":body,"author":null,"createdAt":"2026-01-02T00:00:00Z","url":null,"path":null,"reviewState":null})
}
fn activity(posted: bool, replied: bool) -> Value {
    let mut notes = vec![
        message("Review the boundary", "n1"),
        message("Please explain this line", "c1"),
    ];
    if posted {
        notes.push(message("My feedback", "n2"));
    }
    let mut thread = vec![message("Please explain this line", "c1")];
    if replied {
        thread.push(message("Covered by the test", "c2"));
    }
    json!({"comments":notes,"commentCount":notes.len(),"commentsTruncated":false,"commits":[],
        "overviewThreads":[{"id":"general-thread","path":null,"line":null,"side":"right","isResolved":false,"isOutdated":false,"comments":[message("Overall design discussion","g1")]}],
        "reviewThreads":[{"id":"thread-1","path":"src/file.rs","side":"right","line":1,"isResolved":false,"isOutdated":false,"commentCount":thread.len()+1,"nextCommentsCursor":"older","comments":thread}]})
}
#[component]
fn ReviewFixture() -> impl IntoElement {
    fixture(true)
}
#[component]
fn RestrictedFixture() -> impl IntoElement {
    fixture(false)
}
fn fixture(read_scope: bool) -> impl IntoElement {
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let server = FakeEnvironment::new("review", run.clone());
    for method in [
        "pullRequests.activity",
        "pullRequests.threadComments",
        "pullRequests.comment",
        "pullRequests.replyToThread",
        "pullRequests.filesViewed",
        "pullRequests.setFilesViewed",
    ] {
        server.register(method, t3_client::testing::CallKind::Unary);
    }
    server.set_permissions(if read_scope {
        &["orchestration:read", "source-control:write"]
    } else {
        &["source-control:write"]
    });
    let platform = server.platform();
    let fake = platform.fake.clone();
    for (path, word, next) in [
        ("src/file.rs", "first_added", Some("page-2")),
        ("src/next.rs", "next_added", None),
    ] {
        let patch = format!(
            "diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+{word}\n"
        );
        fake.push_http(Ok(HttpResponse {
            status: 200,
            headers: Default::default(),
            body: serde_json::to_vec(
                &json!({"patch":patch,"truncated":next.is_some(),"nextCursor":next}),
            )
            .expect("json"),
        }));
    }
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(14, "Review this change", "open", 1)])),
    );
    let mut d = detail();
    d["capabilities"]["viewedFiles"] = json!("host");
    d["capabilities"]["comment"] = json!(true);
    d["capabilities"]["review"]["reply"] = json!(true);
    d["viewerPermissions"]["comment"] = json!(true);
    server.reply("pullRequests.detail", Ok(d));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads":[]})));
    server.reply(
        "pullRequests.filesViewed",
        Ok(json!({"files":[],"truncated":false})),
    );
    server.reply("pullRequests.setFilesViewed", Ok(Value::Null));
    server.reply(
        "pullRequests.filesViewed",
        Ok(json!({"files":[{"path":"src/file.rs","state":"viewed"}],"truncated":false})),
    );
    for (posted, replied) in [(false, false), (true, false), (true, true)] {
        server.reply("pullRequests.activity", Ok(activity(posted, replied)));
    }
    server.reply("pullRequests.threadComments",Ok(json!({"comments":[message("More context from the reviewer","older1")],"nextCursor":null})));
    server.reply("pullRequests.comment", Ok(Value::Null));
    server.reply("pullRequests.replyToThread", Ok(Value::Null));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        ReviewPlatform(platform),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    let pair = client.pair(server.pairing());
    run(async move {
        pair.await.expect("pair");
    }
    .boxed_local());
    let wire = signal(String::new());
    on_key("f9", move |_| {
        let reference = json!({"projectId":"project","repository":"team/widget","number":14,"host":"github.com"});
        if !read_scope {
            assert!(fake.http_requests().is_empty());
            wire.set("No diff request".into());
            return;
        }
        if !server.calls("pullRequests.setFilesViewed").is_empty() {
            let reads = fake.http_requests();
            let diffs: Vec<_> = reads
                .iter()
                .filter(|r| r.url.ends_with("/api/pull-requests/diff"))
                .collect();
            assert_eq!(diffs.len(), 2);
            for (i, req) in diffs.iter().enumerate() {
                assert_eq!(req.method, "POST");
                assert!(
                    req.headers
                        .keys()
                        .any(|key| key.eq_ignore_ascii_case("authorization")),
                    "paired HTTP authorization"
                );
                let mut expected = reference.clone();
                if i == 1 {
                    expected["cursor"] = json!("page-2");
                }
                assert_eq!(
                    serde_json::from_slice::<Value>(&req.body).expect("diff scope"),
                    expected
                );
            }
            let mut expected = reference;
            expected["files"] = json!([{"path":"src/file.rs","viewed":true,"revision":"2222222"}]);
            assert_eq!(
                json!(server.calls("pullRequests.setFilesViewed")),
                json!([expected])
            );
            wire.set("Verified files".into());
        } else {
            let mut earlier = reference.clone();
            earlier["threadId"] = json!("thread-1");
            earlier["cursor"] = json!("older");
            assert_eq!(
                json!(server.calls("pullRequests.threadComments")),
                json!([earlier])
            );
            let mut comment = reference.clone();
            comment["body"] = json!("My feedback");
            assert_eq!(
                json!(server.calls("pullRequests.comment")),
                json!([comment])
            );
            let mut reply = reference;
            reply["body"] = json!("Covered by the test");
            reply["threadId"] = json!("thread-1");
            assert_eq!(
                json!(server.calls("pullRequests.replyToThread")),
                json!([reply])
            );
            wire.set("Verified feedback".into());
        }
    });
    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::PullRequests,
            move || t3_ui::app::provide_client_services(&client),
        )))
        .child(text(move || wire.get()))
}
fn open(window: (f32, f32)) -> Headless {
    let mut app = app_at(window, Root::of::<ReviewFixture>());
    app.wait_for("Review this change").expect("list");
    app.press("role:button[Open pull request #14]")
        .expect("select");
    app.wait_for("Selected detail from host").expect("detail");
    app
}
fn files(w: (f32, f32)) {
    let mut a = open(w);
    a.press("role:button[Code]").expect("open Code");
    a.wait_for("src/file.rs").expect("file header");
    a.wait_for("first_added").expect("patch line");
    assert!(
        a.screen().shows("Selected detail from host"),
        "PR header on Code"
    );
    a.press("role:button[Mark src/file.rs as viewed]")
        .expect("mark viewed");
    a.wait_for("role:button[Mark src/file.rs as unviewed]")
        .expect("recorded viewed state");
    a.press("role:button[Load more files]").expect("next slice");
    a.wait_for("next_added").expect("second patch");
    a.wait_for("src/file.rs").expect("first file retained");
    a.wait_for("Diff truncated")
        .expect("earlier page omitted content");
    a.press_key("f9").expect("requests");
    a.wait_for("Verified files").expect("wire");
}
fn feedback(w: (f32, f32)) {
    let mut a = open(w);
    a.press("role:button[Timeline]").expect("open Timeline");
    a.wait_for("Review the boundary").expect("conversation");
    a.wait_for("Please explain this line")
        .expect("review thread");
    a.wait_for("Overall design discussion")
        .expect("overview thread");
    assert_eq!(
        a.screen()
            .count("text:Please explain this line")
            .expect("comments"),
        1
    );
    a.press("role:button[Load more replies]")
        .expect("expand thread");
    a.wait_for("More context from the reviewer")
        .expect("thread readback");
    a.type_text("label:Leave a comment", "My feedback")
        .expect("comment");
    a.press("role:button[Comment]").expect("submit");
    a.wait_for("My feedback").expect("server readback");
    a.press("id:pr-thread-thread-1 >> role:button[Reply]")
        .expect("thread reply");
    a.type_text("label:Write a reply", "Covered by the test")
        .expect("reply");
    a.press("role:button[Send reply]").expect("submit reply");
    a.wait_for("Covered by the test").expect("reply readback");
    a.press_key("f9").expect("requests");
    a.wait_for("Verified feedback").expect("wire");
}
#[test]
fn desktop_reviews_changed_files() {
    files(DESKTOP)
}
#[test]
fn phone_reviews_changed_files() {
    files(PHONE)
}
#[test]
fn desktop_posts_review_feedback() {
    feedback(DESKTOP)
}
#[test]
fn phone_posts_review_feedback() {
    feedback(PHONE)
}

fn denied(w: (f32, f32)) {
    let mut a = app_at(w, Root::of::<RestrictedFixture>());
    a.wait_for("Review this change").expect("list");
    a.press("role:button[Open pull request #14]")
        .expect("detail");
    a.press("role:button[Code]").expect("open Code");
    a.wait_for("Pull request diff requires orchestration:read")
        .expect("missing scope");
    a.press_key("f9").expect("wire");
    a.wait_for("No diff request").expect("no HTTP dispatch");
}
#[test]
fn phone_scope_denial_prevents_diff() {
    denied(PHONE);
}
#[test]
fn desktop_scope_denial_prevents_diff() {
    denied(DESKTOP);
}
