use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use t3_forge::{
    FailureReason, Forge,
    forge::*,
    github::{GitHub, transport::*},
    model::*,
    rate_limit::RateLimits,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    body: Value,
    headers: String,
    raw_body: String,
}
struct Answer {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}
fn answer(body: Value) -> Answer {
    Answer {
        status: 200,
        body: body.to_string(),
        headers: vec![],
    }
}
async fn server_with_credentials(
    answers: Vec<Answer>,
    credentials: Arc<dyn Credentials>,
) -> (GitHubHttp, String, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for answer in answers {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            let header_end = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let first: Vec<_> = headers.lines().next().unwrap().split_whitespace().collect();
            requests.push(Request {
                method: first[0].into(),
                path: first[1].into(),
                body: serde_json::from_slice(&bytes[header_end..header_end + length])
                    .unwrap_or(Value::Null),
                headers,
                raw_body: String::from_utf8(bytes[header_end..header_end + length].to_vec())
                    .unwrap(),
            });
            let extras = answer
                .headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>();
            socket.write_all(format!("HTTP/1.1 {} Answer\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{}\r\n{}",answer.status,answer.body.len(),extras,answer.body).as_bytes()).await.unwrap();
        }
        requests
    });
    let http = GitHubHttp::new(credentials, Arc::new(RateLimits::default()))
        .unwrap()
        .with_loopback_endpoint(&endpoint)
        .unwrap();
    (http, endpoint, task)
}
async fn server_http(answers: Vec<Answer>) -> (GitHubHttp, String, JoinHandle<Vec<Request>>) {
    let credentials = Arc::new(GitHubCredentials::new(CredentialSettings {
        tokens: BTreeMap::from([("github.com".into(), "fixture-token".into())]),
        ..Default::default()
    }));
    server_with_credentials(answers, credentials).await
}
async fn server(answers: Vec<Answer>) -> (GitHub, JoinHandle<Vec<Request>>) {
    let (http, _, task) = server_http(answers).await;
    (GitHub::new(http).unwrap(), task)
}
fn pr() -> PullRequestInput {
    PullRequestInput {
        repo: ProviderRepositoryRef {
            cwd: ".".into(),
            host: "github.com".into(),
            repository: "acme/web".into(),
        },
        number: 7,
    }
}
fn core() -> Value {
    serde_json::from_str(include_str!("fixtures/core.json")).unwrap()
}
fn action(action: PullRequestAction) -> ActionInput {
    ActionInput {
        pr: pr(),
        action,
        stack_number: None,
        expected_stack_heads: None,
        remove_agent_credits_on_merge: false,
        merge_method: None,
        update_method: None,
    }
}

fn isolated_git(cwd: &std::path::Path) -> t3_git::Git {
    t3_git::Git::new(cwd).with_environment(
        [
            ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
            (
                "GIT_CONFIG_GLOBAL".into(),
                Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
            ),
            ("GIT_AUTHOR_NAME".into(), Some("Fixture".into())),
            (
                "GIT_AUTHOR_EMAIL".into(),
                Some("fixture@example.invalid".into()),
            ),
            ("GIT_COMMITTER_NAME".into(), Some("Fixture".into())),
            (
                "GIT_COMMITTER_EMAIL".into(),
                Some("fixture@example.invalid".into()),
            ),
        ]
        .into(),
    )
}

