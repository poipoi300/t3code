//! Draft annotations from a region of the managed viewer's captured frame.

use base64::Engine as _;
use serde_json::{Value, json};
use t3_contracts::auth::AuthEnvironmentScope;

use crate::{
    ClientError,
    platform::Platform,
    thread_views::composer::{
        attachment_intake::{AttachmentSession, IncomingFile},
        target::ComposerTarget,
        web_context::{self, Kind, Placement},
    },
};

use super::{browser::BrowserPreview, socket::Viewport};

/// The longest note the contract accepts (`COMPOSER_CONTEXT_PREVIEW_COMMENT_MAX_CHARS`), counted
/// in UTF-16 units like the TypeScript schema.
pub const COMMENT_MAX_UTF16: usize = 8_000;

/// The longest note the editor accepts: the contract's comment limit less room for the region
/// line this client appends.
pub const NOTE_MAX_UTF16: usize = COMMENT_MAX_UTF16 - 200;

/// A region of the captured frame, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnotationRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A region in page CSS pixels: the unit of `regions[].rect` and `screenshot.cropRect`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl AnnotationRegion {
    pub fn fits(self, capture: Viewport) -> bool {
        self.width > 0
            && self.height > 0
            && self.x.saturating_add(self.width) <= capture.width
            && self.y.saturating_add(self.height) <= capture.height
    }

    /// Convert from frame pixels to page CSS pixels. The frame is the page viewport scaled by the
    /// worker's capture ratio (device pixel ratio and the frame cap), so the factor is
    /// `page / frame` on each axis.
    pub fn to_page(self, frame: Viewport, page: Viewport) -> PageRect {
        let sx = f64::from(page.width) / f64::from(frame.width);
        let sy = f64::from(page.height) / f64::from(frame.height);
        PageRect {
            x: f64::from(self.x) * sx,
            y: f64::from(self.y) * sy,
            width: f64::from(self.width) * sx,
            height: f64::from(self.height) * sy,
        }
    }
}

/// Everything one attach needs. `page` and `frame` come from the same moment as `png`.
pub struct AnnotationRequest {
    pub thread_id: String,
    pub url: String,
    pub title: Option<String>,
    /// The page viewport in CSS pixels when the frame was captured.
    pub page: Viewport,
    /// The captured frame's pixel size.
    pub frame: Viewport,
    /// The marked region in frame pixels.
    pub region: AnnotationRegion,
    pub note: String,
    /// The region cropped from the frame, as PNG (the web contract's screenshot format).
    pub png: Vec<u8>,
    pub created_at: String,
}

