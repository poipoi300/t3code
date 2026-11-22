//! Pull requests Slice 5: stacked PRs, advanced pickers, and merge-method persistence.

use crate::{
    pr_1::{StaticUrl, answer, entry},
    pr_2::{app_at, detail},
    support::{DESKTOP, PHONE},
};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::Root;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, time::Duration};
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    testing::{CallKind, environment::FakeEnvironment},
    view_logic::shell::ui_state::{PREFERENCES_NAMESPACE, STORAGE_KEY, UiPreferences, UiState},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    state::nav::Route,
};

fn parse_ms(s: &str) -> Option<i64> {
    s.parse().ok()
}

fn spawner_for_owner(owner: Owner) -> Spawner {
    Rc::new(move |f| {
        owner.run(|| {
            spawn(f);
        })
    })
}

fn seed_stack_detail(number: u64, title: &str, head: &str, base: &str, is_draft: bool) -> Value {
    let mut d = detail();
    d["number"] = json!(number);
    d["title"] = json!(title);
    d["url"] = json!(format!("https://github.com/team/widget/pull/{number}"));
    d["headBranch"] = json!(head);
    d["baseBranch"] = json!(base);
    d["isDraft"] = json!(is_draft);
    d["headSha"] = json!(format!("sha{number}"));
    d["capabilities"]["actions"] = json!([
        "merge",
        "ready",
        "draft",
        "close",
        "reopen",
        "update-branch"
    ]);
    d["viewerPermissions"]["actions"] = json!([
        "merge",
        "ready",
        "draft",
        "close",
        "reopen",
        "update-branch"
    ]);
    d["capabilities"]["mergeMethods"] = json!(["merge", "squash", "rebase"]);
    d["mergeCapabilities"] = json!({"merge": true, "squash": true, "rebase": false});
    d["capabilities"]["stacks"] = json!(true);
    d["capabilities"]["stackActions"] = json!(true);
    d["viewerPermissions"]["stackRebase"] = json!(true);
    d["capabilities"]["updateMethods"] = json!(["merge", "rebase"]);
    d["viewerPermissions"]["updateMethods"] = json!(["merge", "rebase"]);
    d["baseComparison"] = json!("behind");
    d["behindBy"] = json!(3);
    d
}

fn stack_payload() -> Value {
    json!({
        "id": "stack-42",
        "number": 42,
        "url": "https://github.com/team/widget/pulls",
        "base": "main",
        "layers": [
            {
                "number": 13,
                "title": "Base layer",
                "url": "https://github.com/team/widget/pull/13",
                "headBranch": "feat-1",
                "headSha": "sha13",
                "state": "merged",
                "isDraft": false
            },
            {
                "number": 14,
                "title": "Middle layer",
                "url": "https://github.com/team/widget/pull/14",
                "headBranch": "feat-2",
                "headSha": "sha14",
                "state": "open",
                "isDraft": false
            },
            {
                "number": 15,
                "title": "Top layer",
                "url": "https://github.com/team/widget/pull/15",
                "headBranch": "feat-3",
                "headSha": "sha15",
                "state": "open",
                "isDraft": true
            }
        ]
    })
}

