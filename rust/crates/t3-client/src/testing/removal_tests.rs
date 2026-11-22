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

#[test]
fn a_removal_whose_caller_is_dropped_still_removes_everything() {
    // Coordinator, round 6: the menu that starts a removal is disposed when its row goes, which
    // dropped `remove` at its wait for the driver's end, so the store was never cleared. A
    // navigation, a closing window or a second caller does the same.
    use crate::platform::{Storage, StorageArea};
    use std::task::Context;
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: crate::composition::Spawner = Rc::new(move |future| {
        spawner.spawn_local(future).unwrap();
    });
    let server = FakeEnvironment::new("env", spawn.clone());
    let platform = server.platform();
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform.clone(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawn,
    );
    let environment = pool.run_until(client.pair(server.pairing())).unwrap();
    pool.run_until_stalled();
    assert!(store.snapshot().environments.contains_key(&environment));
    let mut removal = client.remove(&environment);
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    // Until the list no longer has it, as the row that started it disappears.
    while !client.environments().is_empty() {
        assert!(
            removal.as_mut().poll(&mut context).is_pending(),
            "still removing when the row goes"
        );
        pool.run_until_stalled();
    }
    drop(removal);
    pool.run_until_stalled();
    assert!(
        !store.snapshot().environments.contains_key(&environment),
        "{:?}",
        store.snapshot().environments.get(&environment)
    );
    let left = pool.run_until(async move {
        let mut left = Vec::new();
        for (area, namespace) in [
            (StorageArea::Ordinary, "connection-catalog"),
            (StorageArea::Secure, "connection-credential"),
            (StorageArea::Secure, "connection-profile"),
        ] {
            for key in platform.list(area, namespace.into()).await.unwrap() {
                let bytes = platform
                    .get(area, key.clone())
                    .await
                    .unwrap()
                    .unwrap_or_default();
                if String::from_utf8_lossy(&bytes).contains("\"env\"") {
                    left.push(key);
                }
            }
        }
        left
    });
    assert!(left.is_empty(), "{left:?}");
}
