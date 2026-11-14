use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("JSON-RPC {code}: {message}")]
pub struct RpcError {
    #[serde(serialize_with = "serialize_code")]
    pub code: f64,
    pub message: String,
    #[serde(default, skip_serializing_if = "crate::types::Field::is_missing")]
    pub data: crate::types::Field<Value>,
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601.0,
            message: format!("Method not found: {method}"),
            data: crate::types::Field::Missing,
        }
    }
    pub fn invalid_params() -> Self {
        Self {
            code: -32602.0,
            message: "Invalid params".into(),
            data: crate::types::Field::Missing,
        }
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Codex transport closed")]
    Closed,
    #[error("Codex {operation} failed: {message}")]
    Transport {
        operation: &'static str,
        message: String,
    },
    #[error("Codex protocol violation: {0}")]
    Protocol(&'static str),
    #[error("Codex frame exceeds {limit} bytes")]
    FrameTooLarge { limit: usize },
    #[error("Invalid payload for {method} ({category}, path {path})")]
    Payload {
        method: String,
        category: String,
        path: String,
    },
    #[error("Request {id} ({method}) failed: {error}")]
    Rpc {
        id: crate::wire::RequestId,
        method: String,
        error: Box<RpcError>,
    },
    #[error("Server request has already been resolved")]
    RequestResolved,
}

impl Error {
    pub(crate) fn payload(method: &str, error: serde_json::Error) -> Self {
        Self::Payload {
            method: method.into(),
            category: format!("{:?}", error.classify()),
            path: ".".into(),
        }
    }
}

pub(crate) fn decode_payload<T: serde::de::DeserializeOwned>(
    method: &str,
    value: &Value,
) -> Result<T> {
    serde_path_to_error::deserialize(value).map_err(|error| Error::Payload {
        method: method.into(),
        category: format!("{:?}", error.inner().classify()),
        path: error.path().to_string(),
    })
}

fn serialize_code<S: serde::Serializer>(
    code: &f64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    if code.fract() == 0.0 && (-9223372036854775808.0..9223372036854775808.0).contains(code) {
        serializer.serialize_i64(*code as i64)
    } else {
        serializer.serialize_f64(*code)
    }
}