#[component]
fn StackFixture() -> impl IntoElement {
    let spawner = spawner_for_owner(Owner::current());
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "source-control:write"]);
    for tag in [
        "pullRequests.stack",
        "pullRequests.runAction",
        "vcs.listRefs",
    ] {
        server.register(tag, CallKind::Unary);
    }

    let mut row14 = entry(14, "Middle layer", "open", 10);
    row14["stack"] = json!({"number": 42, "position": 2, "size": 3, "base": "main"});
    let mut row15 = entry(15, "Top layer", "open", 5);
    row15["isDraft"] = json!(true);
    row15["stack"] = json!({"number": 42, "position": 3, "size": 3, "base": "main"});
    for _ in 0..6 {
        server.reply(
            "pullRequests.list",
            Ok(answer(vec![row14.clone(), row15.clone()])),
        );
    }

    // Replies covering list-popover selection (#13), opening #14, post-actions refreshes, and nav to #15.
    server.reply("pullRequests.stack", Ok(stack_payload()));
    server.reply(
        "pullRequests.detail",
        Ok(seed_stack_detail(13, "Base layer", "feat-1", "main", false)),
    );
    for _ in 0..10 {
        server.reply(
            "pullRequests.detail",
            Ok(seed_stack_detail(
                14,
                "Middle layer",
                "feat-2",
                "feat-1",
                false,
            )),
        );
        server.reply("pullRequests.stack", Ok(stack_payload()));
        server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
        server.reply(
            "vcs.listRefs",
            Ok(json!({
                "refs": [{
                    "name": "origin/main",
                    "current": false,
                    "isDefault": true,
                    "isRemote": true,
                    "remoteName": "origin",
                    "worktreePath": null
                }],
                "isRepo": true,
                "hasPrimaryRemote": true,
                "nextCursor": null,
                "totalCount": 1
            })),
        );
    }
    server.reply(
        "pullRequests.detail",
        Ok(seed_stack_detail(15, "Top layer", "feat-3", "feat-2", true)),
    );
    server.reply("pullRequests.stack", Ok(stack_payload()));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    server.reply(
        "vcs.listRefs",
        Ok(json!({
            "refs": [{
                "name": "origin/main",
                "current": false,
                "isDefault": true,
                "isRemote": true,
                "remoteName": "origin",
                "worktreePath": null
            }],
            "isRepo": true,
            "hasPrimaryRemote": true,
            "nextCursor": null,
            "totalCount": 1
        })),
    );
    for _ in 0..3 {
        server.reply("pullRequests.runAction", Ok(Value::Null));
    }

    let checked = signal(false);
    let lazy_ok = signal(false);
    let list_stack_before_open = Rc::new(RefCell::new(None::<usize>));
    let before_capture = list_stack_before_open.clone();
    let capture_server = server.clone();
    on_key("f8", move |_| {
        *before_capture.borrow_mut() = Some(capture_server.calls("pullRequests.stack").len());
        lazy_ok.set(true);
    });
    let verify_server = server.clone();
    on_key("f9", move |_| {
        let actions = verify_server.calls("pullRequests.runAction");
        assert_eq!(
            actions.len(),
            3,
            "expected update-branch, merge stack, rebase stack"
        );
        assert_eq!(actions[0]["action"], json!("update-branch"));
        assert_eq!(actions[0]["updateMethod"], json!("rebase"));
        assert_eq!(actions[1]["action"], json!("merge"));
        assert_eq!(actions[1]["number"], json!(14));
        assert_eq!(actions[1]["stackNumber"], json!(42));
        assert_eq!(
            actions[1]["expectedStackHeads"],
            json!([{"number": 14, "headSha": "sha14"}])
        );
        assert_eq!(actions[1]["mergeMethod"], json!("merge"));
        assert_eq!(actions[2]["action"], json!("update-branch"));
        assert_eq!(actions[2]["number"], json!(15));
        assert_eq!(actions[2]["stackNumber"], json!(42));
        assert_eq!(
            actions[2]["expectedStackHeads"],
            json!([
                {"number": 14, "headSha": "sha14"},
                {"number": 15, "headSha": "sha15"}
            ])
        );
        assert_eq!(actions[2]["updateMethod"], json!("rebase"));
        checked.set(true);
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(server.pairing());
    spawner(
        async move {
            pair.await.expect("pair");
        }
        .boxed_local(),
    );

    view()
        .w_full()
        .h_full()
        .child(view().flex_1().min_h(px(0.)).child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::PullRequests,
            move || t3_ui::app::provide_client_services(&client),
        )))
        .child(text(move || {
            if lazy_ok.get() && *list_stack_before_open.borrow() == Some(0) {
                "Lazy stack verified"
            } else {
                ""
            }
        }))
        .child(text(move || {
            if checked.get() {
                "Stack RPCs verified"
            } else {
                ""
            }
        }))
}

fn stacked_prs_flow(window: (f32, f32)) {
    let mut app = app_at(window, Root::of::<StackFixture>());
    app.wait_for("Middle layer").expect("list loaded");
    app.press_key("f8")
        .expect("record stack calls before popover");
    app.wait_for("Lazy stack verified")
        .expect("no eager per-row stack calls");

    app.press("role:button[Stack 42, layer 2 of 3]")
        .expect("open row stack popover");
    app.wait_for("Stack #42").expect("stack header");
    app.wait_for("Base layer").expect("layer 1 in stack");
    app.wait_for("↳ main").expect("stack base");

    // Navigate from row popover into layer #13.
    app.press("Base layer").expect("navigate to layer 13");
    app.wait_for("#13").expect("switched detail to #13");

    // Back to list if phone, then open #14 for detail stack menu & base freshness tests.
    if app.screen().shows("Back to pull requests") {
        let _ = app.press("Back to pull requests");
        app.wait_for_gone("Back to pull requests", Duration::from_secs(2))
            .expect("detail sheet closed on phone");
        app.wait_for("role:button[Open pull request #14]")
            .expect("list visible");
    }
    app.press("role:button[Open pull request #14]")
        .expect("open #14");
    app.wait_for("role:button[Merge stack]")
        .expect("stack merge control");

    // Base branch out-of-date warning + rebase update.
    app.press("role:button[This branch is out-of-date with feat-1 by 3 commits.]")
        .expect("open base freshness popover");
    app.wait_for("Changes can be cleanly merged.")
        .expect("freshness description");
    app.press("role:button[Update with rebase]")
        .expect("trigger rebase update");
    app.wait_for("role:button[Merge stack]")
        .expect("detail refreshed");

    // Stack merge confirmation (cancel once, then confirm).
    app.press("role:button[Merge stack]")
        .expect("open merge stack confirmation");
    app.wait_for("Merge 1 pull request?")
        .expect("merge confirmation dialog");
    app.press("role:button[Cancel]")
        .expect("cancel merge stack");
    app.press("role:button[Merge stack]")
        .expect("reopen merge stack confirmation");
    app.wait_for("Merge 1 pull request?")
        .expect("reopened dialog");
    // Dialog's primary button is inside `pr-stack-confirm-dialog`.
    app.press("id:pr-stack-confirm-dialog >> role:button[Merge stack]")
        .expect("confirm merge stack");
    app.wait_for("Stack merge request completed")
        .expect("merge stack toast");

    // Stack menu -> Rebase stack confirmation -> then navigate to #15.
    app.press("id:pr-detail-stack >> role:button[Stack 42, layer 2 of 3]")
        .expect("open detail stack menu");
    app.wait_for("Merge stack (1)").expect("menu merge item");
    app.press("Rebase stack").expect("open rebase confirmation");
    app.wait_for("Rebase 2 pull requests?")
        .expect("rebase confirmation dialog");
    app.press("id:pr-stack-confirm-dialog >> role:button[Rebase stack]")
        .expect("confirm rebase stack");
    app.wait_for("Stack rebased").expect("rebase stack toast");

    app.press("id:pr-detail-stack >> role:button[Stack 42, layer 2 of 3]")
        .expect("open stack menu for navigation");
    app.press("Top layer").expect("navigate to top layer #15");
    app.wait_for("#15").expect("detail navigated to #15");

    app.press_key("f9").expect("verify stack RPC payloads");
    app.wait_for("Stack RPCs verified").expect("verified");
}

