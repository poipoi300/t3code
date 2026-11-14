use super::{AppUrlRequest, AppUrlSnapshot, Location, app_path};
use crate::{ClientError, platform::Platform};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};

#[derive(Clone)]
pub struct UrlClient<P> {
    platform: P,
}

impl<P: Platform + Clone + 'static> UrlClient<P> {
    pub fn new(platform: P) -> Self {
        Self { platform }
    }

    pub fn launch_address(&self) -> LocalBoxFuture<'static, Result<Option<String>, ClientError>> {
        let platform = self.platform.clone();
        async move { Ok(platform.app_url().await?) }.boxed_local()
    }

    pub fn watch(&self) -> LocalBoxStream<'static, Result<AppUrlSnapshot, ClientError>> {
        let platform = self.platform.clone();
        let changes = platform.app_url_changes();
        changes
            .then(move |change| {
                let platform = platform.clone();
                async move {
                    let mut change = change?;
                    let path = app_path(&change.path)
                        .ok_or_else(|| ClientError::InvalidPayload("Invalid app URL.".into()))?;
                    let location = Location::resolve(&path, change.policy);
                    if location.href() != change.path {
                        // Only host-resolved snapshots are published, including redirect replacements.
                        platform
                            .navigate_app_url(AppUrlRequest::Replace(location.href()))
                            .await?;
                        return Ok(None);
                    }
                    change.path = location.href();
                    Ok(Some(change))
                }
            })
            .filter_map(|result| async { result.transpose() })
            .boxed_local()
    }

    pub fn navigate(
        &self,
        request: AppUrlRequest,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let platform = self.platform.clone();
        async move {
            let href = match &request {
                AppUrlRequest::Push(href) | AppUrlRequest::Replace(href) => Some(href),
                AppUrlRequest::CloseModal(
                    super::CloseModal::Push(href) | super::CloseModal::GoThenPush { href, .. },
                ) => Some(href),
                AppUrlRequest::Go(_) => None,
            };
            if href.is_some_and(|href| !href.starts_with('/') || href.starts_with("//")) {
                return Err(ClientError::InvalidPayload("Invalid app URL.".into()));
            }
            Ok(platform.navigate_app_url(request).await?)
        }
        .boxed_local()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "service_tests.rs"]
mod tests;
