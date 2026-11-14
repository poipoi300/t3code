mod common;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use t3_forge::{
    FailureReason, Forge, ForgeError, Result,
    cli::*,
    forge::*,
    forgejo::{Forgejo, transport::*},
    model::*,
};
fn environment() -> t3_process::Environment {
    t3_process::Environment::from([
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
        ),
    ])
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    link: Option<&'static str>,
}
fn reply(raw: Value) -> Reply {
    Reply {
        status: 200,
        body: raw.to_string().into_bytes(),
        link: Some("<last>; rel=last"),
    }
}
type Handler = Box<dyn Fn(&CliRequest) -> Reply + Send + Sync>;
struct Tea {
    requests: Mutex<Vec<CliRequest>>,
    handler: Handler,
    logins: Value,
}
#[async_trait]
impl ForgeCli for Tea {
    async fn run(&self, request: CliRequest) -> Result<CliResponse> {
        if request.program == "fj" {
            return Err(ForgeError::provider(
                "forgejo",
                "fixture",
                FailureReason::MissingTool,
                "Fixture fj is absent.",
            ));
        }
        let response = if request.operation == "listLogins" {
            reply(self.logins.clone())
        } else {
            (self.handler)(&request)
        };
        self.requests.lock().unwrap().push(request);
        Ok(CliResponse {
            stdout: response.body,
            stderr: format!(
                "HTTP/1.1 {}\n{}",
                response.status,
                response
                    .link
                    .map(|l| format!("link: {l}\n"))
                    .unwrap_or_default()
            )
            .into_bytes(),
            code: 0,
            truncated: false,
        })
    }
}
fn setup(
    handler: impl Fn(&CliRequest) -> Reply + Send + Sync + 'static,
) -> (tempfile::TempDir, Forgejo, Arc<Tea>) {
    let dir = tempfile::tempdir().unwrap();
    let cli = Arc::new(Tea {
        requests: Default::default(),
        handler: Box::new(handler),
        logins: json!([{"name":"fixture-login","url":"https://git.example/mount","user":"fixture","default":"true","valid":"true"}]),
    });
    let forge = Forgejo::new(
        t3_git::Git::new(dir.path()).with_environment(environment()),
        ForgejoConfig {
            authentication_cwd: dir.path().into(),
            keys_paths: vec![],
            environment: environment(),
        },
    )
    .unwrap()
    .with_cli(cli.clone());
    (dir, forge, cli)
}
fn pr(cwd: &Path) -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: cwd.to_string_lossy().into(),
            host: "git.example".into(),
            repository: "mount/acme/web".into(),
        },
        number: 7,
    }
}
fn path(request: &CliRequest) -> String {
    url::Url::parse(request.args.last().unwrap())
        .unwrap()
        .path()
        .to_string()
}
fn body(request: &CliRequest) -> Value {
    serde_json::from_slice(request.stdin.as_ref().unwrap()).unwrap()
}
fn raw() -> Value {
    json!({"number":7,"title":"[WIP] Add page","body":"Body","html_url":"https://git.example/mount/acme/web/pulls/7","user":{"login":"author"},"state":"open","merged":false,"mergeable":false,"head":{"ref":"refs/pull/7/head","label":"original-branch","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","repo":{"full_name":"fork/web","owner":{"login":"fork"}}},"base":{"ref":"main","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":{"full_name":"acme/web","owner":{"login":"acme"}}},"merge_base":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","created_at":"2026-07-01T00:00:00+00:00","updated_at":"2026-07-02T00:00:00+00:00","closed_at":null,"merged_at":null,"labels":[],"requested_reviewers":[{"login":"requested"}]})
}
fn checkout_input(cwd: &Path, branch: &str) -> CheckoutListInput {
    CheckoutListInput{checkout:CheckoutInput{cwd:cwd.into(),context:Some(CheckoutContext{provider:serde_json::from_value(json!({"kind":"forgejo","name":"Forgejo","baseUrl":"https://git.example/mount"})).unwrap(),remote_name:"origin".into(),remote_url:"https://git.example/mount/acme/web.git".into(),requested_host:None})},source:None,head_selector:branch.into(),state:None,limit:None}
}
#[tokio::test(start_paused = true)]
async fn ignored_head_filter_scans_incrementally_and_rechecks_deleted_candidates() {
    let pulls = Arc::new(Mutex::new(
        (1..=120)
            .rev()
            .map(|number| {
                let mut p = raw();
                p["number"] = json!(number);
                p["head"]["ref"] = json!(if number == 5 {
                    "feature".into()
                } else {
                    format!("other-{number}")
                });
                p["head"]["label"] = p["head"]["ref"].clone();
                p["updated_at"] = json!(
                    chrono::DateTime::from_timestamp(number, 0)
                        .unwrap()
                        .to_rfc3339()
                );
                p
            })
            .collect::<Vec<_>>(),
    ));
    let records = pulls.clone();
    let pages = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = pages.clone();
    let (dir, forge, _) = setup(move |r| {
        let url = url::Url::parse(r.args.last().unwrap()).unwrap();
        let pulls = records.lock().unwrap();
        if let Some(number) = url
            .path()
            .strip_prefix("/mount/api/v1/repos/acme/web/pulls/")
            .and_then(|s| s.parse::<u64>().ok())
        {
            return pulls
                .iter()
                .find(|p| p["number"] == number)
                .cloned()
                .map(reply)
                .unwrap_or(Reply {
                    status: 404,
                    body: b"private body".to_vec(),
                    link: None,
                });
        }
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let page = url
            .query_pairs()
            .find(|(k, _)| k == "page")
            .unwrap()
            .1
            .parse::<usize>()
            .unwrap();
        reply(json!(
            pulls
                .iter()
                .skip((page - 1) * 50)
                .take(50)
                .collect::<Vec<_>>()
        ))
    });
    assert!(
        forge
            .checkout_list_change_requests(checkout_input(dir.path(), "missing-one"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert!(
        forge
            .checkout_list_change_requests(checkout_input(dir.path(), "missing-two"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst), 5);
    let result = forge
        .checkout_list_change_requests(checkout_input(dir.path(), "feature"))
        .await
        .unwrap();
    assert_eq!(result[0].number, 5);
    {
        let mut pulls = pulls.lock().unwrap();
        let p = pulls.iter_mut().find(|p| p["number"] == 5).unwrap();
        p["state"] = json!("closed");
        p["updated_at"] = json!("2026-11-13T00:00:00Z");
        pulls.sort_by(|a, b| b["updated_at"].as_str().cmp(&a["updated_at"].as_str()));
    }
    assert_eq!(
        forge
            .checkout_list_change_requests(checkout_input(dir.path(), "feature"))
            .await
            .unwrap()[0]
            .state,
        ChangeRequestState::Closed
    );
    pulls.lock().unwrap().retain(|p| p["number"] != 5);
    assert!(
        forge
            .checkout_list_change_requests(checkout_input(dir.path(), "feature"))
            .await
            .unwrap()
            .is_empty()
    );
    let before = pages.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(std::time::Duration::from_secs(3600)).await;
    assert!(
        forge
            .checkout_list_change_requests(checkout_input(dir.path(), "missing-three"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst) - before, 4);
}
#[tokio::test]
async fn checkout_without_state_includes_merged_requests() {
    let (dir, forge, _) = setup(|r| {
        let url = url::Url::parse(r.args.last().unwrap()).unwrap();
        assert!(url.query_pairs().any(|(k, v)| k == "state" && v == "all"));
        let mut row = raw();
        row["merged"] = json!(true);
        reply(json!([row]))
    });
    let rows=forge.checkout_list_change_requests(CheckoutListInput{checkout:CheckoutInput{cwd:dir.path().into(),context:Some(CheckoutContext{provider:serde_json::from_value(json!({"kind":"forgejo","name":"Forgejo","baseUrl":"https://git.example/mount"})).unwrap(),remote_name:"origin".into(),remote_url:"https://git.example/mount/acme/web.git".into(),requested_host:None})},source:None,head_selector:"original-branch".into(),state:None,limit:Some(1)}).await.unwrap();
    assert_eq!(rows[0].state, ChangeRequestState::Merged);
}
#[tokio::test]
async fn mounted_repository_listing_uses_actual_page_size_and_raw_offsets() {
    let (dir, forge, cli) = setup(|r| {
        let url = url::Url::parse(r.args.last().unwrap()).unwrap();
        let params: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        if params["page"] == "1" {
            reply(json!([null, raw()]))
        } else {
            let mut merged = raw();
            merged["merged"] = json!(true);
            reply(json!([merged, raw()]))
        }
    });
    let page = forge
        .list_change_requests(ListInput {
            repo: pr(dir.path()).repo,
            state: PullRequestListState::Merged,
            involvement: PullRequestInvolvement::All,
            viewer: "fixture".into(),
            limit: 1,
            query: None,
            cursor: Some(ProviderListCursor {
                updated_before: "2026-07-02T00:00:00Z".into(),
                delivered: 2,
                seen_at: None,
                resume_after: None,
            }),
            filters: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].head_branch, "original-branch");
    assert_eq!(page.cursor_advance, Some(1));
    assert!(page.truncated);
    let requests = cli.requests.lock().unwrap();
    let requests: Vec<_> = requests.iter().filter(|r| r.operation == "api").collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(path(requests[0]), "/mount/api/v1/repos/acme/web/pulls");
    assert!(requests[1].args.last().unwrap().contains("page=2"));
}
#[tokio::test]
async fn checks_deduplicate_same_second_updates_and_keep_missing_required_globs_pending() {
    let (dir, forge, _) = setup(|r| {
        let path = path(r);
        if path.contains("/statuses/") {
            reply(
                json!([{"id":20,"context":"ci/unit","status":"pending","description":null,"target_url":"/jobs/20","updated_at":"2026-07-01T00:00:00Z"},{"id":19,"context":"ci/unit","status":"failure","description":null,"target_url":null,"updated_at":"2026-07-01T00:00:00Z"}]),
            )
        } else if path.contains("/branches/") {
            reply(json!({"enable_status_check":true,"status_check_contexts":["ci/*","missing?"]}))
        } else {
            reply(raw())
        }
    });
    let checks = serde_json::to_value(
        forge
            .get_change_request_checks(pr(dir.path()))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(checks["checks"].as_array().unwrap().len(), 2);
    assert_eq!(checks["checks"][0]["status"], "pending");
    assert_eq!(checks["checks"][0]["required"], true);
    assert_eq!(checks["checks"][0]["url"], "https://git.example/jobs/20");
    assert_eq!(checks["checks"][1]["name"], "missing?");
}
#[tokio::test]
async fn inline_review_uses_old_paths_and_opaque_text_never_enters_cli_options() {
    let (dir, forge, cli) = setup(|r| {
        if r.args.iter().any(|a| a == "--method=POST") {
            reply(json!({}))
        } else {
            reply(raw())
        }
    });
    forge.submit_review(ReviewInput{pr:pr(dir.path()),verdict:PullRequestReviewVerdict::RequestChanges,body:"--reviewer=attacker".into(),comments:vec![serde_json::from_value(json!({"path":"new.rs","oldPath":"old.rs","position":{"kind":"deleted","oldLine":4},"body":"Fix"})).unwrap()]}).await.unwrap();
    let requests = cli.requests.lock().unwrap();
    let write = requests
        .iter()
        .find(|r| r.args.iter().any(|a| a == "--method=POST"))
        .unwrap();
    assert_eq!(
        body(write),
        json!({"event":"REQUEST_CHANGES","body":"--reviewer=attacker","commit_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","comments":[{"path":"old.rs","body":"Fix","old_position":4,"new_position":0}]})
    );
    assert!(write.args.contains(&"--data=@-".into()));
    assert!(!write.args.iter().any(|a| a.contains("attacker")));
}
#[tokio::test]
async fn merge_stays_successful_when_only_post_merge_branch_deletion_fails() {
    let state = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = state.clone();
    let (dir, forge, cli) = setup(move |r| {
        let path = path(r);
        if path.ends_with("/merge") {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Reply {
                status: 403,
                body: b"sensitive server diagnostic".to_vec(),
                link: None,
            }
        } else if path.ends_with("/pulls/7") {
            let mut raw = raw();
            raw["merged"] = json!(flag.load(std::sync::atomic::Ordering::SeqCst));
            reply(raw)
        } else {
            reply(json!({"full_name":"acme/web","default_delete_branch_after_merge":true}))
        }
    });
    let result = forge
        .run_action(ActionInput {
            pr: pr(dir.path()),
            action: PullRequestAction::Merge,
            stack_number: None,
            expected_stack_heads: None,
            remove_agent_credits_on_merge: false,
            merge_method: Some(PullRequestMergeMethod::FastForward),
            update_method: None,
        })
        .await
        .unwrap();
    assert!(result.is_none());
    let requests = cli.requests.lock().unwrap();
    let write = requests
        .iter()
        .find(|r| r.args.last().unwrap().ends_with("/merge"))
        .unwrap();
    assert_eq!(
        body(write),
        json!({"Do":"fast-forward-only","delete_branch_after_merge":true})
    );
}
#[tokio::test]
async fn review_reactions_use_issue_comment_id_and_cannot_accept_a_path_as_an_id() {
    let (dir, forge, cli) = setup(|r| {
        if path(r).contains("/pulls/7/reviews/9") {
            reply(
                json!({"id":9,"html_url":"https://git.example/mount/acme/web/pulls/7#issuecomment-12"}),
            )
        } else {
            reply(json!({}))
        }
    });
    forge
        .set_reaction(ReactionInput {
            pr: pr(dir.path()),
            subject_id: Some("review:9".into()),
            content: PullRequestReactionContent::Heart,
            reacted: true,
        })
        .await
        .unwrap();
    assert!(
        forge
            .set_reaction(ReactionInput {
                pr: pr(dir.path()),
                subject_id: Some("../other".into()),
                content: PullRequestReactionContent::Heart,
                reacted: true
            })
            .await
            .is_err()
    );
    let requests = cli.requests.lock().unwrap();
    let write = requests
        .iter()
        .find(|r| r.args.iter().any(|a| a == "--method=POST"))
        .unwrap();
    assert_eq!(
        path(write),
        "/mount/api/v1/repos/acme/web/issues/comments/12/reactions"
    );
    assert_eq!(body(write), json!({"content":"heart"}));
}
#[test]
fn serving_logins_reject_ambiguous_aliases_and_distinguish_mounts() {
    let logins:Vec<Login>=serde_json::from_value(json!([{"name":"one","url":"https://git.example/one","ssh_host":"alias","user":"one","default":"true"},{"name":"two","url":"https://git.example/two","ssh_host":"alias","user":"two","default":"true"}])).unwrap();
    let ssh = parse_remote("git@alias:acme/web.git").unwrap();
    assert!(match_login(&logins, &ssh, None, false).is_none());
    let web = parse_remote("https://git.example/two/acme/web.git").unwrap();
    assert_eq!(match_login(&logins, &web, None, false).unwrap().name, "two");
    let mut same = logins.clone();
    same[1].url = same[0].url.clone();
    same[1].default = "false".into();
    assert_eq!(match_login(&same, &ssh, None, false).unwrap().name, "one");
}
#[tokio::test]
async fn identity_refinement_uses_login_web_host_for_ssh_aliases() {
    let (dir, forge, _) = setup(|_| reply(json!({})));
    let identity:RepositoryIdentity=serde_json::from_value(json!({"canonicalKey":"git:git.example/mount/acme/web","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"ssh://git@git.example/acme/web.git"},"rootPath":dir.path(),"provider":"unknown"})).unwrap();
    let identity = forge.refine_repository_identity(identity).await.unwrap();
    assert_eq!(identity.provider.unwrap().0, "forgejo");
    assert_eq!(
        identity.web_url.unwrap().0,
        "https://git.example/mount/acme/web"
    );
}
struct Fj {
    requests: Mutex<Vec<CliRequest>>,
    keys: std::path::PathBuf,
    host: String,
    reject: bool,
}
#[async_trait]
impl ForgeCli for Fj {
    async fn run(&self, request: CliRequest) -> Result<CliResponse> {
        assert_eq!(request.program, "fj");
        if request.operation == "authenticate" && !self.reject {
            std::fs::write(
                &self.keys,
                json!({"hosts":{self.host.clone():{"type":"OAuth","token":"refreshed-token"}}})
                    .to_string(),
            )
            .unwrap();
        }
        self.requests.lock().unwrap().push(request);
        Ok(CliResponse {
            stdout: vec![],
            stderr: if self.reject {
                b"Authentication failed; secret diagnostic".to_vec()
            } else {
                vec![]
            },
            code: if self.reject { 1 } else { 0 },
            truncated: false,
        })
    }
}
#[tokio::test]
async fn fj_renews_oauth_then_rereads_keys_and_caches_the_authenticated_token() {
    let(mut base,task)=common::server(2,|r|{assert_eq!(r.method,"GET");assert_eq!(r.path,"/api/v1/repos/acme/web");assert_eq!(r.body,Value::Null);assert!(r.headers.contains("token refreshed-token"));common::answer(json!({"full_name":"acme/web","clone_url":"https://git.example/acme/web.git","ssh_url":"git@git.example:acme/web.git"}))}).await;
    base.set_path("");
    let dir = tempfile::tempdir().unwrap();
    let keys = dir.path().join("keys.json");
    let host = base.authority().to_string();
    std::fs::write(
        &keys,
        json!({"hosts":{host.clone():{"type":"OAuth","token":"expired-token"}}}).to_string(),
    )
    .unwrap();
    let cli = Arc::new(Fj {
        requests: Default::default(),
        keys: keys.clone(),
        host,
        reject: false,
    });
    let forge = Forgejo::new(
        t3_git::Git::new(dir.path()).with_environment(environment()),
        ForgejoConfig {
            authentication_cwd: dir.path().into(),
            keys_paths: vec![keys],
            environment: environment(),
        },
    )
    .unwrap()
    .with_cli(cli.clone());
    for _ in 0..2 {
        let context = CheckoutContext {
            provider: serde_json::from_value(
                json!({"kind":"forgejo","name":"Forgejo","baseUrl":base.as_str()}),
            )
            .unwrap(),
            remote_name: "origin".into(),
            remote_url: format!("{base}acme/web.git"),
            requested_host: None,
        };
        let urls = forge
            .get_repository_clone_urls(CloneUrlsInput {
                checkout: CheckoutInput {
                    cwd: dir.path().into(),
                    context: Some(context),
                },
                repository: "acme/web".into(),
                host: None,
            })
            .await
            .unwrap();
        assert_eq!(urls.name_with_owner.0, "acme/web");
    }
    assert_eq!(cli.requests.lock().unwrap().len(), 1);
    task.await.unwrap();
}
#[tokio::test]
async fn a_configured_but_rejected_fj_account_does_not_fall_back_to_tea() {
    let dir = tempfile::tempdir().unwrap();
    let keys = dir.path().join("keys.json");
    std::fs::write(
        &keys,
        json!({"hosts":{"git.example":{"type":"Application","token":"fixture-token"}}}).to_string(),
    )
    .unwrap();
    let cli = Arc::new(Fj {
        requests: Default::default(),
        keys: keys.clone(),
        host: "git.example".into(),
        reject: true,
    });
    let forge = Forgejo::new(
        t3_git::Git::new(dir.path()).with_environment(environment()),
        ForgejoConfig {
            authentication_cwd: dir.path().into(),
            keys_paths: vec![keys],
            environment: environment(),
        },
    )
    .unwrap()
    .with_cli(cli.clone());
    let error = forge
        .get_change_request_preview(pr(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Unauthenticated));
    assert!(!error.to_string().contains("secret"));
    let requests = cli.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].program, "fj");
}
