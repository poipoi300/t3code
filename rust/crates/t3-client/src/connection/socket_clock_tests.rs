use super::*;
use crate::{environment::SessionIdentity, testing::FakePlatform, types::Generation};
use futures::executor::block_on;
use serde_json::Value;
use t3_rpc::{ClientMessage, Exit, ServerMessage, codec};

#[test]
fn readiness_samples_each_connections_first_config_and_ignores_repeated_timestamps() {
    block_on(async {
        let platform = Rc::new(FakePlatform::default());
        let mut config: ServerConfig = serde_json::from_str(
            include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
                .lines()
                .next()
                .expect("fixture"),
        )
        .expect("config");
        let environment = config.environment.environment_id.clone();
        let authorization = RemoteAuthorization::direct(platform.clone());
        for generation in 1..=2 {
            let identity = SessionIdentity {
                environment: environment.clone(),
                generation: Generation(generation),
                account: None,
                dpop_thumbprint: None,
            };
            authorization
                .begin_session(&identity)
                .await
                .expect("authorized");
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut replies, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = crate::rpc::session(
                wire,
                incoming,
                environment.clone(),
                SessionGrants {
                    valid: true,
                    scopes: Default::default(),
                },
                0,
            );
            let session = PlatformSession {
                inner: Rc::new(SessionScope {
                    endpoint,
                    platform: platform.clone(),
                    driver: RefCell::new(
                        driver.run(futures::stream::pending(), || 0).boxed_local(),
                    ),
                    config: RefCell::new(None),
                    revision: authorization.session_revision(&identity),
                    authorization: authorization.clone(),
                    identity,
                }),
            };
            let started = platform.now_epoch_ms();
            config.server_time = (generation == 1).then(|| {
                serde_json::from_value(serde_json::json!("1970-01-01T01:00:00.000Z"))
                    .expect("timestamp")
            });
            let ready = async {
                session.initial_config().await.expect("ready");
                let expected = if generation == 1 {
                    3_600_000.0 - (started + 50) as f64
                } else {
                    0.0
                };
                assert_eq!(session.to_server_clock(0.0), expected);
                assert!(session.config().is_some());
                session.initial_config().await.expect("repeated config");
                assert_eq!(session.to_server_clock(0.0), expected);
                session.invalidate();
            };
            let peer = async {
                for _ in 0..2 {
                    let frame = requests.next().await.expect("request frame");
                    let messages: Vec<ClientMessage> = codec::decode(&frame).expect("request");
                    let ClientMessage::Request(request) = &messages[0] else {
                        panic!("request")
                    };
                    platform.advance(50);
                    replies
                        .send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                            request_id: request.id.clone(),
                            exit: Exit::success(serde_json::to_value(&config).expect("config")),
                        })
                        .expect("reply")))
                        .await
                        .expect("send");
                }
            };
            futures::join!(ready, peer);
        }
    });
}
