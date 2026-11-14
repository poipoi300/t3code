use futures::executor::block_on;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    authorization::{AuthorizedEnvironment, HttpAuthorization, Sensitive},
    environment::SessionIdentity,
    permissions::EnvironmentHttpClient,
    platform::{Credentials, HttpResponse},
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
};

#[test]
fn approval_commands_use_canonical_payloads_and_destination_credentials() {
    block_on(async {
        let platform = FakePlatform::default();
        let client = EnvironmentHttpClient::new(
            Rc::new(platform.clone()),
            AuthorizedEnvironment {
                session: SessionIdentity {
                    environment: EnvironmentId("env".into()),
                    generation: Generation(1),
                    account: None,
                    dpop_thumbprint: None,
                },
                http_base_url: "https://host.test".into(),
                socket_url: Sensitive("wss://host.test/ws".into()),
                authorization: HttpAuthorization::Cookie,
                descriptor: None,
            },
            None,
            None,
            false,
        );
        platform.push_http(Ok(HttpResponse {
            status: 200,
            headers: Default::default(),
            body: br#"{"approved":true}"#.to_vec(),
        }));
        let input = serde_json::from_value(json!({"pairingLinkId":"pending-link"})).unwrap();
        assert!(
            client
                .approve_connect_device(&input)
                .await
                .unwrap()
                .approved
        );
        platform.push_http(Ok(HttpResponse {
            status: 200,
            headers: Default::default(),
            body: br#"{"revoked":false}"#.to_vec(),
        }));
        let input = serde_json::from_value(json!({"proofKeyThumbprint":"public-key"})).unwrap();
        assert!(!client.revoke_connect_device(&input).await.unwrap().revoked);
        let requests = platform.http_requests();
        for (request, path, payload) in [
            (
                &requests[0],
                "/api/auth/connect-devices/approve",
                json!({"pairingLinkId":"pending-link"}),
            ),
            (
                &requests[1],
                "/api/auth/connect-devices/revoke",
                json!({"proofKeyThumbprint":"public-key"}),
            ),
        ] {
            assert_eq!(request.url, format!("https://host.test{path}"));
            assert_eq!(request.method, "POST");
            assert_eq!(request.credentials, Credentials::Include);
            assert_eq!(
                serde_json::from_slice::<Value>(&request.body).unwrap(),
                payload
            );
        }
    });
}
