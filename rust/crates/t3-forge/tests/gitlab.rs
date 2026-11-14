use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use t3_forge::{FailureReason, Forge, Result, cli::*, forge::*, gitlab::GitLab, model::*};

struct Step {
    operation: &'static str,
    body: Value,
    code: i32,
}
fn step(operation: &'static str, body: Value) -> Step {
    Step {
        operation,
        body,
        code: 0,
    }
}
fn graph(operation: &'static str, data: Value) -> Step {
    step(operation, json!({"data":data}))
}
#[derive(Default)]
struct FixtureCli {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<CliRequest>>,
}
#[async_trait]
impl ForgeCli for FixtureCli {
    async fn run(&self, request: CliRequest) -> Result<CliResponse> {
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected CLI operation");
        assert_eq!(request.operation, step.operation);
        assert_eq!(request.program, "glab");
        self.requests.lock().unwrap().push(request);
        Ok(CliResponse {
            stdout: step
                .body
                .as_str()
                .map(str::as_bytes)
                .map(Vec::from)
                .unwrap_or_else(|| step.body.to_string().into_bytes()),
            stderr: if step.code == 0 {
                vec![]
            } else if step.code == 404 {
                b"HTTP 404".to_vec()
            } else {
                b"HTTP 500 https://secret:credential@example.invalid/?token=secret".to_vec()
            },
            code: step.code,
            truncated: false,
        })
    }
}
fn setup(steps: Vec<Step>) -> (GitLab, Arc<FixtureCli>) {
    let cli = Arc::new(FixtureCli {
        steps: Mutex::new(steps.into()),
        ..Default::default()
    });
    let forge = GitLab::new(t3_git::Git::new("."))
        .unwrap()
        .with_cli(cli.clone());
    (forge, cli)
}
fn input() -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: "fixture-checkout".into(),
            host: "gitlab.com".into(),
            repository: "acme/web".into(),
        },
        number: 42,
    }
}
fn raw() -> Value {
    json!({"iid":42,"title":"Add merge requests","web_url":"https://gitlab.com/acme/web/-/merge_requests/42","source_branch":"feature/page","target_branch":"main","created_at":"2026-07-01T00:00:00Z","updated_at":"2026-07-02T00:00:00Z","author":{"username":"bilal","name":"Bilal"},"reviewers":[{"id":9,"username":"julius"}],"labels":["backend","  "],"state":"opened","merge_status":"can_be_merged","draft":false,"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","diff_refs":{"base_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","start_sha":"cccccccccccccccccccccccccccccccccccccccc"},"description":"Body","changes_count":"1000+","diverged_commits_count":2})
}
fn action(action: PullRequestAction) -> ActionInput {
    ActionInput {
        pr: input(),
        action,
        stack_number: None,
        expected_stack_heads: None,
        remove_agent_credits_on_merge: false,
        merge_method: None,
        update_method: None,
    }
}
fn body(request: &CliRequest) -> Value {
    serde_json::from_slice(request.stdin.as_ref().unwrap()).unwrap()
}
fn drained(cli: &FixtureCli) {
    assert!(cli.steps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rest_pages_count_malformed_rows_and_keep_host_review_state() {
    let (forge, cli) = setup(vec![
        graph(
            "supportsGraphqlList",
            json!({"metadata":{"version":"17.8.0"}}),
        ),
        step("listMergeRequests", json!([null, raw(), raw()])),
        graph(
            "getListStates",
            json!({"metadata":{"enterprise":true},"project":{"mergeRequests":{"nodes":[{"iid":"42","approved":false,"approvedBy":{"nodes":[{"username":"bot"}]},"headPipeline":{"status":"CANCELED"},"reviewers":{"nodes":[]}}]}}}),
        ),
    ]);
    let page = forge
        .list_change_requests(ListInput {
            repo: input().repo,
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::Authored,
            viewer: "bilal".into(),
            limit: 1,
            query: Some("--evil&search".into()),
            cursor: None,
            filters: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.truncated);
    assert_eq!(page.cursor_advance, Some(2));
    let value = serde_json::to_value(&page.items[0]).unwrap();
    assert_eq!(value["author"]["login"], "bilal");
    assert_eq!(value["labels"], json!([{"name":"backend","color":null}]));
    assert_eq!(value["checksState"], "failing");
    assert_eq!(value["reviewDecision"], "review-required");
    let requests = cli.requests.lock().unwrap();
    assert!(
        requests[1]
            .args
            .last()
            .unwrap()
            .contains("search=--evil%26search")
    );
    assert_eq!(requests[1].args[requests[1].args.len() - 2], "--");
    drained(&cli);
}
#[tokio::test]
async fn detail_preserves_permissions_counts_checks_and_cache() {
    let mut raw = raw();
    raw["head_pipeline"] = json!({"id":21,"project_id":1,"status":"success"});
    let (forge, cli) = setup(vec![
        step("getMergeRequestDetail", raw),
        graph(
            "getLabelPermission",
            json!({"project":{"mergeRequest":{"userPermissions":{"adminMergeRequest":true}}}}),
        ),
        step(
            "getProjectMergeCapabilities",
            json!({"merge_method":"rebase_merge","squash_option":"default_on"}),
        ),
        graph(
            "getPipelineStageChecks",
            json!({"project":{"mergeRequest":{"headPipeline":{"stages":{"nodes":[{"name":"test","jobs":{"nodes":[{"name":"unit","status":"failed","allowFailure":true,"webPath":"/acme/web/jobs/3"}],"pageInfo":{"hasNextPage":false}}}],"pageInfo":{"hasNextPage":false}}}}}}),
        ),
        graph(
            "getMergeRequestStats",
            json!({"project":{"mergeRequests":{"nodes":[{"iid":"42","diffStatsSummary":{"additions":12,"deletions":3}}]}}}),
        ),
        graph(
            "getListStates",
            json!({"project":{"mergeRequests":{"nodes":[{"iid":"42","headPipeline":{"status":"success"}}]}}}),
        ),
    ]);
    let detail = forge.get_change_request(input()).await.unwrap();
    let checks = forge.get_change_request_checks(input()).await.unwrap();
    let value = serde_json::to_value(detail).unwrap();
    assert_eq!(value["changedFiles"], 1000);
    assert_eq!(value["additions"], 12);
    assert_eq!(value["deletions"], 3);
    assert_eq!(value["behindBy"], 2);
    assert_eq!(value["baseComparison"], "behind");
    assert_eq!(value["checksState"], "passing");
    assert_eq!(value["viewerPermissions"]["labels"], true);
    assert_eq!(
        value["mergeCapabilities"],
        json!({"merge":false,"rebase":true,"squash":true})
    );
    assert_eq!(
        serde_json::to_value(checks).unwrap()["checks"][0]["status"],
        "neutral"
    );
    drained(&cli);
}
#[tokio::test]
async fn summary_keeps_auto_merge_and_optional_enrichment_failure() {
    let mut raw = raw();
    raw["auto_merge_enabled"] = json!(true);
    raw["squash_on_merge"] = json!(true);
    let (forge, cli) = setup(vec![
        step("getMergeRequestDetail", raw),
        Step {
            operation: "getMergeRequestStats",
            body: Value::Null,
            code: 1,
        },
        graph(
            "getListStates",
            json!({"project":{"mergeRequests":{"nodes":[]}}}),
        ),
    ]);
    let value =
        serde_json::to_value(forge.get_change_request_summary(input()).await.unwrap()).unwrap();
    assert_eq!(value["autoMergeEnabled"], true);
    assert_eq!(value["autoMergeMethod"], "squash");
    assert_eq!(value["changedFiles"], 1000);
    drained(&cli);
}
#[tokio::test]
async fn diff_fallback_preserves_rename_modes_and_overflow() {
    let (forge, cli) = setup(vec![
        Step {
            operation: "getMergeRequestDiff",
            body: json!({}),
            code: 404,
        },
        step(
            "getMergeRequestDiff",
            json!({"overflow":true,"changes":[{"old_path":"old.txt","new_path":"new.txt","renamed_file":true,"a_mode":"100644","b_mode":"100755","diff":"@@ -1 +1 @@\n-old\n+new\n"}]}),
        ),
    ]);
    let result = forge
        .get_diff(DiffInput {
            pr: input(),
            cursor: None,
            commit: None,
        })
        .await
        .unwrap();
    assert!(result.truncated);
    assert!(result.next_cursor.is_none());
    assert_eq!(
        result.patch,
        "diff --git a/old.txt b/new.txt\nsimilarity index 0%\nrename from old.txt\nrename to new.txt\n--- a/old.txt\n+++ b/new.txt\n@@ -1 +1 @@\n-old\n+new\n"
    );
    drained(&cli);
}
#[tokio::test]
async fn inline_review_uses_full_diff_refs_and_approves_last() {
    let (forge, cli) = setup(vec![
        step("getDiffRefs", raw()),
        step(
            "getMergeRequestDiff",
            json!([{"old_path":"src/a.rs","new_path":"src/a.rs","diff":"@@ -1,2 +1,3 @@\n keep\n+one\n+two\n-last\n"}]),
        ),
        step("submitReview", json!({})),
        step("submitReview", json!({})),
        step("submitReview", json!({})),
    ]);
    forge.submit_review(ReviewInput {pr:input(),verdict:PullRequestReviewVerdict::Approve,body:"--dangerous\nreview".into(),comments:vec![serde_json::from_value(json!({"path":"src/a.rs","position":{"kind":"added","newLine":3},"startPosition":{"kind":"added","newLine":2},"body":"Range"})).unwrap()]}).await.unwrap();
    let requests = cli.requests.lock().unwrap();
    let payload = body(&requests[2]);
    assert_eq!(
        payload["position"]["base_sha"],
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert_eq!(
        payload["position"]["line_range"]["start"]["line_code"],
        "c46b6f3386ba91fc8fcb5f960fd24012ff3c1460_2_2"
    );
    assert_eq!(
        payload["position"]["line_range"]["end"]["line_code"],
        "c46b6f3386ba91fc8fcb5f960fd24012ff3c1460_2_3"
    );
    assert_eq!(body(&requests[3])["body"], "--dangerous\nreview");
    assert!(requests[4].args.last().unwrap().ends_with("/approve"));
    drained(&cli);
}
#[tokio::test]
async fn changed_source_head_refuses_branch_deletion() {
    let (forge, cli) = setup(vec![
        graph(
            "getSourceBranch",
            json!({"project":{"mergeRequest":{"state":"merged","sourceBranch":"feature/page","diffHeadSha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceProject":{"fullPath":"fork/web"},"userPermissions":{"removeSourceBranch":true}}}}),
        ),
        step(
            "getSourceBranch",
            json!({"default":false,"commit":{"id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}),
        ),
    ]);
    assert!(
        forge
            .run_action(action(PullRequestAction::DeleteBranch))
            .await
            .is_err()
    );
    drained(&cli);
}
#[tokio::test]
async fn failed_revert_cleans_up_created_branch_before_shutdown_finishes() {
    let mut raw = raw();
    raw["state"] = json!("merged");
    raw["merge_commit_sha"] = json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let (forge, cli) = setup(vec![
        step("revertMergeRequest", raw),
        step("revertMergeRequest", json!({})),
        Step {
            operation: "revertMergeRequest",
            body: Value::Null,
            code: 1,
        },
        step("revertCleanup", Value::Null),
    ]);
    let error = forge
        .run_action(action(PullRequestAction::Revert))
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Failed));
    assert!(!error.to_string().contains("secret"));
    forge.shutdown().await;
    let requests = cli.requests.lock().unwrap();
    assert_eq!(
        requests[3].args,
        vec![
            "api",
            "--method=DELETE",
            "--",
            "projects/acme%2Fweb/repository/branches/revert-aaaaaaaa"
        ]
    );
    drained(&cli);
}
#[tokio::test]
async fn revisions_distinguish_unknown_blob_connection_from_deleted_files() {
    let (forge, cli) = setup(vec![
        step("getDiffRefs", raw()),
        graph(
            "getFileRevisions",
            json!({"project":{"repository":{"blobs":{"nodes":[{"path":"present","oid":"blob-oid"}]}}}}),
        ),
    ]);
    let result = forge
        .get_file_revisions(FileRevisionsInput {
            pr: input(),
            paths: vec!["present".into(), "missing".into()],
        })
        .await
        .unwrap();
    assert_eq!(result.revisions.get("present").unwrap(), "blob-oid");
    assert_eq!(result.revisions.get("missing").unwrap(), "");
    let empty = forge
        .get_file_revisions(FileRevisionsInput {
            pr: input(),
            paths: vec![],
        })
        .await
        .unwrap();
    assert!(empty.revisions.is_empty());
    drained(&cli);
    let (forge, cli) = setup(vec![
        step("getDiffRefs", raw()),
        graph("getFileRevisions", json!({"project":{"repository":null}})),
    ]);
    assert!(
        forge
            .get_file_revisions(FileRevisionsInput {
                pr: input(),
                paths: vec!["unknown".into()]
            })
            .await
            .unwrap()
            .revisions
            .is_empty()
    );
    drained(&cli);
}
#[tokio::test]
async fn mutations_send_untrusted_text_in_json_and_preserve_empty_body() {
    let (forge, cli) = setup(vec![
        step("updateMergeRequest", json!({})),
        step("getMergeRequestDetail", raw()),
    ]);
    forge
        .update_change_request(UpdateInput {
            pr: input(),
            title: Some("--flag=value".into()),
            body: Some(String::new()),
            maintainer_can_modify: Some(false),
        })
        .await
        .unwrap();
    let preview = forge.get_change_request_preview(input()).await.unwrap();
    assert_eq!(preview.number, 42);
    let requests = cli.requests.lock().unwrap();
    assert_eq!(
        body(&requests[0]),
        json!({"title":"--flag=value","description":"","allow_collaboration":false})
    );
    assert!(!requests[0].args.iter().any(|arg| arg.contains("--flag")));
    drained(&cli);
}
#[tokio::test]
async fn identity_refinement_requires_matching_authenticated_host_and_preserves_case_for_glab() {
    let (forge, cli) = setup(vec![step(
        "discover",
        json!("GitLab.EXAMPLE\n  ✓ Logged in to GitLab.EXAMPLE as fixture (token)\n"),
    )]);
    let identity=serde_json::from_value(json!({"canonicalKey":"git:gitlab.example/acme/web","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"git@GitLab.EXAMPLE:acme/web.git"},"rootPath":"fixture-checkout","provider":"unknown"})).unwrap();
    let identity = forge.refine_repository_identity(identity).await.unwrap();
    assert_eq!(identity.provider.unwrap().0, "gitlab");
    assert_eq!(
        cli.requests.lock().unwrap()[0].args,
        vec!["auth", "status", "--hostname=GitLab.EXAMPLE"]
    );
    drained(&cli);
    let (forge, cli) = setup(vec![step(
        "discover",
        json!("other.example\n  ✓ Logged in to other.example as fixture (token)\n"),
    )]);
    let refined=forge.refine_unknown_remote(CheckoutInput {cwd:"fixture".into(),context:Some(CheckoutContext {provider:serde_json::from_value(json!({"kind":"unknown","name":"gitlab.example","baseUrl":"https://gitlab.example"})).unwrap(),remote_name:"origin".into(),remote_url:"git@gitlab.example:acme/web.git".into(),requested_host:None})}).await.unwrap();
    assert!(refined.is_none());
    drained(&cli);
}
#[tokio::test]
async fn activity_retains_inline_and_overview_findings_and_reaction_authors() {
    let date = "2026-07-01T00:00:00Z";
    let note = json!({"id":11,"body":"Fix this","created_at":date,"author":{"username":"reviewer"},"resolvable":true,"resolved":false});
    let mut inline = note.clone();
    inline["id"] = json!(12);
    inline["position"] = json!({"position_type":"text","new_path":"src/a.rs","new_line":5,"base_sha":"old","head_sha":"old","start_sha":"old"});
    let (forge, cli) = setup(vec![
        step(
            "getMergeRequestActivity",
            json!([note.clone(),inline.clone(),{"id":99,"body":"internal","created_at":date,"system":true}]),
        ),
        step(
            "getMergeRequestActivity",
            json!([{"id":"overview","notes":[note]},{"id":"inline","notes":[inline]}]),
        ),
        step(
            "listCommits",
            json!([{"id":"abc","title":"Fix","committed_date":date,"author_name":"Author","parent_ids":["a","b"],"stats":{"additions":999,"deletions":999}}]),
        ),
        step("getMergeRequestDetail", raw()),
        graph(
            "listReactions",
            json!({"currentUser":{"username":"me"},"project":{"mergeRequest":{"awardEmoji":{"nodes":[{"name":"thumbsup","user":{"username":"me"}},{"name":"thumbsup","user":{"username":"other"}}]},"notes":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}),
        ),
    ]);
    let activity =
        serde_json::to_value(forge.get_change_request_activity(input()).await.unwrap()).unwrap();
    assert_eq!(activity["comments"].as_array().unwrap().len(), 2);
    assert_eq!(activity["comments"][0]["kind"], "review-comment");
    assert_eq!(activity["overviewThreads"][0]["id"], "overview");
    assert_eq!(activity["reviewThreads"][0]["line"], 5);
    assert_eq!(activity["reviewThreads"][0]["isOutdated"], true);
    assert!(activity["commits"][0].get("additions").is_none());
    assert_eq!(activity["reactions"][0]["content"], "thumbs-up");
    assert_eq!(activity["reactions"][0]["count"], 2);
    assert_eq!(activity["reactions"][0]["viewerHasReacted"], true);
    drained(&cli);
}
