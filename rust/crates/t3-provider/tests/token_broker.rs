#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_provider::{chatgpt::*, sensitive::*, token_broker::*, transport::*};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Clock;
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        0
    }
}
struct Store;
#[async_trait]
impl ProfileStore for Store {
    async fn load(&self) -> ServiceResult<SavedProfiles> {
        Ok(SavedProfiles {sessions:Sessions {active_client_id:Some("client".into()),sessions:vec![serde_json::from_value(json!({"clientId":"client","accessToken":"never-log-this-token","refreshToken":"never-log-this-refresh","expiresAt":3600000,"earliestRefreshAt":null,"scopes":[REQUIRED_SCOPE],"subject":"subject","email":null,"idToken":"never-log-this-id"})).unwrap()]},registrations:Default::default()})
    }
    async fn save(&self, _: &SavedProfiles) -> ServiceResult<()> {
        panic!("Fresh tokens must not be renewed")
    }
}
struct Effects;
#[async_trait]
impl HttpTransport for Effects {
    async fn send(&self, _: HttpRequest) -> ServiceResult<HttpResponse> {
        panic!("No live HTTP or token renewal")
    }
}
#[async_trait]
impl IdentityVerifier for Effects {
    async fn verify(
        &self,
        _: &SecretString,
        _: &Discovery,
        _: &str,
        _: i64,
    ) -> ServiceResult<VerifiedIdentity> {
        panic!("No login")
    }
}
impl Entropy for Effects {
    fn bytes(&self, _: usize) -> ServiceResult<Vec<u8>> {
        panic!("No login")
    }
}
#[async_trait]
impl CallbackFactory for Effects {
    async fn listen(&self) -> ServiceResult<Box<dyn CallbackListener>> {
        panic!("No browser")
    }
}
fn auth() -> Arc<ChatGptAuth> {
    ChatGptAuth::new(
        Arc::new(Store),
        Arc::new(Effects),
        Arc::new(Effects),
        Arc::new(Effects),
        Arc::new(Clock),
        Arc::new(Effects),
        "environment".into(),
    )
}

#[tokio::test]
async fn broker_binds_tokens_to_the_environment_instance_and_active_profile_over_real_local_transport()
 {
    let broker = ManagedTokenBroker::new("environment".into());
    let auth = auth();
    let credentials = auth.read().await.unwrap().unwrap();
    let mut request = TokenRequest {
        version: 1,
        instance_id: "work".into(),
        environment_id: "environment".into(),
        profile_key: ChatGptAuth::profile_key(&credentials).unwrap(),
    };
    broker.register("work".into(), auth).await;
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let uid = {
        use std::os::unix::fs::MetadataExt;
        Some(std::fs::metadata(directory.path()).unwrap().uid())
    };
    #[cfg(not(unix))]
    let uid = None;
    let listener = broker.listen(directory.path(), uid).await.unwrap();
    let token = request_endpoint_token(&listener.endpoint, &request)
        .await
        .unwrap();
    assert_eq!(token.expose(), "never-log-this-token");
    assert!(!format!("{token:?}").contains("never-log"));
    request.environment_id = "other-environment".into();
    assert!(
        request_endpoint_token(&listener.endpoint, &request)
            .await
            .is_err()
    );
    request.environment_id = "environment".into();
    request.profile_key = "0".repeat(64);
    assert!(
        request_endpoint_token(&listener.endpoint, &request)
            .await
            .is_err()
    );
    request.instance_id = "missing".into();
    assert!(
        request_endpoint_token(&listener.endpoint, &request)
            .await
            .is_err()
    );
    listener.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn bounded_frames_require_a_newline_and_reject_trailing_and_oversize_input() {
    for bytes in [
        b"valid\ntrailing".as_slice(),
        b"123456\n",
        b"missing-newline",
    ] {
        let (mut sender, mut receiver) = tokio::io::duplex(100);
        sender.write_all(bytes).await.unwrap();
        sender.shutdown().await.unwrap();
        assert!(
            read_frame(&mut receiver, 6, Duration::from_secs(5))
                .await
                .is_err()
        );
    }
    let (mut sender, mut receiver) = tokio::io::duplex(100);
    sender.write_all(b"valid\n").await.unwrap();
    assert_eq!(
        read_frame(&mut receiver, 6, Duration::from_secs(5))
            .await
            .unwrap()
            .expose(),
        b"valid"
    );
    let (_sender, mut receiver) = tokio::io::duplex(100);
    assert!(
        read_frame(&mut receiver, 6, Duration::from_secs(5))
            .await
            .is_err()
    );
    let (mut sender, mut receiver) = tokio::io::duplex(100);
    write_frame(&mut sender, b"valid", 6).await.unwrap();
    let mut bytes = vec![];
    receiver.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"valid\n");
}

#[cfg(unix)]
#[tokio::test]
async fn post_creation_owner_must_match_even_when_the_root_owned_parent_is_trusted() {
    use std::os::unix::fs::MetadataExt;
    let parent = std::fs::canonicalize("/tmp").unwrap();
    let uid = std::fs::metadata(tempfile::tempdir().unwrap().path())
        .unwrap()
        .uid();
    let metadata = std::fs::symlink_metadata(&parent).unwrap();
    assert_eq!(metadata.uid(), 0);
    assert!(metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0);
    let result = ManagedTokenBroker::new("fixture".into())
        .listen(&parent, Some(uid.wrapping_add(1)))
        .await;
    assert!(matches!(result, Err(ServiceError::Identity)));
}
