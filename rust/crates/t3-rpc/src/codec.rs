use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Number, Value};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(Number),
    String(String),
}

impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.as_f64() == b.as_f64(),
            (Self::String(a), Self::String(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for RequestId {}
impl Hash for RequestId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::String(s) => s.hash(state),
            Self::Number(n) => {
                let n = n.as_f64().unwrap_or_default();
                (if n == 0.0 { 0 } else { n.to_bits() }).hash(state);
            }
        }
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self::Number(value.into())
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected literal true"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FiberId {
    Number(Number),
    Null,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request<P = Value> {
    pub id: RequestId,
    pub tag: String,
    pub payload: P,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled: Option<bool>,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_notification: Option<True>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard: Option<True>,
}

impl<P> Request<P> {
    pub fn new(id: RequestId, tag: impl Into<String>, payload: P) -> Self {
        Self {
            id,
            tag: tag.into(),
            payload,
            trace_id: None,
            span_id: None,
            sampled: None,
            headers: Vec::new(),
            is_notification: None,
            discard: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ClientMessage<P = Value> {
    Request(Request<P>),
    Ack {
        #[serde(rename = "requestId")]
        request_id: RequestId,
    },
    Interrupt {
        #[serde(rename = "requestId")]
        request_id: RequestId,
    },
    Ping,
    Eof,
    #[serde(untagged)]
    Unknown(Value),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum Cause<E = Value> {
    Fail {
        error: E,
    },
    Die {
        defect: Value,
    },
    Interrupt {
        #[serde(rename = "fiberId", default, deserialize_with = "present_fiber_id")]
        fiber_id: Option<FiberId>,
    },
    #[serde(untagged)]
    Unknown(Value),
}

fn present_fiber_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<FiberId>, D::Error> {
    FiberId::deserialize(d).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum Exit<A = Value, E = Value> {
    Success { value: Option<A> },
    Failure { cause: Vec<Cause<E>> },
}

impl<A, E> Exit<A, E> {
    pub fn success(value: A) -> Self {
        Self::Success { value: Some(value) }
    }

    pub fn void() -> Self {
        Self::Success { value: None }
    }

    pub fn fail(error: E) -> Self {
        Self::Failure {
            cause: vec![Cause::Fail { error }],
        }
    }

    pub fn die(defect: impl Into<Value>) -> Self {
        Self::Failure {
            cause: vec![Cause::Die {
                defect: defect.into(),
            }],
        }
    }

    pub fn interrupt(fiber_id: Option<i64>) -> Self {
        Self::Failure {
            cause: vec![Cause::Interrupt {
                fiber_id: fiber_id.map(|id| FiberId::Number(id.into())),
            }],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ServerMessage<A = Value, E = Value, P = Value> {
    Chunk {
        #[serde(rename = "requestId")]
        request_id: RequestId,
        values: Vec<A>,
    },
    Exit {
        #[serde(rename = "requestId")]
        request_id: RequestId,
        exit: Exit<A, E>,
    },
    Defect {
        defect: Value,
    },
    Pong,
    Request(Request<P>),
    // Effect synthesizes this locally on transport errors; it is not a socket frame.
    ClientProtocolError {
        error: Value,
        #[serde(
            rename = "requestIds",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        request_ids: Option<Vec<RequestId>>,
    },
    #[serde(untagged)]
    Unknown(Value),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("invalid RPC JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("RPC JSON frame is not UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

/// JSON serialization accepts a single envelope or an array of envelopes per frame.
pub fn decode<T: DeserializeOwned>(frame: &Frame) -> Result<Vec<T>, CodecError> {
    let text = match frame {
        Frame::Text(text) => std::borrow::Cow::Borrowed(text.as_str()),
        Frame::Binary(bytes) => String::from_utf8_lossy(bytes),
    };
    let value: Value = serde_json::from_str(&text)?;
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>(),
        value => serde_json::from_value(value).map(|value| vec![value]),
    }
    .map_err(Into::into)
}

pub fn encode<T: Serialize>(message: &T) -> Result<Frame, CodecError> {
    Ok(Frame::Text(serde_json::to_string(message)?))
}
