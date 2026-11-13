use super::{delivery_policy::ThreadSettings, send::Overlay};
use crate::{
    ClientError,
    operations::{ComposerDispatchMode, message_command},
    thread_views::outbox::{DispatchMode, QueuedMessage},
};
use serde_json::json;
use t3_contracts::{chat_attachment::ChatAttachment, orchestration_v2::*};

pub(super) fn message(
    message: &QueuedMessage,
    settings: &ThreadSettings,
    attachments: Vec<ChatAttachment>,
    source: OrchestrationV2CreationSource,
    compact: bool,
) -> Result<OrchestrationV2Command, ClientError> {
    let context = remapped_context(message, &attachments);
    if let Some(run) = message.extra.get("queuedRunId") {
        let mut value = json!({"type":"queued-run.edit","commandId":message.command_id,"threadId":message.thread_id,"runId":run,"text":message.text.trim(),"attachments":attachments});
        if let Some(context) = context {
            value["context"] = json!(context);
        }
        return Ok(serde_json::from_value(value)?);
    }
    let mut value = json!({
        "commandId":if compact{format!("{}:compact",message.command_id.0)}else{message.command_id.0.clone()},
        "threadId":message.thread_id,"messageId":if compact{format!("{}:compact",message.message_id.0)}else{message.message_id.0.clone()},
        "text":if compact{"/compact"}else{&message.text},
        "attachments":if compact{vec![]}else{attachments},"modelSelection":settings.model_selection,
        "createdBy":"user","creationSource":source,"dispatchMode":{"type":"start_immediately"}
    });
    if !compact && let Some(context) = context {
        value["context"] = json!(context);
    }
    if !compact && message.fresh_context == Some(true) {
        value["freshContext"] = json!(true);
    }
    if compact {
        value["idleCompaction"] = json!("onReturn");
    }
    let input: CommandMessageDispatch = serde_json::from_value(value)?;
    let mode = if compact {
        ComposerDispatchMode::Queue
    } else {
        match message.dispatch_mode {
            Some(DispatchMode::Auto) => ComposerDispatchMode::Auto,
            Some(DispatchMode::Queue) => ComposerDispatchMode::Queue,
            Some(DispatchMode::Steer) => ComposerDispatchMode::Steer,
            Some(DispatchMode::Restart) => ComposerDispatchMode::Restart,
            None => ComposerDispatchMode::Start,
        }
    };
    Ok(message_command(input, mode))
}
pub(super) fn remapped_context(
    message: &QueuedMessage,
    attachments: &[ChatAttachment],
) -> Option<t3_contracts::composer_context::OrchestrationMessageContext> {
    let mut context = message.context.clone();
    if let Some(context) = context.as_mut() {
        use t3_contracts::composer_context::ComposerContextRecord;
        for record in &mut context.records {
            let id = match record {
                ComposerContextRecord::Image(record) => &mut record.attachment_id,
                ComposerContextRecord::File(record) => &mut record.attachment_id,
                _ => continue,
            };
            if let Some(index) = message
                .attachments
                .iter()
                .position(|attachment| attachment.id() == id.0)
                && let Some(attachment) = attachments.get(index)
            {
                id.0 = attachment_id(attachment).into();
            }
        }
    }
    context
}
pub(super) fn attachment_id(attachment: &ChatAttachment) -> &str {
    match attachment {
        ChatAttachment::Image(image) => &image.id,
        ChatAttachment::File(file) => &file.id,
        ChatAttachment::Unknown(file) => &file.id,
    }
}
pub(super) fn overlay(
    message: &QueuedMessage,
    visible: bool,
) -> crate::optimistic::ViewChange<Overlay> {
    crate::optimistic::ViewChange {
        apply: Overlay(visible.then(|| message.clone())),
        revert: Overlay(None),
    }
}
pub(super) fn epoch_iso(epoch_ms: u64) -> String {
    let z = (epoch_ms / 86_400_000) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let year = if year <= 9999 {
        format!("{year:04}")
    } else {
        format!("+{year:06}")
    };
    let seconds = epoch_ms / 1000;
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        epoch_ms % 1000
    )
}
