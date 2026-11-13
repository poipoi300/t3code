//! Bounded request-body reading and decoding.
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Cap for JSON and form bodies on auth and OAuth routes: far above any legitimate request
/// (registrations and approvals are a few hundred bytes), small enough to bound memory per call.
pub const JSON_BODY_LIMIT: usize = 64 * 1024;

pub enum BodyError {
    TooLarge,
    Invalid,
}
impl IntoResponse for BodyError {
    fn into_response(self) -> Response {
        match self {
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Invalid => StatusCode::BAD_REQUEST,
        }
        .into_response()
    }
}

/// Reads at most `limit` bytes. A declared `Content-Length` over the limit is refused before the
/// body is touched; a chunked or lying body is cut off by the bounded read.
pub async fn read_limited(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<axum::body::Bytes, BodyError> {
    if let Some(declared) = headers.get(header::CONTENT_LENGTH) {
        let declared = declared
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or(BodyError::Invalid)?;
        if declared > limit as u64 {
            return Err(BodyError::TooLarge);
        }
    }
    axum::body::to_bytes(body, limit)
        .await
        .map_err(|_| BodyError::TooLarge)
}

fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| {
            v.trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

/// Decodes a JSON object body.
pub async fn json<T: DeserializeOwned>(headers: &HeaderMap, body: Body) -> Result<T, BodyError> {
    let bytes = read_limited(headers, body, JSON_BODY_LIMIT).await?;
    serde_json::from_slice(&bytes).map_err(|_| BodyError::Invalid)
}

/// Decodes `application/x-www-form-urlencoded` (the contract encoding for the OAuth token
/// endpoints) or, for callers that already send JSON, a JSON object. Form fields are strings; a
/// repeated field is ambiguous and refused.
pub async fn form_or_json<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, BodyError> {
    let bytes = read_limited(headers, body, JSON_BODY_LIMIT).await?;
    if !is_form(headers) {
        return serde_json::from_slice(&bytes).map_err(|_| BodyError::Invalid);
    }
    let mut fields = Map::new();
    for (key, value) in url::form_urlencoded::parse(&bytes) {
        if fields
            .insert(key.into_owned(), Value::String(value.into_owned()))
            .is_some()
        {
            return Err(BodyError::Invalid);
        }
    }
    serde_json::from_value(Value::Object(fields)).map_err(|_| BodyError::Invalid)
}
