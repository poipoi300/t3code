//! Port of apps/web/src/components/threadSidebarWidth.ts and the width persistence of
//! AppSidebarLayout.tsx (`chat_thread_sidebar_width`): the thread sidebar's width rules and its
//! own storage record, written alone as the web writes that one localStorage key.

use super::ui_state::{PREFERENCES_NAMESPACE, SIDEBAR_WIDTH_KEY};
use crate::platform::{PlatformError, Storage, StorageArea, StorageKey, StorageWrite};

const THREAD_SIDEBAR_DEFAULT_WIDTH: f64 = 16.0 * 16.0;
pub const THREAD_SIDEBAR_MIN_WIDTH: f64 = 13.0 * 16.0;
pub const THREAD_MAIN_CONTENT_MIN_WIDTH: f64 = 40.0 * 16.0;

/// `resolveThreadSidebarMinimumWidth`: at least the brand row's width.
pub fn resolve_thread_sidebar_minimum_width(brand_width: f64) -> f64 {
    THREAD_SIDEBAR_MIN_WIDTH.max(brand_width.ceil())
}

/// `resolveThreadSidebarMaximumWidth`: what leaves the main content its minimum width.
pub fn resolve_thread_sidebar_maximum_width(viewport_width: f64, minimum_width: f64) -> f64 {
    minimum_width.max(viewport_width.floor() - THREAD_MAIN_CONTENT_MIN_WIDTH)
}

/// `clampThreadSidebarWidth`.
pub fn clamp_thread_sidebar_width(width: f64, minimum_width: f64, maximum_width: f64) -> f64 {
    maximum_width.min(minimum_width.max(width))
}

/// `resolveInitialThreadSidebarWidth`.
pub fn resolve_initial_thread_sidebar_width(stored_width: Option<f64>, viewport_width: f64) -> f64 {
    let preferred = stored_width.map_or(THREAD_SIDEBAR_DEFAULT_WIDTH, |width| {
        THREAD_SIDEBAR_MIN_WIDTH.max(width)
    });
    preferred.min(resolve_thread_sidebar_maximum_width(
        viewport_width,
        THREAD_SIDEBAR_MIN_WIDTH,
    ))
}

/// AppSidebarLayout's `shouldAcceptWidth`: shrinking always, growing only while the main
/// content keeps its minimum width.
pub fn should_accept_thread_sidebar_width(
    current_width: f64,
    next_width: f64,
    wrapper_width: f64,
) -> bool {
    next_width <= current_width || wrapper_width - next_width >= THREAD_MAIN_CONTENT_MIN_WIDTH
}

fn width_key() -> StorageKey {
    StorageKey {
        namespace: PREFERENCES_NAMESPACE.into(),
        environment: None,
        record: SIDEBAR_WIDTH_KEY.into(),
    }
}

/// `readInitialSidebarWidth`: the stored width (finite numbers only), resolved for the viewport.
/// A failed read logs nothing here and starts from the default, as the web's catch does.
pub async fn load_thread_sidebar_width(
    storage: &(impl Storage + ?Sized),
    viewport_width: f64,
) -> f64 {
    let stored = storage
        .get(StorageArea::Ordinary, width_key())
        .await
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<f64>(&bytes).ok())
        .filter(|width| width.is_finite());
    resolve_initial_thread_sidebar_width(stored, viewport_width)
}

/// The rail's commit (`setLocalStorageItem(storageKey, finalWidth)`); non-finite widths are
/// refused, as `Schema.Finite` refuses them.
pub async fn save_thread_sidebar_width(
    storage: &(impl Storage + ?Sized),
    width: f64,
) -> Result<(), PlatformError> {
    if !width.is_finite() {
        return Err(PlatformError::InvalidData(
            "sidebar width is not finite".into(),
        ));
    }
    let value = serde_json::to_vec(&width)
        .map_err(|error| PlatformError::InvalidData(error.to_string()))?;
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: width_key(),
                value: Some(value),
            }],
        )
        .await
}

