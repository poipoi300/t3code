use crate::{Error, Result, VerifiedRelayDelivery};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey, pkcs8::DecodePublicKey};
use serde_json::Value;
use std::collections::BTreeMap;
use t3_contracts::relay::RelayHookDeliveryProofPayload;

/// A linked environment's mint key, issuer and audience. Does not read the live secret store.
pub struct RelayDeliveryProof {
    key: VerifyingKey,
    issuer: String,
    environment_id: String,
}
impl RelayDeliveryProof {
    pub fn new(public_key_pem: &str, issuer: &str, environment_id: &str) -> Result<Self> {
        let key = VerifyingKey::from_public_key_pem(public_key_pem.replace("\\n", "\n").trim())
            .map_err(|e| Error::Invalid(format!("Invalid relay mint key: {e}")))?;
        Ok(Self {
            key,
            issuer: issuer.trim().trim_end_matches('/').into(),
            environment_id: environment_id.into(),
        })
    }

    /// Unverified, missing or mismatched relay metadata is a direct request, never trusted metadata.
    pub fn verify(
        &self,
        headers: &BTreeMap<String, String>,
        hook_id: &str,
        now: DateTime<Utc>,
    ) -> Option<VerifiedRelayDelivery> {
        let token = headers.get("x-t3-relay-delivery")?;
        let delivery_id = headers.get("x-t3-relay-delivery-id")?;
        let received_at = headers.get("x-t3-relay-received-at")?;
        let mut parts = token.split('.');
        let protected = parts.next()?;
        let payload = parts.next()?;
        let signature = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(protected).ok()?).ok()?;
        let typ = header["typ"].as_str()?.to_lowercase();
        if header["alg"] != "EdDSA"
            || typ.strip_prefix("application/").unwrap_or(&typ) != "t3-relay-hook-delivery+jwt"
            || header
                .get("crit")
                .is_some_and(|v| v.as_array().is_none_or(|a| a.iter().any(|v| v != "b64")))
            || header.get("b64") == Some(&Value::Bool(false))
        {
            return None;
        }
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).ok()?).ok()?;
        self.key
            .verify(format!("{protected}.{payload}").as_bytes(), &signature)
            .ok()?;
        let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        let claims: RelayHookDeliveryProofPayload = serde_json::from_value(value.clone()).ok()?;
        let now = now.timestamp();
        if value["iss"] != self.issuer
            || value["aud"] != format!("t3-env:{}", self.environment_id)
            || claims.environment_id.0 != self.environment_id
            || claims.delivery_id.0 != *delivery_id
            || claims.received_at.0 != *received_at
            || claims.hook_id.0 != hook_id
            || claims.exp <= now.saturating_sub(60)
            || claims.iat > now.saturating_add(60)
            || now.saturating_sub(claims.iat) > 25 * 60 * 60 + 60
            || value
                .get("nbf")
                .is_some_and(|v| v.as_f64().is_none_or(|t| t > now.saturating_add(60) as f64))
            || [
                claims.sub.0.as_str(),
                claims.jti.0.as_str(),
                delivery_id.as_str(),
                received_at.as_str(),
                hook_id,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return None;
        }
        Some(VerifiedRelayDelivery {
            delivery_id: delivery_id.clone(),
            received_at: received_at.clone(),
        })
    }
}
