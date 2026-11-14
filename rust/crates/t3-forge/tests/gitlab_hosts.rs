use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use t3_forge::{
    Forge,
    cli::*,
    gitlab::GitLab,
    registry::{ForgeHandle, ForgeRegistry},
    review_host::{ForgeReviewHost, ReviewSetup},
    services::SourceControlRepositoryService,
};

#[derive(Default)]
struct Recorder(Mutex<Vec<CliRequest>>);
#[async_trait]
impl ForgeCli for Recorder {
    async fn run(&self, request: CliRequest) -> t3_forge::Result<CliResponse> {
        let body = if request.operation == "getMergeRequest" {
            json!({"iid":1,"title":"Fixture","web_url":"https://gitlab.com/a/b/-/merge_requests/1","source_branch":"feature","target_branch":"main","state":"opened"})
        } else {
            json!({"path_with_namespace":"a/b", "web_url":"https://gitlab.com/a/b", "ssh_url_to_repo":"git@gitlab.com:a/b.git"})
        };
        self.0.lock().unwrap().push(request);
        Ok(CliResponse {
            code: 0,
            stdout: body.to_string().into_bytes(),
            stderr: vec![],
            truncated: false,
        })
    }
}
struct Setup;
#[async_trait]
impl ReviewSetup for Setup {
    async fn setup(
        &self,
        _: &std::path::Path,
        _: &str,
        _: &std::path::Path,
    ) -> Result<(), t3_git::GitError> {
        panic!("foreign requests cannot run setup")
    }
}
fn git(root: &std::path::Path) -> t3_git::Git {
    let program = root.join(if cfg!(windows) {
        "record-git.cmd"
    } else {
        "record-git"
    });
    let source = if cfg!(windows) {
        "@echo off\r\n>\"%~dp0git-spawned\" echo spawned\r\nexit /b 1\r\n"
    } else {
        "#!/bin/sh\n: > \"$(dirname \"$0\")/git-spawned\"\nexit 1\n"
    };
    std::fs::write(&program, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    t3_git::Git::new(root).with_program(program)
}
fn forge(cli: Arc<Recorder>, root: &std::path::Path) -> Arc<dyn Forge> {
    Arc::new(
        GitLab::new(git(root))
            .unwrap()
            .with_cli(cli)
            .with_environment(
                [
                    ("GITLAB_HOST".into(), Some("gitlab.com".into())),
                    ("GITLAB_TOKEN".into(), Some("fixture-private-token".into())),
                ]
                .into(),
            ),
    )
}
fn assert_no_process(cli: &Recorder, root: &std::path::Path) {
    assert!(
        !root.join("git-spawned").exists(),
        "a refused operation spawned Git"
    );
    let requests = cli.0.lock().unwrap();
    assert!(
        requests.is_empty(),
        "foreign host reached the child-process/environment boundary"
    );
}
#[tokio::test]
async fn lookup_refuses_foreign_authorities_before_building_a_cli_request() {
    for repository in [
        "https://attacker.example/a/b",
        "https://gitlab.com:8443/a/b",
        "http://127.0.0.1:8888/a/b",
    ] {
        let root = tempfile::tempdir().unwrap();
        let cli = Arc::new(Recorder::default());
        let service = SourceControlRepositoryService::new(
            Arc::new(ForgeRegistry::new([forge(cli.clone(), root.path())])),
            git(root.path()),
            root.path().into(),
        );
        assert!(
            service
                .lookup_repository(
                    serde_json::from_value(json!({"provider":"gitlab", "repository":repository}))
                        .unwrap()
                )
                .await
                .is_err()
        );
        assert_no_process(&cli, root.path());
    }
}
#[tokio::test]
async fn clone_refuses_foreign_authority_before_building_a_cli_request() {
    let root = tempfile::tempdir().unwrap();
    let cli = Arc::new(Recorder::default());
    let service = SourceControlRepositoryService::new(
        Arc::new(ForgeRegistry::new([forge(cli.clone(), root.path())])),
        git(root.path()),
        root.path().into(),
    );
    assert!(service.clone_repository(serde_json::from_value(json!({"provider":"gitlab", "repository":"https://attacker.example/a/b", "destinationPath":root.path().join("clone")})).unwrap(), Default::default()).await.is_err());
    assert_no_process(&cli, root.path());
}
#[tokio::test]
async fn prepare_thread_refuses_foreign_authority_before_building_a_cli_request() {
    let root = tempfile::tempdir().unwrap();
    let cli = Arc::new(Recorder::default());
    let host = ForgeReviewHost::new(
        ForgeHandle {
            forge: forge(cli.clone(), root.path()),
            context: None,
        },
        Arc::new(Setup),
    );
    let service = t3_git::GitWorkflowService::new(git(root.path()));
    assert!(Arc::new(host).prepare_pull_request_thread(service, &serde_json::from_value(json!({"cwd":root.path(), "reference":"https://attacker.example/a/b/-/merge_requests/1", "mode":"local"})).unwrap()).await.is_err());
    assert_no_process(&cli, root.path());
}
#[tokio::test]
async fn trusted_thread_preflight_enters_the_configured_git_workflow() {
    let root = tempfile::tempdir().unwrap();
    let cli = Arc::new(Recorder::default());
    let host = Arc::new(ForgeReviewHost::new(
        ForgeHandle {
            forge: forge(cli.clone(), root.path()),
            context: None,
        },
        Arc::new(Setup),
    ));
    let workflow = t3_git::GitWorkflowService::new(git(root.path()));
    let input = serde_json::from_value(json!({"cwd":root.path(),"reference":"https://gitlab.com/a/b/-/merge_requests/1","mode":"local"})).unwrap();
    assert!(
        host.prepare_pull_request_thread(workflow, &input)
            .await
            .is_err()
    );
    let requests = cli.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].operation, "getMergeRequest");
    assert_eq!(
        requests[0].environment[std::ffi::OsStr::new("GITLAB_HOST")],
        Some("gitlab.com".into())
    );
    assert!(
        root.path().join("git-spawned").exists(),
        "trusted preflight did not reach the rejecting Git fixture"
    );
}
#[tokio::test]
async fn review_clone_urls_refuse_foreign_host_overrides() {
    use t3_git::ReviewHost;
    let root = tempfile::tempdir().unwrap();
    let cli = Arc::new(Recorder::default());
    let host = ForgeReviewHost::new(
        ForgeHandle {
            forge: forge(cli.clone(), root.path()),
            context: None,
        },
        Arc::new(Setup),
    );
    assert!(
        host.clone_urls(
            std::path::Path::new("unused"),
            "a/b",
            "https://attacker.example/a/b/-/merge_requests/1"
        )
        .await
        .is_err()
    );
    assert_no_process(&cli, root.path());
}