#[test]
fn desktop_stacked_prs() {
    stacked_prs_flow(DESKTOP);
}
#[test]
fn phone_stacked_prs() {
    stacked_prs_flow(PHONE);
}

#[component]
fn PickerFixture() -> impl IntoElement {
    let spawner = spawner_for_owner(Owner::current());
    let env_a = FakeEnvironment::new("forge-a", spawner.clone());
    let env_b = FakeEnvironment::at_origin("forge-b", "https://b.test", spawner.clone());
    for env in [&env_a, &env_b] {
        env.set_permissions(&["orchestration:read", "source-control:write"]);
        for tag in [
            "pullRequests.activity",
            "pullRequests.filterOptions",
            "pullRequests.reviewerCandidates",
            "pullRequests.requestReviewers",
            "pullRequests.labelCandidates",
            "pullRequests.setLabels",
        ] {
            env.register(tag, CallKind::Unary);
        }
    }

    let mut shell_a = t3_client::testing::environment::shell(&[]);
    shell_a["projects"][0]["id"] = json!("proj-a");
    shell_a["projects"][0]["title"] = json!("Project A");
    env_a.set_shell(shell_a);

    let mut shell_b = t3_client::testing::environment::shell(&[]);
    shell_b["projects"][0]["id"] = json!("proj-b");
    shell_b["projects"][0]["title"] = json!("Project B");
    env_b.set_shell(shell_b);

    let mut row_a = entry(14, "Picker PR", "open", 5);
    row_a["projectId"] = json!("proj-a");
    row_a["projectTitle"] = json!("Project A");
    let mut answer_a = answer(vec![row_a]);
    answer_a["providers"] = json!([
        {"host": "github.com", "kind": "github", "searchesOnHost": true, "projectCount": 1, "configured": true, "detail": null},
        {"host": "gitlab.example", "kind": "gitlab", "searchesOnHost": true, "projectCount": 1, "configured": true, "detail": null}
    ]);
    for _ in 0..12 {
        env_a.reply("pullRequests.list", Ok(answer_a.clone()));
        env_b.reply("pullRequests.list", Ok(answer(vec![])));
    }
    for _ in 0..2 {
        env_a.reply(
            "pullRequests.filterOptions",
            Ok(json!({
                "labels":[{"name":"urgent","color":"ff0000","description":null}],
                "authors":[],
                "references":[],
                "truncated":false
            })),
        );
        env_b.reply(
            "pullRequests.filterOptions",
            Ok(json!({"labels":[],"authors":[],"references":[],"truncated":false})),
        );
    }
    for _ in 0..14 {
        env_a.reply(
            "pullRequests.filterOptions",
            Ok(json!({
                "labels":[{"name":"urgent","color":"ff0000","description":null}],
                "authors":[{"login":"octocat","name":"Octo Cat","avatarUrl":null}],
                "references":[],
                "truncated":false
            })),
        );
        env_b.reply(
            "pullRequests.filterOptions",
            Ok(json!({"labels":[],"authors":[],"references":[],"truncated":false})),
        );
    }

    let mut picker_detail = detail();
    picker_detail["projectId"] = json!("proj-a");
    picker_detail["capabilities"]["reviewers"] =
        json!({"request": true, "listCandidates": true, "searchCandidates": true});
    picker_detail["viewerPermissions"]["requestReviewers"] = json!(true);
    picker_detail["capabilities"]["labels"] = json!(true);
    picker_detail["viewerPermissions"]["labels"] = json!(true);
    picker_detail["reviewers"] = json!([{"login": "Alice", "name": "Alice", "avatarUrl": null}]);
    picker_detail["labels"] = json!([{"name": "docs", "color": "0075ca", "description": "Docs"}]);
    let activity_payload = json!({
        "author": null,
        "reviewers": [{"login": "Alice", "name": "Alice", "avatarUrl": null}],
        "comments": [{
            "id": "rev-1",
            "kind": "review",
            "url": "https://github.com/acme/web/pull/14#review-1",
            "path": null,
            "author": {"login": "Bob", "name": "Bob", "avatarUrl": null},
            "body": "Please fix",
            "reviewState": "CHANGES_REQUESTED",
            "createdAt": "2026-01-03T00:00:00Z"
        }],
        "commentCount": 1,
        "commentsTruncated": false,
        "reviewThreads": [],
        "commits": []
    });
    for _ in 0..8 {
        env_a.reply("pullRequests.activity", Ok(activity_payload.clone()));
    }
    env_a.reply("pullRequests.detail", Ok(picker_detail.clone()));
    env_a.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    let mut after_reviewers = picker_detail.clone();
    after_reviewers["reviewers"] = json!([
        {"login": "Alice", "name": "Alice", "avatarUrl": null},
        {"login": "carol", "name": "Carol", "avatarUrl": null}
    ]);
    env_a.reply("pullRequests.detail", Ok(after_reviewers.clone()));
    env_a.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    env_a.reply("pullRequests.detail", Ok(picker_detail.clone()));
    env_a.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    let mut after_labels = picker_detail.clone();
    after_labels["labels"] = json!([
        {"name": "docs", "color": "0075ca", "description": "Docs"},
        {"name": "bug", "color": "d73a4a", "description": "Broken"}
    ]);
    env_a.reply("pullRequests.detail", Ok(after_labels));
    env_a.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    env_a.reply("pullRequests.detail", Ok(picker_detail));
    env_a.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    env_a.reply(
        "pullRequests.reviewerCandidates",
        Ok(json!({
            "candidates": [{"id": "u-bob", "kind": "user", "login": "bob", "name": "Bob", "avatarUrl": null, "isRequested": false}],
            "truncated": true
        })),
    );
    env_a.reply(
        "pullRequests.reviewerCandidates",
        Ok(json!({
            "candidates": [{"id": "u-carol", "kind": "user", "login": "carol", "name": "Carol", "avatarUrl": null, "isRequested": false}],
            "truncated": false
        })),
    );
    env_a.reply("pullRequests.requestReviewers", Ok(Value::Null));
    env_a.reply("pullRequests.requestReviewers", Ok(Value::Null));
    env_a.reply(
        "pullRequests.labelCandidates",
        Ok(json!({
            "candidates": [
                {"name": "bug", "color": "d73a4a", "description": "Broken", "isApplied": false},
                {"name": "docs", "color": "0075ca", "description": "Docs", "isApplied": true}
            ],
            "truncated": false
        })),
    );
    env_a.reply("pullRequests.setLabels", Ok(Value::Null));
    env_a.reply("pullRequests.setLabels", Ok(Value::Null));

    let verified = signal(false);
    let check_a = env_a.clone();
    let check_b = env_b.clone();
    on_key("f9", move |_| {
        let candidate_calls = check_a.calls("pullRequests.reviewerCandidates");
        assert_eq!(
            candidate_calls.len(),
            2,
            "trailing whitespace or unchanged debounced query must not resend search: {candidate_calls:?}"
        );
        let reviewer_calls = check_a.calls("pullRequests.requestReviewers");
        assert_eq!(reviewer_calls.len(), 2);
        assert_eq!(
            reviewer_calls[0]["reviewers"],
            json!([{"id": "u-carol", "kind": "user"}])
        );
        assert_eq!(reviewer_calls[0]["requested"], json!(true));
        assert_eq!(reviewer_calls[1]["requested"], json!(false));
        let label_calls = check_a.calls("pullRequests.setLabels");
        assert_eq!(label_calls.len(), 2);
        assert_eq!(label_calls[0]["labels"], json!(["bug"]));
        assert_eq!(label_calls[0]["applied"], json!(true));
        assert_eq!(label_calls[1]["labels"], json!(["bug"]));
        assert_eq!(label_calls[1]["applied"], json!(false));

        let lists_a = check_a.calls("pullRequests.list");
        let lists_b = check_b.calls("pullRequests.list");
        let filter_calls = check_a.calls("pullRequests.filterOptions");
        assert!(
            filter_calls
                .iter()
                .any(|c| c["kind"] == json!("authors") && c["query"] == json!("octo"))
        );
        assert!(
            lists_a
                .iter()
                .any(|c| c["filters"]["author"] == json!("octocat")),
            "expected author=octocat in lists_a: {lists_a:?}"
        );
        assert!(
            lists_a
                .iter()
                .any(|c| c["filters"]["labels"] == json!([["urgent"]]))
        );
        assert!(lists_a.iter().any(|c| c["host"] == json!("gitlab.example")));
        assert!(lists_b.iter().any(|c| c["projectId"] == json!("proj-b")));
        verified.set(true);
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        env_a.platform().with_environment(&env_b),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let p1 = client.pair(env_a.pairing());
    let p2 = env_b.pairing();
    let paired = signal(false);
    let c2 = client.clone();
    spawner(
        async move {
            p1.await.expect("pair a");
            c2.pair(p2).await.expect("pair b");
            paired.set(true);
        }
        .boxed_local(),
    );

    view()
        .w_full()
        .h_full()
        .child(when(paired, move || {
            let client = client.clone();
            t3_ui::app::app_tree(
                Rt::new(store.clone(), Rc::new(client.clone()), Choice::Client),
                Route::PullRequests,
                move || {
                    t3_ui::state::url::UrlState::install(
                        t3_ui::state::url::Location::parse("/pull-requests"),
                        t3_ui::state::nav::navigator(),
                        Rc::new(StaticUrl),
                    );
                    t3_ui::app::provide_client_services(&client);
                },
            )
        }))
        .child(text(move || {
            if verified.get() {
                "Pickers verified"
            } else {
                ""
            }
        }))
}

fn pickers_flow(window: (f32, f32)) {
    let mut app = app_at(window, Root::of::<PickerFixture>());
    app.wait_for("Picker PR").expect("list loaded");

    // 1. Open PR detail & test Reviewer and Label pickers on Summary tab.
    app.press("role:button[Open pull request #14]")
        .expect("open PR");
    app.wait_for("Reviewers").expect("reviewers row");
    app.wait_for("Alice").expect("initial reviewer");
    app.wait_for("Bob: Changes requested")
        .expect("activity review verdict rendered on summary");
    app.wait_for("Labels").expect("labels row");
    app.wait_for("docs").expect("initial label");

    app.press("role:button[Request a review]")
        .expect("open reviewer picker");
    app.wait_for(
        "This repository has more people with access than are listed here. Search to find them.",
    )
    .expect("searchable truncation message");
    app.type_text("label:Search people with access", "carol")
        .expect("search reviewers on host");
    app.wait(Duration::from_millis(300)).expect("debounce");
    app.wait_for("carol").expect("host searched candidate");
    app.type_text("label:Search people with access", " ")
        .expect("trailing space with same debounced query");
    app.wait(Duration::from_millis(300))
        .expect("debounce after trailing space");
    app.press("role:button[carol]")
        .expect("request review from carol");
    app.wait_for("Review requested from carol")
        .expect("reviewer requested toast");
    app.wait_for("Carol (@carol)")
        .expect("reviewer chip updated on summary while popover stays open");
    app.press("role:button[carol]")
        .expect("second reviewer toggle inside the same open popover");
    app.wait_for("Review request to carol taken back")
        .expect("second reviewer toggle toast");
    app.press_key("Escape").expect("close reviewer picker");

    app.press("role:button[Change labels]")
        .expect("open label picker");
    app.type_text("label:Search labels", "bug")
        .expect("filter labels");
    app.wait_for("bug").expect("filtered label candidate");
    app.press("role:button[bug]").expect("apply bug label");
    app.wait_for("bug")
        .expect("label chip reflected while popover stays open");
    app.press("role:button[bug]")
        .expect("second label toggle inside the same open popover");
    app.press_key("Escape").expect("close label picker");
    app.wait_for_gone("label:Search labels", Duration::from_secs(2))
        .expect("label picker closed");

    if app.screen().shows("Back to pull requests") {
        app.press("role:button[Back to pull requests]")
            .expect("close detail sheet on phone");
        app.wait_for_gone("Selected detail from host", Duration::from_secs(2))
            .expect("detail sheet closed on phone");
        app.wait_for("role:button[Filters]").expect("list visible");
    }

    // 2. List filter pickers: Author, Labels, Host, Server, Project.
    app.press("role:button[Filters]").expect("open filters");
    app.press("Author").expect("open author submenu");
    app.type_text("label:Search authors", "octo")
        .expect("type author search");
    app.wait(Duration::from_millis(300)).expect("debounce");
    app.wait_for("octocat").expect("hosted author result");
    app.press("octocat").expect("select author");

    app.press("role:button[Filters]").expect("reopen filters");
    app.press("Labels").expect("open labels submenu");
    app.wait_for("urgent").expect("hosted label option");
    app.press("urgent").expect("toggle label filter");
    app.wait_for("label:Search labels")
        .expect("Filters Labels submenu search input stays open after label toggle");
    assert!(
        app.screen().shows("urgent"),
        "toggling a label checkbox must keep Filters popup and Labels submenu open: {:?}",
        app.screen().texts()
    );
    app.press_key("Escape").expect("close labels submenu");
    app.press("Host")
        .expect("host submenu remains directly reachable");
    app.press("GitLab").expect("select host");

    app.press("role:button[Filters]").expect("reopen filters");
    app.press("Server").expect("open server submenu");
    app.press("Fake host >> nth=0").expect("select server");

    app.press("role:button[Filters]").expect("reopen filters");
    app.press("Project").expect("open project submenu");
    app.press("Project B").expect("select project B");

    app.press_key("f9").expect("verify picker RPCs");
    app.wait_for("Pickers verified").expect("verified");
}

#[test]
fn desktop_pickers() {
    pickers_flow(DESKTOP);
}
#[test]
fn phone_pickers() {
    pickers_flow(PHONE);
}

type PersistSlot = std::sync::Mutex<(Vec<u8>, Option<Vec<u8>>)>;

static PERSIST_SLOTS: [PersistSlot; 2] = [
    std::sync::Mutex::new((Vec::new(), None)),
    std::sync::Mutex::new((Vec::new(), None)),
];

fn merge_persist_ui(slot: usize) -> AnyElement {
    let spawner = spawner_for_owner(Owner::current());
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "source-control:write"]);
    let platform = server.platform();
    let ready = signal(false);
    let seed = PERSIST_SLOTS[slot].lock().expect("lock").0.clone();
    let init_platform = platform.clone();
    spawner(
        async move {
            init_platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY),
                        value: Some(seed),
                    }],
                )
                .await
                .expect("write seed");
            ready.set(true);
        }
        .boxed_local(),
    );

    let mut d = detail();
    d["capabilities"]["actions"] = json!(["merge"]);
    d["viewerPermissions"]["actions"] = json!(["merge"]);
    d["capabilities"]["mergeMethods"] = json!(["merge", "squash", "rebase"]);
    d["mergeCapabilities"] = json!({"merge": true, "squash": true, "rebase": false});
    server.reply(
        "pullRequests.list",
        Ok(answer(vec![entry(14, "Merge persist", "open", 1)])),
    );
    server.reply("pullRequests.detail", Ok(d));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));

    let storage_ok = signal(false);
    let check_platform = platform.clone();
    let check_spawner = spawner.clone();
    on_key("f9", move |_| {
        let platform = check_platform.clone();
        check_spawner(
            async move {
                let raw = platform
                    .get(
                        StorageArea::Ordinary,
                        StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY),
                    )
                    .await
                    .expect("get")
                    .expect("present");
                let state = UiState::decode(std::str::from_utf8(&raw).expect("utf8"), &parse_ms)
                    .expect("decode");
                assert_eq!(
                    state.pull_request_merge_method,
                    t3_contracts::pull_request::PullRequestMergeMethod::Squash
                );
                assert_eq!(
                    state.project_expanded_by_id.get("logical-keep"),
                    Some(&false)
                );
                assert_eq!(state.project_order, vec!["logical-keep".to_string()]);
                let _ = UiPreferences::load(&platform).await.expect("load");
                PERSIST_SLOTS[slot].lock().expect("lock").1 = Some(raw);
                storage_ok.set(true);
            }
            .boxed_local(),
        );
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(server.pairing());
    spawner(
        async move {
            pair.await.expect("pair");
        }
        .boxed_local(),
    );

    view()
        .w_full()
        .h_full()
        .child(when(ready, move || {
            let client = client.clone();
            view()
                .w_full()
                .h_full()
                .child(t3_ui::app::app_tree(
                    Rt::new(store.clone(), Rc::new(client.clone()), Choice::Client),
                    Route::PullRequests,
                    move || t3_ui::app::provide_client_services(&client),
                ))
                .child(text(move || {
                    if storage_ok.get() {
                        "Persisted merge method"
                    } else {
                        ""
                    }
                }))
        }))
        .into_element()
}

