//! Open a browser viewer using the installed connection's credentials and address.

use crate::{
    permissions::EnvironmentHttpClient,
    platform::{SocketRequest, WebSocket},
    preview::PreviewTarget,
    services::http::resolve_device_hub_access,
};

impl EnvironmentHttpClient {
    pub(crate) async fn open_preview_socket(
        &self,
        thread_id: String,
        tab_id: String,
        (width, height): (u32, u32),
    ) -> Result<(PreviewTarget, Box<dyn WebSocket>), crate::preview::socket::BrowserTransportError>
    {
        if let Some(remote) = &self.remote {
            remote
                .validate_session_at(&self.prepared.session, self.authorization_revision)
                .await?;
        }
        let access = resolve_device_hub_access(self, &self.prepared, "/api/preview-stream").await?;
        if let Some(remote) = &self.remote {
            remote
                .validate_session_at(&self.prepared.session, self.authorization_revision)
                .await?;
        }
        let target = PreviewTarget {
            access: access.into_inner(),
            thread_id,
            tab_id,
            max_width: f64::from(width),
            max_height: f64::from(height),
            interactive: true,
        };
        let socket = self
            .platform
            .websocket(SocketRequest {
                url: target.websocket_url(),
                protocols: Vec::new(),
            })
            .await?;
        if let Some(remote) = &self.remote {
            remote
                .validate_session_at(&self.prepared.session, self.authorization_revision)
                .await?;
        }
        Ok((target, socket))
    }
}
