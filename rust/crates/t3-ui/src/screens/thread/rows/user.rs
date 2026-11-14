//! Port of apps/web/src/components/chat/TimelineUserMessageRow.tsx UserMessageRow.

use std::collections::BTreeMap;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::session::MessageAttachment;
use t3_contracts::chat_attachment::{ChatAttachment, ChatFileAttachment};
use t3_contracts::composer_context::{ComposerContextRecord, PreviewAnnotationContextRecord};

use crate::i18n::{Value, t, tr};
use crate::runtime::services::assets::AssetUrlBinding;
use crate::screens::thread::timeline::TimelineServices;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, palette, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};
use crate::ui::tooltip::{tooltip, tooltip_create_handle};
use futures::StreamExt;
use t3_client::services::assets::AssetUrlState;
use t3_client::thread_views::messages_timeline::MessagesTimelineRowContent;
use t3_client::view_logic::session::select_message_image_resources;
use t3_contracts::assets::AssetResource;
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;

use nsh_base::text_view::{
    CustomNode, Inline, MarkdownOptions, MarkdownPlugin, ParseContext, Parser, TextViewClasses,
    mdast,
};
use t3_client::thread_views::messages_timeline::MessagesTimelineRow;
use t3_client::thread_views::user_message::present_user_message_row;
pub use t3_client::thread_views::user_message::{Attribution, Intent, UserRow};
use t3_client::view_logic::session::timestamps::{CalendarTimestamp, TimestampHost};
use t3_contracts::settings::TimestampFormat;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FoldState {
    expanded: bool,
    body_expanded: bool,
    content_height: f32,
    lane_width: f32,
    fold_threshold: f32,
    copied: bool,
}

/// Construct above the virtual list: message-keyed state survives row recycling.
#[derive(Clone, Copy)]
pub struct UserRowState(
    Signal<BTreeMap<String, FoldState>>,
    Stored<BTreeMap<String, Spawned<()>>>,
    Owner,
);

impl Default for UserRowState {
    fn default() -> Self {
        Self::new()
    }
}

impl UserRowState {
    pub fn new() -> Self {
        Self(
            signal(BTreeMap::new()),
            stored(BTreeMap::new()),
            Owner::current(),
        )
    }

    fn read(self, id: &str) -> FoldState {
        self.0
            .with(|rows| rows.get(id).copied().unwrap_or_default())
    }

    fn update(self, id: &str, change: impl FnOnce(&mut FoldState)) {
        self.0
            .update(|rows| change(rows.entry(id.to_owned()).or_default()));
    }

    pub fn reveal(self, id: &str) {
        self.update(id, |state| {
            state.expanded = true;
            state.body_expanded = true;
        });
    }

    fn copy(self, id: String, source: String, anchor: ElementRef) {
        if source.is_empty() {
            return;
        }
        set_clipboard(source.clone());
        self.2.run(|| {
            spawn(async move {
                if clipboard_string().await != source {
                    return;
                }
                self.update(&id, |state| state.copied = true);
                crate::ui::toast::show_anchored_copy_success_toast(Some(&anchor));
                self.1.with(|timers| {
                    if let Some(timer) = timers.remove(&id) {
                        timer.cancel();
                    }
                    let key = id.clone();
                    timers.insert(
                        id,
                        set_timeout(
                            std::time::Duration::from_millis(
                                crate::ui::toast::ANCHORED_COPY_TOAST_TIMEOUT_MS,
                            ),
                            move || self.update(&key, |state| state.copied = false),
                        ),
                    );
                });
            })
        });
    }
}

pub type RowAction = Rc<dyn Fn(&UserRow)>;
pub type RevertAction = Rc<dyn Fn(&UserRow, usize)>;
pub type FileAction = Rc<dyn Fn(&ChatFileAttachment)>;
pub type ImageAction = Rc<dyn Fn(&MessageAttachment)>;
pub type AttachmentRenderer = Rc<dyn Fn(&MessageAttachment) -> AnyElement>;
pub type EntryIconRenderer = Rc<dyn Fn(&str) -> AnyElement>;

pub type ContextChipRenderer = Rc<
    dyn Fn(
        &t3_client::thread_views::user_message::Reference,
        Option<&ComposerContextRecord>,
        &UserRow,
    ) -> AnyElement,
>;

#[derive(Clone)]
struct UserMarkdownPlugin {
    row: Rc<UserRow>,
    chip: ContextChipRenderer,
}

impl MarkdownPlugin for UserMarkdownPlugin {
    fn name(&self) -> &str {
        "user-message"
    }
    fn block(
        &self,
        node: &mdast::Node,
        context: &mut ParseContext<'_>,
    ) -> Option<nsh_base::text_view::Block> {
        let mdast::Node::Heading(heading) = node else {
            return None;
        };
        Some(nsh_base::text_view::Block::Heading {
            level: (heading.depth + 3).min(6),
            content: context.inlines(&heading.children),
            anchor: None,
        })
    }
    fn inline(&self, node: &mdast::Node, context: &mut ParseContext<'_>) -> Option<Inline> {
        if let mdast::Node::Text(text) = node {
            return Some(Inline::Text {
                text: text.value.clone(),
            });
        }
        if !matches!(node, mdast::Node::Link(_) | mdast::Node::Image(_)) {
            return None;
        }
        let source = context.node_source(node)?;
        let reference = t3_client::thread_views::user_message::context_references(source)
            .into_iter()
            .next()?;
        Some(Inline::Custom(
            CustomNode::new("user-context-reference")
                .with_attribute("context-id", reference.context_id)
                .with_attribute("kind", reference.kind)
                .with_text(reference.label),
        ))
    }
    fn render(&self, node: &CustomNode) -> Option<AnyElement> {
        if node.name != "user-context-reference" {
            return None;
        }
        let reference =
            self.row.references.iter().find(|reference| {
                node.attributes.get("context-id") == Some(&reference.context_id)
            })?;
        let record = self.row.records.iter().find(|record| {
            serde_json::to_value(record)
                .ok()
                .and_then(|value| value["contextId"].as_str().map(str::to_owned))
                .as_deref()
                == Some(&reference.context_id)
        });
        if let Some(ComposerContextRecord::File(record)) = record
            && self.row.audio.iter().any(|attachment| {
                matches!(&attachment.attachment,
                ChatAttachment::File(file) if file.id == record.attachment_id.0)
            })
        {
            return Some(view().into_element());
        }
        Some((self.chip)(reference, record, &self.row))
    }
}

struct LocalTimestampHost {
    formats: t3_i18n::Formats,
    zone: t3_i18n::TimeZone,
    hour12: bool,
}

impl LocalTimestampHost {
    fn current() -> Self {
        let settings = locale_settings();
        let name = settings.time_zone.to_string();
        let zone = if matches!(name.as_str(), "UTC" | "Etc/UTC" | "GMT") {
            t3_i18n::TimeZone::Utc
        } else {
            t3_i18n::TimeZone::Iana(name)
        };
        Self {
            formats: crate::i18n::formats(),
            zone,
            hour12: settings.hour_cycle.twelve_hour(),
        }
    }
}

