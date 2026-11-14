#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};
use t3_provider::{
    chatgpt::{Discovery, IdentityVerifier},
    identity::JwksVerifier,
    sensitive::*,
    transport::*,
};
use tokio::sync::Mutex;
struct Clock(AtomicI64);
impl ProviderClock for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Http {
    keys: Mutex<Value>,
    calls: AtomicUsize,
}
#[async_trait]
impl HttpTransport for Http {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        assert_eq!(request.method, "GET");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HttpResponse {
            status: 200,
            body: Sensitive::new(self.keys.lock().await.clone()),
        })
    }
}
fn signing_key(kid: &str) -> (signature::EcdsaKeyPair, Value) {
    let rng = SystemRandom::new();
    let doc =
        signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .unwrap();
    let pair = signature::EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        doc.as_ref(),
        &rng,
    )
    .unwrap();
    let bytes = pair.public_key().as_ref();
    let jwk = json!({"kty":"EC","crv":"P-256","kid":kid,"alg":"ES256","use":"sig","x":URL_SAFE_NO_PAD.encode(&bytes[1..33]),"y":URL_SAFE_NO_PAD.encode(&bytes[33..])});
    (pair, jwk)
}
fn signed(pair: &signature::EcdsaKeyPair, kid: &str, claims: Value) -> SecretString {
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(json!({"alg":"ES256","kid":kid}).to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature = pair.sign(&SystemRandom::new(), input.as_bytes()).unwrap();
    SecretString::new(format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}
fn metadata() -> Discovery {
    serde_json::from_value(json!({"issuer":"https://auth.openai.com","authorization_endpoint":"https://auth.openai.com/authorize","token_endpoint":"https://auth.openai.com/token","jwks_uri":"https://auth.openai.com/jwks"})).unwrap()
}
#[tokio::test]
async fn verifies_real_signatures_required_claims_and_cached_keys_with_rotation_and_expiry() {
    let (pair, jwk) = signing_key("original");
    let clock = Arc::new(Clock(AtomicI64::new(100_000)));
    let http = Arc::new(Http {
        keys: Mutex::new(json!({"keys":[jwk]})),
        calls: AtomicUsize::new(0),
    });
    let verifier = JwksVerifier::new(http.clone(), clock.clone());
    let claims = json!({"iss":"https://auth.openai.com","aud":"oaiapp_test","exp":200,"sub":"subject","email":"ada@example.com","nonce":"secret-nonce"});
    let token = signed(&pair, "original", claims.clone());
    let verified = verifier
        .verify(&token, &metadata(), "oaiapp_test", 100_000)
        .await
        .unwrap();
    assert_eq!(verified.nonce.as_deref(), Some("secret-nonce"));
    assert!(!format!("{verified:?} {verifier:?}").contains("secret-nonce"));
    verifier
        .verify(&token, &metadata(), "oaiapp_test", 100_000)
        .await
        .unwrap();
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    for (field, value) in [
        ("iss", json!("https://evil.example")),
        ("aud", json!("another-client")),
        ("exp", json!(94)),
        ("exp", json!(null)),
        ("exp", json!("200")),
        ("aud", json!(["another-client"])),
        ("nbf", json!(106)),
    ] {
        let mut claims = claims.clone();
        claims[field] = value;
        assert_eq!(
            verifier
                .verify(
                    &signed(&pair, "original", claims),
                    &metadata(),
                    "oaiapp_test",
                    100_000
                )
                .await
                .unwrap_err(),
            ServiceError::Identity
        );
    }
    let mut array_claims = claims.clone();
    array_claims["aud"] = json!(["another-client", "oaiapp_test"]);
    verifier
        .verify(
            &signed(&pair, "original", array_claims),
            &metadata(),
            "oaiapp_test",
            100_000,
        )
        .await
        .unwrap();
    let (impostor, _) = signing_key("original");
    assert_eq!(
        verifier
            .verify(
                &signed(&impostor, "original", claims.clone()),
                &metadata(),
                "oaiapp_test",
                100_000
            )
            .await
            .unwrap_err(),
        ServiceError::Identity
    );
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    let (rotated, jwk) = signing_key("rotated");
    *http.keys.lock().await = json!({"keys":[jwk]});
    let token = signed(&rotated, "rotated", claims);
    verifier
        .verify(&token, &metadata(), "oaiapp_test", 100_000)
        .await
        .unwrap();
    assert_eq!(http.calls.load(Ordering::SeqCst), 2);
    clock.0.store(700_000, Ordering::SeqCst);
    verifier
        .verify(&token, &metadata(), "oaiapp_test", 100_000)
        .await
        .unwrap();
    assert_eq!(http.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn algorithm_confusion_and_ambiguous_key_ids_are_rejected() {
    let http = Arc::new(Http {
        keys: Mutex::new(
            json!({"keys":[{"kty":"RSA","kid":"original","alg":"RS256","n":URL_SAFE_NO_PAD.encode([1u8;256]),"e":"AQAB"}]}),
        ),
        calls: AtomicUsize::new(0),
    });
    let verifier = JwksVerifier::new(http.clone(), Arc::new(Clock(AtomicI64::new(100_000))));
    let claims =
        json!({"iss":"https://auth.openai.com","aud":"oaiapp_test","exp":200,"sub":"subject"});
    for algorithm in ["none", "HS256"] {
        let token = SecretString::new(format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(json!({"alg":algorithm,"kid":"original"}).to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            URL_SAFE_NO_PAD.encode([0u8; 32])
        ));
        assert_eq!(
            verifier
                .verify(&token, &metadata(), "oaiapp_test", 100_000)
                .await
                .unwrap_err(),
            ServiceError::Identity
        );
    }
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    let (pair, jwk) = signing_key("original");
    *http.keys.lock().await = json!({"keys":[jwk.clone(), jwk]});
    let token = signed(&pair, "original", claims);
    assert_eq!(
        verifier
            .verify(&token, &metadata(), "oaiapp_test", 100_000)
            .await
            .unwrap_err(),
        ServiceError::Identity
    );
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}
