#![cfg(feature = "acp-adapter")]
#![allow(
    dead_code,
    reason = "Native conformance entry points await the named provider and host bindings."
)]

use std::sync::{Arc, Mutex};
use t3_orchestration::{
    error::{Error, Result},
    ports::acp_replay::testkit::{
        provider_replay_fixtures::{fixture_cases, native::*},
        provider_replay_harness::ReplayFixtureHostFactory,
    },
};

pub async fn run_codex_recorded_fixtures(
    factory: &dyn FixtureAdapterFactory,
    host: &dyn ReplayFixtureHostFactory,
) -> Result<usize> {
    run_fixture_suite("codex", factory, host).await
}

pub async fn codex_snapshots_use_bounded_provider_transactions(
    factory: &dyn FixtureAdapterFactory,
    host: &dyn ReplayFixtureHostFactory,
) -> Result<()> {
    let (fixture, variant) = fixture_cases("codex")?
        .into_iter()
        .find(|(fixture, _)| fixture.name == "multi_turn")
        .ok_or(Error::Unsupported("Codex multi_turn replay fixture"))?;
    let writes = Arc::new(Mutex::new(Vec::new()));
    let captured = writes.clone();
    run_fixture_case(
        &fixture,
        variant,
        factory,
        host,
        FixtureObservations {
            provider_batches: Some(Arc::new(move |count| {
                captured
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(count)
            })),
            ..Default::default()
        },
    )
    .await?;
    let writes = writes.lock().unwrap_or_else(|e| e.into_inner());
    if writes.iter().sum::<usize>() != 6 || !writes.iter().all(|count| *count <= 3) {
        return Err(Error::Service {
            operation: "Codex provider transactions",
            detail: format!("expected 6 writes in batches <=3, got {writes:?}"),
        });
    }
    Ok(())
}

#[cfg(feature = "codex-adapter")]
pub mod native {
    use super::*;
    use std::path::Path;
    use t3_orchestration::{
        adapters::codex::CodexAdapterOptions,
        ports::{
            acp_replay::testkit::{
                fixtures::shared::{OrchestratorFixtureInput, OrchestratorReplayFixture},
                orchestrator_scenario::ScenarioResult,
                provider_replay_harness::event_tap::observe_adapter,
            },
            codex::adapters::codex_adapter_v2_testkit as codex,
        },
        provider::ProviderAdapter,
        testing::replay::{ReplayTranscript, materialize_transcript_workspace},
    };

    pub struct CodexFixtureFactory;
    struct CodexFixtureAdapter {
        native: Arc<codex::CodexReplayAdapter>,
        observed: Arc<dyn ProviderAdapter>,
    }
    #[async_trait::async_trait]
    impl FixtureAdapter for CodexFixtureAdapter {
        fn adapter(&self) -> Arc<dyn ProviderAdapter> {
            self.observed.clone()
        }
        fn assert_complete(&self) -> Result<()> {
            Ok(self.native.assert_complete()?)
        }
        async fn shutdown(&self) -> Result<()> {
            self.native.shutdown().await;
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl FixtureAdapterFactory for CodexFixtureFactory {
        async fn create(&self, input: FixtureAdapterInput) -> Result<Arc<dyn FixtureAdapter>> {
            let options = CodexAdapterOptions::new(
                input.variant.model_selection.instance_id,
                input.clock,
                input.ids,
            );
            let native = Arc::new(codex::CodexReplayAdapter::new(
                options,
                input.transcript,
                input.gate,
            )?);
            let adapter: Arc<dyn ProviderAdapter> = native.clone();
            let observed = input.observe.map_or(adapter.clone(), |observer| {
                observe_adapter(adapter, observer)
            });
            Ok(Arc::new(CodexFixtureAdapter { native, observed }))
        }
        fn runtime_instructions(&self, _harness: &str, _model: &str) -> String {
            String::new()
        }
        fn materialize_workspace(
            &self,
            transcript: ReplayTranscript,
            workspace: &Path,
        ) -> Result<ReplayTranscript> {
            Ok(materialize_transcript_workspace(
                transcript,
                &workspace.to_string_lossy(),
            ))
        }
        fn assert_output(
            &self,
            name: &str,
            result: &ScenarioResult,
            transcript: &ReplayTranscript,
        ) -> Result<()> {
            let fixture = fixture_cases("codex")?
                .into_iter()
                .find(|(_, variant)| variant.assert_output == name)
                .ok_or_else(|| Error::Service {
                    operation: "Codex fixture output",
                    detail: format!("unknown output assertion {name}"),
                })?
                .0;
            codex::assert_scenario_output(&fixture.name, result, transcript)
        }
        fn build_input(
            &self,
            fixture: &OrchestratorReplayFixture,
        ) -> Result<OrchestratorFixtureInput> {
            fixture.build_owned_input()
        }
    }
}

#[cfg(all(feature = "codex-adapter", feature = "git-backend"))]
#[tokio::test(start_paused = true)]
async fn codex_recorded_simple_runs_through_fixture_host_and_real_git_checkpoints() -> Result<()> {
    let (fixture, variant) = fixture_cases("codex")?
        .into_iter()
        .find(|(fixture, _)| fixture.name == "simple")
        .ok_or(Error::Unsupported("Codex simple replay fixture"))?;
    run_fixture_case(
        &fixture, variant, &native::CodexFixtureFactory,
        &t3_orchestration::ports::acp_replay::testkit::provider_replay_harness::git_fixture::GitReplayFixtureHost,
        FixtureObservations::default(),
    ).await?;
    Ok(())
}
