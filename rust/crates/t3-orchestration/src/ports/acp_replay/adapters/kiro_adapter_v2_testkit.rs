#![cfg(feature = "acp-adapter")]

use crate::{
    adapters::acp::AcpAdapter,
    ports::acp_replay::testkit::provider_replay_harness::{
        AcpFlavorAdapterFactory, ProviderReplayHarness,
    },
    provider::{ProviderError, ProviderResult},
    testing::replay::{ReplayGate, ReplayTranscript},
};

pub const DRIVER: &str = "kiro";

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

#[derive(Clone, Default)]
pub struct KiroCancelTestHooks {
    pub on_cancel_held: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    pub on_cancel_decided: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    pub on_cancel_written: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

// Local shim: Kiro owns the pre-start cancel hold and acknowledgement wait.
#[async_trait::async_trait]
pub trait KiroAdapterFactoryWithTestHooks: Send + Sync {
    async fn create(
        &self,
        base: AcpAdapter,
        gate: ReplayGate,
        hooks: KiroCancelTestHooks,
    ) -> ProviderResult<std::sync::Arc<dyn crate::provider::ProviderAdapter>>;
}

pub async fn make_replay_harness_with_test_hooks(
    transcript: ReplayTranscript,
    base: AcpAdapter,
    gate: ReplayGate,
    hooks: KiroCancelTestHooks,
    factory: &dyn KiroAdapterFactoryWithTestHooks,
) -> ProviderResult<ProviderReplayHarness> {
    struct WithHooks<'a> {
        factory: &'a dyn KiroAdapterFactoryWithTestHooks,
        hooks: KiroCancelTestHooks,
    }
    #[async_trait::async_trait]
    impl AcpFlavorAdapterFactory for WithHooks<'_> {
        async fn create(
            &self,
            base: AcpAdapter,
            gate: ReplayGate,
        ) -> ProviderResult<std::sync::Arc<dyn crate::provider::ProviderAdapter>> {
            self.factory.create(base, gate, self.hooks.clone()).await
        }
    }
    make_replay_harness(transcript, base, gate, &WithHooks { factory, hooks }).await
}
