use crate::error::Result;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    base_schemas::TurnItemId, orchestration_v2::OrchestrationV2SubscribeCommandOutputInput,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordedCommandOutput<State> {
    pub chunks: u64,
    pub output: State,
}
pub type OutputFold<State> = dyn Fn(&State, &str) -> State + Send + Sync;
pub type FallibleOutputFold<State> = dyn Fn(&State, &str) -> Result<State> + Send + Sync;
pub struct CommandOutputRecorder<State> {
    recorded: Mutex<BTreeMap<TurnItemId, RecordedCommandOutput<State>>>,
    initial: State,
    fold: Arc<FallibleOutputFold<State>>,
}
impl<State: Clone> CommandOutputRecorder<State> {
    pub fn new(initial: State, fold: Arc<OutputFold<State>>) -> Self
    where
        State: 'static,
    {
        Self::new_fallible(
            initial,
            Arc::new(move |state, chunk| Ok(fold(state, chunk))),
        )
    }
    pub fn new_fallible(initial: State, fold: Arc<FallibleOutputFold<State>>) -> Self {
        Self {
            recorded: Mutex::new(BTreeMap::new()),
            initial,
            fold,
        }
    }
    pub fn record(&self, item_id: TurnItemId, chunk: &str) -> Result<()> {
        let mut recorded = self
            .recorded
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = recorded.get(&item_id);
        let next = RecordedCommandOutput {
            chunks: previous.map_or(1, |previous| previous.chunks + 1),
            output: (self.fold)(
                previous.map_or(&self.initial, |previous| &previous.output),
                chunk,
            )?,
        };
        recorded.insert(item_id, next);
        Ok(())
    }
    pub fn snapshot(&self) -> BTreeMap<TurnItemId, RecordedCommandOutput<State>> {
        self.recorded
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    pub async fn append(
        &self,
        inner: &dyn CommandOutputAppender,
        target: OrchestrationV2SubscribeCommandOutputInput,
        chunk: String,
    ) -> Result<()> {
        self.record(target.item_id.clone(), &chunk)?;
        inner.append(target, chunk).await
    }
}

impl CommandOutputRecorder<serde_json::Value> {
    pub fn with_terminal_fold(
        terminal: Arc<dyn crate::ports::transport::command_output_hub::TerminalOutputFold>,
    ) -> Self {
        Self::new_fallible(
            terminal.empty(),
            Arc::new(move |state, chunk| terminal.append(state, chunk)),
        )
    }
}

#[async_trait::async_trait]
pub trait CommandOutputAppender: Send + Sync {
    async fn append(
        &self,
        target: OrchestrationV2SubscribeCommandOutputInput,
        chunk: String,
    ) -> Result<()>;
}

#[async_trait::async_trait]
impl CommandOutputAppender for crate::ports::transport::command_output_hub::CommandOutputHub {
    async fn append(
        &self,
        target: OrchestrationV2SubscribeCommandOutputInput,
        chunk: String,
    ) -> Result<()> {
        Self::append(self, target, chunk).await
    }
}