impl TimestampHost for LocalTimestampHost {
    fn parse_epoch_ms(&self, iso: &str) -> Option<i64> {
        // t3-i18n's frozen dependency needs its iana feature to resolve named host zones.
        self.zone
            .is_known()
            .then(|| t3_client::time::epoch_ms(iso))
            .flatten()
    }
    fn calendar(&self, epoch_ms: i64, _: Option<&str>) -> CalendarTimestamp {
        let day = t3_i18n::format::civil(epoch_ms, &self.zone).day;
        CalendarTimestamp {
            epoch_ms,
            day_index: day.index,
            year: day.year,
            month: day.month,
            day: day.day,
        }
    }
    fn short_time(&self, epoch_ms: i64, format: &TimestampFormat, _: Option<&str>) -> String {
        self.formats.date(
            t3_i18n::DateFormat::TimeHm,
            epoch_ms,
            &self.zone,
            Some(t3_client::view_logic::session::timestamps::resolve_hour12(
                format,
                self.hour12,
            )),
        )
    }
    fn numeric_date(&self, epoch_ms: i64, year: bool, _: Option<&str>) -> String {
        self.formats.date(
            if year {
                t3_i18n::DateFormat::MonthDayYearNumeric
            } else {
                t3_i18n::DateFormat::MonthDayNumeric
            },
            epoch_ms,
            &self.zone,
            None,
        )
    }
    fn full_timestamp(&self, iso: &str, format: &TimestampFormat) -> String {
        self.parse_epoch_ms(iso)
            .map_or_else(String::new, |epoch_ms| {
                self.formats.date(
                    t3_i18n::DateFormat::DefaultDateTime,
                    epoch_ms,
                    &self.zone,
                    Some(t3_client::view_logic::session::timestamps::resolve_hour12(
                        format,
                        self.hour12,
                    )),
                )
            })
    }
    fn locale_hour12(&self) -> bool {
        self.hour12
    }
}

/// MessagesTimeline's row context for user rows (`TimelineRowSharedState`), from the thread's
/// [`TimelineServices`]. Not built: InlineAudioPreview (CMP-ATTACH), WorkflowOpeningBlock, and
/// file download (the platform has no URL download), so those parts render nothing.
#[derive(Clone)]
pub struct UserRowContext {
    pub state: UserRowState,
    pub find_active: Memo<bool>,
    pub working: Memo<bool>,
    pub reverting: Memo<bool>,
    pub timestamp_format: Memo<TimestampFormat>,
    environment: t3_client::types::EnvironmentId,
    context_chip: ContextChipRenderer,
    events: crate::screens::thread::rows::events::EventRowContext,
    assets: Option<AssetUrlBinding>,
    open_sender: Option<RowAction>,
    open_automation: Option<RowAction>,
    revert: Option<RevertAction>,
    file_open: Option<FileAction>,
    file_download: Option<FileAction>,
}

impl UserRowContext {
    pub fn new(state: UserRowState, services: &TimelineServices) -> Self {
        let key = services.key.clone();
        let mut context = Self {
            state,
            find_active: services.find_active,
            working: services.working,
            reverting: services.rewind.reverting,
            timestamp_format: services.timestamp_format,
            environment: key.environment.clone(),
            context_chip: services.chips.clone(),
            events: services.events.clone(),
            assets: services.assets.clone(),
            open_sender: None,
            open_automation: None,
            revert: None,
            file_open: None,
            file_download: None,
        };
        context.revert = services.rewind.revert_action();
        if let Some(panel) = use_context::<crate::screens::panel::PanelHost>() {
            let key = key.clone();
            context.file_open = Some(Rc::new(move |file| {
                panel.apply(
                    &key,
                    t3_client::view_logic::shell::panels::Action::OpenAttachment(file.clone()),
                );
            }));
        }
        if let Some(nav) = use_context::<crate::state::nav::Navigator>()
            && let Some(runtime) = use_context::<crate::runtime::Rt>()
        {
            context.open_sender = Some(Rc::new(move |row| {
                if let Some(sender) = t3_client::thread_views::user_message::sender_thread_key(
                    &row.message,
                    &key.environment,
                ) && runtime
                    .environments()
                    .iter()
                    .any(|entry| entry.id == sender.environment)
                {
                    nav.open(crate::state::nav::Route::Thread(sender));
                }
            }));
        }
        context
    }
}

pub fn user_message_row(
    projection: Memo<Option<Rc<MessagesTimelineRow>>>,
    context: UserRowContext,
) -> impl IntoElement {
    let timestamp_format = context.timestamp_format;
    let presented = computed(move || {
        projection.with(|projection| {
            projection.as_ref().and_then(|row| {
                present_user_message_row(
                    row,
                    LocalTimestampHost::current(),
                    &timestamp_format.get(),
                    nsh_core::time::unix_ms() as i64,
                )
            })
        })
    });
    let urls = committed_image_urls(presented, &context);
    let row = computed(move || {
        presented.with(|row| {
            row.as_ref()
                .map(|row| urls.with(|urls| with_asset_previews(row, urls)))
        })
    });
    let model_switch = computed(move || {
        projection.with(|projection| {
            let MessagesTimelineRowContent::Message {
                projected_item: Some(item),
                ..
            } = &projection.as_ref()?.content
            else {
                return None;
            };
            match &item.item {
                OrchestrationV2TurnItem::UserMessage(item) => item.model_switch.clone().flatten(),
                _ => None,
            }
        })
    });
    dynamic(move || {
        row.get().map(|row| {
            untracked(|| {
                let mut root = view();
                if row.fresh_context {
                    root = root.child(crate::screens::thread::rows::events::fresh_session_row());
                }
                if let Some(change) = model_switch.get() {
                    root = root.child(crate::screens::thread::rows::events::model_switch_row(
                        &change,
                        &context.events,
                    ));
                }
                root.child(user_message_content(Rc::new(row), context.clone()))
            })
        })
    })
}

/// UserTimelineRow's `useAssetUrls` over `selectMessageImageResources`, and each
/// UserVideoAttachment's `useAssetUrlState`: a committed image's or video's signed URL, by
/// attachment id, while it resolves; a failed or loading one keeps its own preview.
fn committed_image_urls(
    row: Memo<Option<UserRow>>,
    context: &UserRowContext,
) -> Memo<BTreeMap<String, String>> {
    let urls = signal(BTreeMap::<String, String>::new());
    if let Some(assets) = context.assets.clone() {
        let environment = context.environment.clone();
        let resources = computed(move || {
            row.with(|row| {
                row.as_ref()
                    .map(|row| {
                        let mut resources = select_message_image_resources(&row.images);
                        resources.extend(video_resources(&row.videos));
                        resources
                    })
                    .unwrap_or_default()
            })
        });
        // Re-run only when the image set changes; the effect owns its watchers.
        effect(move || {
            for resource in resources.get() {
                let AssetResource::Attachment { attachment_id, .. } = &resource else {
                    continue;
                };
                let id = attachment_id.0.clone();
                let mut states = assets.0.watch(environment.clone(), resource);
                spawn(async move {
                    while let Some(state) = states.next().await {
                        let url = match state {
                            AssetUrlState::Success { url, .. } => Some(url),
                            _ => None,
                        };
                        urls.update(|urls| {
                            match url {
                                Some(url) => urls.insert(id.clone(), url),
                                None => urls.remove(&id),
                            };
                        });
                    }
                });
            }
        });
    }
    computed(move || urls.get())
}