#[component]
fn MergePersistDesktop() -> impl IntoElement {
    merge_persist_ui(0)
}

#[component]
fn MergePersistPhone() -> impl IntoElement {
    merge_persist_ui(1)
}

fn merge_method_persistence_flow(window: (f32, f32), slot: usize) {
    let initial = json!({
        "projectExpandedById": {"logical-keep": false},
        "projectOrder": ["logical-keep"],
        "pullRequestMergeMethod": "merge"
    });
    {
        let mut guard = PERSIST_SLOTS[slot].lock().expect("lock");
        guard.0 = serde_json::to_vec(&initial).expect("seed");
        guard.1 = None;
    }

    let root = || {
        if slot == 0 {
            Root::of::<MergePersistDesktop>()
        } else {
            Root::of::<MergePersistPhone>()
        }
    };

    let mut app1 = app_at(window, root());
    app1.wait_for("Merge persist").expect("list");
    app1.press("role:button[Open pull request #14]")
        .expect("open PR");
    app1.wait_for("role:button[Merge]")
        .expect("initial merge method");
    app1.press("role:button[More pull request actions]")
        .expect("actions menu");
    app1.press("Squash and merge").expect("pick squash");
    app1.wait_for("role:button[Squash and merge]")
        .expect("selected squash");
    app1.press_key("f9").expect("verify storage CAS");
    app1.wait_for("Persisted merge method")
        .expect("sidebar fields preserved");
    let persisted = PERSIST_SLOTS[slot]
        .lock()
        .expect("lock")
        .1
        .clone()
        .expect("saved bytes from session 1");
    drop(app1);

    // Boot a second independent Headless App/ClientRuntime seeded with Session 1's persisted storage.
    PERSIST_SLOTS[slot].lock().expect("lock").0 = persisted;
    let mut app2 = app_at(window, root());
    app2.wait_for("Merge persist").expect("reloaded list");
    app2.press("role:button[Open pull request #14]")
        .expect("reopen PR in session 2");
    app2.wait_for("role:button[Squash and merge]")
        .expect("persisted squash restored");
}

