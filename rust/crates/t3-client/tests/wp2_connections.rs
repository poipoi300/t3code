#![allow(clippy::unwrap_used)]

use t3_client::{
    connection::*,
    platform::{NetworkStatus, Platform},
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
};

fn route(id: &str, mode: ConnectionMode, base: &str) -> Route {
    Route {
        id: id.into(),
        environment: EnvironmentId("environment-1".into()),
        mode,
        http_base: base.into(),
        ws_base: base.replace("http", "ws"),
    }
}

fn lan() -> Route {
    route("lan", ConnectionMode::Remote, "http://192.168.1.10:3773/")
}

fn relay() -> Route {
    route("relay", ConnectionMode::Relay, "https://relay.test/")
}

fn failure(kind: FailureKind) -> ConnectionFailure {
    ConnectionFailure::new(kind, "Connection failed.")
}

struct Harness {
    platform: FakePlatform,
    supervisor: Supervisor,
}

impl Harness {
    fn new(routes: Vec<Route>, primary: bool, network: NetworkStatus) -> Self {
        Self {
            platform: FakePlatform::default(),
            supervisor: Supervisor::new(routes, primary, true, network),
        }
    }

    fn event(&mut self, event: ConnectionEvent) -> Vec<ConnectionAction> {
        self.supervisor
            .handle(event, Time::from_platform(&self.platform), 1.0)
    }

    fn advance(&mut self, milliseconds: u64) -> Vec<ConnectionAction> {
        self.platform.advance(milliseconds);
        self.event(ConnectionEvent::Tick)
    }

    fn connect(&mut self) -> AttemptId {
        self.event(ConnectionEvent::Connect);
        self.supervisor.active_attempt().unwrap()
    }

    fn ready(&mut self, route_id: &str) -> Generation {
        let attempt = self.supervisor.active_attempt().unwrap();
        self.event(ConnectionEvent::Ready {
            attempt,
            route_id: route_id.into(),
        });
        self.supervisor.state().generation
    }

    fn fail(&mut self, kind: FailureKind) {
        let attempt = self.supervisor.active_attempt().unwrap();
        self.event(ConnectionEvent::Failed {
            attempt,
            failure: failure(kind),
        });
    }

    fn wake(&mut self, reason: Wakeup) -> Vec<ConnectionAction> {
        self.event(ConnectionEvent::Wake(reason))
    }

    fn probe(&self) -> ProbeId {
        self.supervisor.active_probe().unwrap()
    }
}

#[test]
fn retry_ladder_has_equal_jitter_and_caps_at_five_minutes() {
    for (count, low, high) in [
        (0, 1_000, 2_000),
        (1, 2_000, 4_000),
        (2, 4_000, 8_000),
        (3, 8_000, 16_000),
        (4, 16_000, 32_000),
        (5, 32_000, 64_000),
        (6, 64_000, 128_000),
        (7, 128_000, 256_000),
        (8, 150_000, 300_000),
        (u32::MAX, 150_000, 300_000),
    ] {
        assert_eq!(retry_delay_ms(count, 0.0), low);
        assert_eq!(retry_delay_ms(count, 0.5), (low + high) / 2);
        assert_eq!(retry_delay_ms(count, 1.0), high);
    }
    assert_eq!(session_loss_delay_ms(0.0), 125);
    assert_eq!(session_loss_delay_ms(0.5), 188);
    assert_eq!(session_loss_delay_ms(1.0), 250);
}

#[test]
fn connection_waits_for_desire_and_network_but_primary_ignores_offline() {
    for primary in [false, true] {
        let mut h = Harness::new(vec![lan()], primary, NetworkStatus::Offline);
        assert!(h.advance(60_000).is_empty());
        assert_eq!(h.supervisor.state().phase, Phase::Available);
        h.event(ConnectionEvent::Connect);
        assert_eq!(
            h.supervisor.state().phase,
            if primary {
                Phase::Connecting
            } else {
                Phase::Offline
            }
        );
        h.event(ConnectionEvent::Network(NetworkStatus::Online));
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
        h.ready("lan");
        assert_eq!(h.supervisor.state().generation, Generation(1));
    }
}

#[test]
fn initial_connect_and_ordinary_wakeups_do_not_cancel_setup() {
    let mut h = Harness::new(vec![lan()], true, NetworkStatus::Online);
    let attempt = h.connect();
    for event in [
        ConnectionEvent::Connect,
        ConnectionEvent::Wake(Wakeup::Foreground),
        ConnectionEvent::Wake(Wakeup::CredentialsChanged),
        ConnectionEvent::Network(NetworkStatus::Offline),
    ] {
        h.event(event);
        assert_eq!(h.supervisor.active_attempt(), Some(attempt));
    }
    h.event(ConnectionEvent::Network(NetworkStatus::Online));
    assert_ne!(h.supervisor.active_attempt(), Some(attempt));
}

#[test]
fn failed_attempt_keeps_its_failure_visible_during_retry() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.fail(FailureKind::Transport);
    assert_eq!(h.supervisor.state().retry_at, Some(2_000));
    h.advance(1_999);
    assert_eq!(h.supervisor.state().phase, Phase::Backoff);
    h.advance(1);
    assert_eq!(h.supervisor.state().attempt, 2);
    assert_eq!(
        h.supervisor.state().last_failure,
        Some(failure(FailureKind::Transport))
    );
    h.ready("relay");
    assert_eq!(h.supervisor.state().last_failure, None);
}

#[test]
fn blocked_failures_idle_until_external_signal() {
    for kind in [
        FailureKind::Authentication,
        FailureKind::Configuration,
        FailureKind::Permission,
        FailureKind::Unsupported,
    ] {
        let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
        h.connect();
        h.fail(kind);
        h.advance(3_600_000);
        assert_eq!(h.supervisor.state().phase, Phase::Blocked);
        h.wake(Wakeup::CredentialsChanged);
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    }
}

#[test]
fn explicit_retry_and_wakeup_reset_backoff() {
    for event in [
        ConnectionEvent::Retry,
        ConnectionEvent::Wake(Wakeup::Foreground),
        ConnectionEvent::Wake(Wakeup::NetworkChanged),
    ] {
        let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
        h.connect();
        h.fail(FailureKind::Transport);
        h.advance(2_000);
        h.fail(FailureKind::Transport);
        h.event(event);
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
        assert_eq!(h.supervisor.state().attempt, 1);
    }
}

#[test]
fn setup_deadline_counts_routes_and_each_synchronization_budget() {
    for count in [1, 2, 3] {
        let mut h = Harness::new(
            (0..count)
                .map(|i| route(&i.to_string(), ConnectionMode::Remote, "https://host.test/"))
                .collect(),
            false,
            NetworkStatus::Online,
        );
        let attempt = h.connect();
        h.advance(10_000);
        for _ in 0..count {
            h.event(ConnectionEvent::Progress {
                attempt,
                stage: AttemptStage::Synchronizing,
            });
        }
        h.advance(count * (15_000 + 75_000) - 10_001);
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
        h.advance(1);
        assert_eq!(h.supervisor.state().phase, Phase::Backoff);
        assert_eq!(
            h.supervisor.state().last_failure.as_ref().unwrap().kind,
            FailureKind::Timeout
        );
    }
}

