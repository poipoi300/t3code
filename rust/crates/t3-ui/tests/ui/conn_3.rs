//! CONN-3: an agent's sign-in approved on the environment's own page, through the production
//! constructor over a client whose platform answers the environment's two consent calls.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::platform::HttpResponse;
use t3_client::testing::FakePlatform;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

use crate::support::{DESKTOP, PHONE};

const PAGE: &str = "https://box.test/connect-agent?response_type=code&client_id=agent&redirect_uri=http%3A%2F%2F127.0.0.1%3A43123%2Fcb&state=s";
const DETAILS: &str =
    r#"{"clientName":"Claude Code","redirectHost":"127.0.0.1:43123","environmentHost":"box.test"}"#;

#[derive(Clone, PartialEq, Props)]
struct ConsentProps {
    /// Each scripted HTTP answer as `"<status> <body>"`; props must be plain values.
    answers: Vec<String>,
}

#[component]
fn Consent(props: ConsentProps) -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let platform = FakePlatform::default();
    for answer in props.answers {
        let (status, body) = answer.split_once(' ').expect("status and body");
        platform.push_http(Ok(HttpResponse {
            status: status.parse().expect("a status"),
            headers: BTreeMap::new(),
            body: body.as_bytes().to_vec(),
        }));
    }
    let client = ClientRuntime::new(
        platform,
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|_| {}),
    );
    t3_ui::screens::agent::connect_agent_surface(Some(client))
}

fn start(window: (f32, f32), answers: &[(u16, &str)]) -> Headless {
    let answers: Vec<String> = answers
        .iter()
        .map(|(status, body)| format!("{status} {body}"))
        .collect();
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        // Approve's arming delay runs on the engine clock, advanced by `run_for`.
        clock: nsh_core::time::Clock::manual(std::time::SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::new::<Consent>(move || ConsentProps {
            answers: answers.clone(),
        }))
    };
    Headless::start_at_page(config, PAGE).expect("the engine starts on the page")
}

#[test]
fn a_level_bound_code_approves_at_its_own_level_and_the_agent_gets_its_redirect() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(
            window,
            &[
                (200, DETAILS),
                (
                    200,
                    r#"{"redirectTo":"http://127.0.0.1:43123/cb?code=granted"}"#,
                ),
            ],
        );
        app.wait_for("Connect Claude Code")
            .expect("the request's details");
        assert!(app.screen().shows("Agent sign-in"));
        assert!(app.screen().shows("on the computer that opened this page"));
        app.type_text("label:Sign-in code", "auto-q8rz3nwd5ktc")
            .expect("type a code");
        app.wait_for("Approve Auto")
            .expect("the code names its level");
        assert!(
            !app.screen().shows("What it may do"),
            "a level-bound code does not ask for a level again"
        );
        // Approve arms 700 ms after the page shows (ARM_DELAY_MS): a click aimed elsewhere
        // cannot land on it.
        assert_eq!(app.screen().enabled("role:button[Approve Auto]"), Ok(false));
        app.run_for(Duration::from_millis(800)).expect("armed");
        assert_eq!(app.screen().enabled("role:button[Approve Auto]"), Ok(true));
        app.press("role:button[Approve Auto]").expect("approve");
        app.wait_for("Approving…").expect("the decision is pending");
        app.run_for(Duration::from_millis(50))
            .expect("the redirect opens");
        assert_eq!(
            app.screen().opened_urls(),
            ["http://127.0.0.1:43123/cb?code=granted".to_owned()],
            "the agent's redirect is opened, once"
        );
    }
}

#[test]
fn deny_sends_the_refusal_and_a_refused_sign_in_explains_itself() {
    let mut app = start(
        DESKTOP,
        &[
            (200, DETAILS),
            (
                200,
                r#"{"redirectTo":"http://127.0.0.1:43123/cb?error=access_denied"}"#,
            ),
        ],
    );
    app.wait_for("Connect Claude Code").expect("details");
    app.press("role:button[Deny]").expect("deny");
    app.wait_for("Denying…").expect("the refusal is pending");
    app.run_for(Duration::from_millis(50))
        .expect("the redirect opens");
    assert_eq!(
        app.screen().opened_urls(),
        ["http://127.0.0.1:43123/cb?error=access_denied".to_owned()],
        "the refusal goes back to the agent"
    );

    let mut app = start(
        DESKTOP,
        &[(
            403,
            r#"{"_tag":"AuthMcpApprovalError","message":"This sign-in request has expired."}"#,
        )],
    );
    app.wait_for("This sign-in cannot continue")
        .expect("invalid state");
    assert!(app.screen().shows("This sign-in request has expired."));
    assert!(
        app.screen()
            .shows("Close this page and start the sign-in again from your agent.")
    );
}

#[test]
fn an_unsafe_redirect_is_never_opened() {
    let mut app = start(
        DESKTOP,
        &[
            (200, DETAILS),
            (200, r#"{"redirectTo":"javascript:alert(1)"}"#),
        ],
    );
    app.wait_for("Connect Claude Code").expect("details");
    app.press("role:button[Deny]").expect("deny");
    app.wait_for("The environment returned an address this page will not open.")
        .expect("the page refuses it");
    assert!(app.screen().opened_urls().is_empty());
}
