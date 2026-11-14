//! Removing an environment over the connectable fake, as the connection lane's UI test does:
//! nothing that ran for it may leave it in the store, or bring it back.
#![allow(clippy::unwrap_used)]
use super::environment::FakeEnvironment;
use crate::{ClientStore, composition::ClientRuntime, types::EnvironmentId};
use futures::{executor::LocalPool, task::LocalSpawnExt};
use std::rc::Rc;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

#[test]
fn removing_a_paired_environment_leaves_nothing_of_it_in_the_store() {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: crate::composition::Spawner = Rc::new(move |future| {
        spawner.spawn_local(future).unwrap();
    });
    let server = FakeEnvironment::new("env", spawn.clone());
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawn,
    );
    let environment = pool.run_until(client.pair(server.pairing())).unwrap();
    pool.run_until_stalled();
    assert_eq!(environment, EnvironmentId("env".into()));
    assert!(
        store
            .snapshot()
            .environments
            .get(&environment)
            .is_some_and(|state| state.config.is_some()),
        "connected, with its config"
    );
    pool.run_until(client.remove(&environment)).unwrap();
    pool.run_until_stalled();
    assert!(client.environments().is_empty());
    assert!(
        !store.snapshot().environments.contains_key(&environment),
        "{:?}",
        store.snapshot().environments.get(&environment)
    );
}