#[test]
fn ssh_preparation_pauses_setup_clock_for_every_route() {
    let mut h = Harness::new(
        vec![
            route("ssh-a", ConnectionMode::Ssh, ""),
            route("ssh-b", ConnectionMode::Ssh, ""),
        ],
        false,
        NetworkStatus::Online,
    );
    let attempt = h.connect();
    h.advance(240_000);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    h.event(ConnectionEvent::Progress {
        attempt,
        stage: AttemptStage::Synchronizing,
    });
    h.advance(10_000);
    h.event(ConnectionEvent::Progress {
        attempt,
        stage: AttemptStage::Preparing,
    });
    h.advance(240_000);
    h.event(ConnectionEvent::Progress {
        attempt,
        stage: AttemptStage::Opening,
    });
    h.advance(94_999);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    h.advance(1);
    assert_eq!(h.supervisor.state().phase, Phase::Backoff);
}

#[test]
fn session_loss_is_quick_once_then_backs_off_and_hides_old_errors() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let first = h.ready("relay");
    h.event(ConnectionEvent::Lost {
        generation: first,
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(h.supervisor.state().retry_at, Some(250));
    assert_eq!(h.supervisor.state().last_failure, None);
    h.advance(250);
    let second = h.ready("relay");
    assert_eq!(second, Generation(2));
    h.event(ConnectionEvent::Lost {
        generation: second,
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(h.supervisor.state().retry_at, Some(4_250));
    h.advance(4_000);
    let third = h.ready("relay");
    h.advance(30_000);
    h.event(ConnectionEvent::Lost {
        generation: third,
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(
        h.supervisor.state().retry_at,
        Some(h.platform.now_epoch_ms() + 250)
    );
}

#[test]
fn only_one_candidate_can_publish_and_stale_callbacks_cannot_replace_it() {
    let mut h = Harness::new(vec![lan(), relay()], false, NetworkStatus::Online);
    let attempt = h.connect();
    h.ready("relay");
    h.event(ConnectionEvent::Ready {
        attempt,
        route_id: "lan".into(),
    });
    h.event(ConnectionEvent::Progress {
        attempt,
        stage: AttemptStage::Opening,
    });
    h.event(ConnectionEvent::Failed {
        attempt,
        failure: failure(FailureKind::Authentication),
    });
    assert_eq!(h.supervisor.active_route(), Some("relay"));
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    h.event(ConnectionEvent::Disconnect);
    h.event(ConnectionEvent::Ready {
        attempt,
        route_id: "lan".into(),
    });
    assert_eq!(h.supervisor.state().phase, Phase::Available);
}

#[test]
fn old_attempt_and_session_events_do_not_mutate_replacement() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    let old = h.connect();
    h.event(ConnectionEvent::Retry);
    let current = h.supervisor.active_attempt().unwrap();
    h.event(ConnectionEvent::Ready {
        attempt: old,
        route_id: "relay".into(),
    });
    assert_eq!(h.supervisor.active_attempt(), Some(current));
    let generation = h.ready("relay");
    h.event(ConnectionEvent::Lost {
        generation: Generation(0),
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    assert_eq!(h.supervisor.state().generation, generation);
}

#[test]
fn disabled_environments_ignore_network_foreground_and_connect_requests() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.event(ConnectionEvent::SetEnabled(false));
    for event in [
        ConnectionEvent::Connect,
        ConnectionEvent::Retry,
        ConnectionEvent::Wake(Wakeup::Foreground),
        ConnectionEvent::Network(NetworkStatus::Online),
    ] {
        h.event(event);
        assert_eq!(h.supervisor.state().phase, Phase::Available);
        assert!(!h.supervisor.state().desired);
    }
    h.event(ConnectionEvent::SetEnabled(true));
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn foreground_probes_and_resubscribes_without_replacing_healthy_session() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("relay");
    let actions = h.wake(Wakeup::Foreground);
    assert!(actions.contains(&ConnectionAction::Resubscribe { generation }));
    let probe = h.probe();
    h.event(ConnectionEvent::ProbeResult {
        probe,
        result: Ok(()),
    });
    assert_eq!(h.supervisor.state().generation, generation);
    assert!(h.supervisor.active_probe().is_none());
    h.advance(60_000);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
}

#[test]
fn foreground_tolerates_one_miss_and_replaces_on_followup_failure() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.wake(Wakeup::Foreground);
    let first = h.probe();
    h.advance(15_000);
    assert_ne!(h.probe(), first);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    h.advance(14_999);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    h.advance(1);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    assert_eq!(h.supervisor.state().attempt, 1);
    assert_eq!(h.supervisor.state().last_failure, None);
}

#[test]
fn foreground_followup_success_preserves_the_session() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    h.ready("lan");
    h.wake(Wakeup::Foreground);
    h.advance(15_000);
    h.event(ConnectionEvent::ProbeResult {
        probe: h.probe(),
        result: Ok(()),
    });
    h.advance(60_000);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
}

#[test]
fn probe_deadlines_distinguish_lan_remote_retry_and_long_resume() {
    for (route, wake, timeout) in [
        (lan(), Wakeup::ForegroundProbe, 3_000),
        (relay(), Wakeup::ForegroundProbe, 15_000),
        (relay(), Wakeup::NetworkChanged, 15_000),
        (relay(), Wakeup::LongResume, 1_500),
        (lan(), Wakeup::LongResume, 1_500),
    ] {
        let mut h = Harness::new(vec![route.clone()], false, NetworkStatus::Online);
        h.connect();
        h.ready(&route.id);
        h.wake(wake);
        h.advance(timeout - 1);
        assert_eq!(h.supervisor.state().phase, Phase::Connected);
        h.advance(1);
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    }
}

#[test]
fn silent_socket_check_waits_for_fresh_host_and_remote_silence_threshold() {
    for (route, wake, silence) in [
        (lan(), Wakeup::ForegroundProbe, 500),
        (relay(), Wakeup::ForegroundProbe, 2_500),
        (relay(), Wakeup::LongResume, 500),
    ] {
        let mut h = Harness::new(vec![route.clone()], false, NetworkStatus::Online);
        h.connect();
        let generation = h.ready(&route.id);
        h.event(ConnectionEvent::ConfigReady { generation });
        h.wake(wake);
        h.advance(499);
        assert!(h.supervisor.host_check().is_none());
        h.advance(1);
        let check = h.supervisor.host_check().unwrap();
        h.event(ConnectionEvent::HostResult {
            check,
            answered: true,
        });
        if silence > 500 {
            h.advance(silence - 501);
            assert_eq!(h.supervisor.state().phase, Phase::Connected);
            h.advance(1);
        }
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
        assert_eq!(h.supervisor.state().attempt, 1);
    }
}

#[test]
fn foreground_probe_does_not_arm_host_checks_until_a_network_signal() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("lan");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::Foreground);
    h.advance(1_000);
    assert!(h.supervisor.host_check().is_none());
    h.wake(Wakeup::NetworkChanged);
    h.advance(500);
    assert!(h.supervisor.host_check().is_some());
}

#[test]
fn frames_cancel_silent_checks_and_extend_long_resume_to_three_seconds() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("relay");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::LongResume);
    h.advance(500);
    let check = h.supervisor.host_check().unwrap();
    h.advance(500);
    h.event(ConnectionEvent::Frame { generation });
    h.event(ConnectionEvent::HostResult {
        check,
        answered: true,
    });
    h.advance(1_999);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    h.advance(1);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn failed_host_checks_retry_without_proving_socket_dead() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("lan");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::ForegroundProbe);
    h.advance(500);
    let check = h.supervisor.host_check().unwrap();
    h.event(ConnectionEvent::HostResult {
        check,
        answered: false,
    });
    h.advance(999);
    assert!(h.supervisor.host_check().is_none());
    h.advance(1);
    assert_ne!(h.supervisor.host_check(), Some(check));
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
}

