use super::workflow_file_read::*;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason;

pub struct TranscriptRead {
    pub lines: Vec<Value>,
    pub cursor: WorkflowCursor,
    pub truncated: bool,
}
#[async_trait]
pub trait WorkflowTranscriptObserver: Send + Sync {
    async fn read(&self, agent_id: &str, start_offset: u64, end_offset: u64, line_count: usize);
}
pub fn parse_workflow_agent_transcript_lines(contents: &str) -> Vec<Value> {
    contents
        .split('\n')
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| {
            let text = |key| {
                entry[key]
                    .as_str()
                    .map(t3_contracts::wire::trim_js)
                    .filter(|text| !text.is_empty())
            };
            matches!(text("type"), Some("assistant" | "user"))
                && entry["isMeta"] != true
                && text("uuid").is_some()
        })
        .collect()
}
pub async fn read_workflow_agent_transcript(
    backend: &dyn WorkflowFileBackend,
    projects_root: &Path,
    transcript_dir: &Path,
    agent_id: &str,
    cursor: &WorkflowCursor,
    observer: Option<&dyn WorkflowTranscriptObserver>,
) -> std::result::Result<TranscriptRead, WorkflowReadError> {
    if agent_id.is_empty()
        || agent_id.len() > 128
        || !agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(WorkflowReadError::new(
            OrchestrationWorkflowFileErrorReason::InvalidPath,
            Path::new(agent_id),
            None,
        ));
    }
    let path = transcript_dir.join(format!("agent-{agent_id}.jsonl"));
    let file = read_contained_workflow_file(
        backend,
        projects_root,
        WorkflowReadInput {
            path: &path,
            extension: ".jsonl",
            byte_cap: 512 * 1024,
            tail: false,
            from_offset: Some(cursor),
        },
    )
    .await?;
    let lines = parse_workflow_agent_transcript_lines(&file.contents);
    if let Some(observer) = observer {
        observer
            .read(agent_id, file.start_offset, file.end_offset, lines.len())
            .await;
    }
    Ok(TranscriptRead {
        lines,
        cursor: WorkflowCursor {
            offset: file.end_offset,
            identity: Some(file.identity),
        },
        truncated: file.truncated,
    })
}