/// UserVideoAttachment's `buildAttachmentVideoAsset` for each committed video: a local upload's
/// `blob:`/`data:` preview stays as it is, as an uploading file is not downloadable yet.
fn video_resources(videos: &[MessageAttachment]) -> Vec<AssetResource> {
    videos
        .iter()
        .filter(|video| {
            !video
                .preview_url
                .as_deref()
                .is_some_and(|url| url.starts_with("blob:") || url.starts_with("data:"))
        })
        .filter_map(|video| match &video.attachment {
            ChatAttachment::File(file) => Some(AssetResource::Attachment {
                attachment_id: t3_contracts::base_schemas::TrimmedNonEmptyString(file.id.clone()),
                file_name: Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                    file.name.clone(),
                )),
                mime_type: Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
                    t3_client::view_logic::chat_view::video_mime_type(&file.name, &file.mime_type)
                        .unwrap_or_else(|| file.mime_type.clone()),
                )),
                disposition: None,
                render_intent: None,
            }),
            _ => None,
        })
        .collect()
}

/// `createMessageAttachmentPreviewProjector` for images, and UserVideoAttachment's
/// `assetUrl.url ?? file.previewUrl` for videos: a signed URL replaces the preview.
fn with_asset_previews(row: &UserRow, urls: &BTreeMap<String, String>) -> UserRow {
    let mut row = row.clone();
    let media = row
        .images
        .iter_mut()
        .chain(row.preview_images.iter_mut())
        .chain(row.videos.iter_mut());
    for attachment in media {
        let id = match &attachment.attachment {
            ChatAttachment::Image(file) => &file.id,
            ChatAttachment::File(file) => &file.id,
            ChatAttachment::Unknown(_) => continue,
        };
        if let Some(url) = urls.get(id).filter(|url| !url.is_empty()) {
            attachment.preview_url = Some(url.clone());
        }
    }
    row
}

fn user_message_content(row: Rc<UserRow>, context: UserRowContext) -> AnyElement {
    if row.attribution == Attribution::Automation {
        automation_message_row(row, context).into_element()
    } else {
        user_timeline_row(row, context).into_element()
    }
}

fn automation_message_row(row: Rc<UserRow>, context: UserRowContext) -> impl IntoElement {
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let summary = row
        .scheduled_task_title
        .clone()
        .unwrap_or_else(|| row.summary.clone());
    let body_row = row.clone();
    let body_context = context.clone();
    let actions_row = row.clone();
    let actions_context = context.clone();
    // NSH has no arbitrary DOM attributes: test_id keeps the stable hook's name, not its CSS semantics.
    view()
        .app_class(tw("group flex flex-col items-end gap-1"))
        .child(row.intent.as_ref().map(user_message_intent_marker))
        .child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("min-w-0 max-w-[80%] rounded-2xl bg-message text-message-foreground"),
                        fold.get().expanded.then_some("w-full"),
                    ])))
                })
                .test_id("data-user-message-bubble")
                .label(SharedString::from(t("Automation")))
                .child(folded_message_header(
                    Icon::CalendarClock,
                    SharedString::from(t("Automation")),
                    summary,
                    fold,
                    state,
                    row.id.clone(),
                ))
                .child(dynamic(move || {
                    (fold.get().expanded || body_context.find_active.get()).then(|| {
                        let body = untracked(|| user_message_body(&body_row, &body_context));
                        let body = if body_context.find_active.get() {
                            view().app_class(tw("px-3 pb-3")).child(body).into_element()
                        } else {
                            scroll()
                                .app_class(tw("px-3 pb-3 max-h-96"))
                                .child(body)
                                .into_element()
                        };
                        view()
                            .app_class(move || (!fold.get().expanded).then(|| tw("hidden")))
                            .child(body)
                    })
                })),
        )
        .child(user_message_projected_status(row.status.as_deref()))
        .child(dynamic(move || {
            fold.get().expanded.then(|| {
                untracked(|| {
                    actions_frame(message_actions(&actions_row, &actions_context, true, false))
                })
            })
        }))
}

