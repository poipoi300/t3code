use crate::{Error, Result, RpcError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
    Number(serde_json::Number),
}
impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(Self::String(s)),
            Value::Number(n) => {
                let f = n
                    .as_f64()
                    .ok_or_else(|| serde::de::Error::custom("invalid numeric id"))?;
                if f.fract() == 0.0 && (-9223372036854775808.0..9223372036854775808.0).contains(&f)
                {
                    Ok(Self::Integer(f as i64))
                } else {
                    serde_json::Number::from_f64(f)
                        .map(Self::Number)
                        .ok_or_else(|| serde::de::Error::custom("invalid numeric id"))
                }
            }
            _ => Err(serde::de::Error::custom("invalid request id")),
        }
    }
}
impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (Self::String(a), Self::String(b)) if a == b)
            || (!matches!(self, Self::String(_))
                && !matches!(other, Self::String(_))
                && self.to_string() == other.to_string())
    }
}
impl Eq for RequestId {}
impl std::hash::Hash for RequestId {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        matches!(self, Self::String(_)).hash(h);
        self.to_string().hash(h);
    }
}
impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(id) => id.fmt(f),
            Self::Integer(id) => f.write_str(ryu_js::Buffer::new().format(*id as f64)),
            Self::Number(id) => {
                f.write_str(ryu_js::Buffer::new().format(id.as_f64().unwrap_or_default()))
            }
        }
    }
}

/// Keeps absent parameters distinct from explicitly null parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Params {
    #[default]
    Missing,
    Present(Value),
}

impl Params {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
    pub fn decode<T: serde::de::DeserializeOwned>(&self, method: &str) -> Result<T> {
        crate::error::decode_payload(
            method,
            match self {
                Self::Missing => &Value::Null,
                Self::Present(value) => value,
            },
        )
    }
}

impl Serialize for Params {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Missing => s.serialize_unit(),
            Self::Present(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for Params {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Value::deserialize(d).map(Self::Present)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Params::is_missing")]
    pub params: Params,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Params,
    },
    Notification(Notification),
    Response {
        id: RequestId,
        result: std::result::Result<Value, RpcError>,
    },
}

impl Message {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_str(&String::from_utf8_lossy(bytes))
            .map_err(|_| Error::Protocol("invalid JSON"))?;
        let Value::Object(mut fields) = value else {
            return Err(Error::Protocol("expected an object"));
        };
        let id = fields
            .remove("id")
            .map(serde_json::from_value::<RequestId>)
            .transpose()
            .map_err(|_| Error::Protocol("invalid request id"))?;
        let params = fields
            .remove("params")
            .map(Params::Present)
            .unwrap_or_default();
        if let Some(method) = fields.remove("method") {
            let Value::String(method) = method else {
                return Err(Error::Protocol("invalid method"));
            };
            return Ok(match id {
                Some(id) => Self::Request { id, method, params },
                None => Self::Notification(Notification { method, params }),
            });
        }
        let Some(id) = id else {
            return Err(Error::Protocol("missing response id"));
        };
        // The TS binding gives errors priority and treats a missing result as null.
        let result =
            match fields.remove("error") {
                Some(error) => Err(serde_json::from_value(error)
                    .map_err(|_| Error::Protocol("invalid RPC error"))?),
                None => Ok(fields.remove("result").unwrap_or(Value::Null)),
            };
        Ok(Self::Response { id, result })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut fields = Map::new();
        match self {
            Self::Request { id, method, params } => {
                fields.insert(
                    "id".into(),
                    serde_json::to_value(id).map_err(|e| Error::payload(method, e))?,
                );
                fields.insert("method".into(), Value::String(method.clone()));
                if let Params::Present(value) = params {
                    fields.insert("params".into(), value.clone());
                }
            }
            Self::Notification(n) => {
                fields.insert("method".into(), Value::String(n.method.clone()));
                if let Params::Present(value) = &n.params {
                    fields.insert("params".into(), value.clone());
                }
            }
            Self::Response { id, result } => {
                fields.insert(
                    "id".into(),
                    serde_json::to_value(id).map_err(|e| Error::payload("response", e))?,
                );
                match result {
                    Ok(v) => {
                        fields.insert("result".into(), v.clone());
                    }
                    Err(e) => {
                        fields.insert(
                            "error".into(),
                            serde_json::to_value(e).map_err(|e| Error::payload("response", e))?,
                        );
                    }
                }
            }
        }
        let mut bytes =
            serde_json::to_vec(&fields).map_err(|_| Error::Protocol("cannot encode JSON"))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
