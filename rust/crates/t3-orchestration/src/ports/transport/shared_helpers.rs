//! The orchestration ports implemented by `t3-shared`'s pure helpers.
use super::{
    command_output_hub::{TerminalOutputFold, TerminalText},
    thread_find::ThreadFindText,
    thread_search::{MessageHit, MessageSearchRequest, ThreadSearchPolicy, TitleCandidate},
};
use crate::error::{Error, Result};
use serde_json::Value;
use t3_contracts::orchestration_v2::{OrchestrationV2TurnItem, SearchThreadInputSkillsItem};
use t3_shared::{
    find_text::NativeThreadFindText, search_policy::NativeThreadSearchPolicy,
    terminal_output::NativeTerminalOutputFold,
};

fn shared<T>(result: t3_shared::Result<T>) -> Result<T> {
    result.map_err(|error| Error::Decode {
        entity: "shared helper",
        detail: error.to_string(),
    })
}

impl TerminalOutputFold for NativeTerminalOutputFold {
    fn empty(&self) -> Value {
        NativeTerminalOutputFold::empty(self)
    }
    fn append(&self, state: &Value, chunk: &str) -> Result<Value> {
        shared(NativeTerminalOutputFold::append(self, state, chunk))
    }
    fn normalize(&self, text: &str) -> Result<TerminalText> {
        shared(NativeTerminalOutputFold::normalize(self, text))
    }
    fn resume(&self, state: &Value) -> Result<TerminalText> {
        shared(NativeTerminalOutputFold::resume(self, state))
    }
    fn settled(&self, state: &Value) -> Result<TerminalText> {
        shared(NativeTerminalOutputFold::settled(self, state))
    }
}

impl ThreadFindText for NativeThreadFindText {
    fn segments(
        &self,
        item: &OrchestrationV2TurnItem,
        cwd: Option<&str>,
        skills: &[SearchThreadInputSkillsItem],
    ) -> Result<Vec<String>> {
        shared(NativeThreadFindText::segments(self, item, cwd, skills))
    }
    fn occurrences(&self, text: &str, query: &str) -> u64 {
        NativeThreadFindText::occurrences(self, text, query)
    }
    fn embeds_result(&self, item: &OrchestrationV2TurnItem) -> bool {
        NativeThreadFindText::embeds_result(self, item)
    }
}

impl ThreadSearchPolicy for NativeThreadSearchPolicy {
    fn message_search(&self, query: &str, has_index: bool) -> Option<MessageSearchRequest> {
        NativeThreadSearchPolicy::message_search(self, query, has_index)
    }
    fn shown_message(&self, text: &str) -> String {
        NativeThreadSearchPolicy::shown_message(self, text)
    }
    fn match_message(&self, text: &str, query: &str) -> Option<(f64, usize)> {
        NativeThreadSearchPolicy::match_message(self, text, query)
    }
    fn rank_messages(
        &self,
        hits: &[MessageHit],
        query: &str,
        now_ms: i64,
        picks: bool,
    ) -> Result<Vec<usize>> {
        shared(NativeThreadSearchPolicy::rank_messages(
            self, hits, query, now_ms, picks,
        ))
    }
    fn rank_titles(
        &self,
        rows: &[TitleCandidate],
        query: &str,
        environment_label: Option<&str>,
        now_ms: i64,
        picks: bool,
    ) -> Result<Vec<usize>> {
        shared(NativeThreadSearchPolicy::rank_titles(
            self,
            rows,
            query,
            environment_label,
            now_ms,
            picks,
        ))
    }
}
