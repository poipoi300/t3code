#![cfg(feature = "acp-adapter")]
#![allow(
    dead_code,
    reason = "Native conformance entry points await the named provider and host bindings."
)]

use t3_orchestration::{
    error::Result,
    ports::acp_replay::testkit::{
        provider_replay_fixtures::native::{FixtureAdapterFactory, run_fixture_suite},
        provider_replay_harness::ReplayFixtureHostFactory,
    },
};

// Native factory and host binding are supplied by the integrator; no real provider is started.
pub async fn run_kiro_recorded_fixtures(
    factory: &dyn FixtureAdapterFactory,
    host: &dyn ReplayFixtureHostFactory,
) -> Result<usize> {
    run_fixture_suite("kiro", factory, host).await
}
