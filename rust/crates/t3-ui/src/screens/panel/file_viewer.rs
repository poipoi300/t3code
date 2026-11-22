//! Port of apps/web/src/components/files/FilePreviewPanel.tsx FilePreviewPanel (read-only), with
//! ReadOnlySourcePreview.tsx, FileMarkdownPreview.tsx, PreviewImage(Content).tsx,
//! FileBreadcrumbs.tsx (crumbs) and fileSurfaceChrome.tsx. Path rules, preview modes, reads and
//! signed URLs are the client's `workspace::project_files`; this file draws them.
//!
//! Dropped classes: `in-data-[preview-panel-mode=inline]:*` (PNL-HOST hosts every surface in
//! one panel placement; there is no inline preview mode), `scrollbar-gutter-both`.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::prelude::*;
use t3_client::{
    types::{EnvironmentId, ThreadKey},
    workspace::project_files::{
        FilesWorkspace,
        assets::{AssetReadError, AttachmentPreview, attachment_preview},
        preview::{self, AssetKind, CrumbKind, PathKinds, RenderedMode},
        read::FileReadView,
    },
};
use t3_contracts::{
    assets::AssetResource, base_schemas::TrimmedNonEmptyString,
    chat_attachment::ChatFileAttachment, project::ProjectFileFailure,
};

use super::files_tree::{
    FileBrowserProps, FilesRuntime, file_browser_panel, file_explorer_pane, files_runtime,
    problem_text,
};
use super::{Action, PanelHost, PanelRegistration, SurfaceContent, SurfaceContext, SurfaceKind};
use crate::i18n::{Text, Value, t, tr};
use crate::runtime::Shared;
use crate::screens::thread::blocks::{
    code::{CodeHighlightState, code_chunk_row, highlighted_chunk},
    flatten::CodeChunk,
};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::toggle::{self, toggle};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

use crate::runtime::tasks::watch;

/// The client file read the viewer draws (`ProjectFiles::read_file`).
pub trait FileReadHandle {
    fn view(&self) -> FileReadView;
    fn changes(&self) -> LocalBoxStream<'static, ()>;
    fn refresh(&self);
    fn observe_mutation(&self, mutation: Option<&str>);
}

/// The viewer's client services (`runtime/services/files.rs`).
pub trait FileViewerService {
    fn workspace(&self, thread: &ThreadKey) -> Option<FilesWorkspace>;
    fn workspace_mutation(&self, thread: &ThreadKey) -> Option<String>;
    fn can_write(&self, environment: &EnvironmentId) -> bool;
    fn read(
        &self,
        environment: &EnvironmentId,
        cwd: &str,
        relative_path: &str,
    ) -> Rc<dyn FileReadHandle>;
    fn asset_bytes(
        &self,
        environment: &EnvironmentId,
        resource: AssetResource,
        limit: usize,
        revision: Option<String>,
    ) -> LocalBoxFuture<'static, Result<Vec<u8>, AssetReadError>>;
}

pub(super) struct PreviewAsset {
    pub viewer: FileViewerRuntime,
    pub environment: EnvironmentId,
    pub resource: AssetResource,
    pub revision: Option<String>,
}

#[derive(Clone)]
pub struct FileViewerRuntime(pub Rc<dyn FileViewerService>);

pub fn file_viewer_runtime() -> Option<FileViewerRuntime> {
    use_context::<FileViewerRuntime>().or_else(|| {
        crate::runtime::rt()
            .backend()
            .client()
            .map(|client| crate::runtime::services::files::file_viewer(&client))
    })
}