#[test]
fn offline_live_session_is_probed_and_long_resume_can_replace_it_offline() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.event(ConnectionEvent::Network(NetworkStatus::Offline));
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
    h.wake(Wakeup::LongResume);
    h.advance(1_500);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    h.ready("relay");
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
}

#[test]
fn failed_normal_probe_while_offline_parks_until_network_returns() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.event(ConnectionEvent::Network(NetworkStatus::Offline));
    h.advance(3_000);
    assert_eq!(h.supervisor.state().phase, Phase::Offline);
    h.event(ConnectionEvent::Network(NetworkStatus::Online));
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn retry_during_foreground_probe_removes_tolerance_without_extending_deadline() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    h.ready("lan");
    h.wake(Wakeup::Foreground);
    h.advance(14_500);
    h.event(ConnectionEvent::Retry);
    h.advance(500);
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn session_loss_during_probe_reconnects_immediately_and_disconnect_always_wins() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("relay");
    h.wake(Wakeup::Foreground);
    h.event(ConnectionEvent::Lost {
        generation,
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    h.event(ConnectionEvent::Disconnect);
    for wake in [
        Wakeup::Foreground,
        Wakeup::NetworkChanged,
        Wakeup::CredentialsChanged,
    ] {
        h.wake(wake);
    }
    assert_eq!(h.supervisor.state().phase, Phase::Available);
    assert!(h.supervisor.active_attempt().is_none());
}

#[test]
fn only_relay_sessions_restart_on_account_change() {
    for route in [lan(), relay()] {
        let mut h = Harness::new(vec![route.clone()], false, NetworkStatus::Online);
        h.connect();
        h.ready(&route.id);
        h.wake(Wakeup::CredentialsChanged);
        assert_eq!(
            h.supervisor.state().phase,
            if route.mode == ConnectionMode::Relay {
                Phase::Connecting
            } else {
                Phase::Connected
            }
        );
    }
}

#[test]
fn long_resume_interrupts_setup_and_restarts_retry_ladder() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    h.fail(FailureKind::Transport);
    h.advance(2_000);
    let old = h.supervisor.active_attempt().unwrap();
    h.wake(Wakeup::LongResume);
    assert_ne!(h.supervisor.active_attempt(), Some(old));
    assert_eq!(h.supervisor.state().attempt, 1);
}

