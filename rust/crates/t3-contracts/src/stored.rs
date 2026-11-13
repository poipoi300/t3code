//! The stored JSON dialect.
//!
//! TypeScript persists events, projections, settings and a few records with the
//! plain schema codec (`Schema.decodeUnknown*` / `Schema.fromJsonString`, see
//! `PORTING.md`). There a `Schema.optional(T)` key that is `undefined` is
//! *absent*; an explicit `null` is rejected. The WebSocket RPC and HTTP API use
//! `Schema.toCodecJson`, which writes the same `undefined` as `null`.
//!
//! A Rust `Option<Option<T>>` field (`wire::double_option`) holds that undefined
//! as `Some(None)`. Plain `serde_json` writes it as `null` (the RPC dialect);
//! the functions here drop it instead (the stored dialect). Fields whose
//! TypeScript type admits `null` itself use `wire::nullable_optional` and keep
//! their `null` in both dialects.

use serde::ser::{self, Impossible, Serialize, Serializer};
use serde_json::Value;

/// `double_option` encodes an undefined optional value as this unit struct.
/// `serde_json` writes any unit struct as `null`, so the RPC dialect is unchanged.
pub(crate) const UNDEFINED: &str = "$t3_contracts::undefined";

/// Encodes `value` as the TypeScript server stores it.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut out = Vec::with_capacity(128);
    to_writer(&mut out, value)?;
    String::from_utf8(out).map_err(ser::Error::custom)
}

pub fn to_writer<W: std::io::Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    let mut serializer = serde_json::Serializer::new(writer);
    value.serialize(Stored(&mut serializer))
}

pub fn to_value<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Value> {
    value.serialize(Stored(serde_json::value::Serializer))
}

/// Wraps any serializer so struct fields holding [`UNDEFINED`] are skipped.
pub struct Stored<S>(pub S);

struct Nested<'a, T: ?Sized>(&'a T);
impl<T: Serialize + ?Sized> Serialize for Nested<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Stored(serializer))
    }
}

fn is_undefined<T: Serialize + ?Sized>(value: &T) -> bool {
    value.serialize(UndefinedProbe).unwrap_or(false)
}