impl<P: Platform + Clone + 'static> BrowserPreview<P> {
    pub fn can_annotate(&self) -> bool {
        self.endpoint().is_ok_and(|endpoint| {
            endpoint.grants_scope(AuthEnvironmentScope::OrchestrationOperate) == Some(true)
        })
    }

    /// Stage the cropped PNG through composer intake, then associate the region with the image
    /// identity assigned by that intake. Dropping the future before it finishes removes any image
    /// it staged, so a cancelled save leaves the draft as it was.
    pub async fn attach_annotation(&self, request: AnnotationRequest) -> Result<(), ClientError> {
        if !self.can_annotate() {
            return Err(ClientError::PermissionDenied {
                method: "preview annotation".into(),
            });
        }
        let AnnotationRequest {
            thread_id,
            url,
            title,
            page,
            frame,
            region,
            note,
            png,
            created_at,
        } = request;
        if thread_id != self.thread.thread.0
            || !region.fits(frame)
            || png_size(&png) != Some((region.width, region.height))
            || note.encode_utf16().count() > NOTE_MAX_UTF16
        {
            return Err(ClientError::InvalidPayload(
                "Preview annotation capture".into(),
            ));
        }
        let mut url = url::Url::parse(&url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or_else(|| ClientError::InvalidPayload("Preview annotation URL".into()))?;
        let _ = url.set_username("");
        let _ = url.set_password(None);

        let drafts = self.runtime.composer_drafts();
        let key = drafts.thread_draft_key(&self.thread);
        let intake = self
            .runtime
            .composer_attachments(&ComposerTarget::Thread(self.thread.clone()))?;
        let mut staged = Staged::new(intake.clone());
        // Like the web's `preview-annotation-{id}.png`: a name that differs per attach, so the
        // draft store never mistakes a later crop for a copy of an earlier one.
        let name = format!(
            "preview-annotation-{}.png",
            created_at.replace([':', '.'], "-")
        );
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        intake
            .intake_bytes(vec![IncomingFile {
                name,
                mime: "image/png".into(),
                bytes: png,
                source: None,
                insertion: None,
            }])
            .await?;
        let id = staged.claim(&data_url).ok_or_else(|| {
            ClientError::InvalidPayload("Preview capture was not attached.".into())
        })?;
        let css = region.to_page(frame, page);
        let rect = json!({"x": css.x, "y": css.y, "width": css.width, "height": css.height});
        let annotation = json!({
            "id": id, "pageUrl": url.as_str(), "pageTitle": title,
            "comment": format!(
                "{}\nMarked region of the attached cropped image: x {}, y {}, width {}, height {} (page px).",
                note.trim(), css.x.round(), css.y.round(), css.width.round(), css.height.round()
            ).trim(),
            "elements": [], "regions": [{"id": format!("{id}-region"), "rect": rect}],
            "strokes": [], "styleChanges": [],
            "screenshot": {
                "dataUrl": data_url, "width": region.width, "height": region.height, "cropRect": rect,
            },
            "createdAt": created_at,
        });
        let saved = if self.can_annotate() && drafts.thread_draft_key(&self.thread) == key {
            drafts.update(|document| {
                web_context::add(
                    document,
                    &key,
                    Kind::Preview,
                    &[annotation],
                    &self.thread.thread.0,
                    Placement::default(),
                );
                Ok(())
            })
        } else {
            Err(ClientError::Cancelled)
        };
        match saved {
            Ok(true) => {
                staged.commit();
                Ok(())
            }
            result => result.and(Err(ClientError::Cancelled)),
        }
    }
}

/// Removes the image a save staged unless the save committed: runs on every exit, including the
/// future being dropped mid-await.
struct Staged {
    intake: AttachmentSession,
    before: Vec<String>,
    committed: bool,
}

impl Staged {
    fn new(intake: AttachmentSession) -> Self {
        let before = image_ids(&intake.draft());
        Self {
            intake,
            before,
            committed: false,
        }
    }

    fn fresh(&self) -> Vec<String> {
        image_ids(&self.intake.draft())
            .into_iter()
            .filter(|id| !self.before.contains(id))
            .collect()
    }

    /// The draft image that holds this crop: the one the intake added since this save began, or
    /// else the identical image the draft store already kept (it deduplicates by content, so
    /// marking the same region of an unchanged page twice reuses the first image).
    fn claim(&self, data_url: &str) -> Option<String> {
        let mut fresh = self.fresh();
        if fresh.len() == 1 {
            return Some(fresh.remove(0));
        }
        let draft = self.intake.draft();
        images(&draft)
            .iter()
            .find(|entry| entry["previewUrl"] == data_url)
            .and_then(|entry| entry["id"].as_str().map(str::to_owned))
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            for id in self.fresh() {
                self.intake.remove(&id);
            }
        }
    }
}

fn image_ids(draft: &Value) -> Vec<String> {
    images(draft)
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect()
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || !bytes.starts_with(MAGIC) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let read =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some((read(16), read(20)))
}