#[test]
fn network_changes_clear_silent_route_backoff_and_stale_checks_are_ignored() {
    let mut h = Harness::new(vec![lan(), relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.advance(60_000);
    let old = h.supervisor.route_check().unwrap();
    h.wake(Wakeup::NetworkChanged);
    h.event(ConnectionEvent::RoutesChecked {
        check: old,
        results: vec![("lan".into(), RoutePreflight::Silent)],
    });
    let current = h.supervisor.route_check().unwrap();
    assert_ne!(old, current);
    h.event(ConnectionEvent::RoutesChecked {
        check: current,
        results: vec![("lan".into(), RoutePreflight::Ready)],
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    assert_eq!(h.supervisor.attempt_routes()[0].id, "lan");
}

#[test]
fn silent_better_routes_back_off_but_unprepared_routes_keep_minute_checks() {
    for preflight in [RoutePreflight::Silent, RoutePreflight::Unprepared] {
        let mut h = Harness::new(vec![lan(), relay()], false, NetworkStatus::Online);
        h.connect();
        h.ready("relay");
        h.advance(60_000);
        let check = h.supervisor.route_check().unwrap();
        h.event(ConnectionEvent::RoutesChecked {
            check,
            results: vec![("lan".into(), preflight)],
        });
        h.advance(60_000);
        assert_eq!(
            h.supervisor.route_check().is_some(),
            preflight == RoutePreflight::Unprepared
        );
        if preflight == RoutePreflight::Silent {
            h.advance(60_000);
            assert!(h.supervisor.route_check().is_some());
        }
    }
}

#[test]
fn failed_route_upgrade_has_five_minute_cooldown() {
    let mut h = Harness::new(vec![lan(), relay()], false, NetworkStatus::Online);
    h.connect();
    h.ready("relay");
    h.advance(60_000);
    let check = h.supervisor.route_check().unwrap();
    h.event(ConnectionEvent::RoutesChecked {
        check,
        results: vec![("lan".into(), RoutePreflight::Ready)],
    });
    h.ready("relay");
    for _ in 0..4 {
        h.advance(60_000);
        assert!(h.supervisor.route_check().is_none());
    }
    h.advance(60_000);
    assert!(h.supervisor.route_check().is_some());
}

#[test]
fn route_driver_shares_head_start_and_prioritizes_late_answers_over_silence() {
    let a = lan();
    let b = route("vpn", ConnectionMode::Remote, "http://100.64.0.2/");
    let mut selection = RouteSelection::new(vec![a.clone(), b.clone(), relay()], 0);
    selection.checked("relay", RouteCheck::Unchecked);
    assert_eq!(selection.next_route(499), None);
    assert_eq!(selection.next_route(500), Some(relay()));
    selection.failed(failure(FailureKind::Transport));
    selection.checked("vpn", RouteCheck::Answered);
    selection.checked("lan", RouteCheck::Silent);
    assert_eq!(selection.next_route(2_500), Some(b));
    selection.failed(failure(FailureKind::Transport));
    assert_eq!(selection.next_route(2_500), None);
    assert!(selection.finished());
}

#[test]
fn route_checks_continue_during_a_pending_connection_and_cancel_with_it() {
    use futures::{FutureExt, channel::oneshot, future::LocalBoxFuture};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct Driver {
        answer: RefCell<Option<oneshot::Receiver<()>>>,
        checked: Rc<Cell<bool>>,
        connecting: Rc<Cell<bool>>,
    }
    impl ConnectionDriver for Driver {
        type Lease = ();
        fn check_route<'a>(&'a self, route: &'a Route) -> LocalBoxFuture<'a, RouteCheck> {
            async move {
                if route.id == "lan" {
                    let answer = self.answer.borrow_mut().take().unwrap();
                    answer.await.unwrap();
                    self.checked.set(true);
                    RouteCheck::Answered
                } else {
                    RouteCheck::Unchecked
                }
            }
            .boxed_local()
        }
        fn connect_route<'a>(
            &'a self,
            route: &'a Route,
        ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>> {
            async move {
                assert_eq!(route.id, "relay");
                self.connecting.set(true);
                futures::future::pending().await
            }
            .boxed_local()
        }
    }

    let platform = FakePlatform::default();
    let (answer, response) = oneshot::channel();
    let checked = Rc::new(Cell::new(false));
    let connecting = Rc::new(Cell::new(false));
    let driver = Driver {
        answer: RefCell::new(Some(response)),
        checked: checked.clone(),
        connecting: connecting.clone(),
    };
    let mut connection = Box::pin(connect_over_routes(
        &platform,
        &driver,
        vec![lan(), relay()],
    ));
    assert!(connection.as_mut().now_or_never().is_none());
    platform.advance(ROUTE_CHECK_HEAD_START_MS);
    assert!(connection.as_mut().now_or_never().is_none());
    assert!(connecting.get());
    answer.send(()).unwrap();
    assert!(connection.as_mut().now_or_never().is_none());
    assert!(checked.get());
    drop(connection);
    assert_eq!(platform.pending_timers(), 0);
}

#[test]
fn route_driver_prefers_early_lan_and_tries_silence_if_only_blocked_routes_failed() {
    let mut selection = RouteSelection::new(vec![lan(), relay()], 0);
    selection.checked("lan", RouteCheck::Answered);
    selection.checked("relay", RouteCheck::Unchecked);
    assert_eq!(selection.next_route(100), Some(lan()));
    selection.failed(failure(FailureKind::Authentication));
    assert_eq!(selection.next_route(100), Some(relay()));
    selection.failed(failure(FailureKind::Transport));
    assert_eq!(selection.failure().unwrap().kind, FailureKind::Transport);

    let mut selection = RouteSelection::new(vec![lan(), relay()], 0);
    selection.checked("lan", RouteCheck::Silent);
    selection.checked("relay", RouteCheck::Unchecked);
    assert_eq!(selection.next_route(500), Some(relay()));
    selection.failed(failure(FailureKind::Authentication));
    assert_eq!(selection.next_route(2_500), Some(lan()));
}

#[test]
fn incompatible_server_stops_all_routes_and_unknown_route_cannot_win() {
    let mut selection = RouteSelection::new(vec![lan(), relay()], 0);
    selection.checked("lan", RouteCheck::Answered);
    assert_eq!(selection.next_route(0), Some(lan()));
    selection.failed(failure(FailureKind::Unsupported));
    assert!(selection.finished());
    assert_eq!(selection.next_route(10_000), None);

    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    let attempt = h.connect();
    h.event(ConnectionEvent::Ready {
        attempt,
        route_id: "unregistered".into(),
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn a_network_signal_after_a_frame_starts_a_fresh_host_check() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("relay");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::LongResume);
    h.advance(500);
    h.event(ConnectionEvent::Frame { generation });
    h.wake(Wakeup::NetworkChanged);
    h.advance(500);
    assert!(h.supervisor.host_check().is_some());
    h.event(ConnectionEvent::HostResult {
        check: h.supervisor.host_check().unwrap(),
        answered: true,
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn concurrent_network_signal_keeps_the_long_resume_window() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("relay");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::LongResume);
    h.advance(250);
    h.wake(Wakeup::NetworkChanged);
    h.advance(250);
    h.event(ConnectionEvent::HostResult {
        check: h.supervisor.host_check().unwrap(),
        answered: true,
    });
    assert_eq!(h.supervisor.state().phase, Phase::Connecting);
}

#[test]
fn initial_config_is_required_before_fresh_host_checks() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("lan");
    h.wake(Wakeup::ForegroundProbe);
    h.advance(500);
    assert!(h.supervisor.host_check().is_none());
    h.event(ConnectionEvent::ConfigReady { generation });
    assert!(h.supervisor.host_check().is_some());
}

#[test]
fn stale_probe_and_host_results_cannot_mutate_a_new_session() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    let generation = h.ready("lan");
    h.event(ConnectionEvent::ConfigReady { generation });
    h.wake(Wakeup::ForegroundProbe);
    let probe = h.probe();
    h.advance(500);
    let check = h.supervisor.host_check().unwrap();
    h.event(ConnectionEvent::HostResult {
        check,
        answered: true,
    });
    h.ready("lan");
    let generation = h.supervisor.state().generation;
    h.event(ConnectionEvent::ProbeResult {
        probe,
        result: Err(failure(FailureKind::Authentication)),
    });
    h.event(ConnectionEvent::HostResult {
        check,
        answered: true,
    });
    assert_eq!(h.supervisor.state().generation, generation);
    assert_eq!(h.supervisor.state().phase, Phase::Connected);
}

#[test]
fn retrying_a_healthy_session_does_not_reset_a_later_failure_ladder() {
    let mut h = Harness::new(vec![lan()], false, NetworkStatus::Online);
    h.connect();
    h.fail(FailureKind::Transport);
    h.advance(2_000);
    let generation = h.ready("lan");
    h.event(ConnectionEvent::Retry);
    h.event(ConnectionEvent::ProbeResult {
        probe: h.probe(),
        result: Ok(()),
    });
    h.event(ConnectionEvent::Lost {
        generation,
        failure: failure(FailureKind::Transport),
    });
    assert_eq!(h.supervisor.state().retry_at, Some(6_000));
}

#[test]
fn route_driver_attempts_all_silent_routes_when_none_failed_transiently() {
    let mut selection = RouteSelection::new(vec![lan(), relay()], 0);
    selection.checked("lan", RouteCheck::Silent);
    selection.checked("relay", RouteCheck::Silent);
    assert_eq!(selection.next_route(2_500), Some(lan()));
    selection.failed(failure(FailureKind::Authentication));
    assert_eq!(selection.next_route(2_500), Some(relay()));
    selection.failed(failure(FailureKind::Permission));
    assert!(selection.finished());
    assert_eq!(
        selection.failure().unwrap().kind,
        FailureKind::Authentication
    );
}

#[test]
fn retry_deadline_uses_monotonic_time_while_display_uses_epoch() {
    let mut supervisor = Supervisor::new(vec![relay()], false, true, NetworkStatus::Online);
    supervisor.handle(
        ConnectionEvent::Connect,
        Time {
            monotonic_ms: 50,
            epoch_ms: 10_000,
        },
        0.0,
    );
    let attempt = supervisor.active_attempt().unwrap();
    supervisor.handle(
        ConnectionEvent::Failed {
            attempt,
            failure: failure(FailureKind::Network),
        },
        Time {
            monotonic_ms: 100,
            epoch_ms: 10_050,
        },
        0.0,
    );
    assert_eq!(supervisor.next_deadline(), Some(1_100));
    assert_eq!(supervisor.state().retry_at, Some(11_050));
    supervisor.handle(
        ConnectionEvent::Tick,
        Time {
            monotonic_ms: 1_099,
            epoch_ms: 900_000,
        },
        0.0,
    );
    assert_eq!(supervisor.state().phase, Phase::Backoff);
    supervisor.handle(
        ConnectionEvent::Tick,
        Time {
            monotonic_ms: 1_100,
            epoch_ms: 10,
        },
        0.0,
    );
    assert_eq!(supervisor.state().phase, Phase::Connecting);
}

#[test]
fn simultaneous_candidates_prefer_direct_and_cancel_the_loser() {
    use futures::{FutureExt, future};
    use std::{cell::Cell, rc::Rc};
    struct Cancelled(Rc<Cell<bool>>);
    impl Drop for Cancelled {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let cancelled = Rc::new(Cell::new(false));
    let scope = Cancelled(cancelled.clone());
    let relay = async move {
        let _scope = scope;
        future::pending::<Result<&str, ConnectionFailure>>().await
    }
    .boxed_local();
    let result = futures::executor::block_on(race_candidates(
        future::ready(Ok("direct")).boxed_local(),
        relay,
    ));
    assert_eq!(result.unwrap(), "direct");
    assert!(cancelled.get());
    assert_eq!(
        futures::executor::block_on(race_candidates(
            future::ready(Ok("direct")).boxed_local(),
            future::ready(Ok("relay")).boxed_local()
        ))
        .unwrap(),
        "direct"
    );
}

#[test]
fn candidate_race_keeps_a_fallback_and_prefers_transient_over_blocked_failure() {
    use futures::{FutureExt, future};
    for first in [FailureKind::Transport, FailureKind::Authentication] {
        let result = futures::executor::block_on(race_candidates(
            future::ready(Err(failure(first))).boxed_local(),
            future::ready(Ok("relay")).boxed_local(),
        ));
        assert_eq!(result.unwrap(), "relay");
    }
    let result: Result<(), _> = futures::executor::block_on(race_candidates(
        future::ready(Err(failure(FailureKind::Authentication))).boxed_local(),
        future::ready(Err(failure(FailureKind::Transport))).boxed_local(),
    ));
    assert_eq!(result.unwrap_err().kind, FailureKind::Transport);
    let result: Result<(), _> = futures::executor::block_on(race_candidates(
        future::ready(Err(failure(FailureKind::Authentication))).boxed_local(),
        future::ready(Err(failure(FailureKind::Permission))).boxed_local(),
    ));
    assert_eq!(result.unwrap_err().kind, FailureKind::Authentication);
}

#[test]
fn learned_routes_are_ordered_with_saved_routes_and_reuse_correct_credentials() {
    let saved = vec![relay()];
    let endpoints = vec![
        ReportedEndpoint {
            http_base_url: "http://192.168.1.5:3773/path".into(),
            kind: None,
        },
        ReportedEndpoint {
            http_base_url: "http://100.64.0.5:3773/".into(),
            kind: Some("tailnet".into()),
        },
        ReportedEndpoint {
            http_base_url: "https://host.test/".into(),
            kind: None,
        },
        ReportedEndpoint {
            http_base_url: "http://127.0.0.1:3773/".into(),
            kind: None,
        },
    ];
    let learned = merge_learned_routes(&saved, &saved[0], &endpoints, true, true, &[]).unwrap();
    assert_eq!(
        learned
            .routes
            .iter()
            .map(|r| route_kind(r, learned.tailscale_route_ids.contains(&r.id)))
            .collect::<Vec<_>>(),
        [
            RouteKind::Lan,
            RouteKind::Tailnet,
            RouteKind::Public,
            RouteKind::Relay
        ]
    );
    assert!(
        learned.routes[..3]
            .iter()
            .all(|r| r.id.starts_with("learned:environment-1:")
                && credential_connection_id(&r.id) == r.id)
    );
    assert!(
        merge_learned_routes(
            &learned.routes,
            &saved[0],
            &endpoints,
            true,
            true,
            &learned.tailscale_route_ids
        )
        .is_none()
    );
    let withdrawn = merge_learned_routes(
        &learned.routes,
        &saved[0],
        &[],
        true,
        true,
        &learned.tailscale_route_ids,
    )
    .unwrap();
    assert_eq!(withdrawn.routes, saved);
    let secure = merge_learned_routes(&saved, &saved[0], &endpoints, false, true, &[]).unwrap();
    assert_eq!(secure.routes.len(), 2);
    assert!(secure.routes[0].http_base.starts_with("https:"));
}

#[test]
fn learned_routes_cannot_send_an_https_paired_token_over_plain_lan() {
    let paired = route("paired", ConnectionMode::Remote, "https://host.test/");
    let endpoints = vec![
        ReportedEndpoint {
            http_base_url: "http://192.168.1.5/".into(),
            kind: None,
        },
        ReportedEndpoint {
            http_base_url: "http://100.64.0.5/".into(),
            kind: Some("tailnet".into()),
        },
        ReportedEndpoint {
            http_base_url: "https://other.test/".into(),
            kind: None,
        },
    ];
    let learned = merge_learned_routes(
        std::slice::from_ref(&paired),
        &paired,
        &endpoints,
        true,
        false,
        &[],
    )
    .unwrap();
    assert_eq!(learned.routes.len(), 3);
    assert!(
        learned
            .routes
            .iter()
            .all(|r| !r.http_base.contains("192.168"))
    );
    let tailnet = learned
        .routes
        .iter()
        .find(|r| r.http_base.contains("100.64"))
        .unwrap();
    assert!(sends_token_in_clear(&learned.routes, tailnet, false));
    assert!(!sends_token_in_clear(&learned.routes, tailnet, true));
    let remaining = routes_after_removing(&learned.routes, "paired");
    assert!(remaining.is_empty());
}

#[test]
fn removing_relay_only_removes_its_learned_routes() {
    let saved = vec![lan(), relay()];
    let endpoints = vec![ReportedEndpoint {
        http_base_url: "https://other.test/".into(),
        kind: None,
    }];
    let learned = merge_learned_routes(&saved, &saved[1], &endpoints, true, true, &[]).unwrap();
    assert_eq!(routes_after_removing(&learned.routes, "relay"), vec![lan()]);
    assert_eq!(
        routes_after_removing(&learned.routes, "missing"),
        learned.routes
    );
}

#[test]
fn route_order_overlay_expires_when_catalog_catches_up_or_changes_set() {
    let saved = vec!["a".into(), "b".into()];
    let pending = vec!["b".into(), "a".into()];
    assert_eq!(
        pending_route_order(Some(&pending), &saved),
        Some(pending.clone())
    );
    assert_eq!(pending_route_order(Some(&pending), &pending), None);
    assert_eq!(
        pending_route_order(Some(&pending), &["a".into(), "c".into()]),
        None
    );
    assert_eq!(pending_route_order(Some(&pending), &["a".into()]), None);
    assert_eq!(
        pending_route_order(Some(&["a".into(), "a".into()]), &saved),
        Some(vec!["a".into(), "a".into()])
    );
}

fn diagnosis(state: &ConnectionState, via: ConnectionVia) -> ConnectionDiagnosis {
    present_connection_diagnosis(DiagnosisInput {
        host_label: "Mac mini",
        state,
        via,
        relay_status: None,
        client_version: None,
        server_version: None,
    })
    .unwrap()
}

fn relay_error(raw: serde_json::Value) -> ConnectionFailure {
    use t3_client::{authorization::Sensitive, relay::RelayError};
    let error = serde_json::from_value(raw).unwrap();
    map_managed_relay_error(&RelayError::Request {
        status: 502,
        error: Some(Sensitive(error)),
        trace_id: None,
    })
}

#[test]
fn relay_failures_keep_pending_tunnels_retryable_and_permissions_blocked() {
    use serde_json::json;
    for (reason, kind, cause) in [
        (
            Some("managed_endpoint_allocation_not_ready"),
            FailureKind::EndpointUnavailable,
            Some(FailureCause::TunnelPending),
        ),
        (
            Some("endpoint_provider_not_managed"),
            FailureKind::Permission,
            Some(FailureCause::TunnelNotEnabled),
        ),
        (
            Some("environment_link_not_found"),
            FailureKind::Permission,
            Some(FailureCause::NotLinked),
        ),
        (None, FailureKind::Permission, None),
    ] {
        let mut raw = json!({"_tag":"RelayEnvironmentConnectNotAuthorizedError", "code":"environment_connect_not_authorized", "traceId":"trace-1"});
        if let Some(reason) = reason {
            raw["reason"] = reason.into();
        }
        let failure = relay_error(raw);
        assert_eq!(failure.kind, kind);
        assert_eq!(failure.cause, cause);
        assert_eq!(failure.trace_id.as_deref(), Some("trace-1"));
    }
    let failure = relay_error(
        json!({"_tag":"RelayEnvironmentEndpointTimedOutError", "code":"environment_endpoint_timed_out", "traceId":"trace-2"}),
    );
    assert_eq!(failure.kind, FailureKind::Timeout);
    assert_eq!(
        failure.detail,
        "Relay timed out while contacting the environment endpoint."
    );
}

#[test]
fn dpop_failures_distinguish_clock_skew_from_unknown_proof_failure() {
    use serde_json::json;
    for reason in [None, Some("time_window")] {
        let mut raw = json!({"_tag":"RelayAuthInvalidError", "code":"auth_invalid", "reason":"invalid_dpop", "traceId":"trace-1"});
        if let Some(reason) = reason {
            raw["dpopFailureReason"] = reason.into();
        }
        let failure = relay_error(raw);
        assert_eq!(failure.kind, FailureKind::Authentication);
        assert_eq!(failure.cause, reason.map(|_| FailureCause::ClockSkew));
        assert!(failure.detail.contains("automatic date and time"));
    }
    for error in [
        t3_client::authorization::AuthorizationError::Network,
        t3_client::authorization::AuthorizationError::Timeout,
    ] {
        let failure = map_remote_environment_error(&error, ConnectionVia::Relay);
        assert!(failure.detail.ends_with(NETWORK_BLOCKING_HINT));
        assert_eq!(failure.hop, Some(ConnectionHop::Tunnel));
    }
}

#[test]
fn diagnosis_distinguishes_working_tunnel_from_unreachable_server() {
    use serde_json::json;
    for (cause, status, headline, hop) in [
        (
            "origin_unreachable",
            502,
            "Tunnel works, but T3 Code on Mac mini did not answer",
            ConnectionHop::Server,
        ),
        (
            "tunnel_down",
            530,
            "Mac mini's tunnel is offline",
            ConnectionHop::Tunnel,
        ),
    ] {
        let failure = relay_error(
            json!({"_tag":"RelayEnvironmentEndpointUnavailableError", "code":"environment_endpoint_unavailable", "reason":"endpoint_request_failed", "endpointFailure":cause, "endpointStatus":status, "traceId":"trace-1"}),
        );
        let state = ConnectionState {
            phase: Phase::Backoff,
            attempt: 3,
            last_failure: Some(failure),
            retry_at: Some(1_792_411_230_000),
            ..ConnectionState::default()
        };
        let diagnosis = diagnosis(&state, ConnectionVia::Direct);
        assert_eq!(diagnosis.headline, headline);
        assert_eq!(
            diagnosis
                .hops
                .iter()
                .find(|h| h.result == HopResult::Failed)
                .unwrap()
                .hop,
            hop
        );
        assert_eq!(diagnosis.hops.len(), 4);
        assert!(
            diagnosis
                .facts
                .contains(&("HTTP status".into(), status.to_string()))
        );
        assert!(
            diagnosis
                .facts
                .contains(&("Next retry".into(), "2026-10-19T12:00:30.000Z".into()))
        );
    }
}

#[test]
fn diagnosis_blames_device_for_failed_fetch_after_relay_reached_host() {
    let failure = map_remote_environment_error(
        &t3_client::authorization::AuthorizationError::Network,
        ConnectionVia::Relay,
    );
    let state = ConnectionState {
        last_failure: Some(failure),
        ..ConnectionState::default()
    };
    let diagnosis = diagnosis(&state, ConnectionVia::Direct);
    assert_eq!(
        diagnosis.headline,
        "This device can't reach Mac mini's tunnel"
    );
    assert!(!diagnosis.steps.join(" ").contains("On Mac mini"));
}

#[test]
fn diagnosis_keeps_relay_transport_and_relay_server_errors_distinct() {
    use t3_client::relay::RelayError;
    for (error, expected) in [
        (RelayError::Network, "This device can't reach T3 Connect"),
        (
            RelayError::Request {
                status: 502,
                error: None,
                trace_id: None,
            },
            "T3 Connect returned an error",
        ),
    ] {
        let state = ConnectionState {
            last_failure: Some(map_managed_relay_error(&error)),
            ..ConnectionState::default()
        };
        let diagnosis = diagnosis(&state, ConnectionVia::Relay);
        assert_eq!(diagnosis.headline, expected);
        assert_eq!(
            diagnosis.hops.iter().map(|h| h.result).collect::<Vec<_>>(),
            [
                HopResult::Ok,
                HopResult::Failed,
                HopResult::Unchecked,
                HopResult::Unchecked
            ]
        );
    }
}

#[test]
fn offline_and_session_pending_diagnoses_precede_host_failures() {
    for (kind, network, expected) in [
        (
            FailureKind::Transport,
            NetworkStatus::Offline,
            "This device is offline",
        ),
        (
            FailureKind::SessionPending,
            NetworkStatus::Online,
            "This device is still loading its T3 Connect sign-in",
        ),
    ] {
        let state = ConnectionState {
            network,
            last_failure: Some(failure(kind)),
            ..ConnectionState::default()
        };
        let diagnosis = diagnosis(&state, ConnectionVia::Direct);
        assert_eq!(diagnosis.headline, expected);
        assert_eq!(diagnosis.hops[0].result, HopResult::Failed);
    }
}

#[test]
fn diagnosis_does_not_invent_successful_hops_for_an_unlocated_failure() {
    let state = ConnectionState {
        last_failure: Some(failure(FailureKind::Authentication)),
        ..ConnectionState::default()
    };
    assert!(
        diagnosis(&state, ConnectionVia::Relay)
            .hops
            .iter()
            .all(|h| h.result != HopResult::Ok)
    );
    assert!(
        present_connection_diagnosis(DiagnosisInput {
            host_label: "Host",
            state: &ConnectionState::default(),
            via: ConnectionVia::Direct,
            relay_status: None,
            client_version: None,
            server_version: None
        })
        .is_none()
    );
}

#[test]
fn platform_foreground_policy_preserves_short_and_long_resume_deadlines() {
    use t3_client::platform::PlatformEvent;
    for (policy, background, timeout) in [
        (ForegroundPolicy::TolerateBusyHost, 20_000, 30_000),
        (ForegroundPolicy::ProbeImmediately, 5_000, 15_000),
        (ForegroundPolicy::ProbeImmediately, 10_000, 1_500),
    ] {
        let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
        h.supervisor = h.supervisor.with_foreground_policy(policy);
        h.connect();
        h.ready("relay");
        h.supervisor.platform_event(
            PlatformEvent::Suspended,
            Time::from_platform(&h.platform),
            0.0,
        );
        h.platform.advance(background);
        h.supervisor.platform_event(
            PlatformEvent::Foreground,
            Time::from_platform(&h.platform),
            0.0,
        );
        // A normal foreground allows one miss, so drive that deadline separately.
        if policy == ForegroundPolicy::TolerateBusyHost {
            h.advance(15_000);
            h.advance(14_999);
        } else {
            h.advance(timeout - 1);
        }
        assert_eq!(h.supervisor.state().phase, Phase::Connected);
        h.advance(1);
        assert_eq!(h.supervisor.state().phase, Phase::Connecting);
    }
}

#[test]
fn async_supervisor_consumes_events_and_cancels_on_input_scope_end() {
    use futures::{FutureExt, StreamExt, channel::mpsc};
    use std::{cell::RefCell, rc::Rc};
    let platform = Rc::new(FakePlatform::default());
    let supervisor = Rc::new(RefCell::new(Supervisor::new(
        vec![relay()],
        false,
        true,
        NetworkStatus::Online,
    )));
    let (tx, rx) = mpsc::unbounded();
    let mut stream = supervise(supervisor.clone(), platform.clone(), rx.boxed_local());
    tx.unbounded_send(ConnectionEvent::Connect).unwrap();
    assert!(matches!(
        stream.next().now_or_never().unwrap().unwrap().unwrap(),
        ConnectionAction::Establish { .. }
    ));
    let attempt = supervisor.borrow().active_attempt().unwrap();
    drop(tx);
    assert_eq!(
        stream.next().now_or_never().unwrap().unwrap().unwrap(),
        ConnectionAction::CancelAttempt { attempt }
    );
    assert!(stream.next().now_or_never().unwrap().is_none());
    assert_eq!(supervisor.borrow().state().phase, Phase::Available);
}

#[test]
fn dropping_the_async_supervisor_invalidates_pending_attempts_and_timers() {
    use futures::{FutureExt, StreamExt, channel::mpsc};
    use std::{cell::RefCell, rc::Rc};
    let platform = Rc::new(FakePlatform::default());
    let supervisor = Rc::new(RefCell::new(Supervisor::new(
        vec![relay()],
        false,
        true,
        NetworkStatus::Online,
    )));
    let (tx, rx) = mpsc::unbounded();
    let mut stream = supervise(supervisor.clone(), platform.clone(), rx.boxed_local());
    tx.unbounded_send(ConnectionEvent::Connect).unwrap();
    stream.next().now_or_never().unwrap().unwrap().unwrap();
    let attempt = supervisor.borrow().active_attempt().unwrap();
    assert!(stream.next().now_or_never().is_none());
    assert_eq!(platform.pending_timers(), 1);
    drop(stream);
    assert_eq!(platform.pending_timers(), 0);
    supervisor.borrow_mut().handle(
        ConnectionEvent::Ready {
            attempt,
            route_id: "relay".into(),
        },
        Time::from_platform(platform.as_ref()),
        0.0,
    );
    assert_eq!(supervisor.borrow().state().phase, Phase::Available);
}

#[test]
fn route_kind_and_order_cover_local_lan_tailnet_vpn_public_ssh_and_relay() {
    for (base, kind) in [
        ("http://localhost:3773/", RouteKind::Loopback),
        ("http://[::1]/", RouteKind::Loopback),
        ("http://127.0.0.2/", RouteKind::Loopback),
        ("http://host.local/", RouteKind::Lan),
        ("http://[fd12::1]/", RouteKind::Lan),
        ("http://[::ffff:192.168.1.1]/", RouteKind::Lan),
        ("http://machine.tail.ts.net/", RouteKind::Tailnet),
        ("http://[fd7a:115c:a1e0::1]/", RouteKind::Tailnet),
        ("http://100.64.0.1/", RouteKind::Vpn),
        ("https://host.example/", RouteKind::Public),
    ] {
        assert_eq!(
            route_kind(&route("id", ConnectionMode::Remote, base), false),
            kind
        );
    }
    let mut routes = vec![lan(), relay()];
    let mut second = lan();
    second.id = "new-lan".into();
    upsert_route(&mut routes, second.clone());
    assert_eq!(
        routes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["new-lan", "lan", "relay"]
    );
    second.http_base = "http://192.168.1.11/".into();
    upsert_route(&mut routes, second.clone());
    assert_eq!(routes[0], second);
    assert_eq!(
        credential_connection_id("learned:environment:https://host/@saved-route"),
        "saved-route"
    );
    assert_eq!(
        credential_connection_id("bearer:environment:https://host/"),
        "bearer:environment:https://host/"
    );
}

#[test]
fn routing_trust_is_endpoint_bound_and_independent_of_order_or_learned_routes() {
    let a = lan();
    let b = relay();
    let key = routes_connection_key(&[a.clone(), b.clone()]);
    assert_eq!(key, routes_connection_key(&[b.clone(), a.clone()]));
    let mut learned = a.clone();
    learned.id = "learned:one".into();
    assert_eq!(key, routes_connection_key(&[a.clone(), b, learned]));
    let mut changed = a.clone();
    changed.http_base = "http://192.168.1.50/".into();
    assert_ne!(
        routes_connection_key(std::slice::from_ref(&a)),
        routes_connection_key(&[changed])
    );
    let mut invalid = a;
    invalid.http_base = "https://user:secret@host/".into();
    assert!(routes_connection_key(&[invalid]).is_none());
}

#[test]
fn protocol_gate_accepts_current_and_identifies_updatable_older_hosts() {
    use serde_json::json;
    use t3_contracts::environment::{
        ExecutionEnvironmentDescriptor, ORCHESTRATION_PROTOCOL_VERSION,
    };
    for (protocol, update, desktop, accepted, server_update_required) in [
        (
            Some(ORCHESTRATION_PROTOCOL_VERSION),
            None,
            false,
            true,
            false,
        ),
        (None, None, false, false, false),
        (
            Some(ORCHESTRATION_PROTOCOL_VERSION + 1),
            Some("boot-service"),
            false,
            false,
            false,
        ),
        (
            Some(ORCHESTRATION_PROTOCOL_VERSION - 1),
            Some("boot-service"),
            false,
            false,
            true,
        ),
        (
            Some(ORCHESTRATION_PROTOCOL_VERSION - 1),
            Some("desktop-managed"),
            false,
            false,
            false,
        ),
        (
            Some(ORCHESTRATION_PROTOCOL_VERSION - 1),
            Some("desktop-managed"),
            true,
            false,
            true,
        ),
    ] {
        let mut raw = json!({"environmentId":"remote", "label":"Build Mac", "platform":{"os":"darwin", "arch":"arm64"}, "serverVersion":"9.0.0", "capabilities":{"repositoryIdentity":true}});
        if let Some(protocol) = protocol {
            raw["orchestrationProtocolVersion"] = protocol.into();
        }
        if let Some(update) = update {
            raw["capabilities"]["serverSelfUpdate"] = update.into();
        }
        raw["capabilities"]["desktopAppUpdate"] = desktop.into();
        let descriptor: ExecutionEnvironmentDescriptor = serde_json::from_value(raw).unwrap();
        let error = orchestration_protocol_compatibility_error(&descriptor);
        assert_eq!(error.is_none(), accepted);
        if let Some(error) = error {
            assert_eq!(error.server_update_required, server_update_required);
        }
    }
    let url = append_orchestration_protocol(
        "wss://host.test/ws?wsTicket=secret&connectionMethod=relay&orchestrationProtocol=0",
    )
    .unwrap();
    let url = url::Url::parse(&url).unwrap();
    let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(pairs.get("wsTicket").unwrap(), "secret");
    assert_eq!(pairs.get("connectionMethod").unwrap(), "relay");
    assert_eq!(pairs.get("orchestrationProtocol").unwrap(), "2");
    assert_eq!(
        pairs.get("clientCapabilities").unwrap(),
        "settingsSecretReferences"
    );
}

#[test]
fn presentation_distinguishes_initial_connect_retry_and_available_offline() {
    let mut state = ConnectionState {
        desired: true,
        network: NetworkStatus::Online,
        phase: Phase::Connecting,
        stage: Some(AttemptStage::Preparing),
        attempt: 1,
        ..ConnectionState::default()
    };
    assert_eq!(
        present_connection_state(&state).phase,
        PresentationPhase::Connecting
    );
    let mut error = failure(FailureKind::Transport);
    error.trace_id = Some("trace-1".into());
    state.last_failure = Some(error.clone());
    state.attempt = 2;
    for phase in [Phase::Connecting, Phase::Backoff] {
        state.phase = phase;
        let presentation = present_connection_state(&state);
        assert_eq!(presentation.phase, PresentationPhase::Reconnecting);
        assert_eq!(presentation.error, Some(error.detail.clone()));
        assert_eq!(presentation.trace_id, error.trace_id);
    }
    state.phase = Phase::Available;
    state.network = NetworkStatus::Offline;
    state.desired = false;
    let presentation = present_connection_state(&state);
    assert_eq!(presentation.phase, PresentationPhase::Available);
    assert_eq!(presentation.error, None);
}

#[test]
fn mcp_address_prefers_relay_then_active_route_and_skips_missing_addresses() {
    let a = lan();
    let b = route(
        "tailnet",
        ConnectionMode::Remote,
        "http://100.115.1.44:3773/",
    );
    assert_eq!(
        environment_mcp_url(&[a.clone(), b.clone()], Some("tailnet"), None).unwrap(),
        "http://100.115.1.44:3773/mcp"
    );
    assert_eq!(
        environment_mcp_url(
            &[a.clone(), b.clone()],
            Some("lan"),
            Some("https://tunnel.test/")
        )
        .unwrap(),
        "https://tunnel.test/mcp"
    );
    let mut relay = relay();
    relay.http_base.clear();
    assert_eq!(
        environment_mcp_url(&[relay, a], Some("relay"), None).unwrap(),
        "http://192.168.1.10:3773/mcp"
    );
}

fn approval_pending() -> ConnectionFailure {
    let mut failure = ConnectionFailure::new(
        FailureKind::ApprovalPending,
        "Approve this device on Mac mini. It shows code K7QM-2XFD.",
    );
    failure.cause = Some(FailureCause::ApprovalPending);
    failure
}

fn fail_with(h: &mut Harness, failure: ConnectionFailure) -> u64 {
    let attempt = h.supervisor.active_attempt().unwrap();
    h.event(ConnectionEvent::Failed { attempt, failure });
    assert_eq!(h.supervisor.state().phase, Phase::Backoff);
    h.supervisor.state().retry_at.unwrap() - h.platform.now_epoch_ms()
}

#[test]
fn a_pending_approval_checks_back_every_four_seconds_without_climbing_the_ladder() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    for attempt in 1..=8 {
        assert_eq!(h.supervisor.state().attempt, attempt);
        assert_eq!(fail_with(&mut h, approval_pending()), 4_000);
        h.advance(4_000);
    }
    // The waits do not count toward backoff, so an outage right after starts at the bottom.
    assert!(fail_with(&mut h, failure(FailureKind::Transport)) <= 2_000);
}

#[test]
fn a_device_nobody_approved_backs_off_once_its_request_expired() {
    let mut h = Harness::new(vec![relay()], false, NetworkStatus::Online);
    h.connect();
    let mut delays = Vec::new();
    for _ in 0..156 {
        let delay = fail_with(&mut h, approval_pending());
        delays.push(delay);
        h.advance(delay);
    }
    // Ten minutes of 4 s checks, then backoff that never polls faster than that.
    assert!(delays[..150].iter().all(|delay| *delay == 4_000));
    assert!(delays[150..].iter().all(|delay| *delay >= 4_000));
    assert!(*delays.last().unwrap() >= 32_000);
}

#[test]
fn a_pending_approval_reconnects_as_a_wait_and_says_where_to_approve() {
    let mut pending = approval_pending();
    pending.trace_id = Some("trace-2".into());
    let state = ConnectionState {
        desired: true,
        network: NetworkStatus::Online,
        phase: Phase::Backoff,
        attempt: 1,
        retry_at: Some(1),
        last_failure: Some(pending.clone()),
        ..ConnectionState::default()
    };
    let presentation = present_connection_state(&state);
    assert_eq!(
        presentation,
        ConnectionPresentation {
            phase: PresentationPhase::Reconnecting,
            error: Some("Approve this device on Mac mini. It shows code K7QM-2XFD.".into()),
            trace_id: Some("trace-2".into()),
            awaiting_approval: true,
        }
    );
    assert_eq!(
        connection_status_text(&presentation),
        "Approve this device on Mac mini. It shows code K7QM-2XFD."
    );
    assert_eq!(
        connection_status_title(&presentation),
        "Waiting for approval"
    );

    pending.hop = Some(ConnectionHop::Server);
    pending.via = Some(ConnectionVia::Relay);
    pending.verification_code = Some("K7QM-2XFD".into());
    let diagnosis = diagnosis(
        &ConnectionState {
            last_failure: Some(pending),
            ..state
        },
        ConnectionVia::Relay,
    );
    assert_eq!(
        diagnosis.headline,
        "Mac mini is waiting for someone to approve this device"
    );
    assert!(
        diagnosis
            .steps
            .join(" ")
            .contains("t3 auth connect-device approve K7QM-2XFD")
    );
}

#[test]
fn the_hosts_pending_approval_answer_is_a_wait_not_a_permission_failure() {
    let error: t3_contracts::environment_http::EnvironmentHttpCommonError = serde_json::from_value(
        serde_json::json!({"_tag":"EnvironmentConnectApprovalPendingError","code":"connect_approval_pending","traceId":"trace-9"}),
    )
    .unwrap();
    let failure = map_remote_environment_error(
        &t3_client::authorization::AuthorizationError::Remote {
            status: 403,
            details: t3_client::authorization::Sensitive(Box::new(error)),
        },
        ConnectionVia::Relay,
    );
    assert_eq!(failure.kind, FailureKind::ApprovalPending);
    assert_eq!(failure.cause, Some(FailureCause::ApprovalPending));
    assert_eq!(failure.trace_id.as_deref(), Some("trace-9"));
    assert!(!failure.is_blocked());
}
