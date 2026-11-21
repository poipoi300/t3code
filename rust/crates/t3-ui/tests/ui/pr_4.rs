//! Lifecycle writes and a server-generated guide through the paired production route.
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
    testing::{CallKind, environment::FakeEnvironment},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    state::nav::Route,
};

#[derive(Clone, Copy, PartialEq)]
enum Case {
    Actions,
    Delayed,
    ReadOnly,
    Failure,
    Stack,
    Guide,
    Rewrite,
}
#[component]
fn Actions() -> impl IntoElement {
    fixture(Case::Actions)
}
#[component]
fn Delayed() -> impl IntoElement {
    fixture(Case::Delayed)
}
#[component]
fn ReadOnly() -> impl IntoElement {
    fixture(Case::ReadOnly)
}
#[component]
fn Failed() -> impl IntoElement {
    fixture(Case::Failure)
}
#[component]
fn Stack() -> impl IntoElement {
    fixture(Case::Stack)
}
#[component]
fn Guide() -> impl IntoElement {
    fixture(Case::Guide)
}
#[component]
fn Rewrite() -> impl IntoElement {
    fixture(Case::Rewrite)
}

fn fixture(case: Case) -> impl IntoElement {
    let owner = Owner::current();
    let run: Spawner = Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    });
    let server = FakeEnvironment::new("forge", run.clone());
    server.set_permissions(if case == Case::ReadOnly {
        &["orchestration:read"]
    } else {
        &["orchestration:read", "source-control:write"]
    });
    server.edit_config(move |c| {
        c["settings"]["reviewGuidesEnabled"] = json!(matches!(case, Case::Guide | Case::Rewrite))
    });
    for tag in [
        "pullRequests.runAction",
        "pullRequests.stack",
        "pullRequests.reviewGuide",
        "assets.createUrl",
    ] {
        server.register(tag, CallKind::Unary);
    }
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(14, "Open change", "open", 0)])),
    );
    if case == Case::Delayed {
        server.reply(
            "pullRequests.list",
            Ok(answer(vec![entry(14, "Changed on host", "closed", 0)])),
        );
        let response = stored(Some(server.defer_reply("pullRequests.runAction")));
        on_key("f8", move |_| {
            response.with(|reply| {
                assert!(
                    reply
                        .take()
                        .expect("one action")
                        .send(Ok(Value::Null))
                        .is_ok()
                );
            })
        });
    }
    for (phase, draft) in [
        ("open", false),
        ("open", true),
        ("open", false),
        ("closed", false),
        ("open", false),
        ("merged", false),
    ] {
        let mut d = detail();
        d["state"] = json!(phase);
        d["isDraft"] = json!(draft);
        let actions = json!(["merge", "ready", "draft", "close", "reopen"]);
        d["capabilities"]["actions"] = actions.clone();
        d["viewerPermissions"]["actions"] = actions;
        d["capabilities"]["mergeMethods"] = json!(["merge", "squash", "rebase"]);
        d["mergeCapabilities"] = json!({"merge":true,"squash":true,"rebase":false});
        if case == Case::Stack {
            d["capabilities"]["stacks"] = json!(true);
            d["capabilities"]["stackActions"] = json!(true);
        }
        server.reply("pullRequests.detail", Ok(d));
        server.reply("pullRequests.linkedThreads", Ok(json!({"threads":[]})));
        let action_result = if case == Case::Failure {
            Err(
                json!({"_tag":"PullRequestOperationError","operation":"runAction","detail":"command failed"}),
            )
        } else {
            Ok(Value::Null)
        };
        server.reply("pullRequests.runAction", action_result);
    }
    if case == Case::Stack {
        server.reply(
            "pullRequests.stack",
            Err(json!({"_tag":"PullRequestOperationError","operation":"stack","detail":"offline"})),
        );
        server.reply("pullRequests.stack", Ok(Value::Null));
    }
    if matches!(case, Case::Guide | Case::Rewrite) {
        let patch = "diff --git a/src/change.rs b/src/change.rs\n--- a/src/change.rs\n+++ b/src/change.rs\n@@ -1,3 +1,3 @@\n outside before\n-previous boundary\n+checked boundary\n outside after\n";
        server
            .platform()
            .fake
            .push_http(Ok(t3_client::platform::HttpResponse {
                status: 200,
                headers: Default::default(),
                body: serde_json::to_vec(
                    &json!({"patch":patch,"truncated":false,"nextCursor":null}),
                )
                .expect("diff"),
            }));
        let stop = |title: &str, summary: &str, excerpts: Value| {
            json!({
                "title": title, "summary": summary, "attention": null, "excerpts": excerpts,
                "files": [], "notes": [], "questions": []
            })
        };
        let excerpts = json!([{"path":"src/change.rs","side":"new","start":2,"end":2,"caption":"Why this line changed"}]);
        let guide = json!({
            "headSha":"old","generatedAt":"2026-01-01T00:00:00.000Z",
            "modelSelection":{"instanceId":"codex","model":"fixture"},
            "stops":[stop("Read the boundary","[See ![private figure](https://media.invalid/guide.png)](https://example.org)",excerpts),
                     stop("Review lifecycle","Verify the final state.", json!([]))]
        });
        let running =
            json!({"_tag":"Running","headSha":"new","startedAt":"2026-01-01T00:00:00.000Z"});
        let previous = if case == Case::Rewrite {
            guide.clone()
        } else {
            Value::Null
        };
        server.reply(
            "pullRequests.reviewGuide",
            Ok(json!({"currentHeadSha":"new","run":running,"guide":previous})),
        );
        let terminal = if case == Case::Rewrite {
            json!({"_tag":"Failed","headSha":"new","failedAt":"2026-01-02T00:00:00.000Z","detail":"stopped"})
        } else {
            Value::Null
        };
        server.reply(
            "pullRequests.reviewGuide",
            Ok(json!({"currentHeadSha":"new","run":terminal,"guide":guide})),
        );
        server.reply(
            "assets.createUrl",
            Err(json!({"_tag":"Failure","message":"refused"})),
        );
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        crate::pr_3::ReviewPlatform(server.platform()),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        run.clone(),
    );
    let pair = client.pair(server.pairing());
    run(async move {
        pair.await.expect("pair");
    }
    .boxed_local());
    let verified = signal(false);
    on_key("f9", move |_| {
        let calls = server.calls("pullRequests.runAction");
        if case == Case::Actions {
            let expected = ["draft", "ready", "close", "reopen", "merge"].map(|action| {
                let mut request = json!({
                    "projectId":"project","repository":"team/widget","number":14,
                    "host":"github.com","action":action,"replyWithSummary":true
                });
                if action == "merge" {
                    request["mergeMethod"] = json!("squash");
                }
                request
            });
            assert_eq!(calls, expected);
        } else if case == Case::Guide {
            assert_eq!(server.calls("pullRequests.reviewGuide").len(), 2);
            assert_eq!(
                server.calls("assets.createUrl"),
                vec![
                    json!({"resource":{"_tag":"github-media","cwd":"/project","repository":"team/widget","url":"https://media.invalid/guide.png"}})
                ]
            );
        } else {
            assert!(calls.is_empty())
        }
        verified.set(true)
    });
    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::PullRequests,
            move || t3_ui::app::provide_client_services(&client),
        )))
        .child(text(move || {
            if verified.get() {
                "Requests verified"
            } else {
                ""
            }
        }))
}
fn open(w: (f32, f32), root: Root) -> Headless {
    let mut app = app_at(w, root);
    app.wait_for("Open change").expect("list");
    app.press("role:button[Open pull request #14]")
        .expect("open PR");
    app.wait_for("Selected detail from host").expect("detail");
    app
}
fn more(a: &mut Headless) {
    a.press("role:button[More pull request actions]")
        .expect("actions menu");
}
fn verify(a: &mut Headless) {
    a.press_key("f9").expect("check wire");
    a.wait_for("Requests verified").expect("verified RPCs");
}
fn lifecycle(w: (f32, f32)) {
    let mut a = open(w, Root::of::<Actions>());
    more(&mut a);
    a.press("Convert to draft").expect("draft");
    a.wait_for("role:button[Ready for review]")
        .expect("refresh");
    a.press("role:button[Ready for review]").expect("ready");
    a.wait_for("role:button[Merge]").expect("open state");
    more(&mut a);
    a.press("Close pull request").expect("arm close");
    a.press("Confirm close?").expect("confirm close");
    a.wait_for("role:button[More pull request actions]")
        .expect("refreshed detail");
    more(&mut a);
    a.press("Reopen pull request").expect("reopen");
    a.wait_for("role:button[Merge]").expect("reopened");
    more(&mut a);
    assert!(!a.screen().shows("Rebase and merge"));
    a.press("Squash and merge").expect("method");
    a.press("role:button[Squash and merge]").expect("arm merge");
    a.wait(std::time::Duration::from_secs(3))
        .expect("confirmation expires");
    a.press("role:button[Squash and merge]")
        .expect("rearm after timeout");
    a.press("role:button[Timeline]")
        .expect("leave confirm control");
    assert!(
        a.screen().shows("Squash and merge"),
        "outside press disarms"
    );
    a.press("role:button[Squash and merge]")
        .expect("arm before Escape");
    a.press_key("Escape").expect("disarm");
    a.press("role:button[Squash and merge]").expect("rearm");
    a.press("role:button[Confirm merge?]").expect("merge");
    a.wait_for("Merged").expect("authoritative state");
    verify(&mut a);
}
fn gates(w: (f32, f32)) {
    let mut a = open(w, Root::of::<ReadOnly>());
    let _ = a.press("role:button[Merge]");
    assert!(!a.screen().shows("Confirm merge?"));
    assert!(!a.screen().shows("Guide"));
    verify(&mut a);
    let mut a = open(w, Root::of::<Stack>());
    a.wait_for("Retry stack lookup").expect("failed membership");
    assert!(a.screen().shows("Couldn't check stack membership"));
    let _ = a.press("role:button[Merge]");
    assert!(!a.screen().shows("Confirm merge?"));
    verify(&mut a);
    a.press("Retry stack lookup").expect("retry");
    a.press("role:button[Merge]").expect("fresh membership");
    a.wait_for("Confirm merge?").expect("can arm");
    let mut a = open(w, Root::of::<Failed>());
    more(&mut a);
    a.press("Convert to draft").expect("refused write");
    a.wait_for("Could not convert this to a draft")
        .expect("failure title");
    a.wait_for("The host refused it. Check that you have write access to this repository.")
        .expect("failure hint");
}
fn leave_during_write(w: (f32, f32)) {
    let mut a = open(w, Root::of::<Delayed>());
    more(&mut a);
    a.press("Close pull request").expect("arm");
    a.press("Confirm close?").expect("dispatch");
    a.press("Back to pull requests").expect("leave detail");
    a.press_key("f8").expect("finish on host");
    a.wait_for("Changed on host")
        .expect("list reflects completed write");
}
#[test]
fn desktop_background_write() {
    leave_during_write(DESKTOP);
}
#[test]
fn phone_background_write() {
    leave_during_write(PHONE);
}
fn preserves_tour(w: (f32, f32)) {
    let mut a = open(w, Root::of::<Rewrite>());
    a.press("role:button[Guide]").expect("open running rewrite");
    a.wait_for("Read the boundary")
        .expect("original guide available");
    a.press("role:button[Next stop]").expect("read second stop");
    a.wait(std::time::Duration::from_secs(6)).expect("poll");
    a.wait_for("Could not rewrite this guide.")
        .expect("terminal status");
    assert!(a.screen().shows("Stop 2 of 2"));
}
#[test]
fn desktop_rewrite() {
    preserves_tour(DESKTOP);
}
#[test]
fn phone_rewrite() {
    preserves_tour(PHONE);
}