fn images(draft: &Value) -> &[Value] {
    draft["images"].as_array().map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: u32, y: u32, width: u32, height: u32) -> AnnotationRegion {
        AnnotationRegion {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_region_must_be_non_empty_and_inside_the_capture() {
        let capture = Viewport {
            width: 100,
            height: 50,
        };
        assert!(region(0, 0, 100, 50).fits(capture));
        assert!(!region(0, 0, 0, 10).fits(capture));
        assert!(!region(0, 0, 10, 0).fits(capture));
        assert!(!region(95, 0, 6, 10).fits(capture));
        assert!(!region(0, 45, 10, 6).fits(capture));
        assert!(!region(u32::MAX, 0, 10, 10).fits(capture));
    }

    #[test]
    fn a_scaled_frame_maps_to_exact_page_css_pixels() {
        let frame = Viewport {
            width: 1000,
            height: 500,
        };
        // A 2x capture of a 500 x 250 page.
        let page = Viewport {
            width: 500,
            height: 250,
        };
        assert_eq!(
            region(100, 50, 200, 100).to_page(frame, page),
            PageRect {
                x: 50.0,
                y: 25.0,
                width: 100.0,
                height: 50.0
            }
        );
        // A frame downscaled by the cap maps back up.
        let capped = Viewport {
            width: 3840,
            height: 2160,
        };
        let wide = Viewport {
            width: 7680,
            height: 4320,
        };
        assert_eq!(
            region(10, 20, 30, 40).to_page(capped, wide),
            PageRect {
                x: 20.0,
                y: 40.0,
                width: 60.0,
                height: 80.0
            }
        );
    }

    #[test]
    fn png_size_reads_the_header_and_rejects_other_bytes() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&7_u32.to_be_bytes());
        png.extend_from_slice(&9_u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((7, 9)));
        assert_eq!(png_size(&png[..20]), None);
        assert_eq!(png_size(&[0xff, 0xd8, 0xff, 0xe0]), None);
    }

    mod staging {
        use std::{cell::RefCell, rc::Rc};

        use crate::types::EnvironmentId;
        use futures::executor::block_on;

        use super::*;
        use crate::testing::FakePlatform;

        fn session() -> AttachmentSession {
            AttachmentSession::new(
                Rc::new(FakePlatform::default()),
                Rc::new(RefCell::new(json!({"draftsByThreadKey": {}}))),
                "env:thread".into(),
                EnvironmentId("env".into()),
            )
        }

        fn stage(intake: &AttachmentSession) {
            stage_as(intake, 0);
        }

        fn stage_as(intake: &AttachmentSession, shade: u8) {
            let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1]);
            // The draft store matches images by name and size, so different crops differ in size.
            png.extend(std::iter::repeat_n(shade, usize::from(shade)));
            block_on(intake.intake_bytes(vec![IncomingFile {
                name: "Preview annotation.png".into(),
                mime: "image/png".into(),
                bytes: png,
                source: None,
                insertion: None,
            }]))
            .expect("stage the image");
        }

        #[test]
        fn dropping_an_uncommitted_save_removes_the_image_it_staged() {
            let intake = session();
            let staged = Staged::new(intake.clone());
            stage(&intake);
            assert_eq!(images(&intake.draft()).len(), 1);
            assert!(staged.claim("").is_some());
            drop(staged);
            assert!(images(&intake.draft()).is_empty());
        }

        #[test]
        fn a_committed_save_keeps_its_image_and_never_touches_earlier_ones() {
            let intake = session();
            stage(&intake);
            let earlier = image_ids(&intake.draft());
            let mut staged = Staged::new(intake.clone());
            stage_as(&intake, 1);
            assert_eq!(image_ids(&intake.draft()).len(), 2);
            staged.commit();
            drop(staged);
            assert_eq!(image_ids(&intake.draft()).len(), 2);

            let staged = Staged::new(intake.clone());
            stage_as(&intake, 2);
            drop(staged);
            assert_eq!(image_ids(&intake.draft()).len(), 2);
            assert!(
                earlier
                    .iter()
                    .all(|id| image_ids(&intake.draft()).contains(id))
            );
        }

        #[test]
        fn marking_the_same_crop_twice_reuses_the_kept_image() {
            let intake = session();
            stage(&intake);
            let first = image_ids(&intake.draft());
            let data_url = images(&intake.draft())[0]["previewUrl"]
                .as_str()
                .expect("preview url")
                .to_owned();
            let staged = Staged::new(intake.clone());
            stage(&intake);
            assert_eq!(staged.claim(&data_url), Some(first[0].clone()));
            drop(staged);
            assert_eq!(image_ids(&intake.draft()), first, "the earlier image stays");
        }
    }
}
