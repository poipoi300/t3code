use crate::{
    Error, Request, Result, ServerSecretStore,
    clock::iso,
    environment::dpop_invalid,
    utils::{b64, sha256, unb64},
};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use serde::{Deserialize, Serialize};
use t3_contracts::base_schemas::DpopFailureReason;

#[derive(Clone, Serialize, Deserialize)]
pub struct PublicJwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}
t3_contracts::redacted_debug!(PublicJwk { kty, crv } redact { x, y });
#[derive(Deserialize)]
struct Header {
    typ: String,
    alg: String,
    jwk: serde_json::Value,
}
#[derive(Deserialize)]
struct Payload {
    htm: String,
    htu: String,
    jti: String,
    iat: i64,
    #[serde(default, with = "t3_contracts::wire::optional")]
    ath: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCode {
    MissingProof,
    MalformedProof,
    KeyMismatch,
    MethodMismatch,
    UrlMismatch,
    AccessTokenHashMismatch,
    TimeWindow,
    InvalidSignature,
    InvalidProof,
}
#[derive(Debug)]
pub struct ProofFailure {
    pub code: FailureCode,
    pub reason: &'static str,
}
impl ProofFailure {
    fn new(code: FailureCode, reason: &'static str) -> Self {
        Self { code, reason }
    }
    fn credential(self) -> Error {
        let reason = match self.code {
            FailureCode::TimeWindow => DpopFailureReason::TimeWindow,
            FailureCode::KeyMismatch => DpopFailureReason::KeyMismatch,
            FailureCode::MethodMismatch | FailureCode::UrlMismatch => {
                DpopFailureReason::RequestMismatch
            }
            FailureCode::AccessTokenHashMismatch => DpopFailureReason::TokenMismatch,
            _ => DpopFailureReason::InvalidProof,
        };
        dpop_invalid(self.reason, reason)
    }
}
pub struct VerifiedProof {
    pub thumbprint: String,
    pub jti: String,
    pub iat: i64,
}
t3_contracts::redacted_debug!(VerifiedProof { iat } redact { thumbprint, jti });

pub struct CheckedProof {
    verified: VerifiedProof,
    now_ms: i64,
}
t3_contracts::redacted_debug!(CheckedProof { } redact { verified });
impl CheckedProof {
    pub fn thumbprint(&self) -> &str {
        &self.verified.thumbprint
    }
    pub async fn consume(&self, store: &ServerSecretStore) -> Result<()> {
        record_request_proof(store, &self.verified, self.now_ms).await
    }
}
pub fn check_request(
    request: &Request,
    proof: Option<&str>,
    expected_thumbprint: Option<&str>,
    expected_access_token: Option<&str>,
    now_ms: i64,
) -> Result<CheckedProof> {
    Ok(CheckedProof {
        verified: verify_request_proof(
            request,
            proof,
            expected_thumbprint,
            expected_access_token,
            now_ms,
        )?,
        now_ms,
    })
}
pub struct VerifyProof<'a> {
    pub proof: Option<&'a str>,
    pub method: &'a str,
    pub url: &'a str,
    pub now_epoch_seconds: i64,
    pub expected_thumbprint: Option<&'a str>,
    pub expected_access_token: Option<&'a str>,
    pub max_age_seconds: Option<i64>,
}
pub fn thumbprint(jwk: &PublicJwk) -> String {
    b64(sha256(
        serde_json::json!({ "crv": jwk.crv, "kty": jwk.kty, "x": jwk.x, "y": jwk.y }).to_string(),
    ))
}
pub fn access_token_hash(token: &str) -> String {
    b64(sha256(token))
}
pub fn normalize_htu(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
}
pub fn verify_proof(input: VerifyProof<'_>) -> std::result::Result<VerifiedProof, ProofFailure> {
    use FailureCode as F;
    let fail = || ProofFailure::new(F::InvalidProof, "Invalid DPoP proof.");
    let proof = input
        .proof
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ProofFailure::new(F::MissingProof, "Missing DPoP proof."))?;
    let parts: Vec<_> = proof.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(ProofFailure::new(
            F::MalformedProof,
            "Invalid DPoP compact JWT.",
        ));
    }
    let header_bytes = unb64(parts[0]).ok_or_else(fail)?;
    let payload_bytes = unb64(parts[1]).ok_or_else(fail)?;
    let header: Header = serde_json::from_slice(&header_bytes)
        .map_err(|_| ProofFailure::new(F::MalformedProof, "Invalid DPoP JWT header."))?;
    let jwk: PublicJwk = serde_json::from_value(header.jwk.clone())
        .map_err(|_| ProofFailure::new(F::MalformedProof, "Invalid DPoP JWT header."))?;
    if header.typ != "dpop+jwt"
        || header.alg != "ES256"
        || jwk.kty != "EC"
        || jwk.crv != "P-256"
        || jwk.x.is_empty()
        || jwk.y.is_empty()
        || header.jwk.get("d").is_some()
    {
        return Err(ProofFailure::new(
            F::MalformedProof,
            "Invalid DPoP JWT header.",
        ));
    }
    let payload: Payload = serde_json::from_slice(&payload_bytes)
        .map_err(|_| ProofFailure::new(F::MalformedProof, "Invalid DPoP JWT payload."))?;
    if payload.htm.is_empty() || payload.htu.is_empty() || payload.jti.is_empty() {
        return Err(ProofFailure::new(
            F::MalformedProof,
            "Invalid DPoP JWT payload.",
        ));
    }
    let thumbprint = thumbprint(&jwk);
    if input
        .expected_thumbprint
        .is_some_and(|expected| expected != thumbprint)
    {
        return Err(ProofFailure::new(
            F::KeyMismatch,
            "DPoP key thumbprint mismatch.",
        ));
    }
    if !payload.htm.eq_ignore_ascii_case(input.method) {
        return Err(ProofFailure::new(
            F::MethodMismatch,
            "DPoP method mismatch.",
        ));
    }
    if normalize_htu(input.url).as_deref() != Some(&payload.htu) {
        return Err(ProofFailure::new(F::UrlMismatch, "DPoP URL mismatch."));
    }
    if let Some(token) = input.expected_access_token.filter(|s| !s.is_empty())
        && payload.ath.as_deref() != Some(&access_token_hash(token))
    {
        return Err(ProofFailure::new(
            F::AccessTokenHashMismatch,
            "DPoP access token hash mismatch.",
        ));
    }
    let signature =
        Signature::from_slice(&unb64(parts[2]).ok_or_else(fail)?).map_err(|_| fail())?;
    let x = unb64(&jwk.x).ok_or_else(fail)?;
    let y = unb64(&jwk.y).ok_or_else(fail)?;
    if x.len() != 32 || y.len() != 32 {
        return Err(fail());
    }
    let public: Vec<_> = [vec![4], x, y].concat();
    let key = VerifyingKey::from_sec1_bytes(&public).map_err(|_| fail())?;
    if signature.normalize_s().is_some()
        || key
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .is_err()
    {
        return Err(ProofFailure::new(
            F::InvalidSignature,
            "Invalid DPoP signature.",
        ));
    }
    if (i128::from(input.now_epoch_seconds) - i128::from(payload.iat)).abs()
        > i128::from(input.max_age_seconds.unwrap_or(300))
    {
        return Err(ProofFailure::new(
            F::TimeWindow,
            "DPoP proof is outside the allowed time window.",
        ));
    }
    Ok(VerifiedProof {
        thumbprint,
        jti: payload.jti,
        iat: payload.iat,
    })
}
pub async fn verify_request(
    store: &ServerSecretStore,
    request: &Request,
    proof: Option<&str>,
    expected_thumbprint: Option<&str>,
    expected_access_token: Option<&str>,
    now_ms: i64,
) -> Result<String> {
    let checked = check_request(
        request,
        proof,
        expected_thumbprint,
        expected_access_token,
        now_ms,
    )?;
    checked.consume(store).await?;
    Ok(checked.verified.thumbprint)
}
pub(crate) fn verify_request_proof(
    request: &Request,
    proof: Option<&str>,
    expected_thumbprint: Option<&str>,
    expected_access_token: Option<&str>,
    now_ms: i64,
) -> Result<VerifiedProof> {
    let url = request
        .absolute_url()
        .ok_or_else(|| Error::invalid("Invalid DPoP request URL."))?;
    verify_proof(VerifyProof {
        proof: proof.or_else(|| request.header("dpop")),
        method: &request.method,
        url: url.as_str(),
        now_epoch_seconds: now_ms.div_euclid(1000),
        expected_thumbprint,
        expected_access_token,
        max_age_seconds: None,
    })
    .map_err(ProofFailure::credential)
}
pub(crate) async fn record_request_proof(
    store: &ServerSecretStore,
    verified: &VerifiedProof,
    now_ms: i64,
) -> Result<()> {
    let replay_key = b64(sha256(format!("{}:{}", verified.thumbprint, verified.jti)));
    let marker = format!(
        "thumbprint={}\njti={}\niat={}\nconsumedAt={}",
        verified.thumbprint,
        verified.jti,
        verified.iat,
        iso(now_ms)?.0
    );
    store
        .create(&format!("dpop-proof-{replay_key}"), marker.as_bytes())
        .await
        .map_err(|e| {
            if e.is_already_exists() {
                dpop_invalid("DPoP proof replayed.", DpopFailureReason::Replay)
            } else {
                e
            }
        })?;
    Ok(())
}