#[tokio::test]
async fn configured_and_authorized_checkout_hosts_are_accepted_but_do_not_leak_scope() {
    use t3_forge::forge::*;
    let cli = Arc::new(Recorder::default());
    let forge = GitLab::new(t3_git::Git::new("unused"))
        .unwrap()
        .with_cli(cli.clone())
        .with_environment([("GITLAB_HOST".into(), Some("gitlab.com".into()))].into())
        .with_configured_hosts(["configured.example:8443".into()])
        .unwrap();
    let checkout = CheckoutInput {
        cwd: "unused".into(),
        context: Some(CheckoutContext {
            provider: serde_json::from_value(
                json!({"kind":"gitlab", "name":"GitLab", "baseUrl":"https://attacker.example"}),
            )
            .unwrap(),
            remote_name: "origin".into(),
            remote_url: "https://checkout.example/team/repo.git".into(),
            requested_host: Some("attacker.example".into()),
        }),
    };
    for (checkout, host) in [
        (
            CheckoutInput {
                cwd: "unused".into(),
                context: None,
            },
            "configured.example:8443",
        ),
        (checkout, "checkout.example"),
    ] {
        forge
            .get_repository_clone_urls(CloneUrlsInput {
                checkout,
                repository: format!("https://{host}/a/b"),
                host: Some(host.into()),
            })
            .await
            .unwrap();
    }
    {
        let requests = cli.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].environment[std::ffi::OsStr::new("GITLAB_HOST")],
            Some("configured.example:8443".into())
        );
        assert_eq!(
            requests[1].environment[std::ffi::OsStr::new("GITLAB_HOST")],
            Some("checkout.example".into())
        );
    }
    assert!(
        forge
            .get_repository_clone_urls(CloneUrlsInput {
                checkout: CheckoutInput {
                    cwd: "unused".into(),
                    context: None
                },
                repository: "a/b".into(),
                host: Some("checkout.example".into())
            })
            .await
            .is_err()
    );
    assert_eq!(cli.0.lock().unwrap().len(), 2);
}
