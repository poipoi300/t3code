use std::collections::HashSet;
use t3_contracts::{
    base_schemas::*, composer_context::GuestMessageAttribution, guest_share::*, orchestration_v2::*,
};

pub const INACTIVE: &str = "This guest link is no longer active.";
pub const USED: &str = "This guest link was already used.";
pub const BUSY: &str = "Wait for the owner to answer your earlier messages.";
pub const FILES_BUSY: &str =
    "Wait for the owner to answer your earlier messages before you attach more files.";

fn attachment_id(attachment: &t3_contracts::chat_attachment::ChatAttachment) -> &str {
    use t3_contracts::chat_attachment::ChatAttachment;
    match attachment {
        ChatAttachment::Image(value) => &value.id,
        ChatAttachment::File(value) => &value.id,
        ChatAttachment::Unknown(value) => &value.id,
    }
}
fn attachment_size(attachment: &t3_contracts::chat_attachment::ChatAttachment) -> u64 {
    use t3_contracts::chat_attachment::ChatAttachment;
    match attachment {
        ChatAttachment::Image(value) => value.size_bytes,
        ChatAttachment::File(value) => value.size_bytes,
        ChatAttachment::Unknown(value) => value.size_bytes,
    }
}

pub enum GuestShareCommand<'a> {
    Open(&'a InternalCommandThreadGuestShareOpen),
    Redeem(&'a InternalCommandThreadGuestShareRedeem),
    Close,
    Submit(&'a InternalCommandThreadGuestMessageSubmit),
    Dismiss(&'a GuestMessageId),
}

fn pending_attachments(
    share: Option<&ThreadGuestShare>,
) -> impl Iterator<Item = &t3_contracts::chat_attachment::ChatAttachment> {
    share
        .into_iter()
        .flat_map(|share| &share.messages)
        .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
        .flat_map(|message| {
            message
                .attachments
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
        })
}

pub fn released_guest_attachment_ids(
    before: Option<&ThreadGuestShare>,
    after: Option<&ThreadGuestShare>,
) -> Vec<String> {
    let kept: HashSet<_> = after
        .filter(|after| before.is_some_and(|before| after.id == before.id))
        .into_iter()
        .flat_map(|share| &share.messages)
        .filter(|message| message.status != ThreadGuestMessageStatus::Dismissed)
        .flat_map(|message| {
            message
                .attachments
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
        })
        .map(attachment_id)
        .collect();
    pending_attachments(before)
        .filter(|attachment| !kept.contains(attachment_id(attachment)))
        .map(|attachment| attachment_id(attachment).to_owned())
        .collect()
}

fn trim_resolved(messages: &mut Vec<ThreadGuestMessage>) {
    let mut budget = GUEST_RESOLVED_MESSAGE_LIMIT;
    let mut kept = Vec::with_capacity(messages.len());
    for message in messages.drain(..).rev() {
        if message.status != ThreadGuestMessageStatus::Pending {
            if budget == 0 {
                continue;
            }
            budget -= 1;
        }
        kept.push(message);
    }
    kept.reverse();
    *messages = kept;
}

fn resolve_message(
    share: &mut ThreadGuestShare,
    id: &GuestMessageId,
    status: ThreadGuestMessageStatus,
) -> Result<(), String> {
    let message = share
        .messages
        .iter_mut()
        .find(|message| &message.id == id)
        .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
        .ok_or_else(|| format!("Guest message {} is not pending.", id.0))?;
    message.status = status;
    trim_resolved(&mut share.messages);
    Ok(())
}

pub fn decide_guest_share_command(
    thread: &OrchestrationV2AppThread,
    command: GuestShareCommand<'_>,
    now: &IsoDateTime,
) -> Result<OrchestrationV2AppThread, String> {
    let mut thread = thread.clone();
    let mut share = thread.guest_share.clone().flatten();
    let expired = |share: &ThreadGuestShare| {
        super::orchestrator::epoch_millis(&now.0)
            .zip(super::orchestrator::epoch_millis(&share.expires_at.0))
            .is_some_and(|(now, expires)| now >= expires)
    };
    match command {
        GuestShareCommand::Open(command) => {
            if thread.archived_at.is_some() {
                return Err("Archived threads cannot be shared.".into());
            }
            share = Some(ThreadGuestShare {
                id: command.share_id.clone(),
                host: command.host.clone(),
                created_at: now.clone(),
                expires_at: command.expires_at.clone(),
                guest_name: None,
                messages: Vec::new(),
            });
        }
        GuestShareCommand::Close => {
            if share.is_none() {
                return Ok(thread);
            }
            share = None;
        }
        GuestShareCommand::Redeem(command) => {
            let share = share
                .as_mut()
                .filter(|share| share.id == command.share_id && !expired(share))
                .ok_or(INACTIVE)?;
            if share.guest_name.is_some() {
                return Err(USED.into());
            }
            share.guest_name = Some(command.guest_name.clone());
        }
        GuestShareCommand::Submit(command) => {
            let share = share
                .as_mut()
                .filter(|share| {
                    share.id == command.share_id && share.guest_name.is_some() && !expired(share)
                })
                .ok_or(INACTIVE)?;
            if share
                .messages
                .iter()
                .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
                .count()
                >= GUEST_PENDING_MESSAGE_LIMIT
            {
                return Err(BUSY.into());
            }
            let attachments = command
                .attachments
                .as_ref()
                .and_then(Option::as_ref)
                .cloned()
                .unwrap_or_default();
            let pending_bytes: u64 = pending_attachments(Some(share))
                .chain(&attachments)
                .map(attachment_size)
                .sum();
            if !attachments.is_empty() && pending_bytes > GUEST_PENDING_ATTACHMENT_BYTES_LIMIT {
                return Err(FILES_BUSY.into());
            }
            share.messages.push(ThreadGuestMessage {
                id: command.guest_message_id.clone(),
                text: command.text.clone(),
                status: ThreadGuestMessageStatus::Pending,
                submitted_at: now.clone(),
                attachments: (!attachments.is_empty()).then_some(Some(attachments)),
            });
            trim_resolved(&mut share.messages);
            if thread.settled_override == Some(AppThreadSettledOverride::Settled) {
                thread.settled_override = None;
                thread.settled_at = None;
                thread.unsettled_at = Some(Some(now.clone()));
            }
        }
        GuestShareCommand::Dismiss(id) => {
            resolve_message(
                share.as_mut().ok_or("This thread has no guest link.")?,
                id,
                ThreadGuestMessageStatus::Dismissed,
            )?;
        }
    }
    thread.guest_share = Some(share);
    thread.updated_at = now.clone();
    Ok(thread)
}

pub struct GuestMessageSend {
    pub thread: OrchestrationV2AppThread,
    pub text: String,
    pub guest: GuestMessageAttribution,
}

pub fn decide_guest_message_send(
    thread: &OrchestrationV2AppThread,
    id: &GuestMessageId,
    now: &IsoDateTime,
) -> Result<GuestMessageSend, String> {
    let mut share = thread
        .guest_share
        .clone()
        .flatten()
        .ok_or("This thread has no guest link.")?;
    let name = share
        .guest_name
        .as_ref()
        .map(|name| name.0.clone())
        .unwrap_or_else(|| "Guest".into());
    let text = share
        .messages
        .iter()
        .find(|message| &message.id == id)
        .map(|message| message.text.clone())
        .unwrap_or_default();
    resolve_message(&mut share, id, ThreadGuestMessageStatus::Sent)?;
    let guest = GuestMessageAttribution {
        guest_message_id: id.clone(),
        name: TrimmedNonEmptyString(name.clone()),
        text: text.clone(),
        share_id: Some(Some(share.id.clone())),
    };
    let mut thread = thread.clone();
    thread.guest_share = Some(Some(share));
    thread.updated_at = now.clone();
    Ok(GuestMessageSend {
        thread,
        text: format!("Message from guest {name}, sent by the owner:\n\n{text}"),
        guest,
    })
}

pub fn guest_share_after_cancelled_send(
    thread: &OrchestrationV2AppThread,
    guest: &GuestMessageAttribution,
    now: &IsoDateTime,
) -> OrchestrationV2AppThread {
    let mut thread = thread.clone();
    if let Some(share) = thread
        .guest_share
        .as_mut()
        .and_then(Option::as_mut)
        .filter(|share| Some(Some(&share.id)) == guest.share_id.as_ref().map(Option::as_ref))
        && let Some(message) = share.messages.iter_mut().find(|message| {
            message.id == guest.guest_message_id && message.status == ThreadGuestMessageStatus::Sent
        })
    {
        message.status = ThreadGuestMessageStatus::Dismissed;
        thread.updated_at = now.clone();
    }
    thread
}

pub fn without_guest_attribution(mut command: CommandMessageDispatch) -> CommandMessageDispatch {
    if let Some(context) = command.context.as_mut().and_then(Option::as_mut) {
        context.guest = None;
    }
    command
}