/// `resetSidebarWidth`: removes the record; the caller then applies the default width.
pub async fn clear_thread_sidebar_width(
    storage: &(impl Storage + ?Sized),
) -> Result<(), PlatformError> {
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: width_key(),
                value: None,
            }],
        )
        .await
}

/// The width record for the UI, reached with `ClientRuntime::sidebar_width`.
#[derive(Clone)]
pub struct SidebarWidth<P> {
    platform: P,
}

impl<P: crate::platform::Platform + Clone + 'static> SidebarWidth<P> {
    pub fn new(platform: P) -> Self {
        Self { platform }
    }

    /// The width to start with in a window `viewport_width` wide.
    pub fn load(&self, viewport_width: f64) -> futures::future::LocalBoxFuture<'static, f64> {
        let platform = self.platform.clone();
        Box::pin(async move { load_thread_sidebar_width(&platform, viewport_width).await })
    }

    /// The rail's commit.
    pub fn save(
        &self,
        width: f64,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), PlatformError>> {
        let platform = self.platform.clone();
        Box::pin(async move { save_thread_sidebar_width(&platform, width).await })
    }

    /// The rail's double press: forget the stored width.
    pub fn clear(&self) -> futures::future::LocalBoxFuture<'static, Result<(), PlatformError>> {
        let platform = self.platform.clone();
        Box::pin(async move { clear_thread_sidebar_width(&platform).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakePlatform;
    use futures::executor::block_on;

    #[test]
    fn widths_follow_the_web_rules() {
        assert_eq!(resolve_thread_sidebar_minimum_width(100.0), 208.0);
        assert_eq!(resolve_thread_sidebar_minimum_width(230.2), 231.0);
        assert_eq!(resolve_thread_sidebar_maximum_width(1280.9, 208.0), 640.0);
        assert_eq!(resolve_thread_sidebar_maximum_width(700.0, 208.0), 208.0);
        assert_eq!(clamp_thread_sidebar_width(900.0, 208.0, 640.0), 640.0);
        assert_eq!(clamp_thread_sidebar_width(100.0, 208.0, 640.0), 208.0);
        assert_eq!(resolve_initial_thread_sidebar_width(None, 1280.0), 256.0);
        assert_eq!(
            resolve_initial_thread_sidebar_width(Some(150.0), 1280.0),
            208.0
        );
        assert_eq!(
            resolve_initial_thread_sidebar_width(Some(900.0), 1280.0),
            640.0
        );
        assert!(should_accept_thread_sidebar_width(400.0, 300.0, 700.0));
        assert!(!should_accept_thread_sidebar_width(300.0, 400.0, 1000.0));
        assert!(should_accept_thread_sidebar_width(300.0, 360.0, 1000.0));
    }

    #[test]
    fn the_width_record_round_trips_alone_and_clears() {
        let platform = FakePlatform::default();
        block_on(async {
            assert_eq!(load_thread_sidebar_width(&platform, 1280.0).await, 256.0);
            save_thread_sidebar_width(&platform, 320.0)
                .await
                .expect("saved");
            assert_eq!(load_thread_sidebar_width(&platform, 1280.0).await, 320.0);
            // A narrower window resolves the stored width down without rewriting it.
            assert_eq!(load_thread_sidebar_width(&platform, 900.0).await, 260.0);
            assert_eq!(load_thread_sidebar_width(&platform, 1280.0).await, 320.0);
            assert!(
                save_thread_sidebar_width(&platform, f64::NAN)
                    .await
                    .is_err()
            );
            clear_thread_sidebar_width(&platform)
                .await
                .expect("cleared");
            assert_eq!(load_thread_sidebar_width(&platform, 1280.0).await, 256.0);
        });
    }

    #[test]
    fn a_non_numeric_record_reads_as_the_default() {
        let platform = FakePlatform::default();
        block_on(async {
            platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: width_key(),
                        value: Some(b"\"wide\"".to_vec()),
                    }],
                )
                .await
                .expect("written");
            assert_eq!(load_thread_sidebar_width(&platform, 1280.0).await, 256.0);
        });
    }
}
