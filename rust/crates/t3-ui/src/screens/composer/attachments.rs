//! Port of apps/web/src/components/chat/ComposerAttachmentStrip.tsx and useComposerAttachmentIntake.ts.

use std::rc::Rc;

use futures::StreamExt;
use nsh_base::rich_editor::view::RichEditorPastedFiles;
use nsh_core::picker::{FilePickRequest, PickedStatus};
use nsh_core::prelude::*;
use t3_client::ClientError;
use t3_client::platform::HostResource;
use t3_client::services::assets::{file_attachment_too_large_message, format_attachment_size};
use t3_client::thread_views::composer::attachment_intake::{
    AttachmentItem, AttachmentSession, FileKind, IncomingFile, UploadState, classify,
    resource_from_uri, should_handle_paste,
};
use t3_contracts::chat_attachment::{
    PROVIDER_SEND_TURN_MAX_ATTACHMENTS, PROVIDER_SEND_TURN_MAX_IMAGE_BYTES,
};

use crate::i18n::{Text as UiText, Value, t, tr};
use crate::theme::{palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::confirm::{ConfirmDialogOptions, ConfirmDialogVariant, request_confirm_dialog};
use crate::ui::dialog::{PopupOptions, dialog, dialog_popup};
use crate::ui::icon::{Icon, icon};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

use crate::runtime::tasks::{TaskOwner, watch};

/// The thread host shares this handle with the footer, strip and editor's file-paste callback.
#[derive(Clone, Copy)]
pub struct ComposerAttachments {
    client: Stored<AttachmentSession>,
    items: Signal<Rc<Vec<AttachmentItem>>>,
    revision: Signal<u64>,
    disabled: Signal<bool>,
    error: Signal<Option<UiText>>,
    preview: Signal<Option<AttachmentItem>>,
    preview_open: Signal<bool>,
    picking: Signal<bool>,
    owner: TaskOwner,
}

/// One handle per session: equal when they are the same handle (its signals are).
impl PartialEq for ComposerAttachments {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}

impl ComposerAttachments {
    pub fn new(client: AttachmentSession, disabled: Signal<bool>) -> Self {
        let mut changes = client.changes();
        let items = signal(Rc::new(client.items()));
        let revision = signal(0u64);
        let source = client.clone();
        watch(async move {
            while changes.next().await.is_some() {
                items.set(Rc::new(source.items()));
                revision.update(|value| *value += 1);
            }
        });
        Self {
            client: stored(client),
            items,
            revision,
            disabled,
            error: signal(None),
            preview: signal(None),
            preview_open: signal(false),
            picking: signal(false),
            owner: TaskOwner::here(),
        }
    }

    pub fn picker_available(self) -> bool {
        crate::host::file_picker_granted()
    }

    pub fn items(self) -> Rc<Vec<AttachmentItem>> {
        self.revision.get();
        self.items.get()
    }
    pub fn error(self) -> Option<UiText> {
        self.error.get()
    }
    pub fn block_reason(self) -> Option<UiText> {
        self.revision.get();
        self.client
            .with(|client| client.block_reason())
            .map(UiText::client_key)
    }

    fn report(self, result: Result<(), ClientError>) {
        if let Err(error) = result {
            let message = match error {
                ClientError::InvalidPayload(message) => UiText::client_key(message),
                ClientError::Rejected { message, detail } => UiText::from(
                    t3_client::error::rejection_text(&message, detail.as_deref()),
                ),
                // Other errors can carry transport or server detail: content.
                other => UiText::content(other.to_string()),
            };
            self.error.set(Some(message));
        }
    }

    pub fn pick(self, focus: Rc<dyn Fn()>) {
        if self.disabled.get_untracked() || self.picking.get_untracked() || !self.picker_available()
        {
            return;
        }
        let client = self.client.get();
        self.error.set(None);
        let file_limit = client.capabilities().staging_limit();
        let picker_limit = file_limit
            .unwrap_or_default()
            .max(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES);
        self.picking.set(true);
        let answer = pick_files(
            FilePickRequest::new(true, PROVIDER_SEND_TURN_MAX_ATTACHMENTS as u32)
                .max_size(picker_limit),
        );
        self.owner.spawn(async move {
            let picked = answer.await;
            self.picking.set(false);
            if self.disabled.get_untracked() {
                return;
            }
            let picked = match picked {
                Ok(picked) => picked,
                Err(error) => {
                    self.error.set(Some(UiText::content(error.to_string())));
                    return;
                }
            };
            if picked.files.is_empty() && !picked.truncated {
                return;
            }
            let mut files = Vec::new();
            let mut picker_error = picked.truncated.then(|| {
                UiText::content(tr(
                    "You can attach up to {count} files per message.",
                    &[(
                        "count",
                        Value::Int(PROVIDER_SEND_TURN_MAX_ATTACHMENTS as i64),
                    )],
                ))
            });
            for file in picked.files {
                match file.status {
                    PickedStatus::Read => files.push(IncomingFile {
                        name: file.name,
                        mime: file.mime,
                        bytes: file.data.bytes().to_vec(),
                        source: None,
                        insertion: None,
                    }),
                    PickedStatus::TooLarge => {
                        picker_error.get_or_insert_with(|| {
                            let limit = match classify(&file.name, &file.mime) {
                                FileKind::File => file_limit,
                                _ => Some(PROVIDER_SEND_TURN_MAX_IMAGE_BYTES),
                            };
                            limit.map_or_else(
                                || UiText::from("This server does not support file attachments."),
                                |limit| {
                                    UiText::content(file_attachment_too_large_message(
                                        &file.name, limit,
                                    ))
                                },
                            )
                        });
                    }
                    _ => {
                        picker_error.get_or_insert_with(|| {
                            UiText::client_key(format!(
                                "'{}' is empty or could not be read.",
                                file.name
                            ))
                        });
                    }
                }
            }
            let before = client.draft()["prompt"].clone();
            let intake = client.intake_bytes(files).await;
            let has_picker_error = picker_error.is_some() || intake.is_err();
            self.report(intake);
            if let Some(error) = picker_error {
                self.error.set(Some(error));
            }
            client.save_images().await;
            let upload = client.upload_pending().await;
            if !has_picker_error {
                self.report(upload);
            }
            if client.draft()["prompt"] == before {
                focus();
            }
        });
    }

    /// CMP-EDITOR-1's native editor forwards its already-decoded files here.
    pub fn paste_files(self, paste: &RichEditorPastedFiles) {
        if self.disabled.get_untracked() {
            return;
        }
        let files = paste
            .files
            .iter()
            .map(|file| IncomingFile {
                name: file.name.clone(),
                mime: file.mime.clone(),
                bytes: file.data.clone(),
                source: None,
                insertion: None,
            })
            .collect();
        let resources = paste
            .uris
            .iter()
            .filter_map(|uri| resource_from_uri(uri))
            .collect();
        self.intake(files, resources);
    }

    pub fn paste(self, paste: &nsh_core::PasteEvent) -> bool {
        if self.disabled.get_untracked() {
            return false;
        }
        let mut resources: Vec<_> = paste
            .uris
            .iter()
            .filter_map(|uri| resource_from_uri(uri))
            .collect();
        resources.extend(paste.files.iter().map(|file| HostResource {
            handle: String::new(),
            name: file.name.clone(),
            mime: file.mime.clone(),
        }));
        if !should_handle_paste(&resources, &paste.text) {
            return self.fold_pasted_text(&paste.text, false, None);
        }
        let files = paste
            .files
            .iter()
            .map(|file| IncomingFile {
                name: file.name.clone(),
                mime: file.mime.clone(),
                bytes: file.data.clone(),
                source: None,
                insertion: None,
            })
            .collect();
        resources.retain(|file| !file.handle.is_empty());
        self.intake(files, resources);
        true
    }

    pub fn fold_pasted_text(
        self,
        text: &str,
        bypass: bool,
        selection: Option<(usize, usize)>,
    ) -> bool {
        if self.disabled.get_untracked() {
            return false;
        }
        match self
            .client
            .with(|client| client.prepare_text_paste(text, bypass, selection))
        {
            Ok(Some(file)) => {
                self.intake(vec![file], Vec::new());
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.report(Err(error));
                true
            }
        }
    }

    pub fn drop_resources(self, resources: Vec<HostResource>) {
        if self.disabled.get_untracked() {
            return;
        }
        self.intake(Vec::new(), resources);
    }

    fn intake(self, files: Vec<IncomingFile>, resources: Vec<HostResource>) {
        let client = self.client.get();
        self.error.set(None);
        self.owner.spawn(async move {
            self.report(client.intake_bytes(files).await);
            self.report(client.intake(resources).await);
            client.save_images().await;
            self.report(client.upload_pending().await);
        });
    }

    pub fn remove(self, id: &str) {
        if self.disabled.get_untracked() {
            return;
        }
        let client = self.client.get();
        let id = id.to_owned();
        let referenced = client.image_referenced(&id);
        let name = self
            .items()
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.clone())
            .unwrap_or_else(|| t("this image"));
        let confirmation = referenced.then(|| request_confirm_dialog(
            tr("Remove {name} from the message?\nIt is referenced in your text; removing it also removes every reference.", &[("name", Value::Text(name))]),
            Some(ConfirmDialogOptions { variant: ConfirmDialogVariant::Destructive, ..Default::default() }),
        ));
        self.owner.spawn(async move {
            if let Some(confirmation) = confirmation {
                // The native confirm host treats an unmounted host as cancellation.
                if !match confirmation {
                    Some(answer) => answer.await,
                    None => false,
                } {
                    return;
                }
            }
            if self.disabled.get_untracked() {
                return;
            }
            client.remove(&id);
            if self
                .preview
                .with(|item| item.as_ref().is_some_and(|item| item.id == id))
            {
                self.preview_open.set(false);
            }
        });
    }

    fn retry(self, id: String) {
        if self.disabled.get_untracked() {
            return;
        }
        let client = self.client.get();
        self.owner.spawn(async move {
            self.report(client.upload(&id).await);
        });
    }

    fn open(self, item: AttachmentItem) {
        if item.needs_reattach || item.upload == UploadState::Preparing {
            return;
        }
        // PRV-4 and FILES-2 own video/document viewers; do not manufacture a placeholder preview.
        if !item.image || item.preview.is_none() {
            return;
        }
        self.preview.set(Some(item));
        self.preview_open.set(true);
    }
}

