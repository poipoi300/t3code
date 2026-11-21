//! Draft annotations from a region of the managed viewer's captured frame.

use serde_json::{Value, json};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    preview::{PreviewNavStatus, PreviewSessionSnapshot},
};

use crate::{
    ClientError,
    platform::Platform,
    thread_views::composer::{
        attachment_intake::IncomingFile,
        target::ComposerTarget,
        web_context::{self, Kind, Placement},
    },
};

use super::{browser::BrowserPreview, socket::Viewport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnotationRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AnnotationRegion {
    fn fits(self, capture: Viewport) -> bool {
        self.width > 0
            && self.height > 0
            && self.x.saturating_add(self.width) <= capture.width
            && self.y.saturating_add(self.height) <= capture.height
    }
}

impl<P: Platform + Clone + 'static> BrowserPreview<P> {
    pub fn can_annotate(&self) -> bool {
        self.endpoint().is_ok_and(|endpoint| {
            endpoint.grants_scope(AuthEnvironmentScope::OrchestrationOperate) == Some(true)
        })
    }

    /// Stage the viewer's JPEG through composer intake, then associate the region
    /// with the image identity assigned by that intake.
    pub async fn attach_annotation(
        &self,
        snapshot: &PreviewSessionSnapshot,
        capture: Viewport,
        region: AnnotationRegion,
        note: &str,
        jpeg: Vec<u8>,
    ) -> Result<(), ClientError> {
        if !self.can_annotate() {
            return Err(ClientError::PermissionDenied {
                method: "preview annotation".into(),
            });
        }
        let PreviewNavStatus::Success { url, title, .. } = &snapshot.nav_status else {
            return Err(ClientError::InvalidPayload(
                "Preview annotation page".into(),
            ));
        };
        if snapshot.thread_id.0 != self.thread.thread.0
            || !region.fits(capture)
            || !jpeg.starts_with(&[0xff, 0xd8, 0xff])
        {
            return Err(ClientError::InvalidPayload(
                "Preview annotation capture".into(),
            ));
        }
        let mut url = url::Url::parse(&url.0)
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
        let before = intake.draft();
        const NAME: &str = "Preview annotation.jpg";
        intake
            .intake_bytes(vec![IncomingFile {
                name: NAME.into(),
                mime: "image/jpeg".into(),
                bytes: jpeg,
                source: None,
                insertion: None,
            }])
            .await?;
        let after = intake.draft();
        let id = images(&after)
            .iter()
            .filter(|entry| entry["name"] == NAME)
            .filter_map(|entry| entry["id"].as_str())
            .find(|id| !images(&before).iter().any(|entry| entry["id"] == *id))
            .ok_or_else(|| {
                ClientError::InvalidPayload("Preview capture was not attached.".into())
            })?;
        let annotation = json!({
            "id": id, "pageUrl": url.as_str(), "pageTitle": title,
            "comment": format!(
                "{}\nMarked region in the attached {} × {} px frame: x {}, y {}, width {}, height {}.",
                note.trim(), capture.width, capture.height, region.x, region.y, region.width, region.height
            ).trim(),
            "elements": [], "regions": [{"id": format!("{id}-region"), "rect": {
                "x":region.x, "y":region.y, "width":region.width, "height":region.height
            }}], "strokes": [], "styleChanges": [], "screenshot": null,
            "createdAt": snapshot.updated_at,
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
            Ok(true) => Ok(()),
            result => {
                intake.remove(id);
                result.and(Err(ClientError::Cancelled))
            }
        }
    }
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
}
