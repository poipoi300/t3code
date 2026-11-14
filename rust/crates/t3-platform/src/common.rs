use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{StreamExt, channel::mpsc, stream::LocalBoxStream};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, PoisonError};
use t3_client::platform::{
    HostAction, HostFuture, HostReply, PlatformError, PlatformEvent, StorageKey,
};

/// Shell lifecycle input (network, foreground, credential changes); `Send` so OS callbacks can
/// emit from any thread. Every live `events()` stream receives every event in emission order.
#[derive(Clone, Default)]
pub struct EventSender {
    subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<PlatformEvent>>>>,
}
impl std::fmt::Debug for EventSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventSender")
    }
}
impl EventSender {
    pub fn emit(&self, event: PlatformEvent) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.unbounded_send(event.clone()).is_ok());
    }
    pub(crate) fn subscribe(&self) -> LocalBoxStream<'static, PlatformEvent> {
        let (tx, rx) = mpsc::unbounded();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx.boxed_local()
    }
    pub(crate) fn storage_changed(&self, keys: impl IntoIterator<Item = StorageKey>) {
        for key in keys {
            self.emit(PlatformEvent::StorageChanged(key));
        }
    }
}

/// Host actions this crate cannot answer (pickers, notifications, audio, cloud session, SSH…)
/// go to the shell's extension; without one they fail with `PlatformError::Unsupported`.
pub trait HostExtension {
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply>;
}

pub(crate) fn unsupported(action: &HostAction) -> PlatformError {
    PlatformError::Unsupported(format!(
        "host action {action:?} is not available on this host; the shell must provide it"
    ))
}

#[cfg(any(
    target_arch = "wasm32",
    target_os = "windows",
    target_os = "macos",
    target_os = "linux"
))]
pub(crate) const TEXT_PLAIN: &str = "text/plain";
#[cfg(any(
    target_arch = "wasm32",
    target_os = "windows",
    target_os = "macos",
    target_os = "linux"
))]
pub(crate) const TEXT_HTML: &str = "text/html";

#[cfg(any(
    target_arch = "wasm32",
    target_os = "windows",
    target_os = "macos",
    target_os = "linux"
))]
pub(crate) fn mime_essence(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// The client's one opener policy for this surface; anything else could launch arbitrary
/// handlers.
pub(crate) fn validate_open_url(
    raw: &str,
    surface: t3_client::persistence::external_links::ExternalUrlSurface,
) -> Result<String, PlatformError> {
    t3_client::persistence::external_links::safe_external_url(raw, surface)
        .ok_or_else(|| PlatformError::Denied("opening this link is not allowed".into()))
}

pub(crate) fn uuid_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(not(target_arch = "wasm32"))]
/// The record the mobile importer writes for `t3code.agent-awareness.device-id`.
pub(crate) fn device_identity_key() -> StorageKey {
    StorageKey::new("device-identity", None, "agent-awareness")
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn encode_device_identity(id: &str) -> Result<Vec<u8>, PlatformError> {
    t3_client::persistence::Record::new(
        "device-identity",
        None,
        "agent-awareness",
        1,
        id.to_owned(),
    )
    .encode()
    .map_err(|_| PlatformError::InvalidData("device identity".into()))
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode_device_identity(bytes: &[u8]) -> Result<String, PlatformError> {
    let record = t3_client::persistence::decode_record::<String>(bytes, &device_identity_key(), 1)
        .map_err(|_| PlatformError::InvalidData("device identity".into()))?;
    if record.payload.trim().is_empty() {
        return Err(PlatformError::InvalidData("device identity".into()));
    }
    Ok(record.payload)
}

/// The public P-256 JWK the client signs into DPoP headers.
pub(crate) fn dpop_public_jwk(x: &[u8], y: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(x),
        "y": URL_SAFE_NO_PAD.encode(y),
    })
}
/// RFC 7638 thumbprint; members are written in their required lexicographic order.
pub(crate) fn dpop_thumbprint(x: &[u8], y: &[u8]) -> String {
    let canonical = format!(
        r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
        URL_SAFE_NO_PAD.encode(x),
        URL_SAFE_NO_PAD.encode(y)
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbprint_matches_rfc_7638_example_shape_and_client_derivation() {
        let x = [1_u8; 32];
        let y = [2_u8; 32];
        let jwk = dpop_public_jwk(&x, &y);
        // The client derives its thumbprint from serde_json's rendering of the same members.
        let ordered = serde_json::json!({"crv":"P-256","kty":"EC","x":jwk["x"],"y":jwk["y"]});
        let expected =
            URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&ordered).unwrap()));
        assert_eq!(dpop_thumbprint(&x, &y), expected);
        assert_ne!(dpop_thumbprint(&x, &y), dpop_thumbprint(&y, &x));
    }

    #[test]
    fn uuid_has_version_and_variant_bits() {
        let id = uuid_v4([0xff; 16]);
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn device_identity_round_trips_through_the_import_record_envelope() {
        let bytes = encode_device_identity("device-1").unwrap();
        assert_eq!(decode_device_identity(&bytes).unwrap(), "device-1");
        let other = t3_client::persistence::Record::new(
            "device-identity",
            None,
            "other",
            1,
            "x".to_owned(),
        )
        .encode()
        .unwrap();
        assert!(decode_device_identity(&other).is_err());
    }

    #[test]
    fn events_reach_every_live_subscriber_and_dropped_ones_are_pruned() {
        let sender = EventSender::default();
        let mut a = sender.subscribe();
        let b = sender.subscribe();
        drop(b);
        sender.emit(PlatformEvent::Foreground);
        sender.emit(PlatformEvent::Suspended);
        assert_eq!(sender.subscribers.lock().unwrap().len(), 1);
        let received = futures::executor::block_on(async {
            vec![a.next().await.unwrap(), a.next().await.unwrap()]
        });
        assert_eq!(
            received,
            vec![PlatformEvent::Foreground, PlatformEvent::Suspended]
        );
    }
}