/// The Files kind for PNL-HOST's registry: the launcher opens the workspace tree; file and
/// attachment surfaces (from the tree, file links and breadcrumbs) render here too.
pub fn files_panel_registration(host: PanelHost) -> PanelRegistration {
    let mut registration = PanelRegistration::new(
        SurfaceKind::Files,
        Prop::new(None::<Text>),
        move |key| host.apply(&key, Action::Open("files".into())),
        move |context| file_preview_panel(context).into_element(),
    );
    registration.title = Some(Rc::new(|surface: &super::Surface| match &surface.content {
        SurfaceContent::File { relative_path, .. } => SharedString::from(
            relative_path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(relative_path)
                .to_owned(),
        ),
        _ => SharedString::from(t("Files")),
    }));
    registration
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Target {
    relative_path: Option<String>,
    reveal_line: Option<i64>,
    reveal_request_id: u64,
    folder_trail: Option<String>,
    attachment: Option<ChatFileAttachment>,
}

fn target(surface: &Option<super::Surface>) -> Target {
    match surface.as_ref().map(|s| &s.content) {
        Some(SurfaceContent::File {
            relative_path,
            reveal_line,
            reveal_request_id,
            attachment,
            folder_trail,
        }) => Target {
            relative_path: Some(relative_path.clone()),
            reveal_line: *reveal_line,
            reveal_request_id: *reveal_request_id,
            folder_trail: folder_trail.clone(),
            attachment: attachment.clone(),
        },
        Some(SurfaceContent::Files { folder_trail }) => Target {
            folder_trail: folder_trail.clone(),
            ..Target::default()
        },
        _ => Target::default(),
    }
}

/// Port of FilePreviewPanel, from the surface PNL-HOST hands its renderer.
pub fn file_preview_panel(context: SurfaceContext) -> impl IntoElement {
    let (Some(files), Some(viewer)) = (files_runtime(), file_viewer_runtime()) else {
        // RightPanelSurfaceContent renders nothing without a workspace.
        return view().into_element();
    };
    let thread = context.thread.clone();
    let environment = thread.environment.clone();
    let workspace = signal(viewer.0.workspace(&thread));
    let mutation = signal(viewer.0.workspace_mutation(&thread));
    let access = signal(files.0.read_access(&environment));
    let can_write = signal(viewer.0.can_write(&environment));
    {
        let mut changes = files.0.access_changes(&environment);
        let viewer = viewer.clone();
        let files = files.clone();
        let thread = thread.clone();
        // The client's store and connection change outside NSH; the task ends with the surface.
        watch(async move {
            while changes.next().await.is_some() {
                let alive = workspace.try_set(viewer.0.workspace(&thread))
                    && mutation.try_set(viewer.0.workspace_mutation(&thread))
                    && access.try_set(files.0.read_access(&thread.environment))
                    && can_write.try_set(viewer.0.can_write(&thread.environment));
                if !alive {
                    return;
                }
            }
        });
    }
    let surface = context.surface;
    let target = computed(move || surface.with(target));
    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .test_id("file-preview-panel")
        .child(dynamic(move || {
            let target = target.get();
            if let Some(attachment) = target.attachment {
                return Some(untracked(|| {
                    attachment_file_preview(viewer.clone(), environment.clone(), attachment)
                        .into_element()
                }));
            }
            let workspace = workspace.get()?;
            let access = access.get();
            Some(untracked(|| {
                if !access.allowed {
                    return access_state(access).into_element();
                }
                file_surface(SurfaceProps {
                    files: files.clone(),
                    viewer: viewer.clone(),
                    host: context.host,
                    thread: thread.clone(),
                    workspace,
                    target,
                    mutation,
                    can_write,
                    visible: context.visible,
                })
                .into_element()
            }))
        }))
        .into_element()
}

fn access_state(access: t3_client::services::filesystem::ReadAccess) -> impl IntoElement {
    if access.pending {
        view()
            .app_class(tw("flex min-h-0 flex-1 items-center justify-center gap-2 text-sm text-muted-foreground"))
            // Spinner: a static glyph; a spinning one repaints continuously.
            .child(icon_with_class(Icon::LoaderCircle, "size-4"))
            .child(text(t("Checking file access...")))
            .into_element()
    } else {
        view()
            .app_class(tw("p-4 text-sm text-muted-foreground"))
            .child(text(match access.error {
                Some(error) => Text::client_key(error).resolve(),
                None => SharedString::from(t("This connection cannot read host files.")),
            }))
            .into_element()
    }
}

struct SurfaceProps {
    files: FilesRuntime,
    viewer: FileViewerRuntime,
    host: PanelHost,
    thread: ThreadKey,
    workspace: FilesWorkspace,
    target: Target,
    mutation: Signal<Option<String>>,
    can_write: Signal<bool>,
    visible: Memo<bool>,
}

fn preference_bool(files: &FilesRuntime, key: &'static str, default: bool) -> Signal<bool> {
    let value = signal(default);
    let service = files.0.clone();
    watch(async move {
        if let Some(saved) = service.preference(key).await.and_then(|v| v.as_bool()) {
            value.try_set(saved);
        }
    });
    value
}

/// FileSurfaceAction: a ghost icon button with its tooltip.
pub(super) fn file_surface_button(
    label: String,
    icon: Icon,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let caption = label.clone();
    view()
        .app_class(tw("inline-flex shrink-0"))
        .tooltip(tooltip(
            tooltip_create_handle(),
            TooltipOptions::default(),
            text(caption),
        ))
        .child(button(
            ButtonContent::Custom {
                label,
                child: icon_with_class(icon, "size-3.5").into_element(),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconSm,
            disabled,
            on_press,
        ))
}

/// FileSurfaceAction with `pressed`: a ghost toggle whose name and glyph follow its state. NSH's
/// toggle takes a fixed name and a press must not dispose its own control, so both states stay
/// mounted and the inactive one is hidden (as rows/plan.rs keeps both of its buttons).
pub(super) fn file_surface_toggle(
    pressed: Memo<bool>,
    label: impl Fn(bool) -> String,
    icon: impl Fn(bool) -> Icon,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let on_press = Rc::new(on_press);
    let half = |state: bool| {
        let on_press = on_press.clone();
        let name = label(state);
        view()
            .app_class(move || {
                Some(tw(if pressed.get() == state {
                    "inline-flex shrink-0"
                } else {
                    "hidden"
                }))
            })
            .tooltip(tooltip(
                tooltip_create_handle(),
                TooltipOptions::default(),
                text(name.clone()),
            ))
            .child(toggle(
                toggle::Content::slot(name, icon_with_class(icon(state), "size-3.5")),
                toggle::Variant::Ghost,
                toggle::Size::Sm,
                move || state,
                || false,
                move |_| on_press(),
            ))
    };
    view()
        .app_class(tw("inline-flex shrink-0"))
        .child(half(false))
        .child(half(true))
}

const FILE_SURFACE_SUBHEADER_CLASS: &str =
    "flex h-10 min-h-10 shrink-0 items-center gap-2 border-b border-border/60 bg-background px-3";

fn file_surface(props: SurfaceProps) -> impl IntoElement {
    let SurfaceProps {
        files,
        viewer,
        host,
        thread,
        workspace,
        target,
        mutation,
        can_write,
        visible,
    } = props;
    let environment = thread.environment.clone();
    let cwd = workspace.cwd.clone();
    let relative_path = preview::resolve_file_preview_path(target.relative_path.as_deref(), &cwd);
    let read = relative_path
        .as_ref()
        .map(|path| viewer.0.read(&environment, &cwd, path));
    let file = signal(read.as_ref().map(|r| r.view()).unwrap_or_default());
    if let Some(read) = read.clone() {
        let mut changes = read.changes();
        let observed = read.clone();
        watch(async move {
            while changes.next().await.is_some() {
                // The read lives as long as this task; `file` goes with the surface.
                if !file.try_set(read.view()) {
                    return;
                }
            }
        });
        effect(move || {
            let token = mutation.get();
            if visible.get() {
                untracked(|| observed.observe_mutation(token.as_deref()));
            }
        });
    }
    let kinds = relative_path
        .as_deref()
        .map(|p| PathKinds::of(p, false))
        .unwrap_or_default();
    let host_file = relative_path.as_deref().is_some_and(preview::is_host_path);
    let folder_trail = target.folder_trail.clone();
    let has_trail = folder_trail.is_some();
    let is_folder = computed(move || {
        file.with(|f| f.is_not_file || (has_trail && f.data.is_none() && f.error.is_none()))
    });
    let breadcrumb_path = relative_path
        .clone()
        .or_else(|| folder_trail.as_ref().map(|_| String::new()));
    let breadcrumb_trail = breadcrumb_path
        .as_deref()
        .map(|path| preview::retained_breadcrumb_trail(folder_trail.as_deref(), path));
    let explorer_open = preference_bool(&files, preview::FILE_EXPLORER_STORAGE_KEY, true);
    let render_markdown = preference_bool(&files, preview::RENDER_MARKDOWN_STORAGE_KEY, false);
    let render_browser = preference_bool(&files, preview::RENDER_BROWSER_FILE_STORAGE_KEY, true);
    let render_table = preference_bool(&files, preview::RENDER_TABLE_STORAGE_KEY, true);
    let render_mermaid = preference_bool(&files, preview::RENDER_MERMAID_STORAGE_KEY, true);
    let word_wrap = signal(false);
    let handled_reveal = signal(None::<(String, u64)>);
    let reveal_handled = {
        let path = target.relative_path.clone();
        let request = target.reveal_request_id;
        let line = target.reveal_line;
        computed(move || {
            line.is_none() || handled_reveal.get() == path.clone().map(|p| (p, request))
        })
    };
    let rendered_mode = kinds.rendered_mode();
    let (rendered, rendered_pref) = match rendered_mode {
        Some(RenderedMode::Mermaid) => (
            computed(move || render_mermaid.get() && reveal_handled.get()),
            Some((render_mermaid, preview::RENDER_MERMAID_STORAGE_KEY)),
        ),
        Some(RenderedMode::Markdown) => (
            computed(move || render_markdown.get() && reveal_handled.get()),
            Some((render_markdown, preview::RENDER_MARKDOWN_STORAGE_KEY)),
        ),
        Some(RenderedMode::Table) => (
            computed(move || render_table.get() && reveal_handled.get()),
            Some((render_table, preview::RENDER_TABLE_STORAGE_KEY)),
        ),
        Some(RenderedMode::Html) => (
            computed(move || render_browser.get() && reveal_handled.get()),
            Some((render_browser, preview::RENDER_BROWSER_FILE_STORAGE_KEY)),
        ),
        None => (computed(|| false), None),
    };
    let render_browser_file = computed(move || kinds.pdf || (kinds.html && rendered.get()));
    let shows_raw_text = computed(move || {
        relative_path_is_some(&file)
            && !is_folder.get()
            && !kinds.media()
            && !(rendered_mode.is_some() && rendered.get())
            && !render_browser_file.get()
            && !kinds.document
    });
    let preview_path = {
        let path = relative_path.clone();
        computed(move || if is_folder.get() { None } else { path.clone() })
    };
    let show_explorer = computed(move || {
        preview::should_show_file_explorer(
            preview_path.get().as_deref(),
            explorer_open.get(),
            false,
        )
    });

    let on_open_file: Rc<dyn Fn(String)> = {
        let thread = thread.clone();
        Rc::new(move |path: String| host.apply(&thread, Action::OpenFile { path, line: None }))
    };
    let surface_id = match &relative_path {
        Some(path) => format!("file:{path}"),
        None => "files".to_owned(),
    };

    let subheader =
        breadcrumb_path
            .clone()
            .zip(breadcrumb_trail.clone())
            .map(|(crumb_path, trail)| {
                let files_for_toggle = files.clone();
                let nav_thread = thread.clone();
                let nav_surface = surface_id.clone();
                let trail_for_nav = trail.clone();
                let navigate = Rc::new(move |crumb: String| {
                    let path = if preview::breadcrumb_path_of(&trail_for_nav) == crumb {
                        trail_for_nav.clone()
                    } else {
                        crumb
                    };
                    let keep_source = !is_folder.get_untracked();
                    host.apply(
                        &nav_thread,
                        Action::NavigateFolder {
                            surface: nav_surface.clone(),
                            trail: preview::retained_breadcrumb_trail(Some(&trail_for_nav), &path),
                            path,
                            keep_source,
                        },
                    );
                });
                let open = on_open_file.clone();
                view()
                    .app_class(tw(FILE_SURFACE_SUBHEADER_CLASS))
                    .child(scroll().horizontal().app_class(tw("min-w-0 flex-1")).child(
                        file_breadcrumbs(
                            &workspace.project_name,
                            &crumb_path,
                            &trail,
                            navigate,
                            open,
                        ),
                    ))
                    .child(rendered_mode.zip(rendered_pref).map(|(mode, (pref, key))| {
                        let files = files_for_toggle.clone();
                        let path = target.relative_path.clone();
                        let request = target.reveal_request_id;
                        file_surface_toggle(
                            rendered,
                            move |on| t(mode.toggle_label(on)),
                            move |on| {
                                if on {
                                    Icon::CodeXml
                                } else if mode == RenderedMode::Table {
                                    Icon::Table2
                                } else {
                                    Icon::Eye
                                }
                            },
                            move || {
                                let pressed = !rendered.get_untracked();
                                pref.set(pressed);
                                files.0.set_preference(key, serde_json::json!(pressed));
                                handled_reveal.set(if pressed {
                                    path.clone().map(|p| (p, request))
                                } else {
                                    None
                                });
                            },
                        )
                    }))
                    .child(when(
                        move || shows_raw_text.get(),
                        move || {
                            file_surface_toggle(
                                computed(move || word_wrap.get()),
                                |on| {
                                    if on {
                                        t("Disable word wrap")
                                    } else {
                                        t("Enable word wrap")
                                    }
                                },
                                |_| Icon::TextWrap,
                                move || word_wrap.set(!word_wrap.get_untracked()),
                            )
                        },
                    ))
                    .child((!host_file).then(|| {
                        let files = files_for_toggle.clone();
                        when(
                            move || preview_path.with(Option::is_some),
                            move || {
                                let files = files.clone();
                                file_surface_toggle(
                                    computed(move || explorer_open.get()),
                                    |on| {
                                        if on {
                                            t("Hide file explorer")
                                        } else {
                                            t("Show file explorer")
                                        }
                                    },
                                    |_| Icon::FolderTree,
                                    move || {
                                        let next = !explorer_open.get_untracked();
                                        explorer_open.set(next);
                                        files.0.set_preference(
                                            preview::FILE_EXPLORER_STORAGE_KEY,
                                            serde_json::json!(next),
                                        );
                                    },
                                )
                            },
                        )
                    }))
            });

    let read_only_banner = (relative_path.is_some() && !host_file).then(|| {
        when(
            move || !can_write.get(),
            || {
                view()
                    .app_class(tw(
                        "shrink-0 border-b px-3 py-1.5 text-2xs text-muted-foreground",
                    ))
                    .child(text(t(
                        "Read-only connection. Unsaved edits are kept until write access returns.",
                    )))
            },
        )
    });
    let truncated_banner = when(
        move || {
            preview_path.with(Option::is_some)
                && !kinds.media()
                && !render_browser_file.get()
                && file.with(|f| f.data.as_ref().is_some_and(|d| d.truncated))
                && (kinds.markdown || kinds.mermaid || kinds.html)
        },
        move || {
            let bytes = file.with(|f| f.data.as_ref().map_or(0, |d| d.byte_length));
            view()
                .app_class(tw("shrink-0 border-b border-warning/20 bg-warning-surface px-3 py-1.5 text-2xs text-warning-foreground"))
                .child(text(tr(
                    "Preview limited to the first 1 MB of a {bytes} byte file.",
                    &[("bytes", Value::Text(crate::i18n::formats().decimal(bytes as f64)))],
                )))
        },
    );

    let content = {
        let relative_path = relative_path.clone();
        let viewer = viewer.clone();
        let environment = environment.clone();
        let cwd = cwd.clone();
        let thread_id = thread.thread.clone();
        let reveal_line = target.reveal_line;
        let reveal_request = target.reveal_request_id;
        view()
            .app_class(move || Some(tw(&cn([Some("min-w-0 flex-1 flex-col overflow-hidden"), Some(if preview_path.with(Option::is_some) { "flex" } else { "hidden" })]))))
            .child(dynamic(move || {
                let path = relative_path.clone()?;
                if is_folder.get() {
                    return None;
                }
                let absolute = t3_client::view_logic::session::links::resolve_path_link_target(&path, &cwd);
                let asset = |resource, revision| PreviewAsset {
                    viewer: viewer.clone(),
                    environment: environment.clone(),
                    resource,
                    revision,
                };
                if kinds.image {
                    let resource = preview::workspace_asset_resource(AssetKind::WorkspaceFile, &thread_id, false, &cwd, &absolute);
                    let asset = asset(resource, mutation.get());
                    return Some(untracked(|| super::file_formats::image_preview(asset, path).into_element()));
                }
                let notebook = preview::office_preview_kind(&path) == Some("ipynb");
                if kinds.video || kinds.audio || render_browser_file.get() {
                    // Media and browser documents: PRV-4 and FILES-3 hand-off.
                    return None;
                }
                let rendered_now = rendered.get();
                let (data, error, read_error, missing) = file.with(|f| (f.data.clone(), f.error.clone(), f.read_error.clone(), f.missing));
                let binary_read = read_error.as_ref().is_some_and(|error| {
                    error.failure.as_ref().and_then(Option::as_ref) == Some(&ProjectFileFailure::BinaryFile)
                });
                let known_binary = preview::binary_preview_kind(&path).is_some();
                if known_binary && (binary_read || data.is_some()) {
                    let resource = preview::workspace_asset_resource(AssetKind::WorkspaceFile, &thread_id, false, &cwd, &absolute);
                    let asset = asset(resource, mutation.get());
                    return Some(untracked(|| super::file_formats::binary_preview(asset).into_element()));
                }
                if data.is_none() && (error.is_some() || missing) {
                    let message = match (&read_error, missing) {
                        (_, true) => Text::content(tr("'{path}' does not exist.", &[("path", Value::Text(path.clone()))])),
                        (Some(read_error), _) => {
                            let (text, owned) = preview::file_preview_read_error_message(read_error);
                            if owned { Text::client_key(text) } else { Text::content(text) }
                        }
                        (None, _) => error.as_ref().map(problem_text).unwrap_or_else(|| Text::content(String::new())),
                    };
                    let attempted = read_error.as_ref().and_then(|e| e.resolved_path.clone().flatten().or_else(|| e.operation_path.clone().flatten()));
                    let folder = read_error.as_ref().and_then(|e| e.cwd.clone().flatten()).map_or_else(|| cwd.clone(), |c| c.0);
                    return Some(untracked(|| read_failure(message, attempted, (!host_file).then_some(folder)).into_element()));
                }
                let Some(data) = data else {
                    return Some(untracked(|| file_surface_loading().into_element()));
                };
                if kinds.document && !notebook {
                    // Office and font renderers: FILES-3 hand-off.
                    return None;
                }
                if notebook && data.truncated {
                    let resource = preview::workspace_asset_resource(AssetKind::WorkspaceFile, &thread_id, false, &cwd, &absolute);
                    let asset = asset(resource, mutation.get());
                    return Some(untracked(|| super::file_formats::notebook_asset_preview(asset).into_element()));
                }
                Some(untracked(|| {
                    if notebook {
                        return super::file_formats::notebook_preview(&data.contents, data.byte_length).into_element();
                    }
                    if let Some(delimiter) = kinds.delimiter.filter(|_| rendered_now) {
                        return super::file_formats::delimited_table_preview(&data.contents, delimiter).into_element();
                    }
                    if kinds.markdown && rendered_now {
                        let contents = preview::mask_markdown_frontmatter(&data.contents).into_owned();
                        return scroll()
                            .app_class(tw("min-h-0 flex-1"))
                            .child(crate::screens::thread::markdown::base::chat_markdown_with_root_classes(
                                move || contents.clone(),
                                || false,
                                "mx-auto max-w-4xl px-6 py-5",
                            ))
                            .into_element();
                    }
                    read_only_source_preview(
                        path.clone(),
                        Shared(data.clone()),
                        computed(move || word_wrap.get()),
                        reveal_line.map(|line| (preview::clamp_file_line(&data.contents, line), reveal_request)),
                    )
                    .into_element()
                }))
            }))
    };

    let explorer = {
        let files = files.clone();
        let environment = environment.clone();
        let cwd = cwd.clone();
        let project_name = workspace.project_name.clone();
        let relative = relative_path.clone();
        let row_width = signal(0.0_f32);
        let open = on_open_file.clone();
        let reveal_id = target.reveal_request_id;
        view()
            .app_class(move || {
                Some(tw(if show_explorer.get() {
                    "flex min-h-0"
                } else {
                    "hidden"
                }))
            })
            .on_layout(move |bounds: &Bounds<Points>| row_width.set(bounds.size.width.0))
            .child(dynamic(move || {
                let resizable = preview_path.with(Option::is_some);
                let files = files.clone();
                let props = FileBrowserProps {
                    environment: environment.clone(),
                    cwd: cwd.clone(),
                    project_name: project_name.clone(),
                    selected_path: {
                        let relative = relative.clone();
                        computed(move || relative.clone())
                    },
                    reveal_id: computed(move || reveal_id),
                    on_open_file: open.clone(),
                    host_folder: false,
                    mutation: computed(move || mutation.get()),
                    visible,
                };
                Some(untracked(|| {
                    file_explorer_pane(
                        files.clone(),
                        resizable,
                        computed(move || row_width.get()),
                        file_browser_panel(files, props),
                    )
                    .into_element()
                }))
            }))
    };

    view()
        .app_class(tw(
            "flex min-h-0 flex-1 flex-col overflow-hidden bg-background",
        ))
        .child(subheader)
        .child(read_only_banner)
        .child(truncated_banner)
        .child(
            view()
                .app_class(tw("flex min-h-0 flex-1 flex-row overflow-hidden"))
                .child(content)
                .child(explorer),
        )
}

fn relative_path_is_some(file: &Signal<FileReadView>) -> bool {
    file.with(|f| f.data.is_some())
}

/// FileSurfaceLoading: a static glyph in place of the spinner.
pub(super) fn file_surface_loading() -> impl IntoElement {
    view()
        .label(t("Loading file"))
        .app_class(tw(
            "flex min-h-0 flex-1 items-center justify-center text-muted-foreground",
        ))
        .child(icon_with_class(Icon::LoaderCircle, "size-6"))
}

fn read_failure(
    message: Text,
    attempted: Option<String>,
    folder: Option<String>,
) -> impl IntoElement {
    scroll().app_class(tw("flex min-h-0 flex-1 flex-col")).child(
        view()
            .app_class(tw("my-auto flex shrink-0 flex-col gap-3 px-6 py-6 text-center text-xs leading-relaxed"))
            .child(text(message.resolve()).app_class(tw("text-destructive")))
            .child(attempted.map(|path| {
                view()
                    .app_class(tw("text-muted-foreground"))
                    .child(text(t("Attempted path")))
                    .child(text(path).app_class(tw("block font-mono text-foreground")))
            }))
            .child(folder.map(|folder| {
                view()
                    .app_class(tw("text-muted-foreground"))
                    .child(text(tr(
                        "Workspace folder: {folder}. Check the link's path or locate the file in Files.",
                        &[("folder", Value::Text(folder))],
                    )))
            })),
    )
}

/// FileBreadcrumbs: the project, each folder and the file; folders navigate the surface.
fn file_breadcrumbs(
    project_name: &str,
    relative_path: &str,
    trail: &str,
    navigate: Rc<dyn Fn(String)>,
    open_file: Rc<dyn Fn(String)>,
) -> impl IntoElement {
    let current = preview::breadcrumb_path_of(relative_path);
    let crumbs = preview::file_breadcrumbs(project_name, trail);
    let row = view().app_class(tw("flex h-full w-max min-w-full items-center text-xs"));
    let last = crumbs.len().saturating_sub(1);
    row.children(crumbs.into_iter().enumerate().map(|(index, crumb)| {
        let is_current =
            crumb.path == current || (crumb.kind == CrumbKind::Project && current.is_empty());
        let navigate = navigate.clone();
        let open_file = open_file.clone();
        let path = crumb.path.clone();
        let is_file = crumb.kind == CrumbKind::File;
        view()
            .app_class(tw("flex items-center"))
            .child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(crumb.label.clone())))
                    .on_press(move || {
                        if is_file {
                            open_file(path.clone())
                        } else {
                            navigate(path.clone())
                        }
                    })
                    .app_class(tw(&cn([
                        Some("rounded-md px-1 py-0.5 hover:bg-accent hover:text-foreground"),
                        Some(if is_current {
                            "font-medium text-foreground"
                        } else {
                            "text-muted-foreground"
                        }),
                    ])))
                    .child(text(crumb.label.clone()).app_class(tw("truncate"))),
            )
            .child((index < last).then(|| {
                icon_with_class(
                    Icon::ChevronRight,
                    "size-3 shrink-0 text-muted-foreground/60",
                )
            }))
            .into_element()
    }))
}

