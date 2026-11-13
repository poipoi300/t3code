#![cfg(feature = "acp-adapter")]

use crate::{
    adapters::acp::AcpAdapter,
    ports::acp_replay::testkit::provider_replay_harness::{
        AcpFlavorAdapterFactory, ProviderReplayHarness,
    },
    provider::{ProviderError, ProviderResult},
    testing::replay::{ReplayGate, ReplayTranscript},
};

pub const DRIVER: &str = "grok";

pub async fn make_replay_harness(
    transcript: ReplayTranscript,
    base: AcpAdapter,
    gate: ReplayGate,
    factory: &dyn AcpFlavorAdapterFactory,
) -> ProviderResult<ProviderReplayHarness> {
    if base.driver.0 != DRIVER {
        return Err(ProviderError::Protocol {
            reason: None,
            detail: format!("expected {DRIVER} adapter, received {}", base.driver.0),
            payload: None,
        });
    }
    ProviderReplayHarness::with_flavor_factory(transcript, base, gate, factory).await
}
