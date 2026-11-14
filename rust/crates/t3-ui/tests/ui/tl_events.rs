//! Event rows mounted exactly as the timeline mounts C3 projections.

use std::rc::Rc;

use futures::{FutureExt, future::LocalBoxFuture};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::thread_views::link_request::{LinkAnswerError, LinkableMachine};
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowContent, MessagesTimelineRowsInput, derive_messages_timeline_rows,
};
use t3_client::thread_views::secret_request::SecretAnswerError;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::TimelineEntry;
use t3_contracts::peer_link::PeerLinkRequestAnswerInput;
use t3_contracts::secret_request::{SecretRequestAnswer, SecretRequestAnswerInput};
use t3_ui::i18n::t;
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rows::events::{EventRowContext, v2_event_timeline_row};
use t3_ui::screens::thread::rows::link_request::{LinkRequestClient, LinkRequestService};
use t3_ui::screens::thread::rows::secret_request::{SecretRequestClient, SecretRequestService};
use t3_ui::screens::thread::rows::work::WorkRowState;
use t3_ui::state::nav::{Navigator, Route};

use crate::support::{DESKTOP, PHONE};

const AT: &str = "2026-11-13T12:34:00Z";

fn event(id: &str, kind: &str, extra: Value) -> Rc<TimelineEntry> {
    let mut item = json!({"id":id,"type":kind,"threadId":"thread","runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":1,"status":"completed","title":null,"startedAt":AT,"completedAt":AT,"updatedAt":AT});
    item.as_object_mut()
        .expect("item")
        .extend(extra.as_object().expect("extra").clone());
    Rc::new(serde_json::from_value(json!({"id":id,"kind":"event","createdAt":AT,
        "projectedItem":{"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":id,"item":item}})).expect("canonical event"))
}

fn agent(id: &str, title: &str, failed: bool) -> Rc<TimelineEntry> {
    event(
        id,
        "subagent",
        json!({"subagentId":id,"origin":"app_owned","driver":"codex","providerInstanceId":"codex",
        "childThreadId":format!("child-{id}"),"prompt":"Inspect changes","title":title,
        "status":if failed {"failed"} else {"running"},"result":"- `Settings` [file](/repo)","progress":"Reviewing changes",
        "completedAt":if failed {json!(AT)} else {Value::Null}}),
    )
}

fn message(id: &str, body: &str) -> Rc<TimelineEntry> {
    Rc::new(serde_json::from_value(json!({"id":id,"kind":"message","createdAt":AT,
        "message":{"id":id,"role":"assistant","text":body,"runId":"resource-run","streaming":false,"createdAt":AT,"updatedAt":AT}})).expect("canonical assistant entry"))
}

fn control(label: &'static str, id: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .test_id(id)
        .on_press(action)
        .child(text(t(label)))
}

fn secret(status: &str, visibility: &str) -> Rc<TimelineEntry> {
    let item = json!({"id":"secret","type":"secret_request","threadId":"thread","runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":1,"status":"waiting","title":null,"startedAt":AT,"completedAt":null,"updatedAt":AT,
        "label":"API key","reason":"Needed to call the Acme API.","url":"https://auth.acme.dev/keys",
        "secretStatus":status});
    Rc::new(serde_json::from_value(json!({"id":"secret","kind":"event","createdAt":AT,
        "projectedItem":{"position":0,"visibility":visibility,"sourceThreadId":"thread","sourceItemId":"secret","item":item}})).expect("canonical secret request"))
}

/// The client's `secrets.answerRequest`: records each answer, refuses while `refuse` is set.
struct Secrets {
    answers: Signal<Vec<String>>,
    refuse: Signal<bool>,
}
impl SecretRequestClient for Secrets {
    fn answer(
        &self,
        environment: EnvironmentId,
        input: SecretRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), SecretAnswerError>> {
        let answer = match &input.answer {
            SecretRequestAnswer::Save { secret } => format!("save {secret}"),
            SecretRequestAnswer::Decline => "decline".into(),
            SecretRequestAnswer::Reference { .. } => "reference".into(),
        };
        self.answers.update(|answers| {
            answers.push(format!(
                "{} {} {} {answer}",
                environment.0, input.thread_id.0, input.turn_item_id.0
            ))
        });
        let refuse = self.refuse.get_untracked();
        futures::future::ready(if refuse {
            Err(SecretAnswerError::Failed(
                "This secret request was already answered.".into(),
            ))
        } else {
            Ok(())
        })
        .boxed_local()
    }
}

