use super::super::{Registry, register};
use t3_contracts::chat_attachment::*;

pub fn register_module(registry: &mut Registry) {
    register::<ChatAttachmentId>(registry, "ChatAttachmentId");
    register::<SnapShotAccessibilityNode>(registry, "SnapShotAccessibilityNode");
    register::<SnapShotAccessibility>(registry, "SnapShotAccessibility");
    register::<SnapShotSource>(registry, "SnapShotSource");
    register::<ChatImageAttachment>(registry, "ChatImageAttachment");
    register::<PastedTextAttachmentSource>(registry, "PastedTextAttachmentSource");
    register::<ChatFileAttachment>(registry, "ChatFileAttachment");
    register::<ChatUnknownAttachment>(registry, "ChatUnknownAttachment");
    register::<UploadChatImageAttachment>(registry, "UploadChatImageAttachment");
    register::<ChatAttachment>(registry, "ChatAttachment");
    register::<UploadChatAttachment>(registry, "UploadChatAttachment");
    register::<PersistChatAttachmentsInput>(registry, "PersistChatAttachmentsInput");
    register::<PersistChatAttachmentsResult>(registry, "PersistChatAttachmentsResult");
    register::<PersistChatAttachmentsError>(registry, "PersistChatAttachmentsError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("chat_attachment", register_module)
}