/// Lines per code chunk, as TL-BLOCKS-1 splits fences.
const CHUNK_LINES: usize = 40;

/// Splits `text` into the tokenizer's chunks once per read (never per frame).
pub fn file_code_chunks(language: &str, text: &str) -> Vec<CodeChunk> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut line = 1;
    while start < text.len() || chunks.is_empty() {
        let mut end = start;
        let mut lines = 0;
        while end < text.len() && lines < CHUNK_LINES {
            end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
            lines += 1;
        }
        chunks.push(CodeChunk {
            language: Some(language.to_owned()),
            meta: None,
            text: text[start..end].to_owned(),
            start_line: line,
            first: start == 0,
            last: end >= text.len(),
            value_bytes: start..end,
        });
        line += lines;
        if end == start {
            break;
        }
        start = end;
    }
    chunks
}

/// Port of ReadOnlySourcePreview: virtualised chunks through TL-BLOCKS-2's shared tokenizer
/// queue (one chunk per frame, visible chunks first), so a 1 MB file paints at once in plain text
/// and colours as the queue reaches it.
fn read_only_source_preview(
    name: String,
    data: Shared<t3_contracts::project::ProjectReadFileResult>,
    wrapped: Memo<bool>,
    reveal: Option<(i64, u64)>,
) -> impl IntoElement {
    let language = preview::file_highlight_language(&name);
    let chunks = Rc::new(file_code_chunks(&language, &data.0.contents));
    let state = CodeHighlightState::new();
    for chunk in chunks.iter() {
        state.upsert(chunk, false);
    }
    let count = chunks.len();
    let list = element_ref();
    if let Some((line, _)) = reveal {
        let index = ((line.max(1) as usize - 1) / CHUNK_LINES).min(count.saturating_sub(1));
        // The reveal scrolls once the list lays out; a cleanup-free one-shot after mount.
        effect(move || {
            list.scroll_to_index(index as u32, Percentage(0.5), px(0.0), false);
        });
    }
    let rows = virtual_list(count, move |at| {
        let chunks = chunks.clone();
        let chunk = computed(move || chunks.get(at.index()).cloned());
        let highlighted = highlighted_chunk(chunk, computed(|| false), state.clone());
        code_chunk_row(chunk, highlighted, wrapped)
    });
    rows.element_ref(&list)
        .app_class(tw("min-h-0 flex-1 bg-(--code-background)"))
}

