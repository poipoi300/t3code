#![cfg(feature = "acp-adapter")]

use crate::{
    adapters::acp::AcpAdapter,
    ports::acp_replay::testkit::provider_replay_harness::ProviderReplayHarness,
    testing::replay::{ReplayError, ReplayGate, ReplayTranscript},
};

pub fn make_acp_registry_replay_harness(
    transcript: ReplayTranscript,
    adapter: AcpAdapter,
    gate: ReplayGate,
) -> Result<ProviderReplayHarness, ReplayError> {
    if adapter.driver.0 != "acpRegistry" {
        return Err(ReplayError::Protocol {
            expected: "acpRegistry".into(),
            actual: adapter.driver.0,
            protocol: transcript.protocol.0,
        });
    }
    ProviderReplayHarness::new(transcript, adapter, gate, true)
}

pub async fn make_acp_registry_replay_harness_with_factory(
    mut transcript: ReplayTranscript,
    adapter: AcpAdapter,
    gate: ReplayGate,
    factory:&dyn crate::ports::acp_replay::testkit::provider_replay_harness::AcpFlavorAdapterFactory,
) -> crate::provider::ProviderResult<ProviderReplayHarness> {
    if adapter.driver.0 != "acpRegistry" {
        return Err(crate::provider::ProviderError::Unsupported(
            "registry factory requires acpRegistry driver",
        ));
    }
    transcript.provider.0 = "acpRegistry".into();
    ProviderReplayHarness::with_flavor_factory(transcript, adapter, gate, factory).await
}
