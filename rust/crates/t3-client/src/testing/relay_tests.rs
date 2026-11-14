//! T3 Connect over the connectable fake, as UI tests use it: a signed-in host lists this
//! environment through the relay and connects to it there.
#![allow(clippy::unwrap_used)]
use super::environment::FakeEnvironment;
use crate::{
    ClientStore, composition::ClientRuntime, connection::Phase, types::EnvironmentId,
};
use futures::{executor::LocalPool, task::LocalSpawnExt};
use std::rc::Rc;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

#[test]
fn a_signed_in_host_lists_the_environment_through_the_relay_and_connects_there() {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: crate::composition::Spawner = Rc::new(move |future| {
        spawner.spawn_local(future).unwrap();
    });
    let server = FakeEnvironment::new("env", spawn.clone());
    server.sign_in(true);
    let client = ClientRuntime::with_relay(
        server.platform(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawn,
        FakeEnvironment::relay_config(),
    );
    pool.run_until(client.restore()).unwrap();
    pool.run_until_stalled();
    let environment = EnvironmentId("env".into());
    let discovered = client.relay_environments().unwrap();
    assert!(discovered.listed, "{discovered:?} {:?}", server.relay_requests());
    assert_eq!(
        discovered.environments[&environment].environment.label.0,
        "Fake host"
    );

    pool.run_until(client.add_relay_environment(&environment, "Fake host"))
        .unwrap();
    pool.run_until_stalled();
    let entry = &client.environments()[0];
    assert_eq!(entry.state.phase, Phase::Connected, "{:?}", entry.state);
    assert!(
        server
            .relay_requests()
            .contains(&"POST /v1/environments/env/connect".to_owned()),
        "{:?}",
        server.relay_requests()
    );
}
