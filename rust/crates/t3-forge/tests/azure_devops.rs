use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_forge::{
    FailureReason, Forge, Result, azure_devops::AzureDevOps, cli::*, forge::*, model::*,
};
struct Answer {
    operation: &'static str,
    contains: Option<&'static str>,
    value: Value,
    code: i32,
}
fn answer(operation: &'static str, value: Value) -> Answer {
    Answer {
        operation,
        value,
        contains: None,
        code: 0,
    }
}
#[derive(Default)]
struct Recorded {
    answers: Mutex<Vec<Answer>>,
    requests: Mutex<Vec<CliRequest>>,
}
#[async_trait]
impl ForgeCli for Recorded {
    async fn run(&self, request: CliRequest) -> Result<CliResponse> {
        assert!(request.program == "az" || request.program == "fixture-python.exe");
        let mut answers = self.answers.lock().unwrap();
        let at = answers
            .iter()
            .position(|a| {
                a.operation == request.operation
                    && a.contains
                        .is_none_or(|part| request.args.iter().any(|arg| arg.contains(part)))
            })
            .expect("unexpected Azure CLI operation");
        let answer = answers.remove(at);
        self.requests.lock().unwrap().push(request);
        Ok(CliResponse {
            stdout: answer.value.to_string().into_bytes(),
            stderr: if answer.code == 401 {
                b"Please run 'az login'. https://secret:token@example.invalid".to_vec()
            } else {
                vec![]
            },
            code: answer.code,
            truncated: false,
        })
    }
}
fn setup(answers: Vec<Answer>) -> (AzureDevOps, Arc<Recorded>) {
    let cli = Arc::new(Recorded {
        answers: Mutex::new(answers),
        ..Default::default()
    });
    (
        AzureDevOps::new()
            .unwrap()
            .with_invocation(t3_forge::azure_devops::launcher::Invocation::direct())
            .with_cli(cli.clone()),
        cli,
    )
}
fn drained(cli: &Recorded) {
    assert!(cli.answers.lock().unwrap().is_empty());
}
fn input() -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: "fixture".into(),
            host: "dev.azure.com".into(),
            repository: "Web".into(),
        },
        number: 7,
    }
}
fn raw() -> Value {
    json!({"pullRequestId":7,"title":"Add page","sourceRefName":"refs/heads/feature","targetRefName":"refs/heads/main","creationDate":"2026-07-01T00:00:00Z","description":"Body","status":"active","mergeStatus":"succeeded","createdBy":{"uniqueName":"author@example.invalid","displayName":"Author"},"reviewers":[{"uniqueName":"reviewer@example.invalid"}],"repository":{"name":"Web","project":{"name":"Project Name"}},"url":"https://dev.azure.com/acme/_apis/git/pullrequests/7","lastMergeSourceCommit":{"commitId":"stale-merge-evaluation"}})
}
fn iterations() -> Value {
    json!({"value":[{"id":1,"sourceRefCommit":{"commitId":"head-one"},"commonRefCommit":{"commitId":"base-one"}},{"id":2,"sourceRefCommit":{"commitId":"head-two"},"commonRefCommit":{"commitId":"base-two"}}]})
}
#[tokio::test]
async fn checkout_without_state_includes_completed_requests() {
    let mut value = raw();
    value["status"] = json!("completed");
    let (forge, cli) = setup(vec![answer("checkoutList", json!([value]))]);
    let rows = forge
        .checkout_list_change_requests(CheckoutListInput {
            checkout: CheckoutInput {
                cwd: "fixture".into(),
                context: None,
            },
            source: None,
            head_selector: "feature".into(),
            state: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(rows[0].state, ChangeRequestState::Merged);
    assert!(
        cli.requests.lock().unwrap()[0]
            .args
            .iter()
            .any(|s| s == "--status=all")
    );
    drained(&cli);
}
#[tokio::test]
async fn unicode_calls_use_bundled_python_and_installer() {
    let (forge, cli) = setup(vec![answer("checkoutList", json!([]))]);
    let invocation = t3_forge::azure_devops::launcher::Invocation {
        command: "fixture-python.exe".into(),
        args: vec![
            "-X".into(),
            "utf8".into(),
            "-IBm".into(),
            "azure.cli".into(),
        ],
        installer: Some("MSI"),
    };
    let forge = forge.with_invocation(invocation);
    forge
        .checkout_list_change_requests(CheckoutListInput {
            checkout: CheckoutInput {
                cwd: "fixture".into(),
                context: None,
            },
            source: None,
            head_selector: "café/你好".into(),
            state: None,
            limit: None,
        })
        .await
        .unwrap();
    let requests = cli.requests.lock().unwrap();
    assert_eq!(requests[0].program, "fixture-python.exe");
    assert_eq!(&requests[0].args[..4], ["-X", "utf8", "-IBm", "azure.cli"]);
    assert_eq!(
        requests[0].environment[std::ffi::OsStr::new("AZ_INSTALLER")],
        Some("MSI".into())
    );
}
#[tokio::test]
async fn list_advances_by_raw_rows_and_preserves_review_involvement() {
    let (forge, cli) = setup(vec![
        answer("listPullRequests", json!([null, null])),
        answer("listPullRequests", json!([raw(), raw()])),
    ]);
    let page = forge
        .list_change_requests(ListInput {
            repo: input().repo,
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::Reviewing,
            viewer: "reviewer@example.invalid".into(),
            limit: 1,
            query: None,
            cursor: None,
            filters: Some(serde_json::from_value(json!({"author":"ME"})).unwrap()),
        })
        .await
        .unwrap();
    assert_eq!(page.cursor_advance, Some(3));
    assert!(page.truncated);
    assert_eq!(page.items[0].head_branch, "feature");
    assert_eq!(
        page.items[0].url,
        "https://dev.azure.com/acme/Project%20Name/_git/Web/pullrequest/7"
    );
    let requests = cli.requests.lock().unwrap();
    assert!(requests[1].args.contains(&"--skip=2".into()));
    assert!(
        requests[0]
            .args
            .contains(&"--reviewer=reviewer@example.invalid".into())
    );
    assert!(
        requests[0]
            .args
            .contains(&"--creator=reviewer@example.invalid".into())
    );
    drained(&cli);
}
#[tokio::test]
async fn detail_uses_latest_iteration_head_instead_of_lagging_trial_merge() {
    let mut raw = raw();
    raw["autoCompleteSetBy"] = json!({});
    raw["completionOptions"] = json!({"mergeStrategy":"squash"});
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw),
        answer("pullRequestIterations", iterations()),
        answer(
            "pullRequestIterationChanges",
            json!({"changeEntries":[{"changeType":"edit","item":{"path":"/a.rs","objectId":"blob"}},{"item":{"path":"/directory","isFolder":true}}]}),
        ),
    ]);
    let detail = serde_json::to_value(forge.get_change_request(input()).await.unwrap()).unwrap();
    assert_eq!(detail["headSha"], "head-two");
    assert_eq!(detail["changedFiles"], 1);
    assert_eq!(detail["autoMergeEnabled"], true);
    assert_eq!(detail["autoMergeMethod"], "squash");
    assert_eq!(
        detail["mergeCapabilities"],
        json!({"merge":true,"squash":true,"rebase":false})
    );
    assert_eq!(detail["viewerPermissions"]["comment"], false);
    drained(&cli);
}
#[tokio::test]
async fn diff_cursor_pins_iteration_and_encodes_item_paths() {
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw()),
        answer("pullRequestIterations", iterations()),
        answer(
            "pullRequestIterationChanges",
            json!({"changeEntries":[{"changeType":"rename, edit","sourceServerItem":"/old file","item":{"path":"/new&name#file"}}]}),
        ),
        Answer {
            operation: "items",
            contains: Some("versionDescriptor.version=base-one"),
            value: json!({"content":"old\n"}),
            code: 0,
        },
        Answer {
            operation: "items",
            contains: Some("versionDescriptor.version=head-one"),
            value: json!({"content":"new\n"}),
            code: 0,
        },
    ]);
    let diff = forge
        .get_diff(DiffInput {
            pr: input(),
            cursor: Some("1:0".into()),
            commit: Some("ignored-by-Azure".into()),
        })
        .await
        .unwrap();
    assert!(!diff.truncated);
    assert!(
        diff.patch
            .contains("rename from old file\nrename to new&name#file")
    );
    assert!(diff.patch.contains("@@ -1 +1 @@\n-old\n+new\n"));
    let requests = cli.requests.lock().unwrap();
    assert!(requests[2].args.contains(&"iterationId=1".into()));
    assert!(
        requests
            .iter()
            .any(|r| r.args.contains(&"path=%2Fnew%26name%23file".into()))
    );
    drained(&cli);
}
#[tokio::test]
async fn diff_budget_advances_after_an_oversized_slice() {
    let body = "x\n".repeat(150_000);
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw()),
        answer("pullRequestIterations", iterations()),
        answer(
            "pullRequestIterationChanges",
            json!({"changeEntries":[{"changeType":"add","item":{"path":"/first"}},{"changeType":"add","item":{"path":"/second"}}]}),
        ),
        answer("items", json!({"content":body})),
        answer("items", json!({"content":"small\n"})),
    ]);
    let diff = forge
        .get_diff(DiffInput {
            pr: input(),
            cursor: None,
            commit: None,
        })
        .await
        .unwrap();
    assert_eq!(diff.next_cursor.as_deref(), Some("2:1"));
    assert!(diff.patch.starts_with("diff --git a/first b/first"));
    assert!(!diff.patch.contains("diff --git a/second"));
    drained(&cli);
}
#[tokio::test]
async fn file_revisions_do_not_clear_unknown_paths_after_truncated_changes() {
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw()),
        answer("pullRequestIterations", iterations()),
        answer(
            "pullRequestIterationChanges",
            json!({"changeEntries":[{"item":{"path":"/known","objectId":"blob"}}],"nextSkip":10000}),
        ),
    ]);
    let result = forge
        .get_file_revisions(FileRevisionsInput {
            pr: input(),
            paths: vec!["known".into(), "unknown".into()],
        })
        .await
        .unwrap();
    assert_eq!(result.revisions.get("known").unwrap(), "blob");
    assert!(!result.revisions.contains_key("unknown"));
    drained(&cli);
}
#[tokio::test]
async fn item_authentication_errors_reach_caller_without_emitting_a_fake_patch() {
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw()),
        answer("pullRequestIterations", iterations()),
        answer(
            "pullRequestIterationChanges",
            json!({"changeEntries":[{"changeType":"add","item":{"path":"/a"}}]}),
        ),
        Answer {
            operation: "items",
            contains: None,
            value: Value::Null,
            code: 401,
        },
    ]);
    let error = forge
        .get_diff(DiffInput {
            pr: input(),
            cursor: None,
            commit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Unauthenticated));
    assert!(!error.to_string().contains("secret"));
    drained(&cli);
}
#[tokio::test]
async fn update_keeps_multiline_option_values_and_rejects_azure_file_indirection() {
    let (forge, cli) = setup(vec![answer("updatePullRequest", Value::Null)]);
    forge
        .update_change_request(UpdateInput {
            pr: input(),
            title: Some("--draft=true".into()),
            body: Some("one\ntwo".into()),
            maintainer_can_modify: None,
        })
        .await
        .unwrap();
    let result = forge
        .update_change_request(UpdateInput {
            pr: input(),
            title: Some("@server-file".into()),
            body: None,
            maintainer_can_modify: None,
        })
        .await;
    assert!(result.is_err());
    let requests = cli.requests.lock().unwrap();
    assert!(requests[0].args.contains(&"--title=--draft=true".into()));
    assert!(requests[0].args.contains(&"--description=one\ntwo".into()));
    drained(&cli);
}
#[tokio::test]
async fn default_branch_decodes_repository_name_without_stripping_dot_git() {
    let (forge, cli) = setup(vec![answer(
        "getDefaultBranch",
        json!({"defaultBranch":"refs/heads/main"}),
    )]);
    let context = CheckoutContext {
        provider: serde_json::from_value(
            json!({"kind":"azure-devops","name":"Azure DevOps","baseUrl":"https://dev.azure.com"}),
        )
        .unwrap(),
        remote_name: "origin".into(),
        remote_url: "git@ssh.dev.azure.com:v3/acme/project/Repo%20Name.git".into(),
        requested_host: None,
    };
    assert_eq!(
        forge
            .get_default_branch(CheckoutInput {
                cwd: "fixture".into(),
                context: Some(context)
            })
            .await
            .unwrap()
            .as_deref(),
        Some("main")
    );
    assert!(
        cli.requests.lock().unwrap()[0]
            .args
            .contains(&"--repository=Repo Name.git".into())
    );
    drained(&cli);
}
#[tokio::test]
async fn activity_omits_system_deleted_and_empty_notes() {
    let (forge, cli) = setup(vec![
        answer("getPullRequest", raw()),
        answer(
            "pullRequestThreads",
            json!({"value":[{"id":2,"threadContext":{"filePath":"/a.rs"},"comments":[{"id":1,"content":"Visible","publishedDate":"2026-07-01T00:00:00Z","author":{"uniqueName":"reviewer"}},{"id":2,"content":"Internal","publishedDate":"2026-07-01T00:00:00Z","commentType":"SYSTEM"},{"id":3,"content":"Deleted","publishedDate":"2026-07-01T00:00:00Z","isDeleted":true}]}]}),
        ),
    ]);
    let activity =
        serde_json::to_value(forge.get_change_request_activity(input()).await.unwrap()).unwrap();
    assert_eq!(activity["commentCount"], 1);
    assert_eq!(activity["comments"][0]["id"], "2:1");
    assert_eq!(activity["comments"][0]["path"], "a.rs");
    assert_eq!(activity["commentsTruncated"], false);
    drained(&cli);
}
