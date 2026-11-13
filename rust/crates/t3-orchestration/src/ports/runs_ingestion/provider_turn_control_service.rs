use super::{provider_event_ingestor::expanded_id, provider_turn_start_service::turn_message};
use crate::{
    api::*,
    error::{Error, Result},
    provider::*,
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("provider turn {operation} for {thread_id:?}/{provider_turn_id:?}: {cause}")]
pub struct ProviderTurnControlError {
    pub thread_id: ThreadId,
    pub provider_turn_id: ProviderTurnId,
    pub operation: &'static str,
    pub turn_completed: bool,
    pub steer_declined: bool,
    pub unresponsive: bool,
    #[source]
    pub cause: Box<Error>,
}

impl ProviderTurnControlError {
    fn new(target: &ProviderControlTarget, operation: &'static str, cause: Error) -> Self {
        Self {
            thread_id: target.thread_id.clone(),
            provider_turn_id: target.provider_turn_id.clone(),
            operation,
            turn_completed: false,
            steer_declined: false,
            unresponsive: matches!(
                cause,
                Error::Provider(ProviderError::InterruptTimeout { .. })
            ),
            cause: Box::new(cause),
        }
    }
}

pub struct ProviderTurnControlService {
    pub projections: Arc<dyn ProjectionReader>,
    pub sessions: Arc<dyn SessionDirectory>,
    pub events: Arc<dyn EventCommitter>,
}

struct Loaded {
    context: ControlContext,
    thread: OrchestrationV2ProviderThread,
    turn: OrchestrationV2ProviderTurn,
    session: Option<Arc<dyn SessionRuntime>>,
}

impl ProviderTurnControlService {
    async fn context(
        &self,
        target: &ProviderControlTarget,
        attempt_id: Option<RunAttemptId>,
        message_id: Option<MessageId>,
    ) -> Result<ControlContext> {
        self.projections
            .provider_target(ProviderTargetQuery {
                thread_id: target.thread_id.clone(),
                provider_thread_id: target.provider_thread_id.clone(),
                provider_turn_id: target.provider_turn_id.clone(),
                attempt_id,
                message_id,
            })
            .await?
            .ok_or_else(|| invalid("The recorded provider execution target is no longer valid."))
    }

    async fn load(
        &self,
        target: &ProviderControlTarget,
        operation: &'static str,
        replacement: Option<&ProviderSessionId>,
        message: Option<MessageId>,
    ) -> std::result::Result<Loaded, ProviderTurnControlError> {
        let wrap = |cause| ProviderTurnControlError::new(target, operation, cause);
        let context = self.context(target, None, message).await.map_err(wrap)?;
        let mut thread = context.provider_thread.clone().ok_or_else(|| {
            wrap(invalid(
                "The recorded provider execution target is no longer valid.",
            ))
        })?;
        let turn = context.provider_turn.clone().ok_or_else(|| {
            wrap(invalid(
                "The recorded provider execution target is no longer valid.",
            ))
        })?;
        let recorded = thread.provider_session_id.as_ref()
            == Some(&IdOrIndex::Id(target.provider_session_id.clone()));
        let committed_replacement = operation == "restart"
            && replacement.is_some_and(|replacement| {
                thread.provider_session_id.as_ref() == Some(&IdOrIndex::Id(replacement.clone()))
            });
        if context.thread.id != IdOrIndex::Id(target.thread_id.clone())
            || thread.id != IdOrIndex::Id(target.provider_thread_id.clone())
            || turn.id != IdOrIndex::Id(target.provider_turn_id.clone())
            || turn.provider_thread_id != thread.id
            || !(recorded || committed_replacement)
        {
            return Err(wrap(invalid(
                "The recorded provider execution target is no longer valid.",
            )));
        }
        if !recorded {
            thread.provider_session_id = Some(IdOrIndex::Id(target.provider_session_id.clone()));
        }
        let continuing = operation == "steer"
            && context.run.as_ref().is_some_and(|run| {
                run.status == OrchestrationV2RunStatus::Running
                    && run.active_attempt_id == turn.run_attempt_id
            });
        if turn.status != ProviderTurnStatus::Running && !continuing {
            if operation == "steer" {
                let mut error = wrap(invalid(
                    "The provider turn ended before the steering message was delivered.",
                ));
                error.turn_completed = turn.status == ProviderTurnStatus::Completed;
                return Err(error);
            }
            return Ok(Loaded {
                context,
                thread,
                turn,
                session: None,
            });
        }
        let session = self
            .sessions
            .get(target.provider_session_id.clone())
            .await
            .map_err(wrap)?;
        if session.is_none() && operation == "steer" {
            return Err(wrap(invalid("The provider session is not active.")));
        }
        Ok(Loaded {
            context,
            thread,
            turn,
            session,
        })
    }