#[test]
fn desktop_merge_method_persistence() {
    merge_method_persistence_flow(DESKTOP, 0);
}
#[test]
fn phone_merge_method_persistence() {
    merge_method_persistence_flow(PHONE, 1);
}

#[component]
fn StaleStackFixture() -> impl IntoElement {
    let spawner = spawner_for_owner(Owner::current());
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "source-control:write"]);
    for tag in [
        "pullRequests.stack",
        "pullRequests.runAction",
        "vcs.listRefs",
    ] {
        server.register(tag, CallKind::Unary);
    }
    let mut row14 = entry(14, "Middle layer", "open", 10);
    row14["stack"] = json!({"number": 42, "position": 2, "size": 3, "base": "main"});
    for _ in 0..3 {
        server.reply("pullRequests.list", Ok(answer(vec![row14.clone()])));
    }
    for _ in 0..3 {
        server.reply(
            "pullRequests.detail",
            Ok(seed_stack_detail(
                14,
                "Middle layer",
                "feat-2",
                "feat-1",
                false,
            )),
        );
        server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    }
    // 1st stack fetch (initial panel mount): headSha = "sha14"
    server.reply("pullRequests.stack", Ok(stack_payload()));
    // 2nd stack fetch (dialog open revalidation): headSha = "sha14" (still matches)
    server.reply("pullRequests.stack", Ok(stack_payload()));
    // 3rd stack fetch (confirm revalidation): collaborator pushed "sha14-new"!
    let mut moved = stack_payload();
    moved["layers"][1]["headSha"] = json!("sha14-new");
    server.reply("pullRequests.stack", Ok(moved));
    // Extra fallback so confirm never hits unscripted error if re-opened
    server.reply("pullRequests.stack", Ok(stack_payload()));

    let verified = signal(false);
    let check_server = server.clone();
    on_key("f9", move |_| {
        assert_eq!(
            check_server.calls("pullRequests.runAction").len(),
            0,
            "stale stack heads must abort runAction cleanly"
        );
        verified.set(true);
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(server.pairing());
    spawner(
        async move {
            pair.await.expect("pair");
        }
        .boxed_local(),
    );

    view()
        .w_full()
        .h_full()
        .child(t3_ui::app::app_tree(
            Rt::new(store, Rc::new(client.clone()), Choice::Client),
            Route::PullRequests,
            move || {
                t3_ui::state::url::UrlState::install(
                    t3_ui::state::url::Location::parse("/pull-requests"),
                    t3_ui::state::nav::navigator(),
                    Rc::new(StaticUrl),
                );
                t3_ui::app::provide_client_services(&client);
            },
        ))
        .child(text(move || {
            if verified.get() {
                "Stale check passed"
            } else {
                ""
            }
        }))
}

#[test]
fn stale_stack_dialog_blocks_confirm_instead_of_running() {
    let mut app = app_at(DESKTOP, Root::of::<StaleStackFixture>());
    app.wait_for("Middle layer").expect("list");
    app.press("role:button[Open pull request #14]")
        .expect("open #14");
    app.wait_for("role:button[Merge stack]")
        .expect("merge stack btn");
    app.press("role:button[Merge stack]")
        .expect("open merge stack dialog");
    app.wait_for("Merge 1 pull request?")
        .expect("confirmation dialog open");
    app.press("id:pr-stack-confirm-dialog >> role:button[Merge stack]")
        .expect("press confirm");
    app.wait_for("Stack data may be stale. We couldn’t refresh it.")
        .expect("TSX stale stack notice shown instead of dispatching runAction");
    app.press_key("f9").expect("assert zero runAction calls");
    app.wait_for("Stale check passed").expect("verified");
}

#[component]
fn StacksWithoutActionsFixture() -> impl IntoElement {
    let spawner = spawner_for_owner(Owner::current());
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "source-control:write"]);
    for tag in ["pullRequests.stack", "pullRequests.runAction"] {
        server.register(tag, CallKind::Unary);
    }

    let mut row14 = entry(14, "Stacked single-merge PR", "open", 10);
    row14["stack"] = json!({"number": 42, "position": 2, "size": 3, "base": "main"});
    for _ in 0..4 {
        server.reply("pullRequests.list", Ok(answer(vec![row14.clone()])));
    }
    let mut pr_detail = seed_stack_detail(14, "Stacked single-merge PR", "feat-2", "feat-1", false);
    pr_detail["capabilities"]["stacks"] = json!(true);
    pr_detail["capabilities"]["stackActions"] = json!(false);
    server.reply("pullRequests.detail", Ok(pr_detail.clone()));
    server.reply("pullRequests.detail", Ok(pr_detail));
    server.reply("pullRequests.stack", Ok(stack_payload()));
    server.reply("pullRequests.stack", Ok(stack_payload()));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    server.reply("pullRequests.runAction", Ok(Value::Null));

    let verified = signal(false);
    let check_server = server.clone();
    on_key("f9", move |_| {
        let actions = check_server.calls("pullRequests.runAction");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["action"], json!("merge"));
        assert!(actions[0].get("stackNumber").is_none());
        verified.set(true);
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(server.pairing());
    let paired = signal(false);
    spawner(
        async move {
            pair.await.expect("pair");
            paired.set(true);
        }
        .boxed_local(),
    );

    view()
        .w_full()
        .h_full()
        .child(when(paired, move || {
            let client = client.clone();
            t3_ui::app::app_tree(
                Rt::new(store.clone(), Rc::new(client.clone()), Choice::Client),
                Route::PullRequests,
                move || {
                    t3_ui::state::url::UrlState::install(
                        t3_ui::state::url::Location::parse("/pull-requests"),
                        t3_ui::state::nav::navigator(),
                        Rc::new(StaticUrl),
                    );
                    t3_ui::app::provide_client_services(&client);
                },
            )
        }))
        .child(text(move || {
            if verified.get() {
                "Stacks without actions verified"
            } else {
                ""
            }
        }))
}