fn user_timeline_row(row: Rc<UserRow>, context: UserRowContext) -> impl IntoElement {
    let folded = row.attribution == Attribution::Agent;
    let sender_label = row.sender_label.clone().unwrap_or_else(|| t("Agent"));
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let body_row = row.clone();
    let body_context = context.clone();
    let actions_row = row.clone();
    let actions_context = context.clone();
    let floating_row = row.clone();
    let floating_context = context.clone();
    let measure_id = row.id.clone();
    let group_next = row.grouped_with_next;
    let float_actions = move || group_next && pointer() == Pointer::Fine;
    let float_beside = move || float_actions() && fold.get().lane_width >= 592.0;
    // @container queries use the measured row width; sr-only author headings become accessibility labels.
    let mut root = view()
        .app_class(tw("group flex flex-col items-end gap-1"))
        .on_layout(move |bounds: &Bounds<Points>| {
            let width = bounds.size.width.0;
            if state.read(&measure_id).lane_width != width {
                state.update(&measure_id, |state| state.lane_width = width);
            }
        });
    if row.attribution == Attribution::T3Code {
        root = root.child(
            text(t("Sent by T3 Code")).app_class(tw("me-1 text-2xs text-muted-foreground/70")),
        );
    }
    if let Attribution::Guest(name) = &row.attribution {
        root = root.child(text(name.clone()).app_class(tw("me-1 text-2xs text-guest-foreground")));
    }
    if !row.continues_steer {
        root = root.child(row.intent.as_ref().map(user_message_intent_marker));
    }
    if row.media_above_text {
        root = root.child(
            view()
                .app_class(tw("flex max-w-[80%] justify-end"))
                .label(SharedString::from(t("You")))
                .child(default_media_block(&row, &context)),
        );
    }
    let background = !row.media_only && !row.file_cards_only && !row.audio_only;
    let guest = matches!(row.attribution, Attribution::Guest(_));
    let heading = match &row.attribution {
        Attribution::Agent => sender_label.clone(),
        Attribution::Guest(name) => tr("{name} (guest)", &[("name", Value::Text(name.clone()))]),
        _ => t("You"),
    };
    let previous = row.media_above_text || row.grouped_with_previous;
    let next = row.grouped_with_next;
    let mut bubble = view()
        .test_id("data-user-message-bubble")
        .label(SharedString::from(heading))
        .app_class(move || {
            Some(tw(&cn([
                Some("relative min-w-0 max-w-[80%] rounded-2xl text-message-foreground"),
                previous.then_some("rounded-tr-md"),
                next.then_some("rounded-br-md"),
                guest.then_some("bg-guest-surface"),
                (!guest && background).then_some("bg-message"),
                (!folded && background).then_some("p-3"),
                (folded && fold.get().expanded).then_some("w-full"),
            ])))
        });
    if folded {
        bubble = bubble.child(folded_message_header(
            Icon::MessagesSquare,
            SharedString::from(sender_label),
            row.summary.clone(),
            fold,
            state,
            row.id.clone(),
        ));
    }
    bubble = bubble.child(dynamic(move || {
        (!folded || fold.get().expanded || body_context.find_active.get()).then(|| {
            untracked(|| {
                let body = user_row_body(body_row.clone(), body_context.clone(), !folded);
                let body = if folded && !body_context.find_active.get() {
                    scroll()
                        .app_class(tw("max-h-96"))
                        .child(body)
                        .into_element()
                } else {
                    body.into_element()
                };
                view()
                    .app_class(move || {
                        Some(tw(&cn([
                            folded.then_some("px-3 pb-3"),
                            (folded && !fold.get().expanded).then_some("hidden"),
                        ])))
                    })
                    .child(body)
            })
        })
    }));
    // NSH has no cqw: resolve the source's 20cqw - 0.5rem from the measured lane.
    bubble = bubble.child(dynamic(move || float_beside().then(|| {
        untracked(|| view()
            .app_class(tw("absolute end-full top-1/2 me-2 flex w-max -translate-y-1/2 items-center text-xs tabular-nums opacity-0 transition-opacity duration-200 motion-reduce:transition-none focus-within:opacity-100 group-hover:opacity-100"))
            .max_w(move || px((fold.get().lane_width * 0.2 - 8.0).max(0.0)))
            .child(message_actions(&floating_row, &floating_context, false, true)))
    })));
    root.child(bubble)
        .child(user_message_projected_status(row.status.as_deref()))
        .child(dynamic(move || {
            ((!folded || fold.get().expanded) && !float_beside()).then(|| {
                untracked(|| {
                    actions_frame(message_actions(
                        &actions_row,
                        &actions_context,
                        false,
                        float_actions(),
                    ))
                })
            })
        }))
}

fn folded_message_header(
    which: Icon,
    label: SharedString,
    summary: String,
    fold: Memo<FoldState>,
    state: UserRowState,
    id: String,
) -> impl IntoElement {
    // NSH Button lacks aria-expanded; an expanded prop is needed on the primitive's actual control.
    nsh_base::button().accessibility_label(Some(label.clone()))
        .hit_slop(move || control_hit_slop(36.0))
        .on_press(move || state.update(&id, |state| state.expanded = !state.expanded))
        .app_class(tw("flex w-full min-w-0 items-center gap-1.5 rounded-2xl px-3 py-2 text-left text-sm focus-visible:outline-2 focus-visible:outline-ring"))
        .child(icon(which, 16.0, || palette().muted_foreground))
        .child(text(label).app_class(tw("max-w-1/2 shrink-0 truncate text-muted-foreground")))
        .child(text(summary).app_class(tw("min-w-0 flex-1 truncate")))
        .child(view().app_class(move || Some(tw(&cn([
            Some("size-3.5 shrink-0 text-muted-foreground transition-transform motion-reduce:transition-none"),
            fold.get().expanded.then_some("rotate-180"),
        ])))).child(icon(Icon::ChevronDown, 14.0, || palette().muted_foreground)))
}

pub fn user_message_intent_marker(intent: &Intent) -> impl IntoElement + use<> {
    let label = if intent.unread {
        t("Steer · waiting to be read")
    } else {
        t("Steer")
    };
    let hint = if intent.unread {
        t(
            "Waiting for the agent to read this. It reads steers between steps, after a running command finishes.",
        )
    } else if intent.promoted {
        t("Originally queued, then promoted to steer the active turn")
    } else {
        t("Steered the active turn")
    };

    view()
        .app_class(tw(
            "me-1 flex items-center justify-end gap-1 text-xs leading-none text-muted-foreground",
        ))
        .tooltip(tooltip(
            tooltip_create_handle(),
            Default::default(),
            text(hint),
        ))
        .child(icon(Icon::Redo2, 12.0, || palette().muted_foreground))
        .child(text(label))
}

pub fn user_message_projected_status(status: Option<&str>) -> impl IntoElement + use<> {
    let status = status.map(str::to_owned);
    dynamic(move || {
        status.as_deref().filter(|status| !matches!(*status, "completed" | "pending" | "waiting"))
        .map(|status| view().app_class(tw("me-1 flex items-center gap-1.5"))
            .child(text(crate::i18n::Text::client_key(status.to_owned()).resolve()).app_class(tw("rounded-full border border-destructive/25 bg-destructive/8 px-1.5 py-0.5 text-3xs font-medium text-destructive"))))
    })
}

fn user_row_body(row: Rc<UserRow>, context: UserRowContext, collapsible: bool) -> impl IntoElement {
    let mut body = view();
    if row.has_media && !row.media_above_text {
        body = body.child(default_media_block(&row, &context));
    }
    // InlineAudioPreview is not ported yet (CMP-ATTACH); an audio attachment renders nothing.
    if row.has_files {
        let mut files = view().app_class(tw(if row.file_cards_only {
            "flex flex-col items-end gap-1"
        } else {
            "mb-2 flex flex-col gap-1"
        }));
        for attachment in &row.files {
            if let ChatAttachment::File(file) = &attachment.attachment {
                let props = FilePresentation {
                    file: file.clone(),
                    downloadable: true,
                    extension: file_extension(&file.name),
                    open: context.file_open.clone(),
                    download: context.file_download.clone(),
                    entry_icon: Some(Rc::new(|_| {
                        icon(Icon::File, 16.0, || palette().muted_foreground).into_element()
                    })),
                };
                files = files.child(if row.file_cards_only {
                    user_file_card(props).into_element()
                } else {
                    user_file_attachment(props).into_element()
                });
            }
        }
        for attachment in &row.unknown {
            if let ChatAttachment::Unknown(attachment) = &attachment.attachment {
                files = files.child(
                    view()
                        .app_class(tw("flex min-w-0 items-center gap-2 py-1 text-sm"))
                        .child(icon(Icon::File, 16.0, || palette().muted_foreground))
                        .child(
                            text(attachment.name.clone()).app_class(tw("min-w-0 flex-1 truncate")),
                        ),
                );
            }
        }
        body = body.child(files);
    }
    if !row.file_cards_only && !row.audio_only {
        body = body.child(collapsible_user_message_body(row, context, collapsible));
    }
    body
}

fn file_extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map_or_else(|| t("File"), str::to_ascii_uppercase)
}

