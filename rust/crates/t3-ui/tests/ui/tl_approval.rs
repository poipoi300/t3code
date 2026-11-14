//! TL-APPROVAL: authoritative requests and C3 rows through the production controls.
use crate::support::{DESKTOP, PHONE};
use futures::{FutureExt, channel::oneshot};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::state::sharing::SharedThreadProjection;
use t3_client::thread_views::{messages_timeline::*, requests};
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_client::view_logic::session::{
    TimelineEntriesInput, derive_timeline_entries_from_visible_turn_items_with_state,
};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadProjection;
use t3_ui::runtime::Shared;
use t3_ui::screens::thread::rows::approval::*;

const AT: &str = "2026-11-14T12:00:00Z";
fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("child-environment".into()),
        thread: ThreadId("child-thread".into()),
    }
}
fn source(live: bool, resolved: bool) -> SharedThreadProjection {
    let mut projection: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ThreadProjection.jsonl")
            .lines()
            .next()
            .expect("golden sample"),
    )
    .expect("canonical projection golden");
    projection["thread"]["id"] = json!("child-thread");
    projection["runtimeRequests"] = json!(["command","permission"].into_iter().map(|kind| json!({
        "id":kind,"nodeId":"node","providerTurnId":null,"nativeRequestRef":null,"kind":kind,
        "status":if resolved {"resolved"} else {"pending"},
        "responseCapability":if live {json!({"type":"live","providerSessionId":"session"})} else {json!({"type":"not_resumable","reason":"process_gone"})},
        "createdAt":AT,"resolvedAt":null
    })).collect::<Vec<_>>());
    let items: Vec<_> = ["command","permission"].into_iter().enumerate().map(|(index, kind)| {
        let mut item=json!({"id":format!("item-{kind}"),"threadId":"child-thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index,"status":"waiting","title":null,"startedAt":AT,"completedAt":null,"updatedAt":AT,"type":"approval_request","requestId":kind,"requestKind":kind,"prompt":if kind=="command" {"Run the compiler"} else {"Open the current tab"},"reason":"Needs permission to continue"});
        if kind=="command" {item["fullAccessOverride"]=json!("ask_rule");}
        else {item["appName"]=json!("Settings"); item["options"]=json!([
            {"decision":"decline","label":"Deny"},{"decision":"accept","label":"Allow once"},
            {"decision":"acceptAlways","label":"Always allow app","warning":"Applies to future requests"}
        ]);}
        item
    }).collect();
    projection["turnItems"] = json!(items);
    projection["visibleTurnItems"]=json!(items.iter().enumerate().map(|(position,item)| json!({"position":position,"visibility":"local","sourceThreadId":"child-thread","sourceItemId":item["id"],"item":item})).collect::<Vec<_>>());
    SharedThreadProjection::update(
        Rc::new(
            serde_json::from_value::<OrchestrationV2ThreadProjection>(projection)
                .expect("canonical requests"),
        ),
        None,
    )
}
fn control(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(label.to_owned())
        .on_press(action)
        .child(text(label.to_owned()))
}

#[component]
fn ApprovalFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    let allowed = signal(true);
    let live = signal(true);
    let resolved = signal(false);
    let failed = signal(true);
    let child = signal(false);
    let calls = signal(Vec::<ApprovalResponse>::new());
    let (release, waiting) = oneshot::channel::<()>();
    let release = stored(Some(release));
    let waiting = stored(Some(waiting));
    provide_context(ApprovalServices {
        can_operate: Rc::new(move |destination| {
            assert_eq!(destination, &key());
            allowed.get()
        }),
        respond: Rc::new(move |request| {
            assert_eq!(request.thread, key());
            calls.update(|calls| calls.push(request));
            let gate = waiting.with(Option::take);
            async move {
                if let Some(gate) = gate {
                    gate.await.expect("released operation");
                }
                if failed.get_untracked() {
                    Err("Child environment offline: Settings".into())
                } else {
                    Ok(())
                }
            }
            .boxed_local()
        }),
    });
    let projection = computed(move || Some(Shared(Rc::new(source(live.get(), resolved.get())))));
    let rows = computed(move || {
        projection.with(|projection| {
            let projection = projection.as_ref().expect("fixture projection");
            let source = projection.source();
            let timeline = derive_timeline_entries_from_visible_turn_items_with_state(
                TimelineEntriesInput {
                    visible_turn_items: projection.visible_turn_items.to_vec(),
                    ..Default::default()
                },
                None,
            );
            derive_messages_timeline_rows(&MessagesTimelineRowsInput {
                timeline_entries: timeline.entries.as_ref().clone(),
                pending_approval_request_ids: requests::derive_pending(
                    &source.runtime_requests,
                    &source.turn_items,
                )
                .approvals
                .iter()
                .filter_map(|pending| {
                    serde_json::to_value(&pending.request.id)
                        .ok()?
                        .as_str()
                        .map(str::to_owned)
                })
                .collect(),
                ..Default::default()
            })
        })
    });
    let state = ApprovalRowsState::default();
    let context = ApprovalContext::from_projection(key(), projection, state);
    let child_context = context.clone();
    let pending = computed(move || {
        projection.with(|projection| {
            let projection = projection.as_ref()?;
            let source = projection.source();
            let pending = requests::derive_pending(&source.runtime_requests, &source.turn_items);
            ApprovalRow::from_pending(pending.approvals.first()?, projection)
        })
    });
    let list = view().child(each(
        move || rows.get(),
        |row| row.id.clone(),
        move |row, _| {
            let row = computed(move || Some(Shared(row.get())));
            approval_timeline_row(row, context.clone())
        },
    ));
    view()
        .w_full()
        .h_full()
        .child(control("Permission", move || allowed.update(|v| *v = !*v)))
        .child(control("Process gone", move || live.set(false)))
        .child(control("Resolve", move || resolved.set(true)))
        .child(control("Child", move || child.set(true)))
        .child(control("French", move || {
            t3_ui::i18n::set_language("fr", &[])
        }))
        .child(control("Succeed", move || failed.set(false)))
        .child(control("Release", move || {
            if let Some(tx) = release.with(Option::take) {
                tx.send(()).expect("waiting service");
            }
        }))
        .child(text(move || format!("calls:{}", calls.with(Vec::len))))
        .child(text(move || {
            calls.with(|calls| {
                calls
                    .last()
                    .map(|call| {
                        format!(
                            "decision:{}",
                            serde_json::to_string(&call.decision).expect("decision")
                        )
                    })
                    .unwrap_or_default()
            })
        }))
        .child(
            view()
                .app_class(move || {
                    t3_ui::theme::tw(if child.get() {
                        "hidden"
                    } else {
                        "flex flex-col"
                    })
                })
                .child(list),
        )
        .child(dynamic(move || {
            child.get().then(|| {
                untracked(|| {
                    child_approval(
                        t3_ui::i18n::Text::content(String::from("Settings")).into(),
                        pending,
                        child_context.clone(),
                    )
                })
            })
        }))
}
fn start(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ApprovalFixture>())
    })
    .expect("start request fixture");
    app.first_render().expect("render");
    app
}
#[test]
fn concurrent_client_approvals_preserve_provider_options_and_do_not_disable_each_other() {
    for size in [DESKTOP, PHONE] {
        let mut app = start(size);
        app.wait_for("Run the compiler").expect("complete detail");
        app.press("role:button[Approve]").expect("first request");
        app.wait_for("text:calls:1").expect("one submission");
        assert!(app.press("role:button[Approve]").is_err());
        app.press("role:button[Allow once]")
            .expect("other request remains actionable");
        app.wait_for("text:calls:2").expect("independent request");
        app.press("role:button[Release]").expect("finish first");
        app.wait_for("Child environment offline: Settings")
            .expect("failure remains visible");
        app.press("role:button[Succeed]").expect("recovery");
        app.press("role:button[Approve]").expect("retry");
        app.wait_for("text:calls:3").expect("retried service");
    }
}
#[test]
fn revoked_permission_process_loss_and_resolved_requests_cannot_submit() {
    let mut app = start(DESKTOP);
    app.press("role:button[Permission]").expect("revoke");
    assert!(app.press("role:button[Approve]").is_err());
    assert!(
        app.press("role:group[Command approval] >> role:button[More approval options]")
            .is_err()
    );
    app.press("role:button[Process gone]")
        .expect("provider disconnected");
    app.wait_for("Provider process is gone. Interrupt or restart the run to respond.")
        .expect("source recovery copy");
    assert!(app.press("role:button[Approve]").is_err());
    app.press("role:button[Resolve]")
        .expect("authoritative resolution");
    assert!(!app.screen().shows("Waiting for approval"));
    assert!(app.screen().shows("calls:0"));
}
#[test]
fn child_request_errors_clear_on_retry_and_preserve_destination_and_literal_titles() {
    let mut app = start(DESKTOP);
    app.press("role:button[Child]").expect("child card");
    app.wait_for("Approval needed from Settings")
        .expect("client title");
    app.press("role:button[Approve]").expect("child submission");
    app.press("role:button[Release]").expect("failed response");
    app.wait_for("Child environment offline: Settings")
        .expect("server detail");
    app.press("role:button[Succeed]").expect("recover");
    app.press("role:button[Approve]").expect("retry");
    app.wait_for("text:calls:2")
        .expect("correct child received retry");
    assert!(!app.screen().shows("Child environment offline: Settings"));
    app.press("role:button[French]").expect("language change");
    assert!(
        app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Settings")),
        "client names stay literal"
    );
}

#[test]
fn default_and_provider_specific_more_options_submit_the_exact_decision() {
    let mut app = start(DESKTOP);
    app.press("role:button[Why am I being asked this?]")
        .expect("explanation");
    app.wait_for(requests::full_access_override_explanation(true))
        .expect("client explanation");
    app.press("role:button[Why am I being asked this?]")
        .expect("close explanation");
    app.press("role:group[Command approval] >> role:button[More approval options]")
        .expect("default options");
    app.press("role:menuitem[Always allow this session]")
        .expect("session option");
    app.wait_for("text:decision:\"acceptForSession\"")
        .expect("exact service decision");
    app.press("role:button[Release]")
        .expect("finish first request");
    app.press("role:group[App permission approval] >> role:button[More approval options]")
        .expect("provider options");
    app.press("role:menuitem[Always allow app]")
        .expect("provider-specific option");
    app.wait_for("text:decision:\"acceptAlways\"")
        .expect("option preserved");
}