fn link(status: &str, extra: Value) -> Rc<TimelineEntry> {
    let mut item = json!({"id":"link","type":"link_request","threadId":"thread","runId":null,"nodeId":null,
        "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
        "ordinal":1,"status":"waiting","title":null,"startedAt":AT,"completedAt":null,"updatedAt":AT,
        "reason":"Run the build on your workstation.","url":"https://box.example/pair","linkStatus":status});
    item.as_object_mut()
        .expect("item")
        .extend(extra.as_object().expect("extra").clone());
    Rc::new(serde_json::from_value(json!({"id":"link","kind":"event","createdAt":AT,
        "projectedItem":{"position":0,"visibility":"local","sourceThreadId":"thread","sourceItemId":"link","item":item}})).expect("canonical link request"))
}

/// The client's `peerLinks.answerRequest`: records answers; `allowed` is its permission.
struct Links {
    answers: Signal<Vec<String>>,
    allowed: Signal<bool>,
}
impl LinkRequestClient for Links {
    fn can_answer(&self, _: &EnvironmentId, _: &PeerLinkRequestAnswerInput) -> bool {
        self.allowed.get_untracked()
    }
    fn answer(
        &self,
        environment: EnvironmentId,
        input: PeerLinkRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), LinkAnswerError>> {
        let answer = serde_json::to_value(&input.answer).expect("answer");
        self.answers.update(|answers| {
            answers.push(format!(
                "{} {} {answer}",
                environment.0, input.turn_item_id.0
            ))
        });
        futures::future::ready(Ok(())).boxed_local()
    }
    fn machines(
        &self,
        environment: EnvironmentId,
    ) -> LocalBoxFuture<'static, Vec<LinkableMachine>> {
        assert_eq!(
            environment.0, "environment",
            "the thread environment's links"
        );
        let machine = |id: &str, label: &str, urls: &[&str], linked: bool| LinkableMachine {
            environment_id: EnvironmentId(id.into()),
            label: label.into(),
            connected: true,
            connect_only: false,
            linked,
            urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        };
        futures::future::ready(vec![
            machine("laptop", "Laptop", &["http://192.168.1.9:3773"], true),
            machine("ws", "Workstation", &["https://work.tail.ts.net"], false),
        ])
        .boxed_local()
    }
}

