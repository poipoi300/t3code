use super::super::{Registry, register};
use t3_contracts::base_schemas::*;

pub fn register_module(registry: &mut Registry) {
    register::<ApprovalRequestId>(registry, "ApprovalRequestId");
    register::<AuthSessionId>(registry, "AuthSessionId");
    register::<CheckpointId>(registry, "CheckpointId");
    register::<CheckpointRef>(registry, "CheckpointRef");
    register::<CheckpointScopeId>(registry, "CheckpointScopeId");
    register::<ClientConnectionMethod>(registry, "ClientConnectionMethod");
    register::<ClientDeviceType>(registry, "ClientDeviceType");
    register::<ClientOs>(registry, "ClientOs");
    register::<ClientSurface>(registry, "ClientSurface");
    register::<ClientWebDeployment>(registry, "ClientWebDeployment");
    register::<CommandId>(registry, "CommandId");
    register::<ContextHandoffId>(registry, "ContextHandoffId");
    register::<ContextTransferId>(registry, "ContextTransferId");
    register::<DpopFailureReason>(registry, "DpopFailureReason");
    register::<EnvironmentId>(registry, "EnvironmentId");
    register::<EventId>(registry, "EventId");
    register::<GuestMessageId>(registry, "GuestMessageId");
    register::<GuestShareId>(registry, "GuestShareId");
    register::<IsoDateTime>(registry, "IsoDateTime");
    register::<MessageId>(registry, "MessageId");
    register::<NodeId>(registry, "NodeId");
    register::<NonNegativeInt>(registry, "NonNegativeInt");
    register::<PlanId>(registry, "PlanId");
    register::<PortSchema>(registry, "PortSchema");
    register::<PositiveInt>(registry, "PositiveInt");
    register::<ProjectId>(registry, "ProjectId");
    register::<ProviderItemId>(registry, "ProviderItemId");
    register::<ProviderSessionId>(registry, "ProviderSessionId");
    register::<ProviderThreadId>(registry, "ProviderThreadId");
    register::<ProviderTurnId>(registry, "ProviderTurnId");
    register::<RawEventId>(registry, "RawEventId");
    register::<RpcClientId>(registry, "RpcClientId");
    register::<RunAttemptId>(registry, "RunAttemptId");
    register::<RunId>(registry, "RunId");
    register::<RuntimeRequestId>(registry, "RuntimeRequestId");
    register::<ScheduledTaskId>(registry, "ScheduledTaskId");
    register::<SecretRef>(registry, "SecretRef");
    register::<ThreadId>(registry, "ThreadId");
    register::<TrimmedNonEmptyString>(registry, "TrimmedNonEmptyString");
    register::<TrimmedString>(registry, "TrimmedString");
    register::<TurnId>(registry, "TurnId");
    register::<TurnItemId>(registry, "TurnItemId");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("base_schemas", register_module)
}
