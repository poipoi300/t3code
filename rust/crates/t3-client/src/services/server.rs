use super::{
    runtime::Services,
    sessions::{EnvironmentStream, ServiceSession},
};
use crate::{
    ClientError,
    platform::Platform,
    rpc::{RpcEndpoint, RpcMethod},
    types::EnvironmentId,
};
use futures::{
    FutureExt, StreamExt,
    future::{Either, select},
    stream::LocalBoxStream,
};
use t3_contracts::{
    rpc::{WsSubscribeServerConfigRpc, WsSubscribeServerLifecycleRpc},
    server::{ServerConfigStreamEvent, ServerLifecycleStreamEvent},
};

pub(super) fn installed_sessions<R>(
    services: Services,
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
) -> LocalBoxStream<'static, Option<ServiceSession<R>>>
where
    R: RpcEndpoint + 'static,
{
    sessions
        .inspect(move |session| match session {
            Some(session) if session.identity.environment == environment => {
                services.begin_session(environment.clone(), session.identity.generation)
            }
            _ => services.disconnect(&environment),
        })
        .boxed_local()
}

pub async fn drive_server_config<R: RpcEndpoint + 'static>(
    services: &Services,
    platform: &(impl Platform + ?Sized),
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    input: <WsSubscribeServerConfigRpc as RpcMethod>::Input,
) -> Result<(), ClientError> {
    let config = services.config(environment.clone());
    let _ = config.load(platform).await;
    let sessions = installed_sessions(services.clone(), environment.clone(), sessions);
    let mut events = EnvironmentStream::new(environment.clone(), sessions, move |session| {
        let generation = session.identity.generation;
        session
            .endpoint
            .subscribe::<WsSubscribeServerConfigRpc>(input.clone())
            .map(move |event| event.map(|event| (generation, event)))
            .boxed_local()
    });
    let mut flush = None;
    loop {
        let next = if let Some(write) = flush.as_mut() {
            match select(events.next(), write).await {
                Either::Left((next, _)) => next,
                Either::Right((_result, _)) => {
                    flush = None;
                    continue;
                }
            }
        } else {
            events.next().await
        };
        let Some(event) = next else {
            break;
        };
        let (generation, event): (_, ServerConfigStreamEvent) = event?;
        if services.apply_server_config(environment.clone(), generation, event)? && flush.is_none()
        {
            flush = Some(config.flush(platform).boxed_local());
        }
    }
    if let Some(write) = flush {
        write.await?;
    }
    Ok(())
}

pub async fn drive_server_welcome<R: RpcEndpoint + 'static>(
    services: &Services,
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
) -> Result<(), ClientError> {
    let sessions = installed_sessions(services.clone(), environment.clone(), sessions);
    let owner = services.clone();
    let destination = environment.clone();
    let mut events = EnvironmentStream::new(environment.clone(), sessions, move |session| {
        let generation = session.identity.generation;
        owner.begin_welcome_subscription(&destination, generation);
        session
            .endpoint
            .subscribe::<WsSubscribeServerLifecycleRpc>(t3_contracts::rpc::Empty {})
            .map(move |event| event.map(|event| (generation, event)))
            .boxed_local()
    });
    while let Some(event) = events.next().await {
        let (generation, event): (_, ServerLifecycleStreamEvent) = event?;
        services.apply_lifecycle(&environment, generation, event);
    }
    Ok(())
}

pub async fn drive_terminal_metadata<R: RpcEndpoint + 'static>(
    services: &Services,
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
) -> Result<(), ClientError> {
    let sessions = installed_sessions(services.clone(), environment.clone(), sessions);
    let mut events = EnvironmentStream::new(environment.clone(), sessions, move |session| {
        let generation = session.identity.generation;
        session
            .endpoint
            .subscribe::<t3_contracts::rpc::WsSubscribeTerminalMetadataRpc>(
                t3_contracts::rpc::Empty {},
            )
            .map(move |event| event.map(|event| (generation, event)))
            .boxed_local()
    });
    while let Some(event) = events.next().await {
        let (generation, event) = event?;
        services.apply_terminal_metadata(environment.clone(), generation, event);
    }
    Ok(())
}

pub async fn drive_terminal_attach<R: RpcEndpoint + 'static>(
    services: &Services,
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    input: t3_contracts::terminal::TerminalAttachInput,
) -> Result<(), ClientError> {
    let key = super::terminal::TerminalKey {
        thread: crate::types::ThreadKey {
            environment: environment.clone(),
            thread: crate::types::ThreadId(input.thread_id.0.clone()),
        },
        terminal: input.terminal_id.0.clone(),
    };
    let sessions = installed_sessions(services.clone(), environment.clone(), sessions);
    let resume = std::rc::Rc::new(std::cell::RefCell::new(
        super::terminal::TerminalAttachResume::default(),
    ));
    let mut events = EnvironmentStream::new(environment, sessions, move |session| {
        let generation = session.identity.generation;
        let mut first = true;
        let input = resume.borrow().input(input.clone());
        let resume = resume.clone();
        session
            .endpoint
            .subscribe::<t3_contracts::rpc::WsTerminalAttachRpc>(input)
            .map(move |event| {
                event.map(|event| {
                    resume.borrow_mut().received();
                    let reset = std::mem::replace(&mut first, false);
                    (generation, reset, event)
                })
            })
            .boxed_local()
    });
    while let Some(event) = events.next().await {
        let (generation, reset, event) = event?;
        if reset {
            services.seed_terminal(key.clone());
        }
        services.apply_terminal(key.clone(), generation, &event);
    }
    Ok(())
}
pub async fn drive_terminal_observe<R: RpcEndpoint + 'static>(
    services: &Services,
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    input: t3_contracts::terminal::TerminalObserveInput,
) -> Result<(), ClientError> {
    let key = super::terminal::TerminalKey {
        thread: crate::types::ThreadKey {
            environment: environment.clone(),
            thread: crate::types::ThreadId(input.thread_id.0.clone()),
        },
        terminal: input.terminal_id.0.clone(),
    };
    let sessions = installed_sessions(services.clone(), environment.clone(), sessions);
    let mut events = EnvironmentStream::new(environment, sessions, move |session| {
        let generation = session.identity.generation;
        let mut first = true;
        session
            .endpoint
            .subscribe::<t3_contracts::rpc::WsTerminalObserveRpc>(input.clone())
            .map(move |event| {
                event.map(|event| {
                    let reset = std::mem::replace(&mut first, false);
                    (generation, reset, event)
                })
            })
            .boxed_local()
    });
    while let Some(event) = events.next().await {
        let (generation, reset, event) = event?;
        if reset {
            services.seed_terminal(key.clone());
        }
        services.apply_terminal(key.clone(), generation, &event);
    }
    Ok(())
}