fn tour(w: (f32, f32)) {
    let mut a = open(w, Root::of::<Guide>());
    a.press("role:button[Guide]").expect("open guide");
    a.wait_for("Writing a review guide").expect("running");
    a.wait(std::time::Duration::from_secs(6)).expect("poll");
    a.wait_for("Written before the latest push.")
        .expect("stale version");
    a.wait_for("Read the boundary").expect("first stop");
    a.wait_for("text:-previous boundary\n+checked boundary")
        .expect("paired changed lines from diff");
    assert!(
        a.screen()
            .texts()
            .iter()
            .all(|text| !text.contains("outside before"))
    );
    a.wait_for("text:private figure").expect("denied asset alt");
    a.press("role:button[Next stop]").expect("next");
    a.wait_for("Verify the final state.").expect("second stop");
    verify(&mut a);
}
#[test]
fn desktop_lifecycle() {
    lifecycle(DESKTOP)
}
#[test]
fn phone_lifecycle() {
    lifecycle(PHONE)
}
#[test]
fn desktop_gates() {
    gates(DESKTOP)
}
#[test]
fn phone_gates() {
    gates(PHONE)
}
#[test]
fn desktop_guide() {
    tour(DESKTOP)
}
#[test]
fn phone_guide() {
    tour(PHONE)
}