fn default_media_block(row: &UserRow, _context: &UserRowContext) -> impl IntoElement + use<> {
    let videos = row
        .videos
        .iter()
        .filter_map(|attachment| {
            let ChatAttachment::File(file) = &attachment.attachment else {
                return None;
            };
            let player = attachment.preview_url.as_ref().map(|src| {
                nsh_core::element::video(src.clone())
                    .app_class(tw("block aspect-[4/3] w-full"))
                    .into_element()
            });
            Some(
                user_video_attachment(
                    &file.name,
                    row.media_only || row.media_above_text,
                    "",
                    player,
                )
                .into_element(),
            )
        })
        .collect();
    media_block(&row.images, videos, row, None, None)
}

fn message_actions(
    row: &UserRow,
    context: &UserRowContext,
    automation: bool,
    float_actions: bool,
) -> impl IntoElement + use<> {
    let mut actions = view().app_class(tw(&cn([
        Some("flex items-center justify-end gap-x-2 gap-y-0.5"),
        Some(if float_actions {
            "flex-wrap"
        } else {
            "shrink-0"
        }),
    ])));
    if (automation && row.has_scheduled_task)
        || (row.attribution == Attribution::Agent && row.can_open_sender)
    {
        let (label, action) = if automation {
            (t("Open automation"), &context.open_automation)
        } else {
            (t("Open thread"), &context.open_sender)
        };
        let action = action.clone();
        let runtime = use_context::<crate::runtime::Rt>();
        let sender = use_context::<crate::state::nav::Navigator>()
            .and_then(|nav| nav.top().thread().cloned())
            .and_then(|key| {
                t3_client::thread_views::user_message::sender_thread_key(
                    &row.message,
                    &key.environment,
                )
            });
        let available = computed(move || {
            automation
                || match (runtime, &sender) {
                    (Some(runtime), Some(sender)) => runtime
                        .environments()
                        .iter()
                        .any(|entry| entry.id == sender.environment),
                    (Some(_), None) => false,
                    (None, _) => true,
                }
        });
        let row = row.clone();
        // No handler (Open automation has no route yet): hidden, as the web hides it.
        actions = actions.child(action.map(|action| {
            dynamic(move || {
                available.get().then(|| {
                    let action = action.clone();
                    let row = row.clone();
                    button(
                        ButtonContent::Label(label.clone()),
                        ButtonVariant::Link,
                        ButtonSize::Xs,
                        || false,
                        move || action(&row),
                    )
                })
            })
        }));
    }
    actions = actions.child(
        view()
            .tooltip(tooltip(
                tooltip_create_handle(),
                Default::default(),
                text(row.timestamp_tooltip.clone()),
            ))
            .child(
                text(row.timestamp.clone())
                    .app_class(tw("text-muted-foreground text-xs tabular-nums")),
            ),
    );
    let mut controls = view().app_class(tw(&cn([
        Some("flex items-center gap-0.5"),
        float_actions.then_some("flex-wrap justify-end"),
    ])));
    if let (Some(count), Some(revert)) = (row.revert_turn_count, &context.revert) {
        controls = controls.child(revert_user_message_button(
            row.clone(),
            count,
            revert.clone(),
            context,
        ));
    }
    if !row.copy_text.is_empty() {
        controls = controls.child(message_copy_button(
            row,
            context.state,
            row.copy_text.clone(),
            CopyKind::Message,
        ));
    }
    if let Some(link) = &row.prompt_link {
        controls = controls.child(message_copy_button(
            row,
            context.state,
            link.clone(),
            CopyKind::PromptLink,
        ));
    }
    actions.child(controls)
}

#[derive(Clone, Copy)]
enum CopyKind {
    Message,
    PromptLink,
}
impl CopyKind {
    fn label(self) -> String {
        match self {
            Self::Message => t("Copy message"),
            Self::PromptLink => t("Copy prompt link"),
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Message => Icon::Copy,
            Self::PromptLink => Icon::Link,
        }
    }
}
fn message_copy_button(
    row: &UserRow,
    state: UserRowState,
    source: String,
    kind: CopyKind,
) -> impl IntoElement + use<> {
    let key = format!(
        "{}:{}",
        row.id,
        match kind {
            CopyKind::Message => "message",
            CopyKind::PromptLink => "prompt-link",
        }
    );
    let observe_key = key.clone();
    let copied = computed(move || state.read(&observe_key).copied);
    let source = Rc::new(source);
    let anchor = element_ref();
    view()
        .element_ref(&anchor)
        .tooltip(tooltip(
            tooltip_create_handle(),
            Default::default(),
            text(move || SharedString::from(kind.label())),
        ))
        .child(dynamic(move || {
            let key = key.clone();
            let source = source.clone();
            button(
                ButtonContent::Icon {
                    icon: if copied.get() {
                        Icon::Check
                    } else {
                        kind.icon()
                    },
                    label: kind.label(),
                },
                ButtonVariant::GhostMuted,
                ButtonSize::Xs,
                move || copied.get(),
                move || state.copy(key.clone(), source.as_ref().clone(), anchor),
            )
        }))
}

fn actions_frame(actions: impl IntoElement) -> impl IntoElement {
    view().app_class(move || Some(tw(&cn([
        Some("flex w-full max-w-[80%] justify-end pe-1 items-center text-xs tabular-nums opacity-0 transition-opacity duration-200 motion-reduce:transition-none focus-within:opacity-100 group-hover:opacity-100"),
        (pointer() == Pointer::Coarse).then_some("opacity-100"),
    ])))).child(actions)
}

fn revert_user_message_button(
    row: UserRow,
    count: usize,
    action: RevertAction,
    context: &UserRowContext,
) -> impl IntoElement + use<> {
    let working = context.working;
    let reverting = context.reverting;
    view()
        .tooltip(tooltip(
            tooltip_create_handle(),
            Default::default(),
            text(t("Edit from here")),
        ))
        .child(button(
            ButtonContent::Icon {
                icon: Icon::Pencil,
                label: t("Edit from here"),
            },
            ButtonVariant::Ghost,
            ButtonSize::Xs,
            move || working.get() || reverting.get(),
            move || {
                if !working.get_untracked() && !reverting.get_untracked() {
                    action(&row, count);
                }
            },
        ))
}

