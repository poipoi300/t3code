#![cfg(all(
    test,
    feature = "acp-adapter",
    feature = "claude-adapter",
    feature = "git-backend"
))]

use serde_json::json;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_orchestration::{
    error::{Error, Result},
    ports::acp_replay::testkit::orchestrator_scenario::{ScenarioResult, ScenarioServices},
};

pub async fn assert_provider_native_child_refuses_messages_and_metadata_continuation(
    services: &ScenarioServices,
    result: &ScenarioResult,
) -> Result<()> {
    let child = result
        .projections
        .values()
        .find(|projection| {
            serde_json::to_value(&projection.thread).is_ok_and(|thread| {
                thread["creationSource"] == "provider"
                    && thread["lineage"]["relationshipToParent"] == "subagent"
            })
        })
        .ok_or(Error::Unsupported("Claude recorded subagent projection"))?;
    let IdOrIndex::Id(thread_id) = &child.thread.id else {
        return Err(Error::Unsupported("expanded native child thread ID"));
    };
    let before = services
        .committer
        .latest_sequence(Some(thread_id.clone()))
        .await?;
    for value in [
        json!({"type":"message.dispatch","createdBy":"user","creationSource":"web","commandId":"command:subagent:message-native-child","threadId":thread_id,"messageId":"message:subagent:message-native-child","text":"Also check the tests.","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        json!({"type":"thread.metadata.update","commandId":"command:subagent:continue-native-child","threadId":thread_id,"title":"Renamed","continuation":{"messageId":"message:subagent:continue-native-child","text":"Also check the tests."}}),
    ] {
        let refused = services
            .dispatcher
            .dispatch(serde_json::from_value(value)?)
            .await;
        // Rust's canonical rejection carries the source error's user-facing reason.
        match refused {
            Err(Error::Rejected { reason, .. })
                if reason
                    == "This subagent is run by its provider and cannot take messages. Message the parent thread instead." =>
                {}
            Err(other) => return Err(other),
            Ok(_) => {
                return Err(Error::Service {
                    operation: "native child read-only",
                    detail: "provider-native child accepted delivery".into(),
                });
            }
        }
    }
    assert_eq!(
        services
            .committer
            .latest_sequence(Some(thread_id.clone()))
            .await?,
        before
    );
    let after = services
        .projections
        .projection(thread_id.clone())
        .await?
        .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
    assert!(after.runs.is_empty());
    assert_eq!(after.messages, child.messages);
    assert_eq!(after.thread.title, child.thread.title);
    Ok(())
}

mod native {
    use super::*;
    use std::{path::Path, sync::Arc};
    use t3_orchestration::{
        adapters::claude::{
            ClaudeAdapterOptions, ClaudeAdapterV2, ClaudeHostOptions, DirectoryClaudeAttachments,
        },
        ports::{
            acp_replay::testkit::{
                fixtures::shared::{OrchestratorFixtureInput, OrchestratorReplayFixture},
                provider_replay_fixtures::{fixture_cases, native::*},
                provider_replay_harness::git_fixture::GitReplayFixtureHost,
            },
            claude::adapters::claude_adapter_v2_testkit::{self as claude, ClaudeReplayConnector},
        },
        provider::ProviderAdapter,
        testing::replay::{ReplayTranscript, materialize_transcript_workspace},
    };
    struct NativeFixture {
        adapter: Arc<dyn ProviderAdapter>,
        connector: Arc<ClaudeReplayConnector>,
    }
    #[async_trait::async_trait]
    impl FixtureAdapter for NativeFixture {
        fn adapter(&self) -> Arc<dyn ProviderAdapter> {
            self.adapter.clone()
        }
        fn assert_complete(&self) -> Result<()> {
            Ok(self.connector.assert_complete()?)
        }
        async fn shutdown(&self) -> Result<()> {
            self.connector.join().await;
            Ok(())
        }
    }
    struct ReadOnlyChildFactory;
    #[async_trait::async_trait]
    impl FixtureAdapterFactory for ReadOnlyChildFactory {
        async fn create(&self, input: FixtureAdapterInput) -> Result<Arc<dyn FixtureAdapter>> {
            let connector = Arc::new(ClaudeReplayConnector::new(&input.transcript, input.gate)?);
            let native = input
                .transcript
                .metadata
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|metadata| metadata.get("nativeSessionId"))
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::Unsupported("recorded Claude native session ID"))?
                .to_owned();
            let private_home = input.workspace.join(".git").join("claude-fixture-home");
            std::fs::create_dir_all(&private_home)?;
            let mut options = ClaudeAdapterOptions {
                instance_id: input.variant.model_selection.instance_id,
                launch: Default::default(),
                connector: connector.clone(),
                clock: input.clock,
                ids: input.ids,
                new_native_session_id: Arc::new(move || native.clone()),
                mcp_servers: Default::default(),
                transcript_root: None,
                attachments: Arc::new(DirectoryClaudeAttachments::default()),
                host: ClaudeHostOptions {
                    home_directory: Some(private_home.clone()),
                    settings_directory: Some(private_home),
                    ..Default::default()
                },
            };
            options.launch.cli_path = Some("fixture-claude-never-spawned".into());
            Ok(Arc::new(NativeFixture {
                adapter: Arc::new(ClaudeAdapterV2::new(options)),
                connector,
            }))
        }
        async fn assert_live(
            &self,
            services: &ScenarioServices,
            result: &ScenarioResult,
        ) -> Result<()> {
            assert_provider_native_child_refuses_messages_and_metadata_continuation(
                services, result,
            )
            .await
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
            claude::assert_claude_fixture_output(name, result, transcript)
        }
        fn build_input(
            &self,
            fixture: &OrchestratorReplayFixture,
        ) -> Result<OrchestratorFixtureInput> {
            claude::build_claude_fixture_input(fixture)
        }
    }
    #[tokio::test(start_paused = true)]
    async fn recorded_native_child_refuses_metadata_continuation_without_changing_history()
    -> Result<()> {
        let (fixture, variant) = fixture_cases("claudeAgent")?
            .into_iter()
            .find(|(fixture, _)| fixture.name == "subagent")
            .ok_or(Error::Unsupported("Claude subagent fixture"))?;
        run_fixture_case(
            &fixture,
            variant,
            &ReadOnlyChildFactory,
            &GitReplayFixtureHost,
            FixtureObservations::default(),
        )
        .await?;
        Ok(())
    }
}
