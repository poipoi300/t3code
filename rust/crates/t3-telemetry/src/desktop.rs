//! Serialized desktop control IPC over the host's existing async writer.

use crate::Result;
use t3_contracts::resource_telemetry::{
    DesktopTelemetryControlMessage, DesktopTelemetryServerReady, DesktopTelemetryServerReadyType,
    WireLiteral,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};

/// Default is the web/test host without a desktop control descriptor.
#[derive(Default)]
pub struct DesktopTelemetryControl {
    writer: Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>,
}

impl DesktopTelemetryControl {
    pub fn new(writer: impl AsyncWrite + Unpin + Send + 'static) -> Self {
        Self {
            writer: Mutex::new(Some(Box::new(writer))),
        }
    }

    pub async fn send(&self, message: &DesktopTelemetryControlMessage) -> Result<()> {
        let mut writer = self.writer.lock().await;
        let Some(writer) = writer.as_mut() else {
            return Ok(());
        };
        let mut payload = serde_json::to_vec(message)?;
        payload.push(b'\n');
        writer.write_all(&payload).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Called when the server admits commands, independently of deferred provider probes.
    #[tracing::instrument(name = "DesktopTelemetryReceiver.announceServerReady", skip_all)]
    pub async fn announce_server_ready(&self) -> Result<()> {
        self.send(
            &DesktopTelemetryControlMessage::DesktopTelemetryServerReady(Box::new(
                DesktopTelemetryServerReady {
                    version: WireLiteral,
                    r#type: DesktopTelemetryServerReadyType::ServerReady,
                },
            )),
        )
        .await
    }
}