#[component]
fn EventFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    t3_ui::i18n::set_language("en", &[]);
    let store = ClientStore::default();
    provide_context(Rt::new(
        store,
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let nav = Navigator::new(Route::Home);
    provide_context(nav);
    let answers = signal(Vec::<String>::new());
    let refuse = signal(false);
    provide_context(SecretRequestService(Rc::new(Secrets { answers, refuse })));
    let link_answers = signal(Vec::<String>::new());
    let link_allowed = signal(true);
    provide_context(LinkRequestService(Rc::new(Links {
        answers: link_answers,
        allowed: link_allowed,
    })));
    let context = EventRowContext::new(
        ThreadKey {
            environment: EnvironmentId("environment".into()),
            thread: ThreadId("thread".into()),
        },
        WorkRowState::default(),
    );
    let mode = signal("group");
    let failed = signal(false);
    let mounted = signal(true);
    let rows = computed(move || {
        let entries = match mode.get() {
            "group" => vec![
                agent("a", "First reviewer", failed.get()),
                agent("b", "Second reviewer", false),
            ],
            "created" => vec![
                message("intro", "Creating the chat"),
                event(
                    "created",
                    "thread_created",
                    json!({"runId":"resource-run","targetThreadId":"created","targetRunId":null,"targetProviderInstanceId":"codex","targetModel":"gpt","title":"Settings"}),
                ),
                message("final", "Chat is ready"),
            ],
            "interrupt" => vec![event(
                "interrupt",
                "run_interrupt_result",
                json!({"message":"Settings"}),
            )],
            "fork" => vec![event(
                "fork",
                "fork",
                json!({"source":{"type":"run","threadId":"original","runId":"old"},"targetThreadId":"fork"}),
            )],
            "remote" => vec![event(
                "remote",
                "subagent",
                json!({"subagentId":"remote","origin":"app_owned","driver":"remote","providerInstanceId":"remote","childThreadId":null,"prompt":"Review", "title":"Remote reviewer", "result":null,"remoteChild":{"environmentId":"other","threadId":"remote-child","label":"Workstation"}}),
            )],
            "secret" => vec![secret("pending", "local")],
            "secret-inherited" => vec![secret("pending", "inherited")],
            "secret-saved" => vec![secret("saved", "local")],
            "link" => vec![link("pending", json!({}))],
            "link-pick" => vec![link(
                "pending",
                json!({"url":null,"hint":"my workstation","requestedAccess":null}),
            )],
            "link-existing" => vec![link(
                "pending",
                json!({"url":null,"environmentId":"laptop"}),
            )],
            "link-linked" => vec![link(
                "linked",
                json!({"linkedLabel":"Workstation","linkedAccess":"read-only"}),
            )],
            "handoff" => vec![event(
                "handoff",
                "handoff",
                json!({"contextHandoffId":"context","fromProviderThreadIds":[],"toProviderThreadId":"destination","fromProviderInstanceIds":["codex"],"toProviderInstanceId":"claudeAgent","fromModelSelections":[{"instanceId":"codex","model":"Settings"}],"toModel":"Copy","strategy":"full_thread_summary"}),
            )],
            _ => Vec::new(),
        };
        derive_messages_timeline_rows(&MessagesTimelineRowsInput {
            timeline_entries: entries,
            ..Default::default()
        })
    });
    let row = computed(move || {
        rows.with(|rows| {
            rows.iter()
                .find(|r| {
                    matches!(
                        r.content,
                        MessagesTimelineRowContent::Event {
                            resource_summary: Some(true),
                            ..
                        }
                    )
                })
                .or_else(|| {
                    rows.iter()
                        .find(|r| matches!(r.content, MessagesTimelineRowContent::Event { .. }))
                })
                .cloned()
                .map(Shared)
        })
    });
    let calls = signal(0usize);
    let collapsed = signal(false);
    let mut context = context;
    context.on_toggle_entry = Some(Rc::new(move |_, was_expanded| {
        calls.update(|n| *n += 1);
        collapsed.set(was_expanded);
    }));
    let body_context = context.clone();
    view()
        .w_full()
        .h_full()
        .child(control("Toggle", "recycle", move || {
            mounted.update(|m| *m = !*m)
        }))
        .child(control("Update", "fail", move || failed.set(true)))
        .child(control("French", "fr", move || {
            t3_ui::i18n::set_language("fr", &[])
        }))
        .child(
            [
                ("Created thread", "created"),
                ("Interrupt", "interrupt"),
                ("Fork", "fork"),
                ("Remote", "remote"),
                ("Group", "group"),
                ("Handoff", "handoff"),
                ("Secret", "secret"),
                ("Inherited secret", "secret-inherited"),
                ("Saved secret", "secret-saved"),
                ("Link request", "link"),
                ("Pick a machine", "link-pick"),
                ("Existing link", "link-existing"),
                ("Linked", "link-linked"),
            ]
            .into_iter()
            .fold(view(), |parent, (label, id)| {
                parent.child(control(label, id, move || mode.set(id)))
            }),
        )
        .child(control("Refuse answers", "refuse", move || {
            refuse.set(true)
        }))
        .child(control("Deny link access", "deny-links", move || {
            link_allowed.set(false)
        }))
        .child(text(move || {
            SharedString::from(format!("Link answers: {}", link_answers.get().join("; ")))
        }))
        .child(text(move || {
            SharedString::from(format!("Route: {}", nav.top().path()))
        }))
        .child(text(move || {
            SharedString::from(format!("Answers: {}", answers.get().join("; ")))
        }))
        .child(text(move || {
            SharedString::from(format!(
                "Disclosures: {}, collapsed: {}",
                calls.get(),
                collapsed.get()
            ))
        }))
        .child(dynamic(move || {
            mounted.get().then(|| {
                untracked(|| v2_event_timeline_row(row, body_context.clone()).into_element())
            })
        }))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<EventFixture>();
    sets.push(fixtures);
    let mut app = Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<EventFixture>())
    })
    .expect("fixture");
    app.wait_for("2 subagents").expect("actual C3 group");
    app
}

