#![cfg(feature = "git")]
#![allow(clippy::unwrap_used)]
use futures::future::BoxFuture;
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{
    auth::EnvironmentAuthorizationError, base_schemas::AuthSessionId, vcs::VcsError,
};
use t3_server::adapters::git::{Error, ExecuteOptions, Git, GitAdapter, WorkspaceAuthority};

struct Authority(PathBuf);
impl WorkspaceAuthority for Authority {
    fn roots<'a>(
        &'a self,
        _: &'a AuthSessionId,
    ) -> BoxFuture<'a, Result<Vec<PathBuf>, EnvironmentAuthorizationError>> {
        Box::pin(async move { Ok(vec![self.0.clone()]) })
    }
}

#[tokio::test]
async fn configured_non_git_driver_is_rejected_before_a_git_query() {
    let temp = tempfile::tempdir().unwrap();
    let environment = [
        ("HOME", Some(temp.path().as_os_str().to_owned())),
        ("USERPROFILE", Some(temp.path().as_os_str().to_owned())),
        ("XDG_CONFIG_HOME", Some(temp.path().as_os_str().to_owned())),
        (
            "GIT_CONFIG_GLOBAL",
            Some(temp.path().join("no-config").into_os_string()),
        ),
        (
            "GIT_CONFIG_SYSTEM",
            Some(temp.path().join("no-config").into_os_string()),
        ),
        ("GIT_CONFIG_NOSYSTEM", Some("1".into())),
        ("GIT_CONFIG_COUNT", Some("0".into())),
        ("GIT_CONFIG_PARAMETERS", None),
        ("GIT_CONFIG", None),
        ("GIT_TEMPLATE_DIR", None),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();
    let git = Git::new(temp.path()).with_environment(environment);
    git.execute(&["init"], ExecuteOptions::default())
        .await
        .unwrap();
    std::fs::create_dir(temp.path().join(".t3code")).unwrap();
    std::fs::write(
        temp.path().join(".t3code/vcs.json"),
        r#"{"vcs":{"kind":"jj"}}"#,
    )
    .unwrap();
    let adapter = GitAdapter::new(
        git,
        Arc::new(Authority(temp.path().into())),
        temp.path().join("state"),
        temp.path().join("secrets"),
    );
    let input = serde_json::from_value(serde_json::json!({"cwd":temp.path()})).unwrap();
    let error = adapter
        .list_refs(&AuthSessionId("session:test".into()), input)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Vcs(ref error) if matches!(error.as_ref(), VcsError::UnsupportedOperation(_))),
        "{error:?}"
    );
}
