use super::*;
use crate::services::updates::{UpdateConnectionState, UpdateHost, UpdateSession};
use t3_contracts::{
    environment::{ServerSelfUpdateCapability, ServerSelfUpdateMethod},
    rpc::*,
    server::*,
};

pub(super) struct ComposerUpdateHost<P: Platform + Clone + 'static> {
    pub runtime: ClientRuntime<P>,
    pub environment: EnvironmentId,
}
impl<P: Platform + Clone + 'static> ComposerUpdateHost<P> {
    fn endpoint(&self, generation: Option<Generation>) -> Result<SessionEndpoint, ClientError> {
        let client = self
            .runtime
            .client(&self.environment)
            .ok_or(ClientError::Disconnected)?;
        if generation.is_some_and(|generation| client.state().generation != generation) {
            return Err(ClientError::Cancelled);
        }
        client.session().ok_or(ClientError::Disconnected)
    }
}
struct StreamOwner(AbortHandle);
impl Drop for StreamOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl<P: Platform + Clone + 'static> UpdateHost for ComposerUpdateHost<P> {
    fn session(&self) -> Result<UpdateSession, ClientError> {
        self.endpoint(None)?;
        let snapshot = self.runtime.store().snapshot();
        let environment = snapshot
            .environments
            .get(&self.environment)
            .ok_or(ClientError::Disconnected)?;
        let config = environment
            .config
            .as_ref()
            .ok_or(ClientError::Disconnected)?;
        if let Some(policy) = config.managed_policy.as_ref().filter(|policy| {
            policy
                .limits
                .as_ref()
                .and_then(|limits| limits.updates.as_ref())
                .is_some_and(|updates| {
                    updates.mode == t3_contracts::managed_policy::ManagedUpdatesMode::Off
                })
        }) {
            return Err(ClientError::Rejected {
                message: format!("{} manages T3 Code updates.", policy.organization.0),
                detail: None,
            });
        }
        let method = match config.environment.capabilities.server_self_update {
            Some(ServerSelfUpdateCapability::BootService) => ServerSelfUpdateMethod::BootService,
            Some(ServerSelfUpdateCapability::Respawn) => ServerSelfUpdateMethod::Respawn,
            Some(ServerSelfUpdateCapability::DesktopManaged) => ServerSelfUpdateMethod::DesktopApp,
            None => return Err(ClientError::Unsupported("server self-update")),
        };
        Ok(UpdateSession {
            generation: self
                .runtime
                .client(&self.environment)
                .ok_or(ClientError::Disconnected)?
                .state()
                .generation,
            version: config.environment.server_version.0.clone(),
            method,
            progress: config.environment.capabilities.server_self_update_progress == Some(true),
        })
    }
    fn lifecycle(
        &self,
    ) -> LocalBoxStream<'static, Result<(Generation, ServerLifecycleStreamReadyEvent), ClientError>>
    {
        use futures::SinkExt;
        let sessions = self
            .runtime
            .query_connections(&self.environment)
            .map(|connection| connection.session)
            .boxed_local();
        let platform = self.runtime.inner.platform.clone();
        let environment = self.environment.clone();
        let (mut sender, receiver) = mpsc::channel(8);
        let (handle, registration) = AbortHandle::new_pair();
        (self.runtime.inner.spawn)(
            Abortable::new(
                async move {
                    let mut events = EnvironmentStream::recovering(
                        environment,
                        sessions,
                        &platform,
                        "subscribeServerLifecycle",
                        |session| {
                            let generation = session.identity.generation;
                            session
                                .endpoint
                                .subscribe::<WsSubscribeServerLifecycleRpc>(Default::default())
                                .filter_map(move |event| async move {
                                    match event {
                                        Ok(ServerLifecycleStreamEvent::Ready(ready)) => {
                                            Some(Ok((generation, ready)))
                                        }
                                        Ok(_) => None,
                                        Err(error) => Some(Err(error)),
                                    }
                                })
                                .boxed_local()
                        },
                    );
                    while let Some(event) = events.next().await {
                        if sender.send(event).await.is_err() {
                            break;
                        }
                    }
                },
                registration,
            )
            .map(|_| ())
            .boxed_local(),
        );
        futures::stream::unfold(
            (receiver, StreamOwner(handle)),
            |(mut receiver, owner)| async move {
                receiver
                    .next()
                    .await
                    .map(|event| (event, (receiver, owner)))
            },
        )
        .boxed_local()
    }
    fn states(&self) -> LocalBoxStream<'static, Result<UpdateConnectionState, ClientError>> {
        let runtime = self.runtime.clone();
        let environment = self.environment.clone();
        self.runtime
            .query_connections(&self.environment)
            .map(move |connection| {
                Ok(UpdateConnectionState {
                    phase: connection.phase,
                    authentication_failure: runtime.client(&environment).is_some_and(|client| {
                        client.state().last_failure.is_some_and(|failure| {
                            matches!(
                                failure.kind,
                                crate::connection::FailureKind::Authentication
                                    | crate::connection::FailureKind::Permission
                            )
                        })
                    }),
                })
            })
            .boxed_local()
    }
    fn update(
        &self,
        generation: Generation,
        input: ServerSelfUpdateInput,
    ) -> LocalBoxFuture<'_, Result<ServerSelfUpdateResult, ClientError>> {
        async move {
            self.endpoint(Some(generation))?
                .command::<WsServerUpdateServerRpc>(input)
                .await
                .map_err(crate::rpc::RpcCallError::into_client_error)
        }
        .boxed_local()
    }
    fn progress(
        &self,
        generation: Generation,
        input: ServerSelfUpdateInput,
    ) -> LocalBoxStream<'static, Result<ServerSelfUpdateProgressEvent, ClientError>> {
        match self.endpoint(Some(generation)) {
            Ok(endpoint) => endpoint.subscribe::<WsServerUpdateServerWithProgressRpc>(input),
            Err(error) => futures::stream::once(async { Err(error) }).boxed_local(),
        }
    }
    fn commit(&self, token: String) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        async move {
            self.endpoint(None)?
                .command::<WsServerCommitDesktopUpdateRpc>(DesktopUpdateCommitInput {
                    request_id: t3_contracts::base_schemas::TrimmedNonEmptyString(token),
                })
                .await
                .map(|_| ())
                .map_err(crate::rpc::RpcCallError::into_client_error)
        }
        .boxed_local()
    }
    fn retry_now(&self) -> LocalBoxFuture<'_, Result<(), ClientError>> {
        self.runtime.reconnect(&self.environment)
    }
}
