use crate::{
    chatgpt::{Discovery, IdentityVerifier, VerifiedIdentity},
    sensitive::{SecretString, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

struct CachedKeys {
    url: String,
    expires: i64,
    keys: Vec<Value>,
}
pub struct JwksVerifier {
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    cache: Mutex<Option<CachedKeys>>,
}
impl JwksVerifier {
    pub fn new(http: Arc<dyn HttpTransport>, clock: Arc<dyn ProviderClock>) -> Self {
        Self {
            http,
            clock,
            cache: Mutex::new(None),
        }
    }
    async fn key(
        &self,
        metadata: &Discovery,
        kid: Option<&str>,
        alg: &str,
    ) -> ServiceResult<Value> {
        let select = |keys: &[Value]| -> ServiceResult<Value> {
            let mut matching = keys.iter().filter(|key| {
                kid.is_none_or(|kid| key.get("kid").and_then(Value::as_str) == Some(kid))
                    && key.get("use").is_none_or(|v| v.as_str() == Some("sig"))
                    && key.get("alg").is_none_or(|v| v.as_str() == Some(alg))
                    && key.get("key_ops").is_none_or(|v| {
                        v.as_array()
                            .is_some_and(|ops| ops.iter().any(|op| op.as_str() == Some("verify")))
                    })
                    && if alg == "RS256" {
                        key.get("kty").and_then(Value::as_str) == Some("RSA")
                    } else {
                        key.get("kty").and_then(Value::as_str) == Some("EC")
                            && key.get("crv").and_then(Value::as_str) == Some("P-256")
                    }
            });
            let key = matching.next().ok_or(ServiceError::Identity)?;
            if matching.next().is_some() {
                return Err(ServiceError::Identity);
            }
            Ok(key.clone())
        };
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref().filter(|cached| {
            cached.url == metadata.jwks_uri && cached.expires > self.clock.now_ms()
        }) && let Ok(key) = select(&cached.keys)
        {
            return Ok(key);
        }
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.http.send(HttpRequest {
                method: "GET",
                url: metadata.jwks_uri.clone(),
                headers: BTreeMap::new(),
                body: None,
            }),
        )
        .await
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Identity,
        ))?
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Identity,
        ))?;
        if response.status != 200 {
            return Err(ServiceError::Identity);
        }
        let keys = response
            .body
            .expose()
            .get("keys")
            .and_then(Value::as_array)
            .ok_or(ServiceError::Identity)?
            .clone();
        let selected = select(&keys);
        *cache = Some(CachedKeys {
            url: metadata.jwks_uri.clone(),
            expires: self.clock.now_ms().saturating_add(600_000),
            keys,
        });
        selected
    }
}
#[async_trait]
impl IdentityVerifier for JwksVerifier {
    async fn verify(
        &self,
        token: &SecretString,
        metadata: &Discovery,
        audience: &str,
        now_ms: i64,
    ) -> ServiceResult<VerifiedIdentity> {
        let raw = token.expose();
        if raw.len() > 64 * 1024 {
            return Err(ServiceError::Identity);
        }
        let parts: Vec<_> = raw.split('.').collect();
        if parts.len() != 3 {
            return Err(ServiceError::Identity);
        }
        let decode = |part: &str| {
            URL_SAFE_NO_PAD
                .decode(part)
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Identity,
                ))
        };
        let header: Value = serde_json::from_slice(&decode(parts[0])?).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Identity),
        )?;
        let alg = header
            .get("alg")
            .and_then(Value::as_str)
            .filter(|alg| ["RS256", "ES256"].contains(alg))
            .ok_or(ServiceError::Identity)?;
        if header
            .get("crit")
            .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
            || header.get("b64").is_some_and(|v| v.as_bool() != Some(true))
        {
            return Err(ServiceError::Identity);
        }
        let key = self
            .key(metadata, header.get("kid").and_then(Value::as_str), alg)
            .await?;
        let member = |name: &str| -> ServiceResult<Vec<u8>> {
            decode(
                key.get(name)
                    .and_then(Value::as_str)
                    .ok_or(ServiceError::Identity)?,
            )
        };
        let signed = format!("{}.{}", parts[0], parts[1]);
        let bytes = decode(parts[2])?;
        if alg == "RS256" {
            signature::RsaPublicKeyComponents {
                n: member("n")?,
                e: member("e")?,
            }
            .verify(
                &signature::RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &bytes,
            )
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Identity,
            ))?;
        } else {
            let x = member("x")?;
            let y = member("y")?;
            if x.len() != 32 || y.len() != 32 {
                return Err(ServiceError::Identity);
            }
            let public: Vec<_> = [vec![4], x, y].concat();
            signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, public)
                .verify(signed.as_bytes(), &bytes)
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Identity,
                ))?;
        }
        let claims: Value = serde_json::from_slice(&decode(parts[1])?).map_err(
            crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Identity),
        )?;
        let now = now_ms as f64 / 1000.;
        let audience_ok = claims.get("aud").is_some_and(|v| {
            v.as_str() == Some(audience)
                || v.as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(audience)))
        });
        if claims.get("iss").and_then(Value::as_str) != Some(&metadata.issuer)
            || !audience_ok
            || claims
                .get("exp")
                .and_then(Value::as_f64)
                .is_none_or(|exp| exp <= now - 5.)
            || claims
                .get("nbf")
                .is_some_and(|v| v.as_f64().is_none_or(|nbf| nbf > now + 5.))
        {
            return Err(ServiceError::Identity);
        }
        Ok(VerifiedIdentity {
            subject: claims
                .get("sub")
                .and_then(Value::as_str)
                .ok_or(ServiceError::Identity)?
                .into(),
            email: claims
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_owned),
            nonce: claims
                .get("nonce")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

impl std::fmt::Debug for JwksVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JwksVerifier([REDACTED])")
    }
}

pub fn setup_error(
    instance: t3_contracts::provider_instance::ProviderInstanceId,
    operation: &str,
    error: ServiceError,
) -> t3_contracts::provider_setup::ProviderSetupError {
    t3_contracts::provider_setup::ProviderSetupError {
        _tag: t3_contracts::provider_setup::ProviderSetupErrorTag::Error,
        instance_id: instance,
        operation: operation.into(),
        detail: error.to_string(),
        cause: None,
    }
}
