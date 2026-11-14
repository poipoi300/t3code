use crate::{Error, Result, lock};
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use t3_contracts::preview_automation::PreviewAutomationControlReason as Reason;
use tokio::sync::oneshot;
use tokio::time::Instant;

type Settled = Shared<BoxFuture<'static, ()>>;
pub type ActionResult = Shared<BoxFuture<'static, Result<Value>>>;
pub type Action = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
type LatestEntry = Arc<Mutex<(Value, Action)>>;

struct Latest {
    viewer: String,
    key: String,
    entry: LatestEntry,
    result: ActionResult,
}
struct State {
    owner: Option<String>,
    generation: u64,
    closed: bool,
    tail: Settled,
    pending: Vec<Settled>,
    handoff: Vec<Settled>,
    latest: Option<Latest>,
}

#[derive(Clone)]
pub struct SessionControl {
    agent: Option<String>,
    state: Arc<Mutex<State>>,
}

impl SessionControl {
    pub fn new(agent: Option<String>) -> Self {
        Self {
            agent,
            state: Arc::new(Mutex::new(State {
                owner: None,
                generation: 0,
                closed: false,
                tail: futures_util::future::ready(()).boxed().shared(),
                pending: vec![],
                handoff: vec![],
                latest: None,
            })),
        }
    }
    pub fn agent_id(&self) -> Option<&str> {
        self.agent.as_deref()
    }
    pub fn controller(&self) -> Option<String> {
        lock(&self.state).owner.clone()
    }
    pub fn generation(&self) -> u64 {
        lock(&self.state).generation
    }
    pub fn may_act(&self, agent: &str) -> bool {
        self.agent.as_deref().is_none_or(|id| id == agent)
    }
    pub fn assert_agent(&self, agent: &str) -> Result<()> {
        if !self.may_act(agent) {
            return Err(Error::Control(Reason::AgentMismatch));
        }
        if lock(&self.state).owner.is_some() {
            return Err(Error::Control(Reason::HumanControl));
        }
        self.assert_open()
    }
    pub fn assert_open(&self) -> Result<()> {
        if lock(&self.state).closed {
            Err(Error::Control(Reason::Closed))
        } else {
            Ok(())
        }
    }
    fn enqueue(&self, run: BoxFuture<'static, Result<Value>>) -> ActionResult {
        let (result_tx, result_rx) = oneshot::channel();
        let (done_tx, done_rx) = oneshot::channel();
        let previous = {
            let mut state = lock(&self.state);
            state.latest = None;
            let previous = state.tail.clone();
            state.tail = async move {
                let _ = done_rx.await;
            }
            .boxed()
            .shared();
            previous
        };
        let shared = self.state.clone();
        tokio::spawn(async move {
            previous.await;
            let result = run.await;
            let pending = std::mem::take(&mut lock(&shared).pending);
            let _ = result_tx.send(result);
            futures_util::future::join_all(pending).await;
            let _ = done_tx.send(());
        });
        async move { result_rx.await.unwrap_or(Err(Error::Disconnected)) }
            .boxed()
            .shared()
    }
    fn action(
        &self,
        agent: Option<String>,
        viewer: Option<String>,
        deadline: Option<Instant>,
        run: BoxFuture<'static, Result<Value>>,
    ) -> ActionResult {
        let allowed = |state: &State| {
            if let Some(viewer) = &viewer {
                state.owner.as_ref() == Some(viewer)
            } else {
                state.owner.is_none() && agent.as_deref().is_some_and(|id| self.may_act(id))
            }
        };
        let generation = {
            let state = lock(&self.state);
            if state.closed {
                return futures_util::future::ready(Err(Error::Control(Reason::Closed)))
                    .boxed()
                    .shared();
            }
            if !allowed(&state) {
                return futures_util::future::ready(Err(Error::Control(Reason::Interrupted)))
                    .boxed()
                    .shared();
            }
            state.generation
        };
        let control = self.clone();
        self.enqueue(
            async move {
                {
                    let state = lock(&control.state);
                    if state.closed {
                        return Err(Error::Control(Reason::Closed));
                    }
                    if state.generation != generation
                        || match &viewer {
                            Some(viewer) => state.owner.as_ref() != Some(viewer),
                            None => {
                                state.owner.is_some()
                                    || !agent.as_deref().is_some_and(|id| control.may_act(id))
                            }
                        }
                    {
                        return Err(Error::Control(Reason::Interrupted));
                    }
                }
                if deadline.is_some_and(|end| Instant::now() >= end) {
                    return Err(Error::Timeout(
                        "request expired before it could start".into(),
                    ));
                }
                run.await
            }
            .boxed(),
        )
    }
    pub fn agent(
        &self,
        agent: &str,
        deadline: Option<Instant>,
        run: BoxFuture<'static, Result<Value>>,
    ) -> ActionResult {
        if let Err(error) = self.assert_agent(agent) {
            return futures_util::future::ready(Err(error)).boxed().shared();
        }
        self.action(Some(agent.into()), None, deadline, run)
    }
    pub fn human(&self, viewer: &str, run: BoxFuture<'static, Result<Value>>) -> ActionResult {
        self.action(None, Some(viewer.into()), None, run)
    }
    pub fn system(&self, run: BoxFuture<'static, Result<Value>>) -> ActionResult {
        let control = self.clone();
        self.enqueue(
            async move {
                control.assert_open()?;
                run.await
            }
            .boxed(),
        )
    }
    pub fn track(&self, work: BoxFuture<'static, ()>) {
        lock(&self.state).pending.push(work.shared());
    }
    pub fn track_until_handoff(&self, work: BoxFuture<'static, ()>) {
        lock(&self.state).handoff.push(work.shared());
    }
    pub fn human_latest(
        &self,
        viewer: &str,
        key: &str,
        input: Value,
        run: Action,
        merge: impl FnOnce(&Value, Value) -> Value,
    ) -> ActionResult {
        {
            let state = lock(&self.state);
            if !state.closed
                && state.owner.as_deref() == Some(viewer)
                && let Some(latest) = &state.latest
                && latest.viewer == viewer
                && latest.key == key
            {
                let mut entry = lock(&latest.entry);
                entry.0 = merge(&entry.0, input);
                entry.1 = run;
                return latest.result.clone();
            }
        }
        let entry = Arc::new(Mutex::new((input, run)));
        let owned = entry.clone();
        let state = self.state.clone();
        let (published, publication) = oneshot::channel();
        let result = self.human(
            viewer,
            async move {
                let _ = publication.await;
                let (input, run) = {
                    let mut state = lock(&state);
                    if state
                        .latest
                        .as_ref()
                        .is_some_and(|latest| Arc::ptr_eq(&latest.entry, &owned))
                    {
                        state.latest = None;
                    }
                    lock(&owned).clone()
                };
                run(input).await
            }
            .boxed(),
        );
        let mut state = lock(&self.state);
        if !state.closed && state.owner.as_deref() == Some(viewer) {
            state.latest = Some(Latest {
                viewer: viewer.into(),
                key: key.into(),
                entry,
                result: result.clone(),
            });
        }
        let _ = published.send(());
        result
    }
    pub fn take(&self, viewer: &str) -> Result<ActionResult> {
        {
            let mut state = lock(&self.state);
            if state.closed {
                return Err(Error::Control(Reason::Closed));
            }
            if state.owner.as_deref().is_some_and(|owner| owner != viewer) {
                return Err(Error::Control(Reason::HumanControl));
            }
            if state.owner.as_deref() != Some(viewer) {
                state.owner = Some(viewer.into());
                state.generation += 1;
                state.latest = None;
            }
        }
        Ok(self.human(viewer, futures_util::future::ready(Ok(Value::Null)).boxed()))
    }
    pub fn release(
        &self,
        viewer: &str,
        after: BoxFuture<'static, Result<Value>>,
    ) -> Result<ActionResult> {
        {
            let mut state = lock(&self.state);
            if state.closed {
                return Err(Error::Control(Reason::Closed));
            }
            if state.owner.as_deref() != Some(viewer) {
                return Err(Error::Control(Reason::Interrupted));
            }
            state.owner = None;
            state.generation += 1;
            state.latest = None;
        }
        let state = self.state.clone();
        Ok(self.enqueue(
            async move {
                let handoff = std::mem::take(&mut lock(&state).handoff);
                futures_util::future::join_all(handoff).await;
                after.await
            }
            .boxed(),
        ))
    }
    pub async fn disconnect(
        &self,
        viewer: &str,
        after: BoxFuture<'static, Result<Value>>,
    ) -> Result<()> {
        if self.controller().as_deref() == Some(viewer) && self.assert_open().is_ok() {
            self.release(viewer, after)?.await?;
        }
        Ok(())
    }
    pub fn close_now(&self) {
        let mut state = lock(&self.state);
        if !state.closed {
            state.closed = true;
            state.owner = None;
            state.generation += 1;
            state.latest = None;
        }
    }
    pub async fn close(&self) {
        self.close_now();
        let tail = lock(&self.state).tail.clone();
        tail.await;
    }
}
