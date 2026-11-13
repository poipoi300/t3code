use t3_contracts::{
    base_schemas::{CommandId, ThreadId},
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_instance::ProviderInstanceId,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{provider_instance_id} cannot satisfy {capability} for command {command_id}: {detail}")]
pub struct CapabilityUnsupported {
    pub command_id: String,
    pub thread_id: ThreadId,
    pub provider_instance_id: String,
    pub capability: &'static str,
    pub detail: &'static str,
}

pub struct CapabilityCheck<'a> {
    pub command_id: &'a CommandId,
    pub thread_id: &'a ThreadId,
    pub provider_instance_id: &'a ProviderInstanceId,
    pub capabilities: &'a OrchestrationV2ProviderCapabilities,
}

impl CapabilityCheck<'_> {
    fn unsupported(&self, capability: &'static str, detail: &'static str) -> CapabilityUnsupported {
        CapabilityUnsupported {
            command_id: self.command_id.0.clone(),
            thread_id: self.thread_id.clone(),
            provider_instance_id: self.provider_instance_id.0.clone(),
            capability,
            detail,
        }
    }

    pub fn ensure_queued_messages(&self) -> Result<(), CapabilityUnsupported> {
        self.capabilities
            .turns
            .supports_queued_messages
            .then_some(())
            .ok_or_else(|| {
                self.unsupported(
                    "queued_messages",
                    "providerInstanceId does not support app-owned queued turns",
                )
            })
    }

    pub fn steering(
        &self,
        force_restart: bool,
    ) -> Result<SteeringExecution, CapabilityUnsupported> {
        let turns = &self.capabilities.turns;
        if !force_restart && turns.supports_active_steering {
            return Ok(SteeringExecution::ActiveSteering);
        }
        if turns.supports_interrupt && turns.supports_steering_by_interrupt_restart {
            return Ok(SteeringExecution::InterruptRestart);
        }
        Err(self.unsupported(
            if force_restart || turns.supports_interrupt {
                "interrupt_restart_steering"
            } else {
                "active_steering"
            },
            if force_restart {
                "providerInstanceId cannot satisfy a required interrupt-and-restart"
            } else {
                "providerInstanceId cannot steer active turns directly or by interrupt-and-restart"
            },
        ))
    }

    pub fn ensure_interrupt(&self) -> Result<(), CapabilityUnsupported> {
        self.capabilities
            .turns
            .supports_interrupt
            .then_some(())
            .ok_or_else(|| {
                self.unsupported(
                    "interrupt",
                    "providerInstanceId does not support turn interrupts",
                )
            })
    }

    pub fn ensure_native_fork(
        &self,
        from_specific_turn: bool,
    ) -> Result<(), CapabilityUnsupported> {
        let caps = self.capabilities;
        if !caps.threads.can_fork_thread {
            return Err(self.unsupported(
                "native_fork",
                "providerInstanceId does not support native thread forks",
            ));
        }
        if from_specific_turn && !caps.threads.can_fork_from_turn {
            return Err(self.unsupported(
                "fork_from_turn",
                "providerInstanceId cannot fork from a specific completed turn",
            ));
        }
        if caps.identity.native_thread_ids != OrchestrationV2NativeRefStrength::Strong {
            return Err(self.unsupported(
                "native_fork",
                "providerInstanceId does not expose strong native thread ids",
            ));
        }
        Ok(())
    }

    pub fn ensure_rollback(&self) -> Result<(), CapabilityUnsupported> {
        let caps = self.capabilities;
        if !caps.threads.can_rollback_thread
            || !caps.checkpointing.provider_can_rollback_conversation
        {
            return Err(self.unsupported(
                "rollback",
                "providerInstanceId conversation rollback is unavailable",
            ));
        }
        if !caps.checkpointing.provider_rollback_returns_snapshot {
            return Err(self.unsupported(
                "rollback_snapshot",
                "rollback must return a providerInstanceId thread snapshot",
            ));
        }
        Ok(())
    }

    pub fn ensure_context_handoff(
        &self,
        strategy: HandoffStrategy,
    ) -> Result<(), CapabilityUnsupported> {
        let caps = &self.capabilities.context;
        if !caps.can_consume_handoff_summaries {
            return Err(self.unsupported(
                "context_handoff",
                "providerInstanceId cannot consume handoff summaries",
            ));
        }
        if !caps.accepts_synthetic_user_context {
            return Err(self.unsupported(
                "context_handoff",
                "providerInstanceId cannot receive synthetic user context",
            ));
        }
        match strategy {
            HandoffStrategy::ForkDeltaContext | HandoffStrategy::DeltaContext
                if !caps.supports_delta_handoff =>
            {
                Err(self.unsupported(
                    "context_handoff",
                    "providerInstanceId does not support delta handoff",
                ))
            }
            HandoffStrategy::FullThreadSummary if !caps.supports_full_thread_handoff => Err(self
                .unsupported(
                    "context_handoff",
                    "providerInstanceId does not support full-thread handoff",
                )),
            _ => Ok(()),
        }
    }

    pub fn fork(&self, input: ForkPolicy) -> Result<ForkExecution, CapabilityUnsupported> {
        if matches!(
            input.source_status,
            OrchestrationV2RunStatus::Completed | OrchestrationV2RunStatus::Waiting
        ) && input.same_provider
            && input.strong_native_source
            && !input.source_busy
            && self.ensure_native_fork(input.from_specific_turn).is_ok()
        {
            return Ok(ForkExecution::NativeFork);
        }
        self.ensure_context_handoff(HandoffStrategy::FullThreadSummary)?;
        Ok(ForkExecution::PortableContext)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringExecution {
    ActiveSteering,
    InterruptRestart,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkExecution {
    NativeFork,
    PortableContext,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffStrategy {
    ForkDeltaContext,
    DeltaContext,
    FullThreadSummary,
}
pub struct ForkPolicy {
    pub same_provider: bool,
    pub strong_native_source: bool,
    pub source_status: OrchestrationV2RunStatus,
    pub from_specific_turn: bool,
    pub source_busy: bool,
}

pub fn is_native_maintenance_command(text: &str, attachment_count: usize) -> bool {
    attachment_count == 0
        && matches!(
            t3_contracts::wire::trim_js(text).to_lowercase().as_str(),
            "/compact" | "/logout"
        )
}

pub fn goal_command_type(text: &str, attachment_count: usize) -> Option<&'static str> {
    if attachment_count != 0 {
        return None;
    }
    let text = t3_contracts::wire::trim_js(text);
    let argument = text.strip_prefix("/goal")?;
    if !argument.is_empty()
        && !argument
            .chars()
            .next()
            .is_some_and(|ch| t3_contracts::wire::trim_js(&ch.to_string()).is_empty())
    {
        return None;
    }
    Some(
        match t3_contracts::wire::trim_js(argument)
            .to_lowercase()
            .as_str()
        {
            "" | "edit" => "show",
            "clear" => "clear",
            "pause" => "pause",
            "resume" => "resume",
            _ => "set",
        },
    )
}

pub fn goal_command_stops_active_run(command: &str, goal_status: Option<&str>) -> bool {
    match command {
        "set" | "resume" => true,
        "clear" | "pause" => goal_status == Some("active"),
        _ => false,
    }
}

pub fn is_active_run(run: &OrchestrationV2Run) -> bool {
    matches!(
        run.status,
        OrchestrationV2RunStatus::Preparing
            | OrchestrationV2RunStatus::Starting
            | OrchestrationV2RunStatus::Running
            | OrchestrationV2RunStatus::Waiting
    )
}

pub fn follow_up_owner_run(runs: &[OrchestrationV2Run]) -> Option<&OrchestrationV2Run> {
    runs.iter()
        .filter(|run| is_active_run(run))
        .max_by_key(|run| run.ordinal)
        .or_else(|| super::model_switch::latest_executed_run(runs))
}

fn provider_turn_capabilities<'a>(
    projection: &'a OrchestrationV2ThreadProjection,
    run: &OrchestrationV2Run,
) -> Option<&'a OrchestrationV2TurnCapabilities> {
    let thread = projection
        .provider_threads
        .iter()
        .find(|thread| Some(&thread.id) == run.provider_thread_id.as_ref())?;
    let session_id = thread.provider_session_id.as_ref()?;
    Some(
        &projection
            .provider_sessions
            .iter()
            .find(|session| &session.id == session_id)?
            .capabilities
            .turns,
    )
}

