#![allow(clippy::unwrap_used)]
mod common;
use serde_json::json;
use std::sync::Arc;
use t3_provider::{
    acp_connection::*, registry_service::ResolvedRegistryAgent, sensitive::Sensitive,
};
#[tokio::test]
async fn acp_setup_negotiates_and_drains_metadata_over_an_owned_fixture_without_prompting() {
    let fixture = common::Fixture::new(json!({"protocol":"acp"}));
    let spawn = ResolvedRegistryAgent {
        agent: None,
        distribution: None,
        executable: fixture.binary().into(),
        args: vec![],
        cwd: fixture.directory.path().into(),
        environment: Sensitive::new(fixture.environment.clone()),
    };
    let mut connection = AcpConnection::spawn(&spawn, Arc::new(SetupHandler { consent: None }))
        .await
        .unwrap();
    let exit = connection.process_exit().unwrap();
    let initialized = connection
        .initialize("t3-code-provider-test", false)
        .await
        .unwrap();
    assert_eq!(initialized.expose()["protocolVersion"], 1);
    let setup = connection
        .new_session(fixture.directory.path())
        .await
        .unwrap();
    assert_eq!(setup.expose()["sessionId"], "fixture-session");
    let commands = connection.advertised_commands().await.unwrap();
    assert_eq!(commands.slash_commands.len(), 1);
    assert_eq!(commands.skills.len(), 1);
    connection.authenticate("login").await.unwrap();
    connection.delete_probe_session("fixture-session").await;
    connection.close().await;
    assert!(exit.is_cancelled());
    assert!(
        !fixture
            .calls()
            .iter()
            .flatten()
            .any(|method| method == "session/prompt")
    );
    assert!(
        fixture
            .calls()
            .iter()
            .flatten()
            .any(|method| method == "session/delete")
    );
    fixture.write(json!({"protocol":"acp","authRequired":true}));
    let connection = AcpConnection::spawn(&spawn, Arc::new(SetupHandler { consent: None }))
        .await
        .unwrap();
    connection
        .initialize("t3-code-provider-test", false)
        .await
        .unwrap();
    let error = connection
        .new_session(fixture.directory.path())
        .await
        .unwrap_err();
    assert_eq!(
        error.reason,
        t3_contracts::acp_registry::AcpRegistryOperationErrorReason::AuthenticationFailed
    );
    assert!(!format!("{error:?} {error}").contains("response secret"));
    connection.close().await;
}
#[tokio::test]
async fn setup_handler_never_grants_filesystem_or_terminal_effects_and_requires_explicit_valid_url_consent()
 {
    use async_trait::async_trait;
    use t3_acp::{ClientHandler, ClientRequest, RequestContext, RequestId};
    use t3_contracts::acp_registry::AcpRegistryUrlAuthAction;
    struct Consent;
    #[async_trait]
    impl AcpUrlConsent for Consent {
        async fn consent(&self, action: Sensitive<AcpRegistryUrlAuthAction>) -> bool {
            assert!(!format!("{action:?}").contains("secret-code"));
            true
        }
    }
    let handler = SetupHandler {
        consent: Some(Arc::new(Consent)),
    };
    let context = || RequestContext {
        id: RequestId::Number(1),
        method: "fixture".into(),
    };
    for request in [
        ClientRequest::Permission(json!({})),
        ClientRequest::ReadTextFile(json!({})),
        ClientRequest::WriteTextFile(json!({})),
        ClientRequest::TerminalCreate(json!({})),
    ] {
        assert!(handler.request(request, context()).await.is_err());
    }
    let input = |url: &str| {
        ClientRequest::Elicitation(
            json!({"mode":"url","elicitationId":"auth","url":url,"message":"Sign in"}),
        )
    };
    assert_eq!(
        handler
            .request(input("https://auth.invalid/?secret-code"), context())
            .await
            .unwrap()["action"],
        "accept"
    );
    assert_eq!(
        handler
            .request(input("file:///private"), context())
            .await
            .unwrap()["action"],
        "decline"
    );
    assert_eq!(
        SetupHandler { consent: None }
            .request(input("https://auth.invalid"), context())
            .await
            .unwrap()["action"],
        "decline"
    );
}