#[test]
fn stacks_without_stack_actions_keeps_single_merge() {
    let mut app = app_at(DESKTOP, Root::of::<StacksWithoutActionsFixture>());
    app.wait_for("Stacked single-merge PR").expect("list");
    app.press("role:button[Open pull request #14]")
        .expect("open #14");
    app.wait_for("id:pr-detail-stack >> role:button[Stack 42, layer 2 of 3]")
        .expect("stack menu visible");
    app.wait_for("role:button[Merge]")
        .expect("single Merge allowed when stackActions is false");
    assert!(
        !app.screen().shows("Merge stack"),
        "Merge stack primary button must stay hidden when stackActions is false"
    );
    app.press("id:pr-detail-stack >> role:button[Stack 42, layer 2 of 3]")
        .expect("open stack menu");
    app.wait_for("Stack #42").expect("stack header");
    assert!(
        !app.screen().shows("Merge stack (1)") && !app.screen().shows("Rebase stack"),
        "stack actions must stay hidden inside Stack menu when stackActions is false"
    );
    app.press_key("Escape").expect("close stack menu");
    app.press("role:button[Merge]").expect("arm merge");
    app.press("role:button[Confirm merge?]")
        .expect("confirm single merge");
    app.wait_for_gone("Merging...", Duration::from_secs(2))
        .expect("single merge request finished");
    app.press_key("f9").expect("assert single merge runAction");
    app.wait_for("Stacks without actions verified")
        .expect("runAction completed without stackNumber");
}

