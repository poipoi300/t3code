use crate::{
    Error, Result, Secret,
    store::{Secrets, read_text},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{
    Signature, Signer, SigningKey, VerifyingKey,
    pkcs8::{
        DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey,
        spki::der::pem::LineEnding,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::fmt;

pub const LINK: &str = "t3-env-link+jwt";
pub const MINT_REQUEST: &str = "t3-cloud-mint+jwt";
pub const HEALTH_REQUEST: &str = "t3-cloud-health+jwt";
pub const MINT_RESPONSE: &str = "t3-env-mint+jwt";
pub const HEALTH_RESPONSE: &str = "t3-env-health+jwt";
pub const ACTIVITY: &str = "t3-env-activity+jwt";
pub const RECOVERY: &str = "t3-env-managed-tunnel-recovery+jwt";
pub const HOOK: &str = "t3-relay-hook-delivery+jwt";
pub fn normalize_issuer(s: &str) -> &str {
    s.trim().trim_end_matches('/')
}
pub fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn unb64(s: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| Error::Proof)
}
fn normalize_pem(s: &str) -> String {
    s.replace("\\n", "\n").trim().to_owned()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentKey {
    private_key: Secret,
    pub public_key: String,
}
impl fmt::Debug for EnvironmentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnvironmentKey([REDACTED])")
    }
}
impl EnvironmentKey {
    pub fn generate() -> Result<Self> {
        let mut seed = zeroize::Zeroizing::new([0; 32]);
        getrandom::fill(seed.as_mut()).map_err(|_| Error::Proof)?;
        let key = SigningKey::from_bytes(&seed);
        Ok(Self {
            private_key: Secret::new(
                key.to_pkcs8_pem(LineEnding::LF)
                    .map_err(|_| Error::Proof)?
                    .as_str(),
            ),
            public_key: key
                .verifying_key()
                .to_public_key_pem(LineEnding::LF)
                .map_err(|_| Error::Proof)?,
        })
    }
    pub async fn load(store: &dyn Secrets) -> Result<Self> {
        const NAME: &str = "cloud-link-ed25519-key-pair";
        if let Some(value) = read_text(store, NAME).await? {
            return serde_json::from_str(&value).map_err(|_| Error::Storage);
        }
        let private = read_text(store, "cloud-link-ed25519-private-key").await?;
        let public = read_text(store, "cloud-link-ed25519-public-key").await?;
        let key = match (private, public) {
            (Some(private), Some(public_key)) => Self {
                private_key: Secret::new(private),
                public_key,
            },
            _ => Self::generate()?,
        };
        let encoded =
            zeroize::Zeroizing::new(serde_json::to_vec(&key).map_err(|_| Error::Storage)?);
        if store.create(NAME, &encoded).await? {
            return Ok(key);
        }
        let value = read_text(store, NAME).await?.ok_or(Error::Storage)?;
        serde_json::from_str(&value).map_err(|_| Error::Storage)
    }
    /// Parse lazily: malformed persisted private material fails signing, not service startup.
    pub fn sign(&self, typ: &str, payload: &impl Serialize) -> Result<Secret> {
        let key = SigningKey::from_pkcs8_pem(&normalize_pem(self.private_key.expose()))
            .map_err(|_| Error::Proof)?;
        let header =
            b64(serde_json::to_vec(&json!({"alg":"EdDSA","typ":typ})).map_err(|_| Error::Proof)?);
        let body = b64(serde_json::to_vec(payload).map_err(|_| Error::Proof)?);
        let signed = format!("{header}.{body}");
        Ok(Secret::new(format!(
            "{signed}.{}",
            b64(key.sign(signed.as_bytes()).to_bytes())
        )))
    }
}
pub fn validate_public_key(pem: &str) -> Result<()> {
    VerifyingKey::from_public_key_pem(&normalize_pem(pem))
        .map(|_| ())
        .map_err(|_| Error::Configuration)
}
pub fn unverified(token: &str) -> Result<Value> {
    if token.len() > 128 * 1024 {
        return Err(Error::Proof);
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Proof);
    }
    serde_json::from_slice(&unb64(parts[1])?).map_err(|_| Error::Proof)
}
pub fn verify<T: DeserializeOwned>(
    pem: &str,
    token: &Secret,
    typ: &str,
    issuer: &str,
    audience: &str,
    now: i64,
) -> Result<T> {
    verify_with_max_age(pem, token, typ, issuer, audience, now, 300)
}
pub fn verify_with_max_age<T: DeserializeOwned>(
    pem: &str,
    token: &Secret,
    typ: &str,
    issuer: &str,
    audience: &str,
    now: i64,
    max_age: i64,
) -> Result<T> {
    if token.expose().len() > 128 * 1024 {
        return Err(Error::Proof);
    }
    let parts: Vec<_> = token.expose().split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Proof);
    }
    let header: Value = serde_json::from_slice(&unb64(parts[0])?).map_err(|_| Error::Proof)?;
    if header["alg"] != "EdDSA"
        || header["typ"] != typ
        || header.get("crit").is_some()
        || header.get("b64").is_some()
    {
        return Err(Error::Proof);
    }
    let key = VerifyingKey::from_public_key_pem(&normalize_pem(pem)).map_err(|_| Error::Proof)?;
    let signature = Signature::from_slice(&unb64(parts[2])?).map_err(|_| Error::Proof)?;
    key.verify_strict(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .map_err(|_| Error::Proof)?;
    let payload = unverified(token.expose())?;
    let iat = payload["iat"].as_i64().ok_or(Error::Proof)?;
    let exp = payload["exp"].as_i64().ok_or(Error::Proof)?;
    let aud = &payload["aud"];
    let audience_matches = aud == audience
        || aud
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a == audience));
    if payload["iss"] != issuer
        || !audience_matches
        || exp <= now.saturating_sub(60)
        || iat > now.saturating_add(60)
        || now.saturating_sub(iat) > max_age.saturating_add(60)
    {
        return Err(Error::Proof);
    }
    if payload
        .get("nbf")
        .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now.saturating_add(60)))
    {
        return Err(Error::Proof);
    }
    serde_json::from_value(payload).map_err(|_| Error::Proof)
}
pub fn bounded(iat: i64, exp: i64, now: i64) -> bool {
    exp > iat && exp.checked_sub(iat).is_some_and(|d| d <= 300) && iat <= now.saturating_add(60)
}