pub fn resolve_message_dispatch_intent(
    projection: &OrchestrationV2ThreadProjection,
    requested: CommandMessageDispatchDispatchMode,
    delivery_intent: Option<&str>,
) -> CommandMessageDispatchDispatchMode {
    use CommandMessageDispatchDispatchMode as Mode;
    let queue = || Mode::QueueAfterActive(Box::new(ThreadForkSourcePointLatestStable {}));
    let mode = match delivery_intent {
        None => requested,
        Some(intent) => {
            let Some(run) = projection.runs.iter().rev().find(|run| is_active_run(run)) else {
                return Mode::StartImmediately(Box::new(ThreadForkSourcePointLatestStable {}));
            };
            let IdOrIndex::Id(run_id) = &run.id else {
                return queue();
            };
            let target = || {
                Box::new(CommandMessageDispatchDispatchModeSteerActive {
                    target_run_id: run_id.clone(),
                })
            };
            match intent {
                "steer" => Mode::SteerActive(target()),
                "restart" => Mode::RestartActive(target()),
                _ => {
                    if matches!(
                        run.status,
                        OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting
                    ) || !projection.provider_turns.iter().any(|turn| {
                        turn.run_attempt_id == run.active_attempt_id
                            && turn.status == ProviderTurnStatus::Running
                    }) {
                        return queue();
                    }
                    match provider_turn_capabilities(projection, run) {
                        Some(caps) if caps.supports_active_steering => Mode::SteerActive(target()),
                        Some(caps) if caps.supports_queued_messages => return queue(),
                        Some(caps) if caps.supports_steering_by_interrupt_restart => {
                            Mode::RestartActive(target())
                        }
                        _ => return queue(),
                    }
                }
            }
        }
    };
    let Mode::SteerActive(target) = &mode else {
        return mode;
    };
    let run = projection
        .runs
        .iter()
        .find(|run| run.id == IdOrIndex::Id(target.target_run_id.clone()));
    let message = run.and_then(|run| {
        projection
            .messages
            .iter()
            .find(|message| message.id == run.user_message_id)
    });
    if message.is_some_and(|message| {
        is_native_maintenance_command(&message.text, message.attachments.len())
    }) {
        return queue();
    }
    if run
        .filter(|run| is_active_run(run))
        .and_then(|run| provider_turn_capabilities(projection, run))
        .is_some_and(|caps| {
            !caps.supports_active_steering
                && !(caps.supports_interrupt && caps.supports_steering_by_interrupt_restart)
                && caps.supports_queued_messages
        })
    {
        return queue();
    }
    mode
}