#[component]
fn TabSwitchAndSaveErrorFixture() -> impl IntoElement {
    let spawner = spawner_for_owner(Owner::current());
    let server = FakeEnvironment::new("forge", spawner.clone());
    server.set_permissions(&["orchestration:read", "source-control:write"]);
    for tag in [
        "pullRequests.activity",
        "pullRequests.reviewerCandidates",
        "pullRequests.requestReviewers",
    ] {
        server.register(tag, CallKind::Unary);
    }

    let row14 = entry(14, "Tab survival PR", "open", 5);
    for _ in 0..4 {
        server.reply("pullRequests.list", Ok(answer(vec![row14.clone()])));
    }
    let mut d = detail();
    d["capabilities"]["mergeMethods"] = json!(["merge", "squash"]);
    d["mergeCapabilities"] = json!({"merge": true, "squash": true, "rebase": false});
    d["capabilities"]["actions"] = json!(["merge"]);
    d["viewerPermissions"]["actions"] = json!(["merge"]);
    d["capabilities"]["reviewers"] =
        json!({"request": true, "listCandidates": true, "searchCandidates": false});
    d["viewerPermissions"]["requestReviewers"] = json!(true);
    server.reply("pullRequests.detail", Ok(d));
    server.reply("pullRequests.linkedThreads", Ok(json!({"threads": []})));
    server.reply(
        "pullRequests.activity",
        Ok(json!({
            "author": null,
            "reviewers": [],
            "comments": [],
            "commentCount": 0,
            "commentsTruncated": false,
            "reviewThreads": [],
            "commits": []
        })),
    );
    server.reply(
        "pullRequests.activity",
        Ok(json!({
            "author": null,
            "reviewers": [],
            "comments": [],
            "commentCount": 0,
            "commentsTruncated": false,
            "reviewThreads": [],
            "commits": []
        })),
    );
    server.reply(
        "pullRequests.reviewerCandidates",
        Ok(json!({
            "candidates": [{"id": "u-bob", "kind": "user", "login": "bob", "name": "Bob", "avatarUrl": null, "isRequested": false}],
            "truncated": false
        })),
    );
    let deferred_reviewer = server.defer_reply("pullRequests.requestReviewers");
    let reviewer_tx = Rc::new(RefCell::new(Some(deferred_reviewer)));
    let finish_reviewer = reviewer_tx.clone();
    on_key("f8", move |_| {
        if let Some(tx) = finish_reviewer.borrow_mut().take() {
            let _ = tx.send(Err(json!({"message": "Forbidden"})));
        }
    });

    // Fail the next write read to trigger merge-method save failure toast (R8).
    let fail_read_server = server.clone();
    on_key("f7", move |_| {
        let key = StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY);
        let dropped = fail_read_server.defer_read(key);
        drop(dropped);
    });

    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let pair = client.pair(server.pairing());
    spawner(
        async move {
            pair.await.expect("pair");
        }
        .boxed_local(),
    );

    view().w_full().h_full().child(t3_ui::app::app_tree(
        Rt::new(store, Rc::new(client.clone()), Choice::Client),
        Route::PullRequests,
        move || {
            t3_ui::state::url::UrlState::install(
                t3_ui::state::url::Location::parse("/pull-requests"),
                t3_ui::state::nav::navigator(),
                Rc::new(StaticUrl),
            );
            t3_ui::app::provide_client_services(&client);
        },
    ))
}