#[test]
fn grouped_events_expand_survive_recycling_and_navigate_to_their_own_children() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        assert!(!app.screen().shows("First reviewer"));
        app.press("role:button[2 subagents]").expect("expand");
        app.wait_for("First reviewer").expect("members mounted");
        app.wait_for("Disclosures: 1, collapsed: false")
            .expect("source callback polarity");
        app.press("id:recycle").expect("unmount");
        assert!(!app.screen().shows("First reviewer"));
        app.press("id:recycle").expect("remount");
        app.wait_for("First reviewer")
            .expect("keyed expansion survives");
        app.press("role:button[Open Second reviewer]")
            .expect("open second child");
        app.wait_for("Route: /environment/child-b")
            .expect("correct environment and child");
        app.press("role:button[2 subagents]").expect("collapse");
        assert!(!app.screen().shows("First reviewer"));
        app.wait_for("Disclosures: 2, collapsed: true")
            .expect("collapse callback polarity");
    }
}

#[test]
fn projected_status_updates_and_literal_details_are_not_translated() {
    let mut app = fixture(DESKTOP);
    app.press("role:button[2 subagents]").expect("expand");
    app.wait_for("Reviewing changes").expect("active progress");
    app.press("id:fail").expect("settle first agent");
    app.wait_for("Settings file")
        .expect("settled result and plain markup");
    app.press("id:interrupt").expect("interrupt");
    app.press("id:fr").expect("switch language");
    app.wait_for("Settings")
        .expect("server detail remains content");
    assert!(!app.screen().shows("Paramètres"));
    app.press("id:handoff").expect("handoff endpoints");
    app.wait_for("Settings").expect("source model is literal");
    app.wait_for("Copy").expect("destination model is literal");
    assert!(!app.screen().shows("Copier"));
}

#[test]
fn created_resources_and_forks_own_their_actions_and_unconnected_remote_agents_are_static() {
    let mut app = fixture(DESKTOP);
    app.press("id:created").expect("resource");
    app.wait_for("Settings").expect("title from projection");
    app.press("role:button[Open chat]")
        .expect("row supplies action");
    app.wait_for("Route: /environment/created")
        .expect("created target");
    app.press("id:fork").expect("fork");
    app.press("role:button[Open source conversation]")
        .expect("source action");
    app.wait_for("Route: /environment/original")
        .expect("fork source target");
    app.press("id:remote").expect("remote");
    app.wait_for("Remote reviewer").expect("remote title");
    app.wait_for("on Workstation").expect("remote location");
    assert!(app.press("role:button[Open Remote reviewer]").is_err());
    assert!(!app.screen().shows("unavailable"));
}

/// TimelineV2EventRow mounts SecretRequestCard: the agent's request can be answered and declined.
#[test]
fn a_pending_secret_request_is_answered_through_the_client() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.press("id:secret").expect("secret request");
        app.wait_for("API key").expect("the card");
        app.wait_for("Needed to call the Acme API.")
            .expect("reason");
        app.wait_for("Stored securely, never shown to the agent")
            .expect("privacy note");
        assert!(
            app.screen()
                .enabled("role:button[Open auth.acme.dev]")
                .expect("the agent's sign-in page")
        );
        assert!(
            !app.screen()
                .enabled("role:button[Save securely]")
                .expect("save")
        );
        app.type_text("label:API key", "  sk-test-1  ")
            .expect("type");
        app.press("role:button[Save securely]").expect("save");
        app.wait_for("Answers: environment thread secret save sk-test-1")
            .expect("one trimmed answer to secrets.answerRequest");
        app.press("id:refuse").expect("server refuses");
        app.press("role:button[Decline]").expect("decline");
        app.wait_for("This secret request was already answered.")
            .expect("the server's reason, not a generic line");
    }
}

