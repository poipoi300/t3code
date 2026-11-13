use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
};
use t3_forge::{
    Forge,
    github::{
        GitHub,
        cli::{CliOutput, GitHubCli},
        transport::*,
    },
    rate_limit::RateLimits,
    registry::ForgeRegistry,
};

struct RecordedCli(Mutex<VecDeque<(Vec<String>, CliOutput)>>);
impl RecordedCli {
    fn new(rows: Vec<(&[&str], i32, String)>) -> Self {
        Self(Mutex::new(
            rows.into_iter()
                .map(|(args, code, stdout)| {
                    (
                        args.iter().map(|a| a.to_string()).collect(),
                        CliOutput {
                            code,
                            stdout,
                            stderr: String::new(),
                        },
                    )
                })
                .collect(),
        ))
    }
    fn exhausted(&self) {
        assert!(self.0.lock().unwrap().is_empty());
    }
}
#[async_trait::async_trait]
impl GitHubCli for RecordedCli {
    async fn run(&self, _cwd: &Path, args: &[String]) -> t3_forge::Result<CliOutput> {
        let (expected, result) = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected CLI request");
        assert_eq!(args, expected);
        Ok(result)
    }
}

#[tokio::test]
async fn selected_account_fallback_is_cached_and_live_settings_change_the_credential() {
    let cli = Arc::new(RecordedCli::new(vec![
        (
            &["auth", "token", "--hostname=github.com", "--user=missing"],
            1,
            String::new(),
        ),
        (
            &["auth", "token", "--hostname=github.com"],
            0,
            "recorded-active-token\n".into(),
        ),
        (
            &[
                "auth",
                "token",
                "--hostname=github.com",
                "--user=--show-token",
            ],
            0,
            "recorded-selected-token\n".into(),
        ),
    ]));
    let credentials = GitHubCredentials::new(CredentialSettings {
        accounts: BTreeMap::from([("github.com".into(), "missing".into())]),
        ..Default::default()
    })
    .with_cli(cli.clone());
    let (first, second) = tokio::join!(
        credentials.get("github.com", Path::new(".")),
        credentials.get("github.com", Path::new("."))
    );
    let first = first.unwrap();
    assert_eq!(
        first.fingerprint,
        Credential::new("github.com", "recorded-active-token".into(), "gh").fingerprint
    );
    assert_eq!(first.fingerprint, second.unwrap().fingerprint);
    assert_eq!(first.source, "gh");
    let revision = credentials.revision();
    credentials
        .update_settings(CredentialSettings {
            accounts: BTreeMap::from([("github.com".into(), "--show-token".into())]),
            ..Default::default()
        })
        .await;
    assert_ne!(credentials.revision(), revision);
    let next = credentials.get("github.com", Path::new(".")).await.unwrap();
    assert_eq!(
        next.fingerprint,
        Credential::new("github.com", "recorded-selected-token".into(), "gh").fingerprint
    );
    assert_ne!(next.fingerprint, first.fingerprint);
    cli.exhausted();
}

#[tokio::test]
async fn discovery_combines_real_git_and_recorded_gh_accounts_without_reading_credentials() {
    use t3_contracts::source_control::{
        SourceControlDiscoveryStatus, SourceControlProviderAuthStatus, WireOption,
    };
    let root = tempfile::tempdir().unwrap();
    let cli = Arc::new(RecordedCli::new(vec![
        (&["--version"],0,"gh version 2.81.0 (2026-01-01)\nhttps://github.com/cli/cli/releases/tag/v2.81.0\n".into()),
        (&["auth","status","--json","hosts"],0,json!({"hosts":{"github.com":[{"host":"github.com","login":"octocat","active":true,"state":"success","tokenSource":"keyring"}]}}).to_string()),
        (&["auth","token","--hostname=github.com"],0,"recorded-token".into()),
    ]));
    let credentials =
        Arc::new(GitHubCredentials::new(CredentialSettings::default()).with_cli(cli.clone()));
    let github =
        GitHub::new(GitHubHttp::new(credentials, Arc::new(RateLimits::default())).unwrap())
            .unwrap()
            .with_cli(cli.clone());
    let registry = Arc::new(ForgeRegistry::new([Arc::new(github) as Arc<dyn Forge>]));
    let result = t3_forge::discovery::SourceControlDiscovery::new(registry, root.path().into())
        .discover()
        .await
        .unwrap();
    assert_eq!(result.version_control_systems[0].label.0, "Git");
    assert_eq!(
        result.version_control_systems[0].status,
        SourceControlDiscoveryStatus::Available
    );
    assert!(result.version_control_systems[0].implemented);
    assert_eq!(result.version_control_systems[1].label.0, "Jujutsu");
    assert!(!result.version_control_systems[1].implemented);
    let github = &result.source_control_providers[0];
    assert_eq!(github.kind.0, "github");
    assert_eq!(github.status, SourceControlDiscoveryStatus::Available);
    assert_eq!(
        github.auth.status,
        SourceControlProviderAuthStatus::Authenticated
    );
    let WireOption::Some { value } = &github.auth.account else {
        panic!("missing account");
    };
    assert_eq!(value.0, "octocat");
    assert_eq!(
        github.auth.accounts.as_ref().unwrap()[0].host.0,
        "github.com"
    );
    cli.exhausted();
}
