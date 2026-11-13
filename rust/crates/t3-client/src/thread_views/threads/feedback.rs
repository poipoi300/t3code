use t3_contracts::base_schemas::MessageId;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackStatus {
    Uploading,
    Interrupted,
    Sent { feedback_id: String },
    Failed { error_message: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackSubmission {
    pub id: MessageId,
    pub command: String,
    pub created_at: String,
    pub status: FeedbackStatus,
}
pub fn parse_codex_feedback_command(text: &str) -> Option<Option<&str>> {
    let text = t3_contracts::wire::trim_js(text);
    if !text
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("/feedback"))
    {
        return None;
    }
    let rest = &text[9..];
    if !rest.is_empty()
        && !rest
            .chars()
            .next()
            .is_some_and(crate::view_logic::is_js_whitespace)
    {
        return None;
    }
    let rest = t3_contracts::wire::trim_js(rest);
    Some((!rest.is_empty()).then_some(rest))
}
pub fn codex_feedback_notice(
    submission: &FeedbackSubmission,
) -> Option<(&'static str, Option<String>)> {
    match &submission.status {
        FeedbackStatus::Interrupted => None,
        FeedbackStatus::Uploading => Some(("Sending feedback to OpenAI...", None)),
        FeedbackStatus::Sent { feedback_id } => Some((
            "Feedback sent to OpenAI",
            Some(format!("Thread ID: {feedback_id}")),
        )),
        FeedbackStatus::Failed { error_message } => Some((
            "Could not send feedback to OpenAI",
            Some(error_message.clone()),
        )),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChatMessage {
    pub id: MessageId,
    pub assistant: bool,
    pub text: String,
    pub created_at: String,
    pub updated_at: String,
}
pub fn codex_feedback_message(s: &FeedbackSubmission, assistant: bool) -> LocalChatMessage {
    let text = if !assistant {
        s.command.clone()
    } else {
        match &s.status {
            FeedbackStatus::Sent { feedback_id } => {
                format!("Feedback sent to OpenAI.\n\nThread ID: `{feedback_id}`")
            }
            FeedbackStatus::Failed { error_message } => {
                format!("Could not send feedback to OpenAI.\n\n{error_message}")
            }
            _ => "Sending feedback to OpenAI...".into(),
        }
    };
    LocalChatMessage {
        id: if assistant {
            MessageId(format!("{}:feedback", s.id.0))
        } else {
            s.id.clone()
        },
        assistant,
        text,
        created_at: s.created_at.clone(),
        updated_at: s.created_at.clone(),
    }
}
pub async fn submit_codex_feedback<
    T,
    F: std::future::Future<Output = Result<T, crate::ClientError>>,
>(
    mut submission: FeedbackSubmission,
    clear_draft: impl FnOnce(),
    mut on_update: impl FnMut(FeedbackSubmission),
    upload: impl FnOnce() -> F,
    feedback_id: impl FnOnce(&T) -> String,
) -> Result<T, crate::ClientError> {
    submission.status = FeedbackStatus::Uploading;
    on_update(submission.clone());
    clear_draft();
    let result = upload().await;
    submission.status = match &result {
        Ok(value) => FeedbackStatus::Sent {
            feedback_id: feedback_id(value),
        },
        Err(crate::ClientError::Cancelled) => FeedbackStatus::Interrupted,
        Err(error) => FeedbackStatus::Failed {
            error_message: error.to_string(),
        },
    };
    on_update(submission);
    result
}
