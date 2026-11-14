mod common;
use common::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_forge::{
    FailureReason, Forge, bitbucket::Bitbucket, forge::*, http::*, model::*, rate_limit::RateLimits,
};
fn http(base: url::Url) -> ApiHttp {
    ApiHttp::new(
        "bitbucket",
        base,
        ApiCredential::Bearer("fixture-secret".into()),
        Arc::new(RateLimits::default()),
    )
    .unwrap()
}
fn forge(base: url::Url) -> Bitbucket {
    Bitbucket::new(http(base), t3_git::Git::new(".")).unwrap()
}
fn pr() -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: "fixture".into(),
            host: "bitbucket.org".into(),
            repository: "acme/web".into(),
        },
        number: 7,
    }
}
fn raw() -> Value {
    json!({"id":7,"title":"Add page","description":"Body","state":"MERGED","source":{"branch":{"name":"feature"},"commit":{"hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"repository":{"full_name":"fork/web"}},"destination":{"branch":{"name":"main"},"repository":{"full_name":"acme/web"}},"author":{"nickname":"author","display_name":"Author"},"created_on":"2026-07-01T00:00:00+00:00","updated_on":"2026-07-02T00:00:00.123456+00:00","closed_on":"2026-07-02T00:00:00+00:00","reviewers":[{"uuid":"{hidden}"},{"uuid":"{shown}","nickname":"reviewer"}],"links":{"html":{"href":"https://bitbucket.org/acme/web/pull-requests/7"}}})
}
#[tokio::test]
async fn list_escapes_filters_and_continues_across_malformed_pages() {
    let (base, task) = server(2, |request| {
        if request.path.ends_with("second") {
            answer(json!({"values":[raw()]}))
        } else {
            answer(json!({"values":[null],"next":"/second"}))
        }
    })
    .await;
    let page = forge(base)
        .list_change_requests(ListInput {
            repo: pr().repo,
            state: PullRequestListState::Closed,
            involvement: PullRequestInvolvement::All,
            viewer: "viewer".into(),
            limit: 1,
            query: Some("a\" OR 1=1\\".into()),
            cursor: Some(ProviderListCursor {
                updated_before: "2026-07-02T00:00:00.123Z".into(),
                delivered: 1,
                seen_at: Some(vec![5, 6]),
                resume_after: None,
            }),
            filters: Some(serde_json::from_value(json!({"author":"@me","draft":"only"})).unwrap()),
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(!page.truncated);
    assert_eq!(page.items[0].state, PullRequestState::Merged);
    let requests = task.await.unwrap();
    let url = url::Url::parse(&format!("http://fixture{}", requests[0].path)).unwrap();
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        query["q"],
        "(title ~ \"a\\\" OR 1=1\\\\\" OR description ~ \"a\\\" OR 1=1\\\\\") AND (author.nickname = \"viewer\" OR author.display_name = \"viewer\") AND draft = true AND updated_on < 2026-07-02T00:00:00.124Z AND id != 5 AND id != 6"
    );
    assert!(
        requests
            .iter()
            .all(|r| r.headers.contains("Bearer fixture-secret"))
    );
}
#[tokio::test]
async fn detail_preserves_counts_closed_time_permissions_and_check_identity() {
    let(base,task)=server(6,|r|{if r.path.contains("diffstat"){if r.path.ends_with("page=2"){answer(json!({"values":[{"lines_added":4,"lines_removed":2}]}))}else{answer(json!({"values":[{"lines_added":3,"lines_removed":1}],"next":"/repositories/acme/web/pullrequests/7/diffstat?page=2"}))}}else if r.path.contains("/conflicts"){answer(json!({"values":[{}]}))}else if r.path.contains("/statuses"){answer(json!({"values":[{"name":"test","key":"unit","state":"INPROGRESS"},{"name":"test","key":"integration","state":"FAILED"},{"name":"test","key":"unit","state":"SUCCESSFUL"}]}))}else if r.path.contains("/permissions/"){answer(json!({"values":[{"permission":"read"}]}))}else{answer(raw())}}).await;
    let detail = serde_json::to_value(forge(base).get_change_request(pr()).await.unwrap()).unwrap();
    assert_eq!(detail["additions"], 7);
    assert_eq!(detail["deletions"], 3);
    assert_eq!(detail["changedFiles"], 2);
    assert_eq!(detail["mergeability"], "conflicting");
    assert_eq!(detail["mergedAt"], "2026-07-02T00:00:00+00:00");
    assert_eq!(detail["closedAt"], Value::Null);
    assert_eq!(detail["viewerPermissions"]["actions"], json!(["close"]));
    assert_eq!(detail["checks"][0]["name"], "integration / test");
    assert_eq!(detail["checks"][0]["status"], "failure");
    assert_eq!(detail["checks"][1]["name"], "unit / test");
    assert_eq!(detail["checks"][1]["status"], "success");
    task.await.unwrap();
}
#[tokio::test]
async fn reviewer_replacement_keeps_hidden_uuids_and_partial_update_keeps_empty_body() {
    let (base, task) = server(3, |r| {
        if r.method == "GET" {
            answer(raw())
        } else {
            answer(json!({}))
        }
    })
    .await;
    let forge = forge(base);
    forge
        .set_reviewer_request(ReviewerRequestInput {
            pr: pr(),
            reviewers: vec![
                serde_json::from_value(json!({"id":"{new}","login":"new","kind":"user"})).unwrap(),
            ],
            requested: true,
        })
        .await
        .unwrap();
    forge
        .update_change_request(UpdateInput {
            pr: pr(),
            title: None,
            body: Some(String::new()),
            maintainer_can_modify: None,
        })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_eq!(
        requests[1].body,
        json!({"reviewers":[{"uuid":"{hidden}"},{"uuid":"{new}"},{"uuid":"{shown}"}]})
    );
    assert_eq!(requests[2].body, json!({"description":""}));
}
#[tokio::test]
async fn review_posts_inline_comments_before_body_and_final_verdict() {
    let (base, task) = server(3, |_| answer(json!({}))).await;
    forge(base).submit_review(ReviewInput{pr:pr(),verdict:PullRequestReviewVerdict::Approve,body:"Summary".into(),comments:vec![serde_json::from_value(json!({"path":"a.rs","position":{"kind":"context","side":"left","oldLine":3,"newLine":4},"body":"Fix"})).unwrap()]}).await.unwrap();
    let requests = task.await.unwrap();
    assert_eq!(
        requests[0].body,
        json!({"content":{"raw":"Fix"},"inline":{"path":"a.rs","from":3}})
    );
    assert_eq!(requests[1].body, json!({"content":{"raw":"Summary"}}));
    assert!(requests[2].path.ends_with("/approve"));
}
#[tokio::test]
async fn patch_revisions_cache_ids_and_fill_absent_paths() {
    let(base,task)=server(1,|_|Answer{status:200,headers:vec![],body:"diff --git a/a.rs b/a.rs\nindex 1234567..7654321 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n".into()}).await;
    let forge = forge(base);
    for _ in 0..2 {
        let revisions = forge
            .get_file_revisions(FileRevisionsInput {
                pr: pr(),
                paths: vec!["a.rs".into(), "absent".into()],
            })
            .await
            .unwrap();
        assert_eq!(
            revisions.revisions,
            BTreeMap::from([
                ("a.rs".into(), "7654321".into()),
                ("absent".into(), String::new())
            ])
        );
        assert_eq!(revisions.complete, Some(true));
    }
    task.await.unwrap();
}
#[tokio::test]
async fn http_refuses_foreign_redirect_and_pagination_origins() {
    let (base, task) = server(1, |_| Answer {
        status: 302,
        body: String::new(),
        headers: vec![("Location".into(), "http://127.0.0.1:9/steal".into())],
    })
    .await;
    let error = http(base)
        .json("fixture", reqwest::Method::GET, "/start", None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("fixture-secret"));
    assert!(!error.to_string().contains("steal"));
    task.await.unwrap();
    let (base, task) = server(1, |_| {
        answer(json!({"values":[],"next":"https://attacker.invalid/steal"}))
    })
    .await;
    assert!(
        forge(base)
            .list_change_requests(ListInput {
                repo: pr().repo,
                state: PullRequestListState::Open,
                involvement: PullRequestInvolvement::All,
                viewer: "fixture".into(),
                limit: 5,
                query: None,
                cursor: None,
                filters: None
            })
            .await
            .is_err()
    );
    task.await.unwrap();
}
#[tokio::test]
async fn rate_limit_pauses_background_reads_but_allows_mutations() {
    let (base, task) = server(2, |r| {
        if r.method == "GET" {
            Answer {
                status: 429,
                body: "secret server diagnostic".into(),
                headers: vec![("Retry-After".into(), "300".into())],
            }
        } else {
            answer(json!({}))
        }
    })
    .await;
    let http = http(base);
    let error = http
        .json("fixture", reqwest::Method::GET, "/quota", None)
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::RateLimited));
    assert!(!error.to_string().contains("secret"));
    let paused = http
        .json("fixture", reqwest::Method::GET, "/quota", None)
        .await
        .unwrap_err();
    assert_eq!(paused.reason(), Some(FailureReason::RateLimited));
    http.json("fixture", reqwest::Method::POST, "/write", Some(json!({})))
        .await
        .unwrap();
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn bounded_response_keeps_patch_prefix_but_rejects_truncated_json() {
    let (base, task) = server(2, |_| answer(json!({"value":"xxxxxxxxxxxxxxxx"}))).await;
    let http = http(base);
    let response = http
        .request("fixture", reqwest::Method::GET, "/large", None, 8)
        .await
        .unwrap();
    assert!(response.truncated);
    assert_eq!(response.bytes.len(), 8);
    assert!(response.json("bitbucket", "fixture").is_err());
    let response = http
        .request("fixture", reqwest::Method::GET, "/exact", None, 28)
        .await
        .unwrap();
    assert!(!response.truncated);
    assert_eq!(
        response.json("bitbucket", "fixture").unwrap()["value"],
        "xxxxxxxxxxxxxxxx"
    );
    task.await.unwrap();
}
#[tokio::test]
async fn credential_updates_clear_scoped_cooldown_and_reach_registered_adapter() {
    let (base, task) = server(3, |r| {
        if r.headers.contains("Bearer fixture-secret") {
            Answer {
                status: 429,
                body: String::new(),
                headers: vec![("Retry-After".into(), "300".into())],
            }
        } else if r.path.ends_with("/diff") {
            Answer {
                status: 200,
                body: "diff --git a/a b/a\nindex aaa..bbb\n".into(),
                headers: vec![],
            }
        } else {
            answer(json!({"nickname":"updated"}))
        }
    })
    .await;
    let http = http(base);
    let forge = Bitbucket::new(http.clone(), t3_git::Git::new(".")).unwrap();
    assert!(
        forge
            .get_viewer(ViewerInput {
                cwd: "fixture".into(),
                host: None
            })
            .await
            .is_err()
    );
    http.update_credential(ApiCredential::Basic {
        username: "email".into(),
        password: "api-token".into(),
    })
    .unwrap();
    assert_eq!(forge.settings_revision(), 1);
    assert_eq!(
        forge
            .get_viewer(ViewerInput {
                cwd: "fixture".into(),
                host: None
            })
            .await
            .unwrap(),
        "updated"
    );
    let revisions = forge
        .get_file_revisions(FileRevisionsInput {
            pr: pr(),
            paths: vec!["a".into()],
        })
        .await
        .unwrap();
    assert_eq!(revisions.revisions["a"], "bbb");
    let requests = task.await.unwrap();
    assert!(requests[1].headers.contains("Basic ZW1haWw6YXBpLXRva2Vu"));
}
#[tokio::test]
async fn activity_assembles_threads_across_pages_and_retains_review_votes() {
    let(base,task)=server(4,|r|{if r.path.contains("comments"){let root=json!({"id":1,"content":{"raw":"Root"},"user":{"nickname":"reviewer"},"created_on":"2026-07-01T00:00:00Z","inline":{"path":"a.rs","from":3,"outdated":true},"resolution":{}});let reply=json!({"id":2,"content":{"raw":"Reply"},"parent":{"id":1},"created_on":"2026-07-02T00:00:00Z"});if r.path.contains("page=2"){answer(json!({"values":[reply,{"id":3,"content":{"raw":"private"},"created_on":"2026-07-02T00:00:00Z","pending":true}]}))}else{answer(json!({"values":[root],"next":"/repositories/acme/web/pullrequests/7/comments?page=2"}))}}else if r.path.contains("/commits"){answer(json!({"values":[{"hash":"new","date":"2026-07-02T00:00:00Z","message":"New\nbody"},{"hash":"old","date":"2026-07-01T00:00:00Z","author":{"raw":"Committer"}}]}))}else{let mut raw=raw();raw["participants"]=json!([{"user":{"nickname":"reviewer"},"approved":true,"participated_on":"2026-07-02T00:00:00Z"}]);answer(raw)}}).await;
    let activity =
        serde_json::to_value(forge(base).get_change_request_activity(pr()).await.unwrap()).unwrap();
    assert_eq!(activity["commentCount"], 3);
    assert_eq!(
        activity["reviewThreads"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(activity["reviewThreads"][0]["side"], "left");
    assert_eq!(activity["reviewThreads"][0]["isResolved"], true);
    assert_eq!(activity["reviewThreads"][0]["isOutdated"], true);
    assert_eq!(activity["commits"][0]["oid"], "old");
    assert_eq!(activity["comments"][2]["reviewState"], "approved");
    task.await.unwrap();
}