#[tokio::test]
async fn repository_service_looks_up_publishes_empty_and_retries_ssh_as_https() {
    use t3_contracts::source_control::*;
    use t3_forge::{registry::ForgeRegistry, services::SourceControlRepositoryService};
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("local");
    let remote = root.path().join("remote.git");
    tokio::fs::create_dir(&local).await.unwrap();
    tokio::fs::create_dir(&remote).await.unwrap();
    isolated_git(&remote)
        .execute(
            &["init", "--bare", "--initial-branch=main"],
            Default::default(),
        )
        .await
        .unwrap();
    let driver = isolated_git(&local);
    driver
        .execute(&["init", "--initial-branch=main"], Default::default())
        .await
        .unwrap();
    let file_url = url::Url::from_directory_path(&remote).unwrap().to_string();
    driver
        .write_config_value(
            &format!("url.{file_url}.insteadOf"),
            "https://github.com/acme/web",
        )
        .await
        .unwrap();
    let reject = root.path().join("reject.sh");
    tokio::fs::write(
        &reject,
        "printf '%s\\n' 'Permission denied (publickey).' >&2\nexit 255\n",
    )
    .await
    .unwrap();
    let driver = driver.with_environment(
        [
            ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
            (
                "GIT_CONFIG_GLOBAL".into(),
                Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
            ),
            ("GIT_AUTHOR_NAME".into(), Some("Fixture".into())),
            (
                "GIT_AUTHOR_EMAIL".into(),
                Some("fixture@example.invalid".into()),
            ),
            ("GIT_COMMITTER_NAME".into(), Some("Fixture".into())),
            (
                "GIT_COMMITTER_EMAIL".into(),
                Some("fixture@example.invalid".into()),
            ),
            ("GIT_SSH_VARIANT".into(), Some("ssh".into())),
            (
                "GIT_SSH_COMMAND".into(),
                Some(format!("sh '{}'", reject.to_string_lossy().replace('\\', "/")).into()),
            ),
        ]
        .into(),
    );
    let recorded = json!({"full_name":"acme/web","html_url":"https://github.com/acme/web","ssh_url":"git@github.com:acme/web.git"});
    let (github, task) = server(vec![
        answer(recorded.clone()),
        answer(recorded.clone()),
        answer(recorded),
    ])
    .await;
    let registry = Arc::new(ForgeRegistry::new([Arc::new(github) as Arc<dyn Forge>]));
    let service = SourceControlRepositoryService::new(registry, driver.clone(), local.clone());
    let found = service
        .lookup_repository(
            serde_json::from_value(json!({"provider":"github","repository":"acme/web"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(found.name_with_owner.0, "acme/web");
    assert_eq!(found.provider.0, "github");
    let input: SourceControlPublishRepositoryInput = serde_json::from_value(
        json!({"cwd":local,"provider":"github","repository":"web","visibility":"private"}),
    )
    .unwrap();
    let empty = service.publish_repository(input.clone()).await.unwrap();
    assert_eq!(empty.status, SourceControlPublishStatus::RemoteAdded);
    assert_eq!(empty.remote_name.0, "origin");
    assert_eq!(empty.branch.0, "main");
    tokio::fs::write(local.join("tracked.txt"), "published\n")
        .await
        .unwrap();
    driver
        .execute(&["add", "--", "tracked.txt"], Default::default())
        .await
        .unwrap();
    driver
        .execute(&["commit", "-m", "publish"], Default::default())
        .await
        .unwrap();
    let published = service.publish_repository(input).await.unwrap();
    assert_eq!(published.status, SourceControlPublishStatus::Pushed);
    assert_eq!(published.branch.0, "main");
    assert_eq!(published.remote_url.0, "https://github.com/acme/web");
    assert_eq!(
        driver
            .read_config_value("remote.origin.url")
            .await
            .unwrap()
            .as_deref(),
        Some("https://github.com/acme/web")
    );
    let contents = isolated_git(&remote)
        .execute(
            &["--git-dir=.", "show", "main:tracked.txt"],
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(contents.stdout_text(), "published\n");
    let requests = task.await.unwrap();
    assert_eq!(requests[0].path, "/repos/acme/web");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/user/repos");
    assert_eq!(requests[2].body, json!({"name":"web","private":true}));
}

#[tokio::test]
async fn review_host_resolves_a_pr_through_the_real_git_workflow_service() {
    use t3_forge::{
        registry::ForgeHandle,
        review_host::{ForgeReviewHost, ReviewSetup},
    };
    struct Setup;
    #[async_trait::async_trait]
    impl ReviewSetup for Setup {
        async fn setup(
            &self,
            _cwd: &std::path::Path,
            _thread: &str,
            _worktree: &std::path::Path,
        ) -> std::result::Result<(), t3_git::GitError> {
            panic!("resolve must not run project setup");
        }
    }
    let root = tempfile::tempdir().unwrap();
    let driver = isolated_git(root.path());
    driver
        .execute(&["init", "--initial-branch=main"], Default::default())
        .await
        .unwrap();
    let (github, task) = server(vec![answer(core())]).await;
    let host = ForgeReviewHost::new(
        ForgeHandle {
            forge: Arc::new(github),
            context: None,
        },
        Arc::new(Setup),
    );
    let workflow = t3_git::GitWorkflowService::new(driver).with_review_host(Arc::new(host));
    let result = workflow
        .resolve_pull_request("https://github.com/acme/web/pull/7")
        .await
        .unwrap();
    assert_eq!(result.pull_request.number, 7);
    assert_eq!(
        result.pull_request.url,
        "https://github.com/acme/web/pull/7"
    );
    let requests = task.await.unwrap();
    assert_eq!(requests[0].body["variables"]["owner"], "acme");
    assert_eq!(requests[0].body["variables"]["name"], "web");
    assert_eq!(requests[0].body["variables"]["number"], 7);
}
#[tokio::test]
async fn app_sign_in_polls_saves_and_reads_its_own_viewer() {
    use t3_forge::github::app::{AppSessionStore, AppSignIn};
    let (http,endpoint,task)=server_http(vec![
        answer(json!({"device_code":"device-1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0})),
        answer(json!({"error":"authorization_pending"})),
        answer(json!({"error":"slow_down", "interval":0})),
        answer(json!({"access_token":"ghu_new","expires_in":28800,"refresh_token":"ghr_new","refresh_token_expires_in":15897600})),
        answer(json!({"login":"octocat"})),
        answer(json!({"login":"octocat"})),
    ]).await;
    let root = tempfile::tempdir().unwrap();
    let settings_path = root.path().join("settings.json");
    let secrets = t3_config::secrets::SecretStore::new(root.path().join("secrets"));
    let store = Arc::new(t3_forge::github::session_store::SettingsAppSessions::new(
        t3_config::settings::SettingsService::new(&settings_path),
        secrets.clone(),
    ));
    let app = Arc::new(
        AppSignIn::new("Iv-test-client".into(), store.clone())
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    let github = GitHub::new(http).unwrap().with_app_sign_in(app);
    let sign_in = t3_forge::services::SourceControlSignIn::new(Arc::new(
        t3_forge::registry::ForgeRegistry::new([Arc::new(github) as Arc<dyn Forge>]),
    ));
    let prompt = sign_in
        .start(serde_json::from_value(json!({"provider":"github","host":" GITHUB.COM "})).unwrap())
        .await
        .unwrap();
    assert_eq!(prompt.user_code.0, "ABCD-1234");
    assert_eq!(prompt.verification_uri.0, "https://github.com/login/device");
    let result = sign_in
        .wait(serde_json::from_value(json!({"provider":"github","host":"github.com"})).unwrap())
        .await
        .unwrap();
    assert_eq!(result.account.0, "octocat");
    sign_in
        .cancel(serde_json::from_value(json!({"provider":"github","host":"github.com"})).unwrap())
        .await
        .unwrap();
    assert_eq!(
        sign_in
            .wait(serde_json::from_value(json!({"provider":"github","host":"github.com"})).unwrap())
            .await
            .unwrap()
            .account
            .0,
        "octocat"
    );
    let saved = store.read("github.com").await.unwrap().unwrap();
    assert_eq!(saved.access_token, "ghu_new");
    assert_eq!(saved.refresh_token.as_deref(), Some("ghr_new"));
    let persisted = std::fs::read_to_string(&settings_path).unwrap();
    assert!(!persisted.contains(&saved.access_token));
    assert!(!persisted.contains(saved.refresh_token.as_deref().unwrap()));
    let reopened = t3_forge::github::session_store::SettingsAppSessions::new(
        t3_config::settings::SettingsService::new(&settings_path),
        secrets,
    );
    assert_eq!(
        reopened
            .read("github.com")
            .await
            .unwrap()
            .unwrap()
            .access_token,
        saved.access_token
    );
    let requests = task.await.unwrap();
    let form: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(requests[1].raw_body.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(requests[0].path, "/login/device/code");
    assert_eq!(requests.len(), 6);
    assert!(
        requests[1..4]
            .iter()
            .all(|r| r.path == "/login/oauth/access_token")
    );
    assert_eq!(form["device_code"], "device-1");
    assert_eq!(
        form["grant_type"],
        "urn:ietf:params:oauth:grant-type:device_code"
    );
    assert!(requests[4].headers.contains("Bearer ghu_new"));
}
#[tokio::test]
async fn app_sign_in_refuses_unsafe_links_denial_expiry_and_cancellation_without_saving() {
    use t3_forge::github::app::{AppSessionStore, AppSignIn, MemoryAppSessions};
    for uri in [
        "javascript:private-token",
        "file:///private-token",
        "https://private-token@github.com/login/device",
    ] {
        let (_,endpoint,task)=server_http(vec![answer(json!({"device_code":"private-device","user_code":"code","verification_uri":uri,"expires_in":900,"interval":0}))]).await;
        let store = Arc::new(MemoryAppSessions::default());
        let app = Arc::new(
            AppSignIn::new("public-client".into(), store.clone())
                .unwrap()
                .with_loopback_endpoint(&endpoint)
                .unwrap(),
        );
        let error = app.start("github.com").await.unwrap_err();
        assert!(!format!("{error:?}").contains("private-"));
        assert!(store.read("github.com").await.unwrap().is_none());
        task.await.unwrap();
    }
    for (expires, denial) in [(900, true), (0, false)] {
        let mut replies = vec![answer(
            json!({"device_code":"private-device","user_code":"code","verification_uri":"https://github.com/login/device","expires_in":expires,"interval":0}),
        )];
        if denial {
            replies.push(answer(
                json!({"error":"access_denied","error_description":"private-denial"}),
            ));
        }
        let (_, endpoint, task) = server_http(replies).await;
        let store = Arc::new(MemoryAppSessions::default());
        let app = Arc::new(
            AppSignIn::new("public-client".into(), store.clone())
                .unwrap()
                .with_loopback_endpoint(&endpoint)
                .unwrap(),
        );
        app.start("github.com").await.unwrap();
        let error = app.wait("github.com").await.unwrap_err();
        assert!(!format!("{error:?}").contains("private-"));
        assert!(store.read("github.com").await.unwrap().is_none());
        task.await.unwrap();
        app.shutdown().await;
    }
    let (_,endpoint,task)=server_http(vec![answer(json!({"device_code":"private-device","user_code":"code","verification_uri":"https://github.com/login/device","expires_in":900,"interval":3600}))]).await;
    let store = Arc::new(MemoryAppSessions::default());
    let app = Arc::new(
        AppSignIn::new("public-client".into(), store.clone())
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    assert!(app.start("evil.example").await.is_err());
    app.start("github.com").await.unwrap();
    app.cancel("github.com").await;
    app.cancel("github.com").await;
    assert!(app.wait("github.com").await.is_err());
    assert!(store.read("github.com").await.unwrap().is_none());
    task.await.unwrap();
    app.shutdown().await;
    assert!(app.start("github.com").await.is_err());
}
#[tokio::test]
async fn app_refresh_failure_retains_an_unexpired_token_but_never_a_rejected_one() {
    use t3_forge::github::app::{AppSession, AppSessionStore, AppSignIn, MemoryAppSessions};
    let (http, endpoint, task) = server_http(
        (0..3)
            .map(|_| Answer {
                status: 500,
                body: String::new(),
                headers: vec![],
            })
            .collect(),
    )
    .await;
    drop(http);
    let sessions = Arc::new(MemoryAppSessions::default());
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    sessions
        .write(
            "github.com",
            AppSession {
                access_token: "still-valid".into(),
                access_token_expires_at: Some(expires),
                refresh_token: Some("refresh".into()),
                refresh_token_expires_at: None,
            },
        )
        .await
        .unwrap();
    let app = AppSignIn::new("Iv-test-client".into(), sessions.clone())
        .unwrap()
        .with_loopback_endpoint(&endpoint)
        .unwrap();
    let token = app.token("github.com").await.unwrap().unwrap();
    app.invalidate("github.com", &token).await.unwrap();
    assert!(app.token("github.com").await.is_err());
    assert!(app.token("github.com").await.is_err());
    assert_eq!(
        sessions
            .read("github.com")
            .await
            .unwrap()
            .unwrap()
            .access_token,
        "still-valid"
    );
    assert_eq!(task.await.unwrap().len(), 3);
}

#[tokio::test]
async fn app_refresh_replaces_expiring_tokens_and_coverage_distinguishes_installations() {
    use t3_contracts::source_control::SourceControlAppAccessStatus;
    use t3_forge::github::app::{AppSession, AppSessionStore, AppSignIn, MemoryAppSessions};
    let (http,endpoint,task)=server_http(vec![
        answer(json!({"access_token":"ghu_refreshed","expires_in":28800,"refresh_token":"ghr_refreshed","refresh_token_expires_in":15897600})),
        answer(json!({"installations":[{"id":1,"repository_selection":"all","account":{"login":"pingdotgg"}},{"id":2,"repository_selection":"selected","account":{"login":"acme"}}]})),
        answer(json!({"repositories":[{"full_name":"acme/one"}]})),
    ]).await;
    let store = Arc::new(MemoryAppSessions::default());
    store
        .write(
            "github.com",
            AppSession {
                access_token: "ghu_current".into(),
                access_token_expires_at: Some(0),
                refresh_token: Some("ghr_current".into()),
                refresh_token_expires_at: None,
            },
        )
        .await
        .unwrap();
    let app = Arc::new(
        AppSignIn::new("Iv-test-client".into(), store.clone())
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    let github = GitHub::new(http).unwrap().with_app_sign_in(app);
    assert_eq!(
        github
            .sign_in_access(
                "github.com".into(),
                "https://github.com/PingDotGG/t3code.git".into()
            )
            .await
            .unwrap(),
        SourceControlAppAccessStatus::Covered
    );
    assert_eq!(
        github
            .sign_in_access("github.com".into(), "acme/one".into())
            .await
            .unwrap(),
        SourceControlAppAccessStatus::Covered
    );
    assert_eq!(
        github
            .sign_in_access("github.com".into(), "acme/two".into())
            .await
            .unwrap(),
        SourceControlAppAccessStatus::NotCovered
    );
    assert_eq!(
        store
            .read("github.com")
            .await
            .unwrap()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("ghr_refreshed")
    );
    let requests = task.await.unwrap();
    assert!(requests[0].raw_body.contains("grant_type=refresh_token"));
    assert!(requests[1].headers.contains("Bearer ghu_refreshed"));
    assert_eq!(
        requests[2].path,
        "/user/installations/2/repositories?per_page=100&page=1"
    );
}
#[tokio::test]
async fn media_drops_credentials_on_redirect_and_preserves_ranges() {
    let (github, task) = server(vec![
        Answer {
            status: 302,
            body: String::new(),
            headers: vec![(
                "location".into(),
                "https://objects.example.com/signed?jwt=fixture".into(),
            )],
        },
        Answer {
            status: 206,
            body: "media-bytes".into(),
            headers: vec![("content-type".into(), "video/mp4".into())],
        },
    ])
    .await;
    let response = github
        .fetch_media(MediaRequest {
            url: "https://github.com/user-attachments/assets/1a1842fb".into(),
            headers: BTreeMap::from([("range".into(), "bytes=0-1".into())]),
            repository: None,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"media-bytes");
    let requests = task.await.unwrap();
    assert!(requests[0].headers.contains("Bearer fixture-token"));
    assert!(
        !requests[1]
            .headers
            .to_lowercase()
            .contains("authorization:")
    );
    assert!(requests[1].headers.contains("range: bytes=0-1"));
    assert!(requests[1].headers.contains("accept-encoding: identity"));
}
#[tokio::test]
async fn event_stream_falls_back_to_personal_and_deletes_subscription_when_dropped() {
    use futures_util::StreamExt;
    use t3_forge::github::app::{AppSession, AppSessionStore, AppSignIn, MemoryAppSessions};
    let credentials = Arc::new(GitHubCredentials::new(CredentialSettings {
        environment: BTreeMap::from([("GH_TOKEN".into(), "user-token".into())]),
        ..Default::default()
    }));
    let (http,endpoint,task)=server_with_credentials(vec![
        Answer{status:201,body:json!({"id":1,"events_url":"https://api.github.com/event-subscriptions/1/events"}).to_string(),headers:vec![]},
        Answer{status:404,body:String::new(),headers:vec![]},
        Answer{status:200,body:"event: com.github.pull_request.head.changed.v1\ndata: {}\n\nevent: github.stream.end\ndata: {\"outcome\":\"reconnect\",\"reason\":\"shutdown\"}\n\n".into(),headers:vec![("content-type".into(),"text/event-stream".into())]},
        Answer{status:204,body:String::new(),headers:vec![]},
    ],credentials).await;
    let sessions = Arc::new(MemoryAppSessions::default());
    sessions
        .write(
            "github.com",
            AppSession {
                access_token: "app-token".into(),
                access_token_expires_at: None,
                refresh_token: None,
                refresh_token_expires_at: None,
            },
        )
        .await
        .unwrap();
    let app = Arc::new(
        AppSignIn::new("Iv-test-client".into(), sessions)
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    let github = GitHub::new(http).unwrap().with_app_sign_in(app);
    let mut events = github.change_request_events(pr()).await.unwrap();
    assert_eq!(events.next().await, Some(()));
    assert_eq!(events.next().await, Some(()));
    drop(events);
    github.shutdown().await;
    let requests = task.await.unwrap();
    assert_eq!(
        requests[0].body["resource_url"],
        "https://github.com/acme/web/pull/7"
    );
    assert_eq!(requests[0].body["event_types"].as_array().unwrap().len(), 5);
    assert!(requests[1].headers.contains("Bearer app-token"));
    assert!(requests[2].headers.contains("Bearer user-token"));
    assert_eq!(requests[3].method, "DELETE");
    assert_eq!(requests[3].path, "/event-subscriptions/1");
}
#[tokio::test]
async fn stack_merge_checks_reviewed_heads_and_submits_one_atomic_queued_merge() {
    let stack = json!([{"number":50,"url":"https://api.github.com/repos/acme/web/stacks/50","base":{"ref":"main"},"pull_requests":[{"number":1,"title":"Base","head":{"ref":"base","sha":"aaa"},"state":"closed","merged_at":"2026-01-01T00:00:00Z"},{"number":2,"title":"Middle","head":{"ref":"middle","sha":"bbb"},"state":"open","draft":false},{"number":3,"title":"Top","head":{"ref":"top","sha":"ccc"},"state":"open","draft":false}]}]);
    let (github, task) = server(vec![
        answer(stack.clone()),
        answer(json!({"status":"enqueued","details":{}})),
        answer(stack),
    ])
    .await;
    let mut input = action(PullRequestAction::Merge);
    input.pr.number = 3;
    input.stack_number = Some(50);
    input.merge_method = Some(PullRequestMergeMethod::Squash);
    input.expected_stack_heads = Some(
        serde_json::from_value(json!([{"number":2,"headSha":"bbb"},{"number":3,"headSha":"ccc"}]))
            .unwrap(),
    );
    assert!(github.run_action(input.clone()).await.unwrap().is_none());
    input.expected_stack_heads = Some(
        serde_json::from_value(
            json!([{"number":2,"headSha":"bbb"},{"number":3,"headSha":"changed"}]),
        )
        .unwrap(),
    );
    assert!(matches!(
        github.run_action(input).await.unwrap_err(),
        t3_forge::ForgeError::Stack {
            number: 3,
            completed: 0,
            ..
        }
    ));
    let requests = task.await.unwrap();
    assert_eq!(requests[1].path, "/repos/acme/web/pulls/3/merge-async");
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(
        requests[1].body,
        json!({"merge_method":"squash","merge_action":"default","sha":"ccc"})
    );
}
#[tokio::test]
async fn stack_rebase_batches_layer_permissions_and_refuses_before_running_git() {
    let stack = json!([{"number":50,"url":"https://api.github.com/repos/acme/web/stacks/50","base":"main","pull_requests":[{"number":2,"head":{"ref":"middle","sha":"bbb"},"state":"open","draft":false},{"number":3,"head":{"ref":"top","sha":"ccc"},"state":"open","draft":false}]}]);
    let (github,task) = server(vec![answer(stack),answer(json!({"data":{"t3_0":{"pullRequest":{"headRepository":{"viewerPermission":"WRITE"},"maintainerCanModify":false}},"t3_1":{"pullRequest":{"headRepository":{"viewerPermission":"READ"},"maintainerCanModify":false}}}}))]).await;
    let mut input = action(PullRequestAction::UpdateBranch);
    input.pr.number = 3;
    input.stack_number = Some(50);
    input.expected_stack_heads = Some(
        serde_json::from_value(json!([{"number":2,"headSha":"bbb"},{"number":3,"headSha":"ccc"}]))
            .unwrap(),
    );
    let failure = github.run_action(input).await.unwrap_err();
    assert!(
        matches!(
            failure,
            t3_forge::ForgeError::Stack {
                number: 3,
                completed: 0,
                ..
            }
        ),
        "{failure:?}"
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["operationName"], "T3Batch");
    assert_eq!(requests[1].body["variables"]["number_0"], 2);
    assert_eq!(requests[1].body["variables"]["number_1"], 3);
}

#[tokio::test]
async fn sso_media_renders_in_repository_context_and_caches_signed_urls() {
    let signed =
        "https://private-user-images.githubusercontent.com/1/upload.mp4?jwt=account-a-1&x=1";
    let (github, task) = server(vec![
        Answer {
            status: 200,
            body: "<html>Sign in with SSO</html>".into(),
            headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
        },
        Answer {
            status: 200,
            body: format!(
                "<p><img src=\"{}\" alt=\"\"></p>",
                signed.replace('&', "&amp;")
            ),
            headers: vec![],
        },
        Answer {
            status: 200,
            body: "first".into(),
            headers: vec![("content-type".into(), "video/mp4".into())],
        },
        Answer {
            status: 206,
            body: "range".into(),
            headers: vec![("content-type".into(), "video/mp4".into())],
        },
    ])
    .await;
    let request = MediaRequest {
        url: "https://github.com/user-attachments/assets/explicit-pr-context".into(),
        headers: BTreeMap::new(),
        repository: Some("enterprise/private".into()),
    };
    assert_eq!(
        github
            .fetch_media(request.clone())
            .await
            .unwrap()
            .unwrap()
            .body,
        b"first"
    );
    let mut range = request;
    range.headers.insert("range".into(), "bytes=2-5".into());
    assert_eq!(
        github.fetch_media(range).await.unwrap().unwrap().body,
        b"range"
    );
    let requests = task.await.unwrap();
    assert_eq!(requests[1].body["context"], "enterprise/private");
    assert_eq!(requests[1].body["mode"], "gfm");
    assert_eq!(
        requests[1].body["text"],
        "![](https://github.com/user-attachments/assets/explicit-pr-context)"
    );
    assert_eq!(requests[2].path, "/1/upload.mp4?jwt=account-a-1&x=1");
    assert!(
        !requests[2]
            .headers
            .to_lowercase()
            .contains("authorization:")
    );
    assert!(
        !requests[3]
            .headers
            .to_lowercase()
            .contains("authorization:")
    );
}
#[tokio::test]
async fn detail_and_permissions_read_the_copied_core_fixture() {
    let (github, task) = server(vec![answer(core()), answer(json!({"workflow_runs":[]}))]).await;
    let detail = github.get_change_request(pr()).await.unwrap();
    assert_eq!(detail.number, 7);
    assert_eq!(
        detail.base_comparison,
        Some(PullRequestBaseComparison::Behind)
    );
    assert_eq!(detail.behind_by, Some(2));
    assert!(
        detail
            .viewer_permissions
            .actions
            .contains(&PullRequestAction::UpdateBranch)
    );
    assert!(!detail.merge_capabilities.squash);
    let requests = task.await.unwrap();
    assert!(
        requests[0].body["query"]
            .as_str()
            .unwrap()
            .starts_with("query PullRequestCore(")
    );
    assert_eq!(requests[0].body["variables"]["headRef"], "refs/pull/7/head");
    assert!(requests[0].headers.contains("Bearer fixture-token"));
    assert!(
        requests[0]
            .headers
            .contains("x-github-api-version: 2022-11-28")
    );
}
#[tokio::test]
async fn summary_batch_isolates_a_missing_pull_request() {
    let repository = core()["data"]["repository"].clone();
    let (github,task)=server(vec![answer(json!({"data":{"t3_0":repository,"t3_1":null},"errors":[{"type":"NOT_FOUND","path":["t3_1","pullRequest"]}]}))]).await;
    let mut missing = pr();
    missing.number = 8;
    let (found, missing) = tokio::join!(
        github.get_change_request_summary(pr()),
        github.get_change_request_summary(missing)
    );
    let found = found.unwrap();
    assert_eq!(found.number, 7);
    assert_eq!(found.title, "Pull request 7");
    assert_eq!(found.head_branch, "feature");
    assert_eq!(missing.unwrap_err().reason(), Some(FailureReason::NotFound));
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["operationName"], "T3Batch");
    assert_eq!(requests[0].body["variables"]["number_0"], 7);
    assert_eq!(requests[0].body["variables"]["number_1"], 8);
    let query = requests[0].body["query"].as_str().unwrap();
    assert!(query.contains("t3_0: repository(owner: $owner_0, name: $name_0)"));
    assert!(query.contains("t3_1: repository(owner: $owner_1, name: $name_1)"));
}
#[tokio::test]
async fn stats_chunk_26_reads_and_preserve_recorded_typed_values() {
    let mut rows = serde_json::Map::new();
    for index in 0..25 {
        rows.insert(format!("t3_{index}"),json!({"pullRequest":if index==0 {json!({"additions":4,"deletions":1})} else {Value::Null}}));
    }
    let (github, task) = server(vec![
        answer(json!({"data":rows})),
        answer(json!({"data":{"repository":{"pullRequest":{"additions":4,"deletions":1}}}})),
    ])
    .await;
    let stats = github
        .list_change_request_stats(StatsInput {
            cwd: ".".into(),
            host: "github.com".into(),
            change_requests: (1..=26)
                .map(|number| ChangeRequestKey {
                    repository: "acme/web".into(),
                    number,
                })
                .collect(),
        })
        .await
        .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].repository, "acme/web");
    assert_eq!(stats[0].number, 1);
    assert_eq!(stats[0].additions, 4);
    assert_eq!(stats[0].deletions, 1);
    assert_eq!(stats[1].repository, "acme/web");
    assert_eq!(stats[1].number, 26);
    assert_eq!(stats[1].additions, 4);
    assert_eq!(stats[1].deletions, 1);
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn refused_reviewers_retry_without_teams_and_missing_pull_refs_preserve_the_detail() {
    let mut refused = core();
    refused["errors"] = json!([{"type":"FORBIDDEN","path":["repository","pullRequest","reviewRequests","nodes",0,"requestedReviewer"]}]);
    let mut recovered = core();
    recovered["data"]["repository"]["pullRequest"]["baseRef"] = Value::Null;
    recovered["errors"] =
        json!([{"type":"NOT_FOUND","path":["repository","pullRequest","baseRef","compare"]}]);
    let (github, task) = server(vec![
        answer(refused),
        answer(recovered.clone()),
        answer(json!({"workflow_runs":[]})),
        answer(recovered),
        answer(json!({"workflow_runs":[]})),
    ])
    .await;
    for _ in 0..2 {
        let detail = github.get_change_request(pr()).await.unwrap();
        assert_eq!(detail.number, 7);
        assert_eq!(
            detail.base_comparison,
            Some(PullRequestBaseComparison::Unknown)
        );
        assert_eq!(detail.title, "Pull request 7");
    }
    let requests = task.await.unwrap();
    assert_eq!(requests[0].body["variables"]["requestedReviewers"], 100);
    assert_eq!(requests[1].body["variables"]["requestedReviewers"], 0);
    assert_eq!(requests[3].body["variables"]["requestedReviewers"], 0);
}
#[tokio::test]
async fn pinned_checks_revalidate_etags_and_reuse_typed_checks_on_304() {
    let sha = "a".repeat(40);
    let head = json!({"head":{"sha":sha,"repo":{"id":1}},"base":{"repo":{"id":1}}});
    let mut detail = core();
    detail["data"]["repository"]["pullRequest"]["headRefOid"] = json!(sha);
    detail["data"]["repository"]["pullRequest"]["commits"] = json!({"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"nodes":[{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/acme/web/actions/runs/1","checkSuite":{"workflowRun":{"workflow":{"name":"CI"}}}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]});
    let etag = |body: Value, value: &str| Answer {
        status: 200,
        body: body.to_string(),
        headers: vec![("etag".into(), value.into())],
    };
    let unchanged = || Answer {
        status: 304,
        body: String::new(),
        headers: vec![],
    };
    let (github, task) = server(vec![
        answer(json!({"id":42,"login":"octocat"})),
        etag(head, "head"),
        etag(json!({"check_runs":[]}), "runs"),
        etag(json!({"statuses":[]}), "statuses"),
        answer(detail),
        answer(json!({"workflow_runs":[]})),
        unchanged(),
        unchanged(),
        unchanged(),
    ])
    .await;
    let verified = github
        .with_verified_credential(VerifiedCredentialInput {
            cwd: ".".into(),
            host: "github.com".into(),
            repository: None,
        })
        .await
        .unwrap();
    for _ in 0..2 {
        let result = verified
            .forge
            .get_change_request_checks(pr())
            .await
            .unwrap();
        assert_eq!(result.state, PullRequestState::Open);
        assert_eq!(result.checks.len(), 1);
        assert_eq!(result.checks[0].name.0, "test");
        assert_eq!(result.checks[0].status, PullRequestCheckStatus::Success);
    }
    let requests = task.await.unwrap();
    assert!(requests[6].headers.contains("if-none-match: head"));
    assert!(
        requests[7].headers.contains("if-none-match: runs")
            || requests[7].headers.contains("if-none-match: statuses")
    );
    assert!(
        requests[8].headers.contains("if-none-match: runs")
            || requests[8].headers.contains("if-none-match: statuses")
    );
}
#[tokio::test]
async fn check_pages_refuse_a_changed_head() {
    let first: Value =
        serde_json::from_str(include_str!("fixtures/core-paged-checks.json")).unwrap();
    let second = json!({"data":{"repository":{"pullRequest":{"headRefOid":"different","commits":{"nodes":[]}}}}});
    let (github, task) = server(vec![answer(first), answer(second)]).await;
    assert!(github.get_change_request(pr()).await.is_err());
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn reactions_and_comment_edits_verify_subject_scope_before_writing() {
    let scope = json!({"data":{"repository":{"pullRequest":{"id":"PR1"}},"node":{"id":"C1","pullRequest":{"id":"PR2"}}}});
    let (github, task) = server(vec![answer(scope)]).await;
    let error = github
        .update_comment(UpdateCommentInput {
            pr: pr(),
            comment_id: "C1".into(),
            kind: PullRequestCommentUpdateInputKind::IssueComment,
            body: "edit".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Forbidden));
    assert_eq!(task.await.unwrap().len(), 1);
}
#[tokio::test]
async fn comment_reads_the_pr_id_then_uses_the_graphql_mutation() {
    let (github, task) = server(vec![
        answer(json!({"data":{"repository":{"pullRequest":{"id":"PR1"}}}})),
        answer(json!({"data":{"addComment":{"clientMutationId":null}}})),
    ])
    .await;
    github
        .comment(CommentInput {
            pr: pr(),
            body: "hello".into(),
        })
        .await
        .unwrap();
    let r = task.await.unwrap();
    assert_eq!(
        r[1].body["variables"],
        json!({"subjectId":"PR1","body":"hello"})
    );
    assert!(
        r[1].body["query"]
            .as_str()
            .unwrap()
            .starts_with("mutation AddComment(")
    );
}
#[tokio::test]
async fn merge_uses_current_head_and_selects_queue_auto_merge() {
    let(github,task)=server(vec![answer(json!({"data":{"repository":{"pullRequest":{"id":"PR1","headRefOid":"abc","isMergeQueueEnabled":true,"mergeStateStatus":"BLOCKED"}}}})),answer(json!({"data":{"enablePullRequestAutoMerge":{"pullRequest":null}}}))]).await;
    assert!(
        github
            .run_action(action(PullRequestAction::Merge))
            .await
            .unwrap()
            .is_none()
    );
    let r = task.await.unwrap();
    assert!(
        r[1].body["query"]
            .as_str()
            .unwrap()
            .starts_with("mutation EnablePullRequestAutoMerge(")
    );
    assert_eq!(r[1].body["variables"]["input"]["mergeMethod"], "MERGE");
}
#[tokio::test]
async fn delete_branch_refuses_a_moved_head() {
    let(github,task)=server(vec![answer(json!({"data":{"repository":{"pullRequest":{"state":"MERGED","headRefName":"feature","baseRefName":"main","headRefOid":"old","headRepository":{"id":"R1"}}}}})),answer(json!({"data":{"node":{"__typename":"Repository","defaultBranchRef":{"name":"main"},"ref":{"target":{"oid":"new"}}}}}))]).await;
    assert!(
        github
            .run_action(action(PullRequestAction::DeleteBranch))
            .await
            .is_err()
    );
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn reviewer_requests_and_labels_use_rest_with_exact_bodies() {
    let (github, task) = server(vec![answer(json!({})), answer(json!({}))]).await;
    github
        .set_reviewer_request(ReviewerRequestInput {
            pr: pr(),
            reviewers: serde_json::from_value(
                json!([{"id":"octocat","kind":"user"},{"id":"reviewers","kind":"team"}]),
            )
            .unwrap(),
            requested: false,
        })
        .await
        .unwrap();
    github
        .set_labels(LabelsInput {
            pr: pr(),
            labels: vec!["needs review".into()],
            applied: false,
        })
        .await
        .unwrap();
    let r = task.await.unwrap();
    assert_eq!(r[0].method, "DELETE");
    assert_eq!(r[0].path, "/repos/acme/web/pulls/7/requested_reviewers");
    assert_eq!(
        r[0].body,
        json!({"reviewers":["octocat"],"team_reviewers":["reviewers"]})
    );
    assert_eq!(r[1].path, "/repos/acme/web/issues/7/labels/needs%20review");
}
#[tokio::test]
async fn viewed_files_are_batched_in_input_order() {
    let (github, task) = server(vec![
        answer(json!({"data":{"repository":{"pullRequest":{"id":"PR1"}}}})),
        answer(json!({"data":{"f0":{},"f1":{}}})),
    ])
    .await;
    github
        .set_files_viewed(SetFilesViewedInput {
            pr: pr(),
            files: vec![
                ViewedFile {
                    path: "a".into(),
                    viewed: true,
                },
                ViewedFile {
                    path: "a".into(),
                    viewed: false,
                },
            ],
        })
        .await
        .unwrap();
    let r = task.await.unwrap();
    let q = r[1].body["query"].as_str().unwrap();
    assert!(q.find("f0: markFileAsViewed").unwrap() < q.find("f1: unmarkFileAsViewed").unwrap());
    assert_eq!(r[1].body["variables"]["p0"], "a");
}
#[tokio::test]
async fn rest_and_graphql_limits_produce_typed_errors() {
    for (status, body) in [
        (429, json!({"message":"slow down"})),
        (
            200,
            json!({"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}),
        ),
    ] {
        let (github, task) = server(vec![Answer {
            status,
            body: body.to_string(),
            headers: vec![("retry-after".into(), "120".into())],
        }])
        .await;
        let err = github.get_change_request_preview(pr()).await.unwrap_err();
        assert_eq!(err.reason(), Some(FailureReason::RateLimited));
        assert_eq!(task.await.unwrap().len(), 1);
    }
}
#[tokio::test]
async fn raw_diff_falls_back_to_files_on_http_failure() {
    let(github,task)=server(vec![Answer{status:406,body:"too large".into(),headers:vec![]},answer(json!([{"filename":"new.rs","previous_filename":"old.rs","status":"renamed","additions":0,"deletions":0}]))]).await;
    let diff = github
        .get_diff(DiffInput {
            pr: pr(),
            cursor: None,
            commit: None,
        })
        .await
        .unwrap();
    assert!(!diff.truncated);
    assert!(
        diff.patch
            .contains("similarity index 100%\nrename from old.rs\nrename to new.rs")
    );
    let r = task.await.unwrap();
    assert_eq!(
        r[1].path,
        "/repos/acme/web/pulls/7/files?per_page=100&page=1"
    );
}
#[tokio::test]
async fn thread_tail_rejects_an_unrelated_pull_request() {
    let(github,task)=server(vec![answer(json!({"data":{"viewer":{"login":"me"},"repository":{"pullRequest":{"id":"PR1"}},"node":{"__typename":"PullRequestReviewThread","pullRequest":{"id":"PR2"},"comments":{"nodes":[]}}}}))]).await;
    assert_eq!(
        github
            .get_review_thread_comments(ThreadCommentsInput {
                pr: pr(),
                thread_id: "T1".into(),
                cursor: "next".into()
            })
            .await
            .unwrap_err()
            .reason(),
        Some(FailureReason::Forbidden)
    );
    task.await.unwrap();
}
#[tokio::test]
async fn create_pr_reads_body_file_and_sends_maintainer_edits() {
    let (github, task) = server(vec![answer(json!({}))]).await;
    let dir = tempfile::tempdir().unwrap();
    let body_file = dir.path().join("body.md");
    std::fs::write(&body_file, "description\n").unwrap();
    let github = github.with_repository_environment(None, Some("acme/web".into()));
    github
        .create_change_request(CreateChangeRequestInput {
            checkout: CheckoutInput {
                cwd: dir.path().into(),
                context: None,
            },
            source: None,
            target: None,
            base_ref_name: "main".into(),
            head_selector: "octocat:feature".into(),
            title: "New PR".into(),
            body_file,
        })
        .await
        .unwrap();
    let r = task.await.unwrap();
    assert_eq!(r[0].method, "POST");
    assert_eq!(
        r[0].body,
        json!({"base":"main","head":"octocat:feature","title":"New PR","body":"description\n","maintainer_can_modify":true})
    );
}

#[tokio::test]
async fn activity_pages_comments_and_preserves_thread_truncation() {
    let remark = |id: &str| json!({"id":id,"author":null,"body":id,"createdAt":"2026-07-01T00:00:00Z","lastEditedAt":null,"url":null,"isMinimized":false});
    let first = json!({"data":{"repository":{"pullRequest":{"author":null,"commits":{"nodes":[]},"comments":{"nodes":[remark("I1")],"pageInfo":{"hasNextPage":true,"endCursor":"comments-2"}},"reviews":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}});
    let second = json!({"data":{"repository":{"pullRequest":{"comments":{"nodes":[remark("I2")],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}});
    let mut thread: Value =
        serde_json::from_str(include_str!("fixtures/review-thread.json")).unwrap();
    thread["comments"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":"tail"});
    thread["comments"]["totalCount"] = json!(12);
    let threads = json!({"data":{"viewer":{"login":"viewer"},"repository":{"pullRequest":{"reviewThreads":{"nodes":[thread],"pageInfo":{"hasNextPage":false,"endCursor":null}},"reviewRequests":{"nodes":[]},"latestReviews":{"nodes":[]}}}}});
    let (github, task) = server(vec![answer(first), answer(second), answer(threads)]).await;
    let activity = github.get_change_request_activity(pr()).await.unwrap();
    assert_eq!(activity.comments.len(), 4);
    assert_eq!(activity.comment_count, 14);
    assert!(activity.comments_truncated);
    assert_eq!(activity.review_threads_truncated, Some(false));
    assert_eq!(
        activity.review_threads[0]
            .next_comments_cursor
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        "tail"
    );
    let r = task.await.unwrap();
    assert_eq!(r[1].body["variables"]["commentsAfter"], "comments-2");
    assert_eq!(r[1].body["variables"]["withReviews"], false);
}
#[tokio::test]
async fn activity_enriches_actors_commits_and_later_dismissal_reasons() {
    let first = json!({"data":{"repository":{"pullRequest":{"author":{"login":"julius"},"commits":{"nodes":[]},"comments":{"nodes":[],"pageInfo":{"hasNextPage":false}},"reviews":{"nodes":[{"id":"R1","author":{"login":"code-rabbit"},"body":"<!-- invisible -->","state":"DISMISSED","submittedAt":"2026-07-01T00:00:00Z","lastEditedAt":null,"url":null}],"pageInfo":{"hasNextPage":false}}}}}});
    let threads = json!({"data":{"viewer":{"login":"viewer"},"repository":{"pullRequest":{"author":{"login":"julius","avatarUrl":"https://avatars/j.png"},"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}},"reviewRequests":{"nodes":[]},"latestReviews":{"nodes":[]},"reviews":{"nodes":[{"id":"R1","author":{"login":"code-rabbit","__typename":"Bot","avatarUrl":"https://avatars/b.png"}}]},"reviewDismissals":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"dismissal-2"}},"commits":{"nodes":[{"commit":{"oid":"def456","messageHeadline":"Keep this commit","committedDate":"2026-07-05T00:00:00Z","authors":{"nodes":[{"name":"Julius","avatarUrl":"https://avatars/j.png","user":{"login":"julius"}}]}}}]}}}}});
    let dismissal = json!({"data":{"repository":{"pullRequest":{"timelineItems":{"nodes":[{"review":{"id":"R1"},"dismissalMessage":"No longer applies"}],"pageInfo":{"hasNextPage":false}}}}}});
    let (github, task) = server(vec![answer(first), answer(threads), answer(dismissal)]).await;
    let activity = github.get_change_request_activity(pr()).await.unwrap();
    assert_eq!(activity.comments[0].body, "No longer applies");
    assert_eq!(
        activity.author.flatten().unwrap().avatar_url.as_deref(),
        Some("https://avatars/j.png")
    );
    assert_eq!(activity.commits.len(), 1);
    assert_eq!(activity.commits[0].oid.0, "def456");
    assert_eq!(activity.commits[0].message_headline, "Keep this commit");
    assert_eq!(
        activity.comments[0].author.as_ref().unwrap().is_bot,
        Some(Some(true))
    );
    assert_eq!(
        task.await.unwrap()[2].body["variables"]["after"],
        "dismissal-2"
    );
}
#[tokio::test]
async fn thread_tail_skips_hidden_comments_and_carries_cursor() {
    let mut thread: Value =
        serde_json::from_str(include_str!("fixtures/review-thread.json")).unwrap();
    thread["comments"]["nodes"][0]["isMinimized"] = json!(true);
    thread["pullRequest"] = json!({"id":"PR1"});
    thread["__typename"] = json!("PullRequestReviewThread");
    let(github,task)=server(vec![answer(json!({"data":{"viewer":{"login":"viewer"},"repository":{"pullRequest":{"id":"PR1"}},"node":thread}}))]).await;
    let tail = github
        .get_review_thread_comments(ThreadCommentsInput {
            pr: pr(),
            thread_id: "T1".into(),
            cursor: "after".into(),
        })
        .await
        .unwrap();
    assert_eq!(tail.comments.len(), 1);
    assert_eq!(tail.hidden_count, Some(Some(1)));
    assert!(tail.next_cursor.is_none());
    task.await.unwrap();
}
#[tokio::test]
async fn raw_row_count_keeps_malformed_listing_pages_truncated() {
    let mut row = core()["data"]["repository"]["pullRequest"].clone();
    row["createdAt"] = json!("2026-07-01T00:00:00Z");
    let(github,task)=server(vec![answer(json!({"data":{"search":{"nodes":[row,{"number":"broken"}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}))]).await;
    let page = github
        .list_change_requests(ListInput {
            repo: pr().repo,
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::All,
            viewer: "me".into(),
            limit: 1,
            query: None,
            cursor: None,
            filters: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.truncated);
    assert!(page.continues);
    let r = task.await.unwrap();
    assert_eq!(r[0].body["variables"]["first"], 2);
    assert_eq!(
        r[0].body["variables"]["q"],
        "is:pr is:open sort:updated-desc repo:acme/web"
    );
}
#[tokio::test]
async fn viewed_file_repeated_cursor_is_truncated() {
    let page = json!({"data":{"repository":{"pullRequest":{"files":{"nodes":[{"path":"a.rs","viewerViewedState":"DISMISSED"}],"pageInfo":{"hasNextPage":true,"endCursor":"same"}}}}}});
    let (github, task) = server(vec![answer(page.clone()), answer(page)]).await;
    let result = github.get_files_viewed(pr()).await.unwrap();
    assert!(result.truncated);
    assert_eq!(result.files[0].state, PullRequestFileViewedState::Dismissed);
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn labels_keep_applied_values_missing_from_the_first_page() {
    let(github,task)=server(vec![answer(json!({"data":{"repository":{"labels":{"nodes":[{"name":"bug","color":"ff0000","description":null}],"pageInfo":{"hasNextPage":true}},"pullRequest":{"labels":{"nodes":[{"name":"legacy"}]}}}}}))]).await;
    let labels = github.list_label_candidates(pr()).await.unwrap();
    assert!(labels.truncated);
    assert_eq!(labels.candidates[0].name.0, "legacy");
    assert!(labels.candidates[0].is_applied);
    task.await.unwrap();
}
#[tokio::test]
async fn review_ranges_use_rest_and_omit_one_line_start() {
    let (github, task) = server(vec![answer(json!({}))]).await;
    let comments=serde_json::from_value(json!([{"path":"a.rs","body":"note","position":{"kind":"added","newLine":7},"startPosition":{"kind":"added","newLine":7}},{"path":"a.rs","body":"range","position":{"kind":"added","newLine":9},"startPosition":{"kind":"added","newLine":7}}])).unwrap();
    github
        .submit_review(ReviewInput {
            pr: pr(),
            verdict: PullRequestReviewVerdict::Comment,
            body: "review".into(),
            comments,
        })
        .await
        .unwrap();
    let r = task.await.unwrap();
    assert_eq!(r[0].body["event"], "COMMENT");
    assert!(r[0].body["comments"][0].get("start_line").is_none());
    assert_eq!(r[0].body["comments"][1]["start_line"], 7);
    assert_eq!(r[0].body["comments"][1]["start_side"], "RIGHT");
}
#[tokio::test]
async fn quota_reserve_blocks_background_and_allows_interactive_reads() {
    let mut first = answer(
        json!({"data":{"repository":{"pullRequest":{"number":7,"title":"PR","url":"https://github.com/acme/web/pull/7","state":"OPEN","isDraft":false,"createdAt":"2026-07-01T00:00:00Z","author":null}}}}),
    );
    first.headers = vec![
        ("x-ratelimit-resource".into(), "graphql".into()),
        ("x-ratelimit-limit".into(), "5000".into()),
        ("x-ratelimit-remaining".into(), "100".into()),
        ("x-ratelimit-reset".into(), "9999999999".into()),
    ];
    let second = answer(
        json!({"data":{"repository":{"pullRequest":{"number":7,"title":"PR","url":"https://github.com/acme/web/pull/7","state":"OPEN","isDraft":false,"createdAt":"2026-07-01T00:00:00Z","author":null}}}}),
    );
    let (github, task) = server(vec![first, second]).await;
    github.get_change_request_preview(pr()).await.unwrap();
    assert_eq!(
        github
            .get_change_request_preview(pr())
            .await
            .unwrap_err()
            .reason(),
        Some(FailureReason::RateLimited)
    );
    with_interactive(github.get_change_request_preview(pr()))
        .await
        .unwrap();
    assert_eq!(task.await.unwrap().len(), 2);
}

#[tokio::test]
async fn routing_identity_prefers_rest_and_a_verified_session_cannot_cross_hosts() {
    let (github, task) = server(vec![answer(json!({"id":42,"login":"octocat"}))]).await;
    let verified = github
        .with_verified_credential(VerifiedCredentialInput {
            cwd: ".".into(),
            host: " GITHUB.COM ".into(),
            repository: Some("acme/web".into()),
        })
        .await
        .unwrap();
    assert_eq!(verified.identity.account_id, "42");
    assert_eq!(verified.identity.viewer, "octocat");
    let error = verified
        .forge
        .get_viewer(ViewerInput {
            cwd: ".".into(),
            host: Some("enterprise.example".into()),
        })
        .await
        .unwrap_err();
    assert_eq!(error.reason(), Some(FailureReason::Unauthenticated));
    let r = task.await.unwrap();
    assert_eq!(r[0].path, "/user");
}
#[tokio::test]
async fn unfinished_workflows_fill_the_gap_before_jobs_start() {
    let runs = json!({"workflow_runs":[{"id":8,"name":"CI","status":"in_progress","html_url":"https://github.com/acme/web/actions/runs/8"}]});
    let (github, task) = server(vec![answer(core()), answer(runs)]).await;
    let detail = github.get_change_request(pr()).await.unwrap();
    assert_eq!(detail.checks.len(), 1);
    assert_eq!(detail.checks[0].status, PullRequestCheckStatus::Pending);
    let r = task.await.unwrap();
    assert_eq!(
        r[1].path,
        "/repos/acme/web/actions/runs?head_sha=abc123&per_page=100"
    );
}
#[tokio::test]
async fn workflow_approval_refuses_a_push_before_the_write() {
    let mut old = core();
    old["data"]["repository"]["pullRequest"]["isCrossRepository"] = json!(true);
    old["data"]["repository"]["pullRequest"]["headRepositoryOwner"] = json!({"login":"fork"});
    let mut changed = old.clone();
    changed["data"]["repository"]["pullRequest"]["headRefOid"] = json!("newhead");
    let heads = json!({"data":{"repository":{"pullRequests":{"nodes":[{"number":7,"headRefOid":"abc123","isCrossRepository":true,"headRepositoryOwner":{"login":"fork"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}});
    let (github, task) = server(vec![
        answer(old),
        answer(heads),
        answer(json!({"workflow_runs":[{"id":9,"name":"CI","html_url":null}]})),
        answer(changed),
    ])
    .await;
    assert!(
        github
            .run_action(action(PullRequestAction::ApproveWorkflows))
            .await
            .is_err()
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests.iter().all(|r| !r.path.ends_with("/approve")));
}
#[tokio::test]
async fn owner_qualified_heads_use_rest_filtering() {
    let response = json!([{"number":7,"title":"Fork PR","html_url":"https://github.com/acme/web/pull/7","state":"open","draft":false,"merged_at":null,"closed_at":null,"updated_at":"2026-07-02T00:00:00Z","base":{"ref":"main","repo":{"full_name":"acme/web"}},"head":{"ref":"feature","sha":"abc123","repo":{"name":"web","full_name":"fork/web","owner":{"login":"fork","type":"User"}}}}]);
    let (github, task) = server(vec![answer(response)]).await;
    let github = github.with_repository_environment(None, Some("acme/web".into()));
    let rows = github
        .checkout_list_change_requests(CheckoutListInput {
            checkout: CheckoutInput {
                cwd: ".".into(),
                context: None,
            },
            source: None,
            head_selector: "fork:feature".into(),
            state: Some(ChangeRequestState::Open),
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]
            .head_repository_owner_login
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        "fork"
    );
    let r = task.await.unwrap();
    assert_eq!(
        r[0].path,
        "/repos/acme/web/pulls?head=fork%3Afeature&state=open&sort=created&direction=desc&per_page=100"
    );
}
#[tokio::test]
async fn branch_lookup_prefers_open_then_falls_back_to_all_states() {
    let empty = json!({"data":{"repository":{"pullRequests":{"nodes":[]}}}});
    let mut row = core()["data"]["repository"]["pullRequest"].clone();
    row["state"] = json!("MERGED");
    row["mergedAt"] = json!("2026-07-03T00:00:00Z");
    let (github, task) = server(vec![
        answer(empty),
        answer(json!({"data":{"repository":{"pullRequests":{"nodes":[row]}}}})),
    ])
    .await;
    let github = github.with_repository_environment(None, Some("acme/web".into()));
    let row = github
        .checkout_get_change_request(CheckoutGetInput {
            checkout: CheckoutInput {
                cwd: ".".into(),
                context: None,
            },
            reference: "feature".into(),
            force: false,
        })
        .await
        .unwrap();
    assert_eq!(row.state, ChangeRequestState::Merged);
    let r = task.await.unwrap();
    assert_eq!(r[0].body["variables"]["states"], json!(["OPEN"]));
    assert_eq!(
        r[1].body["variables"]["states"],
        json!(["OPEN", "CLOSED", "MERGED"])
    );
}

#[tokio::test]
async fn checkout_fetches_and_tracks_the_head_using_real_isolated_git() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let checkout = dir.path().join("checkout");
    std::fs::create_dir(&source).unwrap();
    let env: t3_process::Environment = BTreeMap::from([
        ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
        (
            "GIT_CONFIG_GLOBAL".into(),
            Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
        ),
        ("GIT_ALLOW_PROTOCOL".into(), Some("file".into())),
        ("GIT_AUTHOR_NAME".into(), Some("Fixture".into())),
        (
            "GIT_AUTHOR_EMAIL".into(),
            Some("fixture@example.invalid".into()),
        ),
        ("GIT_COMMITTER_NAME".into(), Some("Fixture".into())),
        (
            "GIT_COMMITTER_EMAIL".into(),
            Some("fixture@example.invalid".into()),
        ),
    ]);
    let source_git = t3_git::Git::new(&source).with_environment(env.clone());
    source_git
        .execute(&["init", "--initial-branch=main"], Default::default())
        .await
        .unwrap();
    std::fs::write(source.join("file"), "base").unwrap();
    source_git
        .execute(&["add", "--all"], Default::default())
        .await
        .unwrap();
    source_git
        .execute(
            &[
                "-c",
                "core.hooksPath=",
                "commit",
                "--no-gpg-sign",
                "-m",
                "base",
            ],
            Default::default(),
        )
        .await
        .unwrap();
    source_git
        .execute(&["switch", "-c", "feature"], Default::default())
        .await
        .unwrap();
    std::fs::write(source.join("file"), "feature").unwrap();
    source_git
        .execute(&["add", "--all"], Default::default())
        .await
        .unwrap();
    source_git
        .execute(
            &[
                "-c",
                "core.hooksPath=",
                "commit",
                "--no-gpg-sign",
                "-m",
                "feature",
            ],
            Default::default(),
        )
        .await
        .unwrap();
    source_git
        .execute(
            &[
                "clone",
                "--local",
                "--no-hardlinks",
                "--branch",
                "main",
                source.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
            Default::default(),
        )
        .await
        .unwrap();
    let target = t3_git::Git::new(&checkout).with_environment(env.clone());
    target
        .execute(
            &["remote", "set-url", "origin", "https://github.com/acme/web"],
            Default::default(),
        )
        .await
        .unwrap();
    let base = url::Url::from_directory_path(&source)
        .unwrap()
        .to_string()
        .trim_end_matches('/')
        .to_string();
    target
        .write_config_value(
            &format!("url.{base}.insteadOf"),
            "https://github.com/acme/web",
        )
        .await
        .unwrap();
    let (github, task) = server(vec![answer(core())]).await;
    let github = github
        .with_git_environment(env)
        .with_repository_environment(None, Some("acme/web".into()));
    github
        .checkout_change_request(CheckoutGetInput {
            checkout: CheckoutInput {
                cwd: checkout.clone(),
                context: None,
            },
            reference: "7".into(),
            force: false,
        })
        .await
        .unwrap();
    assert_eq!(
        target.current_branch().await.unwrap().as_deref(),
        Some("feature")
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join("file")).unwrap(),
        "feature"
    );
    assert_eq!(
        target
            .read_config_value("branch.feature.remote")
            .await
            .unwrap()
            .as_deref(),
        Some("origin")
    );
    assert_eq!(task.await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_device_starts_replace_one_pending_flow_and_cancel_without_saving() {
    use t3_forge::github::app::{AppSessionStore, AppSignIn, MemoryAppSessions};
    let (http, endpoint, server) = server_http((1..=2).map(|id| answer(json!({"device_code":format!("device-{id}"), "user_code":format!("code-{id}"), "verification_uri":"https://github.com/login/device", "expires_in":900, "interval":3600}))).collect()).await;
    let store = Arc::new(MemoryAppSessions::default());
    let app = Arc::new(
        AppSignIn::new("fixture-client".into(), store.clone())
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    let registry = Arc::new(t3_forge::registry::ForgeRegistry::new([Arc::new(
        GitHub::new(http).unwrap().with_app_sign_in(app),
    )
        as Arc<dyn Forge>]));
    let sign_in = t3_forge::services::SourceControlSignIn::new(registry.clone());
    let input =
        || serde_json::from_value(json!({"provider":"github", "host":"github.com"})).unwrap();
    let (first, second) = tokio::join!(sign_in.start(input()), sign_in.start(input()));
    let mut codes = [first.unwrap().user_code.0, second.unwrap().user_code.0];
    codes.sort();
    assert_eq!(codes, ["code-1", "code-2"]);
    sign_in.cancel(input()).await.unwrap();
    assert_eq!(
        sign_in.wait(input()).await.unwrap_err().wire.reason,
        t3_contracts::source_control::SourceControlSignInFailure::Cancelled
    );
    assert!(store.read("github.com").await.unwrap().is_none());
    registry.shutdown().await;
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.path == "/login/device/code"));
}
