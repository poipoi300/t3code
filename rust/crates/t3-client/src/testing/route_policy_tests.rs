//! The route policy the runtime hands the platform: nothing before the first restore, then
//! signed in exactly while an environment is saved, published once per change.
#![allow(clippy::unwrap_used)]
use super::environment::FakeEnvironment;
use crate::{ClientStore, composition::ClientRuntime, navigation::RoutePolicy};
use futures::{executor::LocalPool, task::LocalSpawnExt};
use std::rc::Rc;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

#[test]
fn the_route_policy_is_published_from_the_first_restore_and_follows_the_saved_environments() {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: crate::composition::Spawner = Rc::new(move |future| {
        spawner.spawn_local(future).unwrap();
    });
    let server = FakeEnvironment::new("env", spawn.clone());
    let platform = server.platform();
    let client = ClientRuntime::new(
        platform.clone(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawn,
    );
    pool.run_until_stalled();
    assert_eq!(client.route_policy(), None, "unknown before the first restore");
    assert!(platform.fake.route_policies().is_empty());

    pool.run_until(client.restore()).unwrap();
    pool.run_until_stalled();
    let signed_out = RoutePolicy::for_client(false);
    let signed_in = RoutePolicy::for_client(true);
    assert_eq!(client.route_policy(), Some(signed_out));
    assert_eq!(platform.fake.route_policies(), [signed_out]);

    let environment = pool.run_until(client.pair(server.pairing())).unwrap();
    pool.run_until_stalled();
    assert_eq!(client.route_policy(), Some(signed_in));
    assert_eq!(
        platform.fake.route_policies(),
        [signed_out, signed_in],
        "once per change, however often the connection state moves"
    );

    pool.run_until(client.remove(&environment)).unwrap();
    pool.run_until_stalled();
    assert_eq!(
        platform.fake.route_policies(),
        [signed_out, signed_in, signed_out]
    );
}