#[test]
fn answered_and_inherited_secret_requests_show_their_outcome_without_a_form() {
    let mut app = fixture(DESKTOP);
    app.press("id:secret-saved").expect("saved");
    app.wait_for("API key · Saved securely and kept private")
        .expect("answered row");
    assert!(app.screen().enabled("role:button[Save securely]").is_err());
    app.press("id:secret-inherited").expect("inherited");
    app.wait_for("API key · Waiting for an answer in the original thread")
        .expect("answered only where it was asked");
    assert!(app.screen().enabled("role:button[Decline]").is_err());
}

/// TimelineV2EventRow mounts LinkRequestCard: a pending link can be declined where this session
/// may answer, and says who can answer where it may not.
#[test]
fn a_pending_link_request_can_be_declined_through_the_client() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.press("id:link").expect("link request");
        app.wait_for("Link https://box.example to this environment")
            .expect("the target's origin, not its path");
        app.wait_for("Run the build on your workstation.")
            .expect("reason");
        app.press("role:button[Decline]").expect("decline");
        app.wait_for(r#"Link answers: environment link {"type":"decline"}"#)
            .expect("one decline to peerLinks.answerRequest");
    }
    let mut app = fixture(DESKTOP);
    app.press("id:deny-links").expect("no access grant");
    app.press("id:link").expect("link request");
    app.wait_for("Only a session that manages access here can answer this.")
        .expect("the permission gate");
    assert!(app.screen().enabled("role:button[Decline]").is_err());
    app.press("id:link-linked").expect("linked");
    app.wait_for("Linked Workstation · Read only")
        .expect("answered row");
}

/// The review dialog sends exactly the machine, access and code the user chose. It opens on the
/// hint's machine, at the web's default access (supervised), and cannot link without a code.
#[test]
fn the_link_dialog_sends_exactly_the_chosen_machine_access_and_code() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.press("id:link-pick")
            .expect("a request that leaves the machine open");
        app.wait_for("Link a machine to this environment")
            .expect("card");
        app.press("role:button[Choose machine…]")
            .expect("open the dialog");
        app.wait_for("Link a machine").expect("dialog");
        app.wait_for("Workstation").expect("the machine list");
        app.press("role:button[Continue]")
            .expect("the hint's machine is chosen");
        app.wait_for("Pairing code").expect("the access step");
        assert!(
            !app.screen()
                .enabled("role:button[Link]")
                .expect("link button"),
            "nothing is linked without a pairing code"
        );
        assert!(app.screen().shows("Supervised"), "the web's default access");
        app.type_text("label:Pairing code", " CODE-1 ")
            .expect("code");
        app.press("role:button[Link]").expect("link");
        app.wait_for(
            r#"Link answers: environment link {"type":"link","environmentId":"ws","label":"Workstation","urls":["https://work.tail.ts.net"],"access":"approval-required","pairingCode":"CODE-1"}"#,
        )
        .expect("exactly what was chosen, once");
        app.wait_until(
            "dialog closes",
            std::time::Duration::from_secs(2),
            |screen| !screen.shows("Pairing code"),
        )
        .expect("closed after the answer");
    }
}

/// A machine already linked is offered as is: no new pairing, the existing link.
#[test]
fn an_existing_link_is_used_without_a_new_pairing() {
    let mut app = fixture(DESKTOP);
    app.press("id:link-existing").expect("named machine");
    // The agent named an environment, not an address: the card names no machine, as the web.
    app.wait_for("Link a machine to this environment")
        .expect("card");
    app.press("role:button[Review link…]").expect("open");
    app.wait_for("role:button[Use this link]")
        .expect("existing link offered");
    assert!(!app.screen().shows("Pairing code"));
    app.press("role:button[Use this link]").expect("use it");
    app.wait_for(
        r#"Link answers: environment link {"type":"use-existing","environmentId":"laptop"}"#,
    )
    .expect("the existing link, nothing else");
}
