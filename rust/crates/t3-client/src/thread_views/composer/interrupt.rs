//! Stop composition uses the existing planner and durable command queue.
use crate::{
    ClientError, ClientStore,
    operations::{self, CommandOutcome, InterruptInput, InterruptPlan, OperationQueue},
    optimistic::{ProjectionKey, ViewChange},
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint, RpcMethod, SessionEndpoint},
    types::ThreadKey,
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::rpc::{
    WsOrchestrationV2DispatchCommandRpc, WsOrchestrationV2GetThreadProjectionRpc,
};

struct Observed<'a> {
    endpoint: &'a SessionEndpoint,
    error: Rc<RefCell<Option<ClientError>>>,
}
impl RpcEndpoint for Observed<'_> {
    fn destination(&self) -> Option<crate::types::EnvironmentId> {
        self.endpoint.destination()
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        self.endpoint.call::<M>(input)
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.endpoint.subscribe::<M>(input)
    }
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
    where
        M::Output: 'static,
    {
        let pending = self.endpoint.command::<M>(input);
        async move {
            match pending.await {
                Err(RpcCallError::NotSent(error)) => {
                    *self.error.borrow_mut() = Some(error);
                    Err(RpcCallError::NotSent(ClientError::Cancelled))
                }
                result => result,
            }
        }
        .boxed_local()
    }
}

pub async fn interrupt_thread<P: Platform>(
    platform: &P,
    store: &ClientStore,
    endpoint: SessionEndpoint,
    key: &ThreadKey,
    supports_thread_stop: bool,
) -> Result<(), ClientError> {
    if endpoint.destination() != Some(key.environment.clone()) {
        return Err(ClientError::EnvironmentMismatch);
    }
    let availability = serde_json::from_value(serde_json::json!({"type":"thread.stop","commandId":"availability","threadId":key.thread}))?;
    if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&availability) {
        return Err(ClientError::PermissionDenied { method: "orchestration.dispatchCommand".into() });
    }
    let input = InterruptInput {
        command_id: operations::allocate_command_id(platform, None)?,
        thread_id: key.thread.clone(),
        run_id: None,
        subagent_id: None,
        legacy_turn_id: None,
    };
    let mut projections = BTreeMap::new();
    let snapshot = store.snapshot();
    if let Some(projection) = snapshot
        .environments
        .get(&key.environment)
        .and_then(|e| e.threads.get(&key.thread))
        .and_then(|t| t.projection.clone())
    {
        projections.insert(key.thread.clone(), projection);
    }
    drop(snapshot);
    let commands = loop {
        match operations::interrupt_plan(&input, supports_thread_stop, &projections)? {
            InterruptPlan::Ready(commands) => break commands,
            InterruptPlan::NeedProjection(thread) => {
                let projection = endpoint
                    .call::<WsOrchestrationV2GetThreadProjectionRpc>(serde_json::from_value(
                        serde_json::json!({"threadId":thread}),
                    )?)
                    .await?;
                projections.insert(thread, Rc::new(projection));
            }
        }
    };
    let queue = OperationQueue::<()>::new(key.environment.clone());
    for planned in commands {
        if !endpoint.available::<WsOrchestrationV2DispatchCommandRpc>(&planned.command) {
            return Err(ClientError::PermissionDenied {
                method: "orchestration.dispatchCommand".into(),
            });
        }
        let id = queue
            .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                platform,
                &planned.command,
                ProjectionKey::thread(key.environment.clone(), key.thread.clone()),
                0,
                ViewChange {
                    apply: (),
                    revert: (),
                },
            )
            .await
            .map_err(super::send_transport::operation_error)?;
        let error = Rc::new(RefCell::new(None));
        let report = queue
            .dispatch(
                platform,
                &Observed {
                    endpoint: &endpoint,
                    error: error.clone(),
                },
                &id,
            )
            .await
            .map_err(super::send_transport::operation_error)?;
        let result = if let Some(error) = error.borrow_mut().take().or(report.persistence_error) {
            Err(error)
        } else {
            match report.outcome {
                CommandOutcome::Accepted { .. } => Ok(()),
                CommandOutcome::Rejected { reason } => Err(ClientError::Rejected {
                    message: reason,
                    detail: None,
                }),
                CommandOutcome::UnknownAfterDisconnect | CommandOutcome::NotSent => {
                    Err(ClientError::Disconnected)
                }
            }
        };
        if !planned.best_effort {
            result?;
        }
    }
    Ok(())
}