    async fn await_settled(
        &self,
        target: &ProviderControlTarget,
        attempt: Option<RunAttemptId>,
        require_records: bool,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let mut events = self.events.subscribe();
        loop {
            let context = self.context(target, attempt.clone(), None).await?;
            let settled = context
                .provider_turn
                .as_ref()
                .is_none_or(|turn| turn.status != ProviderTurnStatus::Running)
                && context
                    .attempt
                    .as_ref()
                    .is_none_or(|attempt| attempt.status != RunAttemptStatus::Running);
            if settled
                && (!require_records
                    || context.provider_turn.is_some() && context.attempt.is_some())
            {
                return Ok(());
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                received = events.recv() => match received {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {},
                    Err(broadcast::error::RecvError::Closed) => return Err(invalid("Event stream closed before provider turn settlement.")),
                },
            }
        }
    }

    pub async fn interrupt(
        &self,
        input: InterruptTurn,
        cancellation: CancellationToken,
    ) -> std::result::Result<Option<SubagentOutcome>, ProviderTurnControlError> {
        let target = &input.target;
        let wrap = |cause| ProviderTurnControlError::new(target, "interrupt", cause);
        let loaded = self.load(target, "interrupt", None, None).await?;
        let session = match loaded.session {
            Some(session) => Some(session),
            None => self
                .sessions
                .get(target.provider_session_id.clone())
                .await
                .map_err(wrap)?,
        };
        let Some(session) = session else {
            if input.subagent.is_some() {
                return Err(wrap(invalid(
                    "No live provider session can confirm this subagent's outcome.",
                )));
            }
            return Ok(None);
        };
        if let Some(subagent) = input.subagent {
            let extensions = session
                .extensions()
                .filter(|extensions| extensions.capabilities().stop_subagent)
                .ok_or_else(|| {
                    wrap(invalid(
                        "The provider does not support stopping a native subagent.",
                    ))
                })?;
            return tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(wrap(Error::Cancelled)),
                result = extensions.stop_subagent(loaded.thread, subagent.native_task_id, input.requested_at) => result.map_err(|error| wrap(error.into())),
            };
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(wrap(Error::Cancelled)),
            result = session.interrupt_turn(InterruptInput { provider_thread: loaded.thread,
                provider_turn_id: target.provider_turn_id.clone(), request_runtime_restart: true }) => result.map_err(|error| wrap(error.into()))?,
        }
        if loaded.turn.status == ProviderTurnStatus::Running
            && let Ok(result) = tokio::time::timeout(
                Duration::from_secs(2),
                self.await_settled(
                    target,
                    loaded
                        .turn
                        .run_attempt_id
                        .as_ref()
                        .map(expanded_id)
                        .transpose()
                        .map_err(wrap)?,
                    false,
                    cancellation,
                ),
            )
            .await
        {
            result.map_err(wrap)?;
        }
        Ok(None)
    }

    pub async fn interrupt_and_await_terminal(
        &self,
        input: RestartTurn,
        cancellation: CancellationToken,
    ) -> std::result::Result<(), ProviderTurnControlError> {
        let target = &input.target;
        let wrap = |cause| ProviderTurnControlError::new(target, "restart", cause);
        let loaded = self
            .load(
                target,
                "restart",
                input.replacement_provider_session_id.as_ref(),
                None,
            )
            .await?;
        let Some(session) = loaded.session else {
            return Ok(());
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(wrap(Error::Cancelled)),
            result = session.interrupt_turn(InterruptInput { provider_thread: loaded.thread,
                provider_turn_id: target.provider_turn_id.clone(), request_runtime_restart: false }) => result.map_err(|error| wrap(error.into()))?,
        }
        tokio::time::timeout(
            Duration::from_secs(2),
            self.await_settled(
                target,
                Some(input.interrupted_attempt_id),
                true,
                cancellation,
            ),
        )
        .await
        .map_err(|_| wrap(invalid("Provider turn did not terminalize before restart.")))?
        .map_err(wrap)
    }

    pub async fn steer(
        &self,
        input: SteerTurn,
        cancellation: CancellationToken,
    ) -> std::result::Result<(), ProviderTurnControlError> {
        let target = &input.target;
        let wrap = |cause| ProviderTurnControlError::new(target, "steer", cause);
        let context = self
            .context(target, None, Some(input.message_id.clone()))
            .await
            .map_err(wrap)?;
        if let Some(ownership) = context
            .message
            .as_ref()
            .and_then(|message| message.delegated_completion.as_ref())
            .and_then(Option::as_ref)
        {
            let ownership =
                t3_contracts::stored::to_value(&ownership).map_err(|error| wrap(error.into()))?;
            let parent_id = serde_json::from_value(ownership["parentRunId"].clone())
                .map_err(|error| wrap(error.into()))?;
            let parent = self
                .projections
                .run(target.thread_id.clone(), parent_id)
                .await
                .map_err(wrap)?;
            let parent =
                t3_contracts::stored::to_value(&parent).map_err(|error| wrap(error.into()))?;
            let cohort = &parent["delegatedCompletion"];
            if cohort["disposition"] != "open"
                || cohort["delivery"]["messageId"] != input.message_id.0
                || cohort["delivery"]["generation"] != ownership["generation"]
                || cohort["delivery"]["taskIds"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
            {
                return Ok(());
            }
        }
        let loaded = self
            .load(target, "steer", None, Some(input.message_id.clone()))
            .await?;
        let Some(session) = loaded.session else {
            return Ok(());
        };
        let message = loaded.context.message.as_ref().ok_or_else(|| {
            wrap(invalid(
                "The persisted steering message or target run is missing.",
            ))
        })?;
        let run = loaded.context.run.as_ref().ok_or_else(|| {
            wrap(invalid(
                "The persisted steering message or target run is missing.",
            ))
        })?;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(wrap(Error::Cancelled)),
            result = session.steer_turn(SteerInput { thread_id: target.thread_id.clone(), run_id: expanded_id(&run.id).map_err(wrap)?,
                provider_thread: loaded.thread, provider_turn_id: target.provider_turn_id.clone(),
                message: turn_message(message).map_err(wrap)?, model_selection: input.model_selection }) => result,
        };
        if let Err(error) = result {
            let declined = matches!(error, ProviderError::SteerDeclined(_));
            let mut error = wrap(error.into());
            error.steer_declined = declined;
            if !declined {
                error.turn_completed = self
                    .context(target, None, None)
                    .await
                    .map_err(wrap)?
                    .provider_turn
                    .is_some_and(|turn| turn.status == ProviderTurnStatus::Completed);
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn apply_selection(
        &self,
        input: ApplyModelSelection,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let Some(context) = self
            .projections
            .control(ControlQuery::RunningTurn(input.thread_id))
            .await?
        else {
            return Ok(());
        };
        let Some(thread) = context.provider_thread.filter(|thread| {
            thread.id == IdOrIndex::Id(input.provider_thread_id)
                && thread.provider_session_id.as_ref()
                    == Some(&IdOrIndex::Id(input.provider_session_id.clone()))
        }) else {
            return Ok(());
        };
        if context.provider_turn.is_none() {
            return Ok(());
        }
        let Some(session) = self.sessions.get(input.provider_session_id).await? else {
            return Ok(());
        };
        let Some(extensions) = session
            .extensions()
            .filter(|extensions| extensions.capabilities().apply_selection)
        else {
            return Ok(());
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Error::Cancelled),
            result = extensions.apply_selection(thread, input.model_selection) => Ok(result?),
        }
    }
}

fn invalid(detail: &str) -> Error {
    Error::Service {
        operation: "ProviderTurnControlService",
        detail: detail.into(),
    }
}

#[async_trait]
impl ProviderTurnControl for ProviderTurnControlService {
    async fn interrupt(&self, input: InterruptTurn, cancellation: CancellationToken) -> Result<()> {
        ProviderTurnControlService::interrupt(self, input, cancellation)
            .await
            .map(|_| ())
            .map_err(|error| Error::ProviderTurnControl(Box::new(error)))
    }
    async fn interrupt_with_outcome(
        &self,
        input: InterruptTurn,
        cancellation: CancellationToken,
    ) -> Result<Option<SubagentOutcome>> {
        ProviderTurnControlService::interrupt(self, input, cancellation)
            .await
            .map_err(|error| Error::ProviderTurnControl(Box::new(error)))
    }
    async fn interrupt_and_await_terminal(
        &self,
        input: RestartTurn,
        cancellation: CancellationToken,
    ) -> Result<()> {
        ProviderTurnControlService::interrupt_and_await_terminal(self, input, cancellation)
            .await
            .map_err(|error| Error::ProviderTurnControl(Box::new(error)))
    }
    async fn steer(&self, input: SteerTurn, cancellation: CancellationToken) -> Result<()> {
        ProviderTurnControlService::steer(self, input, cancellation)
            .await
            .map_err(|error| Error::ProviderTurnControl(Box::new(error)))
    }
    async fn apply_selection(
        &self,
        input: ApplyModelSelection,
        cancellation: CancellationToken,
    ) -> Result<()> {
        ProviderTurnControlService::apply_selection(self, input, cancellation).await
    }
}
