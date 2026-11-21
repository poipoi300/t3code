//! The agents section on the real welcome route.

#[test]
fn available_agents_expand_on_the_welcome_page() {
    let mut app = super::onb_1::open(t3_ui::state::nav::Route::Welcome);
    app.wait_for("Agents").expect("agents section");
    app.wait_for("More agents in the ACP Registry")
        .expect("empty list opens setup");
    app.press("role:button[Set up an agent]").expect("collapse");
    assert!(!app.screen().shows("More agents in the ACP Registry"));
}

#[test]
fn a_ready_agent_enables_start_and_hides_the_skip_action() {
    let mut app = super::onb_1::open_with(t3_ui::state::nav::Route::Welcome, false, true);
    app.wait_for("Ready").expect("ready agent");
    assert!(app.screen().enabled("role:button[Start]").expect("start"));
    assert!(!app.screen().shows("Start without an agent"));
    app.press("role:button[Start]").expect("finish");
    app.wait_for("Select a thread").expect("home");
}

#[test]
fn setup_terminal_pretypes_once_and_closes_without_submitting() {
    use futures::{StreamExt, executor::LocalPool, task::LocalSpawnExt};
    use std::rc::Rc;
    use t3_client::{
        ClientStore,
        composition::{ClientRuntime, Spawner, first_run::SetupTerminalState},
        testing::environment::FakeEnvironment,
        types::{ThreadId, ThreadKey},
    };
    use t3_contracts::{
        orchestration_v2::OrchestrationV2CreationSource, provider_instance::ProviderInstanceId,
    };
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: Spawner = Rc::new(move |task| spawner.spawn_local(task).expect("schedule"));
    let server = FakeEnvironment::new("env", spawn.clone());
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "terminal:operate",
    ]);
    let client = ClientRuntime::new(
        server.platform(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawn,
    );
    let env = pool.run_until(client.pair(server.pairing())).expect("pair");
    pool.run_until_stalled();
    let key = ThreadKey {
        environment: env,
        thread: ThreadId("onboarding-agent-setup".into()),
    };
    for (command, expected) in [
        ("codex login", SetupTerminalState::Ready),
        ("line\nbreak", SetupTerminalState::WriteFailed),
    ] {
        let mut events = client.first_run().setup_terminal(
            key.clone(),
            "setup".into(),
            "/workspace".into(),
            ProviderInstanceId("codex".into()),
            command.into(),
        );
        assert_eq!(pool.run_until(events.next()), Some(expected));
        drop(events);
        pool.run_until_stalled();
    }
    let writes = server.calls("terminal.write");
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0]["data"], "codex login");
    assert_eq!(server.calls("terminal.close").len(), 2);
}