impl<S: Serializer> Serializer for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Stored<S::SerializeSeq>;
    type SerializeTuple = Stored<S::SerializeTuple>;
    type SerializeTupleStruct = Stored<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Stored<S::SerializeTupleVariant>;
    type SerializeMap = Stored<S::SerializeMap>;
    type SerializeStruct = Stored<S::SerializeStruct>;
    type SerializeStructVariant = Stored<S::SerializeStructVariant>;

    fn serialize_bool(self, v: bool) -> Result<S::Ok, S::Error> {
        self.0.serialize_bool(v)
    }
    fn serialize_i8(self, v: i8) -> Result<S::Ok, S::Error> {
        self.0.serialize_i8(v)
    }
    fn serialize_i16(self, v: i16) -> Result<S::Ok, S::Error> {
        self.0.serialize_i16(v)
    }
    fn serialize_i32(self, v: i32) -> Result<S::Ok, S::Error> {
        self.0.serialize_i32(v)
    }
    fn serialize_i64(self, v: i64) -> Result<S::Ok, S::Error> {
        self.0.serialize_i64(v)
    }
    fn serialize_i128(self, v: i128) -> Result<S::Ok, S::Error> {
        self.0.serialize_i128(v)
    }
    fn serialize_u8(self, v: u8) -> Result<S::Ok, S::Error> {
        self.0.serialize_u8(v)
    }
    fn serialize_u16(self, v: u16) -> Result<S::Ok, S::Error> {
        self.0.serialize_u16(v)
    }
    fn serialize_u32(self, v: u32) -> Result<S::Ok, S::Error> {
        self.0.serialize_u32(v)
    }
    fn serialize_u64(self, v: u64) -> Result<S::Ok, S::Error> {
        self.0.serialize_u64(v)
    }
    fn serialize_u128(self, v: u128) -> Result<S::Ok, S::Error> {
        self.0.serialize_u128(v)
    }
    fn serialize_f32(self, v: f32) -> Result<S::Ok, S::Error> {
        self.0.serialize_f32(v)
    }
    fn serialize_f64(self, v: f64) -> Result<S::Ok, S::Error> {
        self.0.serialize_f64(v)
    }
    fn serialize_char(self, v: char) -> Result<S::Ok, S::Error> {
        self.0.serialize_char(v)
    }
    fn serialize_str(self, v: &str) -> Result<S::Ok, S::Error> {
        self.0.serialize_str(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<S::Ok, S::Error> {
        self.0.serialize_bytes(v)
    }
    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_none()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_some(&Nested(value))
    }
    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        // Outside a struct field (an array element, the top level) JSON has no
        // undefined; JSON.stringify writes null there too.
        if name == UNDEFINED {
            self.0.serialize_unit()
        } else {
            self.0.serialize_unit_struct(name)
        }
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_variant(name, index, variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_struct(name, &Nested(value))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0
            .serialize_newtype_variant(name, index, variant, &Nested(value))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.0.serialize_seq(len).map(Stored)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.0.serialize_tuple(len).map(Stored)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.0.serialize_tuple_struct(name, len).map(Stored)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.0
            .serialize_tuple_variant(name, index, variant, len)
            .map(Stored)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.0.serialize_map(len).map(Stored)
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        self.0.serialize_struct(name, len).map(Stored)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.0
            .serialize_struct_variant(name, index, variant, len)
            .map(Stored)
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

impl<S: ser::SerializeSeq> ser::SerializeSeq for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_element(&Nested(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeTuple> ser::SerializeTuple for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_element(&Nested(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeTupleStruct> ser::SerializeTupleStruct for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_field(&Nested(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeTupleVariant> ser::SerializeTupleVariant for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_field(&Nested(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeMap> ser::SerializeMap for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), S::Error> {
        self.0.serialize_key(&Nested(key))
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), S::Error> {
        self.0.serialize_value(&Nested(value))
    }
    // `#[serde(flatten)]` fields arrive here as entries.
    fn serialize_entry<K: Serialize + ?Sized, V: Serialize + ?Sized>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), S::Error> {
        if is_undefined(value) {
            return Ok(());
        }
        self.0.serialize_entry(&Nested(key), &Nested(value))
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeStruct> ser::SerializeStruct for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), S::Error> {
        if is_undefined(value) {
            return self.0.skip_field(key);
        }
        self.0.serialize_field(key, &Nested(value))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

impl<S: ser::SerializeStructVariant> ser::SerializeStructVariant for Stored<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), S::Error> {
        if is_undefined(value) {
            return self.0.skip_field(key);
        }
        self.0.serialize_field(key, &Nested(value))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

/// Answers "is this value the undefined marker?" without serializing anything
/// else: every compound value stops at its first call.
struct UndefinedProbe;

#[derive(Debug)]
struct NotUndefined;
impl std::fmt::Display for NotUndefined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not the undefined marker")
    }
}
impl std::error::Error for NotUndefined {}
impl ser::Error for NotUndefined {
    fn custom<T: std::fmt::Display>(_msg: T) -> Self {
        Self
    }
}

macro_rules! not_undefined {
    ($($method:ident($($ty:ty),*);)*) => {$(
        fn $method(self, $(_: $ty),*) -> Result<bool, NotUndefined> { Ok(false) }
    )*};
}

impl Serializer for UndefinedProbe {
    type Ok = bool;
    type Error = NotUndefined;
    type SerializeSeq = Impossible<bool, NotUndefined>;
    type SerializeTuple = Impossible<bool, NotUndefined>;
    type SerializeTupleStruct = Impossible<bool, NotUndefined>;
    type SerializeTupleVariant = Impossible<bool, NotUndefined>;
    type SerializeMap = Impossible<bool, NotUndefined>;
    type SerializeStruct = Impossible<bool, NotUndefined>;
    type SerializeStructVariant = Impossible<bool, NotUndefined>;

    not_undefined! {
        serialize_bool(bool); serialize_i8(i8); serialize_i16(i16); serialize_i32(i32);
        serialize_i64(i64); serialize_i128(i128); serialize_u8(u8); serialize_u16(u16);
        serialize_u32(u32); serialize_u64(u64); serialize_u128(u128); serialize_f32(f32);
        serialize_f64(f64); serialize_char(char); serialize_str(&str); serialize_bytes(&[u8]);
        serialize_none(); serialize_unit();
        serialize_unit_variant(&'static str, u32, &'static str);
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<bool, NotUndefined> {
        Ok(name == UNDEFINED)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<bool, NotUndefined> {
        Ok(false)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<bool, NotUndefined> {
        Ok(false)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<bool, NotUndefined> {
        Ok(false)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, NotUndefined> {
        Err(NotUndefined)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NotUndefined> {
        Err(NotUndefined)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Inner {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::double_option"
        )]
        run_id: Option<Option<String>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wire::nullable_optional"
        )]
        pinned_at: Option<Option<String>>,
        #[serde(with = "crate::wire::nullable")]
        required: Option<String>,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "type")]
    enum Event {
        #[serde(rename = "a")]
        A {
            #[serde(flatten)]
            inner: Inner,
            items: Vec<Option<Option<u8>>>,
        },
        #[serde(rename = "b")]
        B(Inner),
    }

    #[test]
    fn undefined_optionals_are_absent_only_in_the_stored_dialect() -> serde_json::Result<()> {
        let inner = || Inner {
            run_id: Some(None),
            pinned_at: Some(None),
            required: None,
        };
        let a = Event::A {
            inner: inner(),
            items: vec![Some(None)],
        };
        assert_eq!(
            serde_json::to_value(&a)?,
            json!({"type": "a", "runId": null, "pinnedAt": null, "required": null, "items": [null]})
        );
        let stored = json!({"type": "a", "pinnedAt": null, "required": null, "items": [null]});
        assert_eq!(super::to_value(&a)?, stored);
        assert_eq!(super::to_string(&a)?, serde_json::to_string(&stored)?);
        assert_eq!(
            super::to_value(&Event::B(inner()))?,
            json!({"type": "b", "pinnedAt": null, "required": null})
        );
        let present = Inner {
            run_id: Some(Some("run".into())),
            pinned_at: None,
            required: Some("x".into()),
        };
        assert_eq!(
            super::to_value(&present)?,
            json!({"runId": "run", "required": "x"})
        );
        Ok(())
    }
}
