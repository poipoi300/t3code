//! Port of apps/web/src/components/chat/TimelineWorkEntryRow.tsx ViewedImagesTimelineRow, with
//! each image drawn by ChatMarkdownMedia.tsx ChatMarkdownAssetImage (`chat_markdown_asset_image`
//! below) over TL-MARKDOWN-3's `chat_markdown_image`.
//!
//! Paths resolve through the client's `resolve_viewed_image_asset`; each image's URL is the
//! client's signed asset URL (`runtime/services/assets.rs`). The row hands NSH only that URL:
//! it never reads the bytes and never picks a format from the file name.

use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::services::assets::AssetUrlState;
use t3_client::state::selectors::thread_shell;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_client::work_log::{ViewedImageAsset, resolve_viewed_image_asset};
use t3_contracts::base_schemas::ThreadId;

use crate::runtime::services::assets::{AssetUrlBinding, host_asset_urls};
use crate::runtime::{Shared, rt};
use crate::screens::thread::markdown::rich::{
    AuthoredImageSize, MarkdownImageState, MarkdownMediaProps, asset_media_props,
    authored_image_size_style, chat_markdown_image,
};
use crate::theme::tw;

/// The client row's data: `{ kind: "viewed-images", paths }`.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewedImagesRow {
    pub paths: Rc<Vec<String>>,
}

impl ViewedImagesRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let MessagesTimelineRowContent::ViewedImages { paths } = &row.content else {
            return None;
        };
        Some(Self {
            paths: Rc::new(paths.clone()),
        })
    }
}

/// MessagesTimeline's row context for this row (`threadRef`, `workspaceRoot`), per thread.
#[derive(Clone)]
pub struct ViewedImagesContext {
    environment: EnvironmentId,
    thread: ThreadId,
    /// ChatView's `workspaceRoot`: the thread's worktree, else its project's root.
    workspace_root: Memo<Option<String>>,
    assets: Option<AssetUrlBinding>,
}

impl ViewedImagesContext {
    pub fn new(key: ThreadKey) -> Self {
        Self::with_assets(key, host_asset_urls())
    }

    pub fn with_assets(key: ThreadKey, assets: Option<AssetUrlBinding>) -> Self {
        let thread_id = key.thread.clone();
        let workspace_root = rt().store.shell_state(key.environment.clone(), move |shell| {
            let shell = shell?;
            let thread = thread_shell(shell, &thread_id)?;
            thread.worktree_path.as_ref().map(|path| path.0.clone()).or_else(|| {
                shell
                    .snapshot
                    .as_ref()?
                    .projects
                    .iter()
                    .find(|project| project.id == thread.project_id)
                    .map(|project| project.workspace_root.0.clone())
            })
        });
        Self {
            environment: key.environment,
            thread: ThreadId(key.thread.0),
            workspace_root,
            assets,
        }
    }
}

/// Port of TimelineWorkEntryRow.tsx ViewedImagesTimelineRow.
pub fn viewed_images_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: ViewedImagesContext,
) -> impl IntoElement {
    let paths = computed(move || {
        row.with(|row| {
            row.as_ref()
                .and_then(|row| ViewedImagesRow::from_row(&row.0))
        })
    });
    let workspace_root = context.workspace_root;
    let images = computed(move || {
        let root = workspace_root.get();
        paths.get().map(|row| {
            Rc::new(
                row.paths
                    .iter()
                    .filter_map(|path| {
                        resolve_viewed_image_asset(path, &context.thread, root.as_deref())
                            .map(|image| (path.clone(), Rc::new(image)))
                    })
                    .collect::<Vec<_>>(),
            )
        })
    });
    let environment = context.environment.clone();
    let assets = context.assets.clone();
    dynamic(move || {
        let images = images.get()?;
        let environment = environment.clone();
        let assets = assets.clone();
        Some(untracked(move || {
            let mut gallery = view()
                .app_class(tw("flex flex-wrap items-start gap-2 px-0.5"))
                .test_id("viewed-images");
            for (_, image) in images.iter() {
                gallery = gallery.child(chat_markdown_asset_image(
                    environment.clone(),
                    image.clone(),
                    10.0,
                    assets.clone(),
                ));
            }
            gallery.into_element()
        }))
    })
}