fn collapsible_user_message_body(
    row: Rc<UserRow>,
    context: UserRowContext,
    collapsible: bool,
) -> impl IntoElement {
    let state = context.state;
    let id = row.id.clone();
    let fold = computed(move || state.read(&id));
    let has_body = !row.text.trim().is_empty();
    let can_collapse = move || {
        collapsible
            && has_body
            && fold.get().fold_threshold > 0.0
            && fold.get().content_height >= fold.get().fold_threshold
    };
    let measure_id = row.id.clone();
    let toggle_id = row.id.clone();
    let threshold_id = row.id.clone();
    view()
        .child(
            view()
                .accessible(false)
                .pointer_events(PointerEvents::None)
                .app_class(tw("absolute w-px h-[22rem] invisible"))
                .on_layout(move |bounds: &Bounds<Points>| {
                    let threshold = bounds.size.height.0;
                    if state.read(&threshold_id).fold_threshold != threshold {
                        state.update(&threshold_id, |state| state.fold_threshold = threshold);
                    }
                }),
        )
        .child(has_body.then(|| {
            view()
                // NSH has clipping but no CSS mask-image: the web's 1.75rem fade needs an NSH mask primitive.
                .app_class(move || {
                    Some(tw(&cn([
                        Some("relative"),
                        (can_collapse() && !fold.get().body_expanded)
                            .then_some("max-h-44 overflow-hidden"),
                    ])))
                })
                .test_id("data-user-message-body")
                .child(
                    view()
                        .on_layout(move |bounds: &Bounds<Points>| {
                            let height = bounds.size.height.0;
                            if state.read(&measure_id).content_height != height {
                                state.update(&measure_id, |state| state.content_height = height);
                            }
                        })
                        .child(user_message_body(&row, &context)),
                )
        }))
        .child(dynamic(move || {
            can_collapse().then(|| {
                let toggle_id = toggle_id.clone();
                let label = if fold.get().body_expanded {
                    t("Show less")
                } else {
                    t("Show full message")
                };
                view()
                    .app_class(tw("mt-1.5 flex items-center justify-end gap-2"))
                    .child(view().app_class(tw("-ml-1")).child(button(
                        ButtonContent::Label(label),
                        ButtonVariant::GhostMuted,
                        ButtonSize::Xs,
                        || false,
                        move || {
                            state.update(&toggle_id, |state| {
                                state.body_expanded = !state.body_expanded
                            })
                        },
                    )))
            })
        }))
}

fn user_message_body(row: &UserRow, context: &UserRowContext) -> AnyElement {
    let plugin = UserMarkdownPlugin {
        row: Rc::new(row.clone()),
        chip: context.context_chip.clone(),
    };
    let document = Parser::new()
        .options(MarkdownOptions {
            html: false,
            ..Default::default()
        })
        .plugin(plugin.clone())
        .markdown(&row.text);
    nsh_base::text_view()
        .document(Some(document))
        .render_custom(move |node| plugin.render(node).unwrap_or_else(|| view().into_element()))
        .render_code_block(user_code_block)
        .selectable(true)
        .classes(TextViewClasses::default())
        .text_color(move || palette().message_foreground)
        .app_class(tw("text-message-foreground text-sm leading-relaxed"))
        .into_element()
}

fn user_code_block(code: &nsh_base::text_view::CodeBlock) -> AnyElement {
    use crate::screens::thread::markdown::code_block::{
        CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title, markdown_code_block,
    };
    let props = CodeBlockProps {
        code: code.code.clone(),
        language: code.language.clone().unwrap_or_else(|| "text".into()),
        fence_title: extract_fence_title(code.meta.as_deref()),
        ..Default::default()
    };
    let body = code.code.clone();
    markdown_code_block(
        computed(move || props.clone()),
        CodeBlockState::new(),
        CodeBlockOptions::default(),
        move |_| {
            text(SharedString::from(body.clone()))
                .select_text()
                .app_class(tw("font-mono"))
                .into_element()
        },
    )
    .into_element()
}

fn control_hit_slop(control_size: f32) -> EdgeInsets {
    EdgeInsets::all(px(((crate::theme::metrics().hit_target - control_size)
        / 2.0)
        .max(0.0)))
}

/// `downloadable` includes the client's optimistic upload state, absent from ChatFileAttachment.
#[derive(Clone)]
pub struct FilePresentation {
    pub file: ChatFileAttachment,
    pub downloadable: bool,
    pub extension: String,
    pub open: Option<FileAction>,
    pub download: Option<FileAction>,
    pub entry_icon: Option<EntryIconRenderer>,
}

pub fn user_file_card(file: FilePresentation) -> impl IntoElement {
    let label = user_message_file_label(&file.file);
    let pasted = file.file.source.as_ref().and_then(Option::as_ref).is_some();
    let size = t3_i18n::display::format_bytes(file.file.size_bytes as f64);
    let details = if !file.downloadable {
        t("Sending…")
    } else if pasted {
        size
    } else {
        format!("{} · {size}", file.extension)
    };
    let entry_icon = view()
        .app_class(tw(
            "flex size-9 shrink-0 items-center justify-center rounded-lg bg-background/60",
        ))
        .child(
            file.entry_icon
                .as_ref()
                .map(|render| render(&file.file.name)),
        );
    let identity = view()
        .app_class(tw("min-w-0 flex-1"))
        .child(text(label.clone()).app_class(tw(
            "block truncate text-sm font-medium group-hover/file:underline",
        )))
        .child(text(details).app_class(tw("block text-xs text-muted-foreground tabular-nums")));
    let identity = if file.downloadable {
        let open = file.open.clone();
        let attachment = file.file.clone();
        nsh_base::button()
            .accessibility_label(Some(SharedString::from(tr("Preview {label}", &[("label", Value::Text(label.clone()))]))))
            .disabled(open.is_none())
            .hit_slop(move || control_hit_slop(36.0))
            .on_press(move || { if let Some(open) = &open { open(&attachment); } })
            .app_class(tw("group/file focus-visible:ring-ring/70 flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-xl text-left focus-visible:ring-2 focus-visible:ring-inset focus-visible:outline-none"))
            .child(entry_icon).child(identity).into_element()
    } else {
        view()
            .app_class(tw("flex min-w-0 flex-1 items-center gap-3"))
            .child(entry_icon)
            .child(identity)
            .into_element()
    };
    let download_label = if file.downloadable {
        tr("Download {label}", &[("label", Value::Text(label))])
    } else {
        t("Sending attachment")
    };
    let card = view().app_class(tw("flex w-64 max-w-full items-center gap-1 rounded-2xl bg-message py-2 ps-2 pe-1 text-message-foreground"))
        .child(identity);
    if !file.downloadable {
        return card.child(button(
            ButtonContent::Icon {
                icon: Icon::Download,
                label: download_label,
            },
            ButtonVariant::GhostMuted,
            ButtonSize::IconXs,
            || true,
            || {},
        ));
    }
    // Without a download handler the button is not drawn, as the web does without onFileDownload.
    let Some(download) = file.download.clone() else {
        return card;
    };
    let hint = download_label.clone();
    let download_button = button(
        ButtonContent::Icon {
            icon: Icon::Download,
            label: download_label,
        },
        ButtonVariant::GhostMuted,
        ButtonSize::IconXs,
        || false,
        move || download(&file.file),
    );
    card.child(
        view()
            .tooltip(tooltip(
                tooltip_create_handle(),
                Default::default(),
                text(hint),
            ))
            .child(download_button),
    )
}