#[test]
fn picker_request_survives_tab_switch_and_surfaces_merge_save_error() {
    let mut app = app_at(DESKTOP, Root::of::<TabSwitchAndSaveErrorFixture>());
    app.wait_for("Tab survival PR").expect("list");
    app.press("role:button[Open pull request #14]")
        .expect("open #14");
    app.wait_for("role:button[Request a review]")
        .expect("reviewer trigger");
    app.press("role:button[Request a review]")
        .expect("open reviewer picker");
    app.wait_for("role:button[bob]").expect("bob candidate");
    app.press("role:button[bob]")
        .expect("start in-flight request");
    app.press_key("Escape").expect("close reviewer popover");
    app.press("role:button[Timeline]")
        .expect("switch to Timeline while request is in flight");
    app.press_key("f8")
        .expect("resolve deferred requestReviewers failure");
    app.wait_for("Could not ask bob for a review")
        .expect("R5: picker error toast must still appear after tab switch");

    // Trigger merge-method persistence failure toast (R8).
    app.press_key("f7").expect("arm failing storage read");
    app.press("role:button[More pull request actions]")
        .expect("open actions menu");
    app.press("Squash and merge").expect("pick squash");
    app.wait_for("Could not save merge method")
        .expect("R8: save failure toast surfaced");
}