/// Port of WorkspaceImagePreview and PreviewImageContent: the signed workspace URL, re-minted
/// per workspace mutation, with fit, zoom and actual size.
/// AttachmentFilePreview's header and the extended formats shared with workspace files.
fn attachment_file_preview(
    viewer: FileViewerRuntime,
    environment: EnvironmentId,
    attachment: ChatFileAttachment,
) -> impl IntoElement {
    let ChatFileAttachment {
        id,
        name,
        mime_type,
        size_bytes,
        ..
    } = attachment;
    let asset = PreviewAsset {
        viewer,
        environment,
        resource: AssetResource::Attachment {
            attachment_id: TrimmedNonEmptyString(id),
            file_name: Some(TrimmedNonEmptyString(name.clone())),
            mime_type: Some(TrimmedNonEmptyString(mime_type.clone())),
            disposition: Some(t3_contracts::assets::AssetDisposition::Inline),
            render_intent: None,
        },
        revision: None,
    };
    let body = attachment_preview(&name, &mime_type).map(|format| match format {
        AttachmentPreview::Raster => {
            super::file_formats::image_preview(asset, name.clone()).into_element()
        }
        AttachmentPreview::Notebook => {
            super::file_formats::notebook_asset_preview(asset).into_element()
        }
        AttachmentPreview::Binary => super::file_formats::binary_preview(asset).into_element(),
    });
    view()
        .app_class(tw(
            "flex min-h-0 flex-1 flex-col overflow-hidden bg-background",
        ))
        .child(
            view().app_class(tw(FILE_SURFACE_SUBHEADER_CLASS)).child(
                view()
                    .app_class(tw("flex min-w-0 flex-1 items-center text-xs"))
                    .child(
                        text(t("Attachment"))
                            .app_class(tw("shrink-0 px-0.5 text-muted-foreground")),
                    )
                    .child(icon_with_class(
                        Icon::ChevronRight,
                        "mx-1 size-3.5 shrink-0 text-muted-foreground/60",
                    ))
                    .child(
                        text(name)
                            .app_class(tw("min-w-0 truncate px-0.5 font-medium text-foreground")),
                    )
                    .child((size_bytes > 0).then(|| {
                        text(t3_client::services::assets::format_attachment_size(
                            size_bytes,
                        ))
                        .app_class(tw("ml-2 shrink-0 text-muted-foreground"))
                    })),
            ),
        )
        .child(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_the_text_in_forty_line_pieces() {
        let text: String = (1..=85).map(|i| format!("line {i}\n")).collect();
        let chunks = file_code_chunks("rs", &text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks.iter().map(|c| c.start_line).collect::<Vec<_>>(),
            [1, 41, 81]
        );
        assert!(chunks[0].first && chunks[2].last && !chunks[1].first);
        assert_eq!(
            chunks.iter().map(|c| c.text.as_str()).collect::<String>(),
            text
        );
        assert_eq!(
            file_code_chunks("rs", "").len(),
            1,
            "an empty file still shows one row"
        );
    }
}