pub fn user_video_attachment(
    name: &str,
    standalone: bool,
    corners: &str,
    player: Option<AnyElement>,
) -> impl IntoElement + use<> {
    // MEDIA_EDGE_CLASS_NAME's after ring is the actual frame border; no pseudo-element is needed.
    let frame = cn([
        Some(if standalone {
            "relative overflow-hidden rounded-2xl ring-1 ring-inset ring-black/9 dark:ring-white/12"
        } else {
            "rounded-lg border border-border/80"
        }),
        standalone.then_some(corners),
    ]);
    view().app_class(tw(&frame)).child(player.unwrap_or_else(|| {
        view().app_class(tw("flex aspect-[4/3] w-full items-center justify-center bg-black px-2 py-3 text-center text-2xs text-white/70"))
            .child(text(name.to_owned())).into_element()
    }))
}

/// The unchipped-file branch of UserTimelineRow, used for files sent beside words.
pub fn user_file_attachment(file: FilePresentation) -> impl IntoElement {
    let label = user_message_file_label(&file.file);
    let pasted = file.file.source.as_ref().and_then(Option::as_ref).is_some();
    let identity = view()
        .app_class(tw("min-w-0 flex-1"))
        .child(text(label.clone()).app_class(tw("block truncate")))
        .child((!file.downloadable || pasted).then(|| {
            text(if file.downloadable {
                t3_i18n::display::format_bytes(file.file.size_bytes as f64)
            } else {
                t("Sending…")
            })
            .app_class(tw("block text-xs text-muted-foreground tabular-nums"))
        }));
    let glyph = file
        .entry_icon
        .as_ref()
        .map(|render| render(&file.file.name));
    if !file.downloadable {
        return view()
            .app_class(tw("flex min-w-0 items-center gap-2 py-1 text-sm"))
            .child(glyph)
            .child(identity)
            .into_element();
    }
    let open = file.open.clone();
    let attachment = file.file.clone();
    let preview = nsh_base::button()
        .accessibility_label(Some(SharedString::from(tr("Preview {label}", &[("label", Value::Text(label.clone()))]))))
        .disabled(open.is_none())
        .hit_slop(move || control_hit_slop(28.0))
        .on_press(move || { if let Some(open) = &open { open(&attachment); } })
        .app_class(tw("focus-visible:ring-ring/70 flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md py-1 text-left text-sm hover:underline focus-visible:ring-2 focus-visible:ring-inset focus-visible:outline-none"))
        .child(glyph).child(identity)
        .child(icon(Icon::Eye, 16.0, || palette().muted_foreground));
    let download_label = tr("Download {label}", &[("label", Value::Text(label))]);
    let hint = download_label.clone();
    view()
        .app_class(tw("flex min-w-0 items-center gap-1"))
        .child(preview)
        .child(file.download.clone().map(|download| {
            view()
                .tooltip(tooltip(
                    tooltip_create_handle(),
                    Default::default(),
                    text(hint),
                ))
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::Download,
                        label: download_label,
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::IconXs,
                    || false,
                    move || download(&file.file),
                ))
        }))
        .into_element()
}

/// The `mediaBlock` expression in UserTimelineRow; classification and asset URLs come from the client.
pub fn media_block(
    images: &[MessageAttachment],
    videos: Vec<AnyElement>,
    row: &UserRow,
    expand: Option<ImageAction>,
    snapshot_details: Option<AttachmentRenderer>,
) -> impl IntoElement + use<> {
    let standalone = row.media_only || row.media_above_text;
    let lone = standalone && videos.is_empty() && images.len() == 1;
    let collage = standalone && videos.is_empty() && images.len() > 1 && images.iter().all(|image| {
        matches!(&image.attachment, ChatAttachment::Image(image) if image.source.as_ref().and_then(Option::as_ref).is_none())
    });
    let corners = cn([
        row.grouped_with_previous.then_some("rounded-tr-md"),
        (row.media_above_text || row.grouped_with_next).then_some("rounded-br-md"),
    ]);
    let mut grid = view().app_class(tw(&cn([
        Some("grid"),
        Some(if !standalone { "mb-2 max-w-[210px] grid-cols-2 gap-2" }
            else if lone { "max-w-full" }
            else if collage { "relative w-80 max-w-full gap-px overflow-hidden rounded-2xl bg-black/9 dark:bg-white/12" }
            else if images.len() + videos.len() == 1 { "w-80 max-w-full" }
            else { "w-80 max-w-full grid-cols-2 gap-2" }),
        collage.then_some(corners.as_str()),
        collage.then_some(match images.len() {
            2 => "auto-rows-[12rem] grid-cols-2",
            3 => "auto-rows-[calc(6rem-0.5px)] grid-cols-[3fr_2fr]",
            _ => "auto-rows-[6rem] grid-cols-2",
        }),
    ])));
    for (index, attachment) in images.iter().enumerate() {
        let ChatAttachment::Image(image) = &attachment.attachment else {
            continue;
        };
        let snapshot = image.source.as_ref().and_then(Option::as_ref).is_some();
        let framed = (snapshot && attachment.preview_url.is_some() && standalone)
            || lone
            || (standalone && !collage);
        let mut frame = view().app_class(tw(&cn([
            Some("bg-background/70"),
            Some(if snapshot && attachment.preview_url.is_some() {
                if standalone { "col-span-2 relative min-h-16 justify-self-end overflow-hidden rounded-2xl" }
                else { "col-span-2 relative h-28 w-52 max-w-full overflow-hidden rounded-lg border border-border/80" }
            } else if lone { "relative overflow-hidden rounded-2xl" }
            else if collage { "overflow-hidden bg-background" }
            else if standalone { "relative aspect-[4/3] overflow-hidden rounded-2xl" }
            else { "aspect-[4/3] overflow-hidden rounded-lg border border-border/80" }),
            lone.then_some(corners.as_str()),
            (collage && images.len() == 3 && index == 0).then_some("row-span-2"),
            (collage && images.len() > 3 && images.len() % 2 == 1 && index == images.len() - 1).then_some("col-span-2"),
        ])));
        if let Some(src) = &attachment.preview_url {
            let label = tr(
                "Preview {name}",
                &[("name", Value::Text(image.name.clone()))],
            );
            let open = expand.clone();
            let selected = attachment.clone();
            frame = frame.child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(label)))
                    .disabled(open.is_none())
                    .on_press(move || {
                        if let Some(open) = &open {
                            open(&selected);
                        }
                    })
                    .app_class(tw("block h-full w-full cursor-zoom-in"))
                    // transform-gpu is a Chromium repaint hint; native image isolation needs no class.
                    .child(
                        nsh_core::element::image(nsh_core::element::ImageSrc::from(src.clone()))
                            .label(SharedString::from(image.name.clone()))
                            .app_class(tw(if standalone && snapshot {
                                "block h-auto max-h-48 w-auto min-w-24 max-w-full object-contain"
                            } else if lone {
                                "block h-48 w-auto min-w-24 max-w-full object-cover"
                            } else {
                                "block size-full object-cover"
                            })),
                    ),
            );
        } else {
            frame = frame.child(view().app_class(tw(&cn([
                Some("flex min-h-[72px] items-center justify-center px-2 py-3 text-center text-2xs text-muted-foreground/70"),
                lone.then_some("h-48 w-64 max-w-full"), collage.then_some("h-full"),
            ]))).child(text(image.name.clone())));
        }
        if snapshot && attachment.preview_url.is_some() {
            frame = frame.child(
                snapshot_details
                    .as_ref()
                    .map_or_else(|| view().into_element(), |render| render(attachment)),
            );
        }
        if framed {
            frame = frame.child(media_edge(if lone { &corners } else { "" }));
        }
        grid = grid.child(frame);
    }
    for video in videos {
        grid = grid.child(video);
    }
    if collage {
        grid = grid.child(media_edge(&corners));
    }
    grid
}