pub fn composer_attach_button(
    state: ComposerAttachments,
    focus: impl Fn() + 'static,
) -> impl IntoElement {
    let focus = Rc::new(focus);
    button_tooltip(
        button(
            ButtonContent::Icon {
                icon: Icon::Paperclip,
                label: t("Attach files"),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconSm,
            move || state.disabled.get() || state.picking.get(),
            move || state.pick(focus.clone()),
        ),
        UiText::from("Attach files"),
    )
    // The shared Button owns its glyph; the web's 1px Paperclip translation needs its glyph slot.
}

fn button_tooltip(control: impl IntoElement, caption: UiText) -> impl IntoElement {
    let handle = tooltip_create_handle();
    let target = element_ref();
    target.on_focus_in(move || handle.set_open(true));
    target.on_focus_out(move || handle.set_open(false));
    // TooltipTrigger creates its own Button; this fitted wrapper preserves the actual control.
    view()
        .element_ref(&target)
        .app_class(tw("w-fit"))
        .tooltip(tooltip(
            handle,
            TooltipOptions::default(),
            text(caption.resolve()),
        ))
        .child(control)
}

fn composer_image_fallback() -> impl IntoElement {
    view()
        .role(AccessibilityRole::Image)
        .label(t("Image attachment"))
        .app_class(tw(
            "flex size-full items-center justify-center p-3 text-secondary-label",
        ))
        .child(icon(Icon::Image, 16.0, move || palette().secondary_label))
}

/// Port of ComposerImageThumbnail: NSH decodes the prepared data URL on every surface.
pub fn composer_image_thumbnail(item: &AttachmentItem, class_name: &str) -> AnyElement {
    if let Some(src) = &item.preview {
        image(nsh_core::element::ImageSrc::from(src.clone()))
            .label(SharedString::from(item.name.clone()))
            .app_class(tw(class_name))
            .into_element()
    } else {
        composer_image_fallback().into_element()
    }
}

/// TL-ROWS-USER supplies the client's resolved URL; filenames remain literal content.
pub fn message_attachment_image(
    attachment: &t3_client::view_logic::session::MessageAttachment,
    class_name: &str,
) -> AnyElement {
    let name = match &attachment.attachment {
        t3_contracts::chat_attachment::ChatAttachment::Image(image) => &image.name,
        _ => return composer_image_fallback().into_element(),
    };
    match &attachment.preview_url {
        Some(src) => image(nsh_core::element::ImageSrc::from(src.clone()))
            .label(SharedString::from(name.clone()))
            .app_class(tw(class_name))
            .into_element(),
        None => composer_image_fallback().into_element(),
    }
}

fn remove_button(
    state: ComposerAttachments,
    item: &AttachmentItem,
    media: bool,
) -> impl IntoElement {
    let id = item.id.clone();
    view()
        .app_class(tw(if media {
            "absolute right-1 top-1"
        } else {
            "shrink-0"
        }))
        .child(button(
            ButtonContent::Icon {
                icon: Icon::X,
                label: tr("Remove {name}", &[("name", Value::Text(item.name.clone()))]),
            },
            // MediaClose and Overlay are missing shared Button variants; requested from the integrator.
            ButtonVariant::Ghost,
            ButtonSize::IconXs,
            move || state.disabled.get(),
            move || state.remove(&id),
        ))
}

fn retry_button(
    state: ComposerAttachments,
    item: &AttachmentItem,
    media: bool,
) -> Option<AnyElement> {
    let UploadState::Failed(reason) = &item.upload else {
        return None;
    };
    let id = item.id.clone();
    Some(
        view()
            .app_class(tw(if media {
                "absolute bottom-1 left-1"
            } else {
                "shrink-0"
            }))
            .child(button_tooltip(
                button(
                    ButtonContent::Icon {
                        icon: Icon::RefreshCw,
                        label: tr(
                            "Retry upload for {name}",
                            &[("name", Value::Text(item.name.clone()))],
                        ),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::IconXs,
                    move || state.disabled.get(),
                    move || state.retry(id.clone()),
                ),
                UiText::from(reason.clone()),
            ))
            .into_element(),
    )
}

pub fn composer_attachment_media_row(state: ComposerAttachments) -> impl IntoElement {
    dynamic(move || {
        let items = state.items();
        let media: Vec<_> = items
            .iter()
            .filter(|item| item.image || item.video)
            .collect();
        if media.is_empty() {
            return None;
        }
        let mut row = view().app_class(tw("mb-3 flex max-w-full gap-2 flex-wrap"));
        for item in media {
            let selected = item.clone();
            let selected_for_open = selected.clone();
            let preparing = item.upload == UploadState::Preparing;
            let mut tile = view().app_class(tw("group/attachment shrink-0 bg-background relative h-16 w-16 overflow-hidden rounded-lg border border-border/80"))
                .child(nsh_base::button()
                    .accessibility_label(Some(SharedString::from(tr("Preview {name}", &[("name", Value::Text(item.name.clone()))]))))
                    .disabled(preparing || !item.image || item.preview.is_none())
                    .on_press(move || state.open(selected_for_open.clone()))
                    .app_class(tw("h-full w-full cursor-zoom-in"))
                    .child(composer_image_thumbnail(&selected, "h-full w-full object-cover")));
            if preparing || item.upload == UploadState::Uploading {
                tile = tile.child(text(if preparing { t("Preparing") } else { t("Uploading") })
                    .app_class(tw("pointer-events-none absolute inset-x-0 bottom-0 bg-background/85 px-1 text-center text-3xs text-foreground")));
            }
            if item.non_persisted {
                tile = tile.child(view().label(t("Draft attachment may not persist"))
                    .app_class(tw("absolute left-1 top-1 inline-flex items-center justify-center rounded bg-background/85 p-0.5 text-warning-foreground"))
                    .child(icon(Icon::CircleAlert, 12.0, move || palette().warning_foreground)));
            }
            row = row.child(
                tile.child(retry_button(state, item, true))
                    .child(remove_button(state, item, true)),
            );
        }
        Some(row)
    })
}

pub fn composer_attachment_file_list(state: ComposerAttachments) -> impl IntoElement {
    dynamic(move || {
        let items = state.items();
        let files: Vec<_> = items
            .iter()
            .filter(|item| !item.image && !item.video)
            .collect();
        if files.is_empty() {
            return None;
        }
        let mut list = view().app_class(tw("mb-3 flex flex-col gap-1"));
        for item in files {
            let status = if item.needs_reattach {
                if state.client.with(|client| {
                    client
                        .capabilities()
                        .staging_limit()
                        .is_some_and(|limit| item.size <= limit)
                }) {
                    t("Attach again")
                } else {
                    t("Remove to send")
                }
            } else if item.upload == UploadState::Preparing {
                t("Preparing")
            } else if item.upload == UploadState::Uploading {
                t("Uploading")
            } else {
                format_attachment_size(item.size)
            };
            list = list.child(view().app_class(tw("flex min-w-0 items-center gap-2 py-1 text-sm text-foreground"))
                // PierreEntryIcon is not a shared primitive yet; request the file glyph seam.
                .child(icon(Icon::File, 16.0, move || palette().muted_foreground))
                .child(nsh_base::button()
                    .accessibility_label(Some(SharedString::from(tr("Preview {name}", &[("name", Value::Text(item.name.clone()))]))))
                    .disabled(true) // FILES-2 owns the document preview; no dead enabled control.
                    .app_class(tw("min-w-0 flex-1 truncate text-left hover:underline focus-visible:outline-2"))
                    .child(text(item.name.clone())))
                .child(text(status).app_class(tw("shrink-0 text-xs text-secondary-label")))
                .child(retry_button(state, item, false)).child(remove_button(state, item, false)));
        }
        Some(list)
    })
}

/// PRV-4 supplies the full gallery; this retains Dialog's outside-press/Escape/focus lifecycle.
pub fn composer_image_preview_dialog(state: ComposerAttachments) -> impl IntoElement {
    dialog(state.preview_open, move |dialog_state| {
        dialog_popup(dialog_state, PopupOptions::default(), move || {
            let Some(item) = state.preview.get() else {
                return view().into_element();
            };
            view()
                .role(AccessibilityRole::Image)
                .label(SharedString::from(item.name.clone()))
                .child(composer_image_thumbnail(
                    &item,
                    "max-h-96 max-w-full object-contain",
                ))
                .into_element()
        })
    })
}

pub fn composer_attachment_strip(state: ComposerAttachments) -> impl IntoElement {
    view()
        .child(composer_attachment_media_row(state))
        .child(composer_attachment_file_list(state))
        .child(composer_image_preview_dialog(state))
}

/// Already-uploaded attachments on the queued message being edited.
pub fn composer_queued_attachment_row(
    attachments: Prop<Rc<Vec<t3_client::view_logic::session::MessageAttachment>>>,
    disabled: Signal<bool>,
    on_remove: impl Fn(&str) + 'static,
) -> impl IntoElement {
    let attachments = stored(attachments);
    let on_remove = Rc::new(on_remove);
    dynamic(move || {
        let attachments = attachments.with(|attachments| attachments.get());
        if attachments.is_empty() {
            return None;
        }
        let mut row = view().app_class(tw("mb-3 flex flex-wrap gap-2"));
        for attachment in attachments.iter() {
            let (id, name, image) = match &attachment.attachment {
                t3_contracts::chat_attachment::ChatAttachment::Image(value) => {
                    (value.id.clone(), value.name.clone(), true)
                }
                t3_contracts::chat_attachment::ChatAttachment::File(value) => {
                    (value.id.clone(), value.name.clone(), false)
                }
                t3_contracts::chat_attachment::ChatAttachment::Unknown(value) => {
                    (value.id.clone(), String::new(), false)
                }
            };
            let remove = on_remove.clone();
            let preview = if image && attachment.preview_url.is_some() {
                message_attachment_image(attachment, "h-full w-full object-cover")
            } else {
                view().app_class(tw("flex h-full w-full items-center justify-center px-1 text-center text-3xs text-secondary-label")).child(text(name.clone())).into_element()
            };
            row = row.child(view().app_class(tw("relative h-16 w-16 overflow-hidden rounded-lg border border-border/80 bg-background"))
                .child(preview)
                .child(view().app_class(tw("absolute right-1 top-1")).child(button(ButtonContent::Icon {icon:Icon::X,label:tr("Remove {name}",&[("name",Value::Text(name))])},ButtonVariant::Ghost,ButtonSize::IconXs,move ||disabled.get(),move || { if !disabled.get_untracked() {remove(&id);} }))));
        }
        Some(row)
    })
}

pub fn composer_resting_image_previews(
    state: ComposerAttachments,
    on_show_more: impl Fn() + 'static,
) -> impl IntoElement {
    let on_show_more = Rc::new(on_show_more);
    dynamic(move || {
        let items = state.items();
        let images: Vec<_> = items
            .iter()
            .filter(|item| item.image && item.upload != UploadState::Preparing)
            .collect();
        if images.is_empty() {
            return None;
        }
        let mut row = view().app_class(tw("flex shrink-0 items-center gap-1 ps-1"));
        for item in images.iter().take(3) {
            let selected = (*item).clone();
            row = row.child(nsh_base::button()
                .accessibility_label(Some(SharedString::from(tr("Preview {name}", &[("name", Value::Text(item.name.clone()))]))))
                .on_press(move || state.open(selected.clone()))
                .app_class(tw("relative size-7 shrink-0 cursor-zoom-in overflow-hidden rounded-md border border-border/70 bg-muted/60"))
                .child(composer_image_thumbnail(item, "size-full object-cover")));
        }
        if images.len() > 3 {
            let count = images.len() - 3;
            let show = on_show_more.clone();
            row = row.child(nsh_base::button()
                .accessibility_label(Some(SharedString::from(tr("Show {count} more image attachments", &[("count", Value::Int(count as i64))]))))
                .on_press(move || show())
                .app_class(tw("flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md border border-border/70 bg-muted/60 font-medium text-secondary-label text-xs tabular-nums outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
                .child(text(format!("+{count}"))));
        }
        Some(row)
    })
}