/// ChatMarkdownAssetImage's style: the server's known size within `maxHeightRem`, else only the
/// height cap when it is not the default 30rem.
fn asset_image_size(state: &AssetUrlState, max_height_rem: f32) -> Option<AuthoredImageSize> {
    let known = match state {
        AssetUrlState::Success { asset, .. } => asset.image_dimensions.clone().flatten(),
        _ => None,
    };
    match known {
        Some(size) => authored_image_size_style(
            Some(&size.width.to_string()),
            Some(&size.height.to_string()),
            max_height_rem,
        ),
        None if max_height_rem != 30.0 => Some(AuthoredImageSize {
            max_height: Some(max_height_rem * nsh_core::geometry::REM_SIZE.0),
            ..Default::default()
        }),
        None => None,
    }
}

/// Port of ChatMarkdownMedia.tsx ChatMarkdownAssetImage for an image (`kind` "image",
/// `standalone`). Without a host client there is no asset service, so the image is the
/// unavailable fallback rather than a loading frame that never resolves.
fn chat_markdown_asset_image(
    environment: EnvironmentId,
    image: Rc<ViewedImageAsset>,
    max_height_rem: f32,
    assets: Option<AssetUrlBinding>,
) -> impl IntoElement {
    let state = signal(if assets.is_some() {
        AssetUrlState::Loading
    } else {
        AssetUrlState::Failure {
            missing_file_access: false,
        }
    });
    if let Some(assets) = assets {
        let mut urls = assets.0.watch(environment, image.resource.clone());
        // Owned by this image: unmounting it ends the read and releases the shared mint.
        spawn(async move {
            while let Some(next) = urls.next().await {
                state.set(next);
            }
        });
    }
    let props = computed(move || {
        state.with(|state| {
            asset_media_props(
                state,
                None,
                &image.src_fragment,
                MarkdownMediaProps {
                    alt: image.alt.clone(),
                    size: asset_image_size(state, max_height_rem),
                    ..Default::default()
                },
            )
        })
    });
    chat_markdown_image(props, MarkdownImageState::default(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use t3_contracts::assets::AssetCreateUrlResult;

    fn success(dimensions: Option<(u64, u64)>) -> AssetUrlState {
        let mut asset = json!({"relativeUrl":"/a","expiresAt":1});
        if let Some((width, height)) = dimensions {
            asset["imageDimensions"] = json!({"width":width,"height":height});
        }
        AssetUrlState::Success {
            url: "https://server.test/a".into(),
            asset: Rc::new(serde_json::from_value::<AssetCreateUrlResult>(asset).expect("fixture")),
        }
    }

    #[test]
    fn known_dimensions_size_the_image_within_ten_rem() {
        let size = asset_image_size(&success(Some((800, 400))), 10.0).expect("fixture");
        assert_eq!(size.width, Some(800.0));
        assert_eq!(size.aspect_ratio, Some(2.0));
        // min(30rem, 10rem * 800 / 400) = 320px.
        assert_eq!(size.max_width, Some(320.0));
    }

    #[test]
    fn unknown_dimensions_cap_only_the_height() {
        for state in [AssetUrlState::Loading, success(None)] {
            let size = asset_image_size(&state, 10.0).expect("fixture");
            assert_eq!(size.max_height, Some(160.0));
            assert_eq!(size.width, None);
        }
        assert_eq!(asset_image_size(&success(None), 30.0), None);
    }

    #[test]
    fn the_row_reads_its_paths_from_the_client_row() {
        let row: MessagesTimelineRow = serde_json::from_value(json!({
            "id":"viewed","createdAt":null,"kind":"viewed-images","paths":["/repo/a.png","/repo/b.png"]
        }))
        .expect("fixture");
        assert_eq!(
            ViewedImagesRow::from_row(&row).expect("fixture").paths.as_slice(),
            ["/repo/a.png", "/repo/b.png"]
        );
    }
}