fn media_edge(corners: &str) -> impl IntoElement + use<> {
    // NSH cannot parse rounded-[inherit]: duplicate the frame's 2xl and tightened corners on ::after.
    view().pointer_events(PointerEvents::None).app_class(tw(&cn([
        Some("absolute inset-0 z-10 rounded-2xl ring-1 ring-inset ring-black/9 dark:ring-white/12"),
        Some(corners),
    ])))
}

/// Port of apps/web/src/components/chat/TimelineUserMessageRow.logic.ts userMessageFileLabel.
pub fn user_message_file_label(file: &ChatFileAttachment) -> String {
    if file.source.as_ref().and_then(Option::as_ref).is_some() {
        t("Pasted text")
    } else {
        file.name.clone()
    }
}

/// Port of apps/web/src/components/chat/TimelineUserMessageRow.logic.ts resolvePreviewAnnotationImage.
pub fn resolve_preview_annotation_image<'a>(
    record: &PreviewAnnotationContextRecord,
    records_by_id: &BTreeMap<String, ComposerContextRecord>,
    user_images: &'a [MessageAttachment],
    preview_images: &'a [MessageAttachment],
    annotation_record_ids: &[String],
) -> Option<&'a MessageAttachment> {
    let screenshot = record
        .screenshot_context_id
        .as_ref()
        .and_then(|id| records_by_id.get(&id.0));
    if let Some(ComposerContextRecord::Image(screenshot)) = screenshot
        && let Some(image) = user_images.iter().find(|image| matches!(
            &image.attachment, ChatAttachment::Image(image) if image.id == screenshot.attachment_id.0
        )) { return Some(image); }
    let name = format!("preview-annotation-{}.png", record.annotation_id);
    preview_images
        .iter()
        .find(|image| {
            matches!(
                &image.attachment, ChatAttachment::Image(image) if image.name == name
            )
        })
        .or_else(|| {
            annotation_record_ids
                .iter()
                .position(|id| id == &record.context_id.0)
                .and_then(|index| preview_images.get(index))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn file(source: Option<Value>) -> ChatFileAttachment {
        let mut value = json!({
            "type": "file", "id": "file", "name": "notes.txt", "mimeType": "text/plain", "sizeBytes": 12
        });
        if let Some(source) = source {
            value["source"] = source;
        }
        serde_json::from_value(value).expect("file")
    }

    fn image(id: &str, name: &str) -> MessageAttachment {
        serde_json::from_value(json!({
            "type": "image", "id": id, "name": name, "mimeType": "image/png", "sizeBytes": 12
        }))
        .expect("image")
    }

    fn annotation(screenshot: Option<&str>) -> PreviewAnnotationContextRecord {
        let mut value = json!({
            "version": 1, "contextId": "annotation", "kind": "preview-annotation", "label": "Region",
            "annotationId": "region", "pageUrl": "https://example.test", "pageTitle": null,
            "comment": "Fix this", "targetSummary": "Button", "styleChanges": []
        });
        if let Some(id) = screenshot {
            value["screenshotContextId"] = json!(id);
        }
        serde_json::from_value(value).expect("annotation")
    }

    fn image_record(attachment: &str) -> ComposerContextRecord {
        serde_json::from_value(json!({
            "version": 1, "contextId": "screenshot", "kind": "image", "label": "Screen",
            "attachmentId": attachment, "name": "screen.png", "mimeType": "image/png", "sizeBytes": 12
        })).expect("record")
    }

    #[test]
    fn pasted_text_uses_its_label_while_absent_and_null_sources_keep_the_filename() {
        for source in [None, Some(Value::Null)] {
            assert_eq!(user_message_file_label(&file(source)), "notes.txt");
        }
        assert_eq!(
            user_message_file_label(&file(Some(json!({"_tag": "pasted-text"})))),
            "Pasted text"
        );
    }

    #[test]
    fn a_bound_screenshot_wins_over_both_legacy_fallbacks() {
        let images = [image("bound", "screen.png")];
        let previews = [
            image("position", "old.png"),
            image("named", "preview-annotation-region.png"),
        ];
        let records = BTreeMap::from([("screenshot".into(), image_record("bound"))]);
        let selected = resolve_preview_annotation_image(
            &annotation(Some("screenshot")),
            &records,
            &images,
            &previews,
            &["annotation".into()],
        );
        assert!(std::ptr::eq(selected.expect("bound image"), &images[0]));
    }

    #[test]
    fn unavailable_or_non_image_screenshots_fall_back_to_the_exact_annotation_name() {
        let previews = [
            image("position", "old.png"),
            image("named", "preview-annotation-region.png"),
        ];
        let file_record = serde_json::from_value(json!({
            "version": 1, "contextId": "screenshot", "kind": "file", "label": "file",
            "attachmentId": "bound", "name": "screen.png", "mimeType": "image/png", "sizeBytes": 12
        }))
        .expect("file record");
        for records in [
            BTreeMap::new(),
            BTreeMap::from([("screenshot".into(), image_record("missing"))]),
            BTreeMap::from([("screenshot".into(), file_record)]),
        ] {
            let selected = resolve_preview_annotation_image(
                &annotation(Some("screenshot")),
                &records,
                &[],
                &previews,
                &["annotation".into()],
            );
            assert!(std::ptr::eq(
                selected.expect("named fallback"),
                &previews[1]
            ));
        }
    }

    #[test]
    fn positional_fallback_uses_the_records_index_and_never_defaults_to_the_first_image() {
        let previews = [image("one", "old-1.png"), image("two", "old-2.png")];
        let records = BTreeMap::new();
        let selected = resolve_preview_annotation_image(
            &annotation(None),
            &records,
            &[],
            &previews,
            &["other".into(), "annotation".into()],
        );
        assert!(std::ptr::eq(selected.expect("second image"), &previews[1]));
        for ids in [
            vec![],
            vec!["other".into()],
            vec!["a".into(), "b".into(), "annotation".into()],
        ] {
            assert!(
                resolve_preview_annotation_image(&annotation(None), &records, &[], &previews, &ids)
                    .is_none()
            );
        }
    }
}
