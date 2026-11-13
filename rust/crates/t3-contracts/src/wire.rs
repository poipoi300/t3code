//! Shared wire representations. Validation remains the owning contract's responsibility.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;

/// The spelling is significant, including extended years and non-date strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireDateTime(pub String);

/// A finite JavaScript number. It holds the IEEE double JavaScript holds and
/// encodes it as `JSON.stringify` would, so TypeScript-written text survives a
/// Rust round trip without serde_json's `arbitrary_precision` feature.
#[derive(Clone)]
pub struct JsNumber(f64);

impl JsNumber {
    pub fn from_f64(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }
    /// Always `Some`; kept `Option` so `serde_json::Number` call sites keep compiling.
    pub fn as_f64(&self) -> Option<f64> {
        Some(self.0)
    }
    pub fn value(&self) -> f64 {
        self.0
    }
    pub fn as_i64(&self) -> Option<i64> {
        self.is_safe_integer().then_some(self.0 as i64)
    }
    pub fn as_u64(&self) -> Option<u64> {
        (self.is_safe_integer() && self.0 >= 0.0).then_some(self.0 as u64)
    }
    pub fn is_i64(&self) -> bool {
        self.as_i64().is_some()
    }
    pub fn is_u64(&self) -> bool {
        self.as_u64().is_some()
    }
    pub fn is_f64(&self) -> bool {
        !self.is_i64()
    }
    fn is_safe_integer(&self) -> bool {
        self.0.fract() == 0.0 && self.0.abs() <= MAX_SAFE_INTEGER
    }
}

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

impl PartialEq for JsNumber {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
// Finite by construction, so `==` is a total equivalence.
impl Eq for JsNumber {}
impl std::hash::Hash for JsNumber {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // 0.0 == -0.0, so they must hash alike.
        (self.0 + 0.0).to_bits().hash(state);
    }
}
impl std::fmt::Debug for JsNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_js_number(self.0))
    }
}
impl std::fmt::Display for JsNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_js_number(self.0))
    }
}

impl TryFrom<f64> for JsNumber {
    type Error = NonFiniteNumber;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::from_f64(value).ok_or(NonFiniteNumber)
    }
}
impl TryFrom<serde_json::Number> for JsNumber {
    type Error = NonFiniteNumber;
    fn try_from(value: serde_json::Number) -> Result<Self, Self::Error> {
        value
            .as_f64()
            .and_then(Self::from_f64)
            .ok_or(NonFiniteNumber)
    }
}
impl From<JsNumber> for f64 {
    fn from(value: JsNumber) -> Self {
        value.0
    }
}
macro_rules! number_from {
    ($($t:ty),*) => {$(impl From<$t> for JsNumber {
        // JavaScript rounds integers beyond 2^53 to the nearest double too.
        #[allow(clippy::cast_precision_loss, clippy::cast_lossless)]
        fn from(value: $t) -> Self { Self(value as f64) }
    })*};
}
number_from!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonFiniteNumber;
impl std::fmt::Display for NonFiniteNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("expected a finite JavaScript number")
    }
}
impl std::error::Error for NonFiniteNumber {}

impl Serialize for JsNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = self.0;
        if self.is_safe_integer() {
            return serializer.serialize_i64(value as i64);
        }
        // serde_json's float text equals JavaScript's except for exponents of
        // 21 and above (`1e21` vs `1e+21`), [1e-6, 1e-5) (`1e-6` vs
        // `0.000001`) and integers past 2^53 (`1.2e17` vs `120000000000000000`).
        let magnitude = value.abs();
        if value.fract() != 0.0 && (magnitude >= 1e-5 || magnitude < 1e-6) {
            return serializer.serialize_f64(value);
        }
        let raw = serde_json::value::RawValue::from_string(format_js_number(value))
            .map_err(serde::ser::Error::custom)?;
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Through `Number` so the decode is the same whether or not another
        // crate turns on `arbitrary_precision`.
        let number = serde_json::Number::deserialize(deserializer)?;
        Self::try_from(number).map_err(serde::de::Error::custom)
    }
}

/// ECMAScript `Number::toString(x)` for finite `x`, which `JSON.stringify` uses.
pub fn format_js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    // Rust's `{:e}` prints the shortest round-tripping digits, as ECMAScript requires.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let k = digits.len() as i32;
    let n = exponent + 1;
    let mut out = String::with_capacity(k as usize + 8);
    if value < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n - 1 < 0 { '-' } else { '+' });
        out.push_str(&(n - 1).abs().to_string());
    }
    out
}

pub type JsInteger = i64;

/// Brands are erased by the wire codec but remain distinct Rust types.
#[macro_export]
macro_rules! string_id {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Validates like the TypeScript decoder; `From<String>` stays unchecked.
            pub fn checked(value: impl AsRef<str>) -> Result<Self, String> {
                $crate::wire::normalize_id(stringify!($name), value.as_ref()).map(Self)
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::checked(value).map_err(serde::de::Error::custom)
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self { Self(value) }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self { Self(value.to_owned()) }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { &self.0 }
        }
    )+};
}

/// `optionalKey(T)`: absence is allowed; explicit null must still decode as T.
pub mod optional {
    use super::*;

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        T::deserialize(deserializer).map(Some)
    }

    pub fn serialize<S, T>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        value.serialize(serializer)
    }
}

/// `NullOr(T)`: use `with` without `default` so a missing required key fails.
pub mod nullable {
    use super::*;

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer)
    }

    pub use super::optional::serialize;
}

/// `optional(T)`: None=absent, Some(None)=undefined (null on the RPC wire, absent when stored).
pub mod double_option {
    use super::*;

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }

    /// `Some(None)` is TypeScript's `undefined`: `null` on the RPC wire, an
    /// absent key in the stored dialect (`crate::stored`).
    pub fn serialize<S, T>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        match value {
            Some(Some(value)) => serializer.serialize_some(value),
            Some(None) => serializer.serialize_unit_struct(crate::stored::UNDEFINED),
            None => serializer.serialize_none(),
        }
    }
}

/// `optional(NullOr(T))` / `optionalKey(NullOr(T))`: None=absent, Some(None)=null
/// in both dialects, because `null` is a value TypeScript stores.
pub mod nullable_optional {
    pub use super::double_option::deserialize;
    pub use super::optional::serialize;
}

/// `ForwardCompatibleOptional` / `ForwardCompatibleNullable`: discard invalid values.
pub mod lenient {
    use super::*;

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: DeserializeOwned,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(serde_json::from_value(value).ok())
    }

    pub use super::optional::serialize;
}

/// `ForwardCompatibleArray` drops invalid entries; tagged unions use the stricter helper below.
pub fn deserialize_lenient_array<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(Vec::<Value>::deserialize(deserializer)?
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

pub trait TaggedUnion: DeserializeOwned + Serialize {
    const TAG: &'static str;
    const KNOWN_TAGS: &'static [&'static str];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownUnionMember {
    pub tag: String,
    pub value: String,
}

/// Unknown tags decode, malformed known tags fail, and unknown members cannot encode.
#[derive(Debug, Clone, PartialEq)]
pub enum ForwardCompatible<K> {
    Known(K),
    Unknown(UnknownUnionMember),
}

impl<'de, K: TaggedUnion> Deserialize<'de> for ForwardCompatible<K> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let tag = value
            .as_object()
            .and_then(|object| object.get(K::TAG))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                serde::de::Error::custom("expected an object with a string union tag")
            })?;
        if K::KNOWN_TAGS.contains(&tag) {
            serde_json::from_value(value)
                .map(Self::Known)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(Self::Unknown(UnknownUnionMember {
                tag: K::TAG.to_owned(),
                value: tag.to_owned(),
            }))
        }
    }
}

impl<K: TaggedUnion> Serialize for ForwardCompatible<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(known) => known.serialize(serializer),
            Self::Unknown(unknown) => Err(serde::ser::Error::custom(format!(
                "Unknown {} values are never sent: {}",
                unknown.tag, unknown.value
            ))),
        }
    }
}

pub fn deserialize_union_array<'de, D, K>(deserializer: D) -> Result<Vec<K>, D::Error>
where
    D: Deserializer<'de>,
    K: TaggedUnion,
{
    Ok(Vec::<ForwardCompatible<K>>::deserialize(deserializer)?
        .into_iter()
        .filter_map(|member| match member {
            ForwardCompatible::Known(known) => Some(known),
            ForwardCompatible::Unknown(_) => None,
        })
        .collect())
}

/// ECMAScript WhiteSpace + LineTerminator; Rust's Unicode whitespace differs.
pub fn trim_js(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
            | '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}

/// Constructors stay permissive for existing callers; decoders enforce the source refinements.
/// Applies the TypeScript refinement behind each branded id: entity ids are
/// `TrimmedNonEmptyString`; slugs, context ids and usage days add a pattern;
/// `UsageLimitSourceId` is a plain string. Returns the decoded (trimmed) text.
pub fn normalize_id(kind: &str, raw: &str) -> Result<String, String> {
    if kind == "UsageLimitSourceId" {
        return Ok(raw.to_owned());
    }
    let value = trim_js(raw);
    if value.is_empty() {
        return Err(format!("expected a non-blank {kind}"));
    }
    let bytes = value.as_bytes();
    let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-';
    let valid = match kind {
        // providerInstance.ts: /^[a-zA-Z][a-zA-Z0-9_-]*$/, at most 64
        "ProviderDriverKind" | "ProviderInstanceId" => {
            bytes.len() <= 64 && bytes[0].is_ascii_alphabetic() && bytes.iter().all(word)
        }
        // secretReference.ts: /^[a-zA-Z0-9][a-zA-Z0-9_-]*$/, at most 64
        "SecretProviderId" => {
            bytes.len() <= 64 && bytes[0].is_ascii_alphanumeric() && bytes.iter().all(word)
        }
        // sourceControl.ts: /^[a-z][a-z0-9-]*$/, at most 64
        "SourceControlProviderKind" => {
            bytes.len() <= 64
                && bytes[0].is_ascii_lowercase()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        }
        // composerContext.ts: /^[a-z0-9_-]+$/i, at most 128
        "ComposerContextId" | "ComposerContextReferenceId" => {
            bytes.len() <= 128 && bytes.iter().all(word)
        }
        // usage.ts: /^\d{4}-\d{2}-\d{2}$/
        "UsageDay" => {
            bytes.len() == 10
                && bytes.iter().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        *b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
        }
        _ => true,
    };
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!("invalid {kind}: {value:?}"))
    }
}

/// `Debug` for a struct holding tokens, codes, proofs or credentials: the
/// fields under `redact` print as `[REDACTED]`. A field missing from both lists
/// is left out, so a newly added field never leaks by default.
#[macro_export]
macro_rules! redacted_debug {
    ($name:ident { $($field:ident),* $(,)? } redact { $($secret:ident),+ $(,)? }) => {
        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!($name))
                    $(.field(stringify!($field), &self.$field))*
                    $(.field(stringify!($secret), &$crate::wire::Redacted::of(&self.$secret)))+
                    .finish()
            }
        }
    };
}

/// Prints `[REDACTED]` in place of a secret value.
pub struct Redacted;
impl Redacted {
    pub fn of<T: ?Sized>(_secret: &T) -> Self {
        Self
    }
}
impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

pub fn default_on_null<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    crate::string_id!(TestId);

    #[test]
    fn branded_strings_erase_to_strings() -> Result<(), serde_json::Error> {
        let id: TestId = serde_json::from_value(json!("id:😀"))?;
        assert_eq!(id.as_ref(), "id:😀");
        assert_eq!(serde_json::to_value(TestId::from(id.0))?, json!("id:😀"));
        assert!(serde_json::from_value::<TestId>(json!(1)).is_err());
        Ok(())
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Presence {
        #[serde(default, skip_serializing_if = "Option::is_none", with = "optional")]
        optional: Option<String>,
        #[serde(with = "nullable")]
        nullable: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "double_option"
        )]
        both: Option<Option<String>>,
    }

    #[test]
    fn distinguishes_absence_null_and_value() -> Result<(), serde_json::Error> {
        for value in [
            json!({"nullable": null}),
            json!({"optional": "", "nullable": "", "both": null}),
            json!({"nullable": null, "both": ""}),
        ] {
            let decoded: Presence = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(decoded)?, value);
        }
        assert!(serde_json::from_value::<Presence>(json!({})).is_err());
        assert!(
            serde_json::from_value::<Presence>(json!({"nullable": null, "optional": null}))
                .is_err()
        );
        let decoded: Presence = serde_json::from_value(json!({"nullable": null, "both": null}))?;
        assert_eq!(decoded.both, Some(None));
        Ok(())
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "type")]
    enum Known {
        #[serde(rename = "message")]
        Message { text: String },
    }

    impl TaggedUnion for Known {
        const TAG: &'static str = "type";
        const KNOWN_TAGS: &'static [&'static str] = &["message"];
    }

    #[test]
    fn unknown_tags_are_decode_only_and_known_tags_still_validate() -> Result<(), serde_json::Error>
    {
        let unknown: ForwardCompatible<Known> =
            serde_json::from_value(json!({"type": "future", "payload": 1}))?;
        assert!(matches!(&unknown, ForwardCompatible::Unknown(member) if member.value == "future"));
        assert!(serde_json::to_value(unknown).is_err());
        for value in [json!({"type": "message"}), json!({"type": 1}), json!(null)] {
            assert!(serde_json::from_value::<ForwardCompatible<Known>>(value).is_err());
        }
        let known = json!({"type": "message", "text": "hello"});
        let decoded: ForwardCompatible<Known> = serde_json::from_value(known.clone())?;
        assert_eq!(serde_json::to_value(decoded)?, known);
        #[derive(Deserialize)]
        struct Array {
            #[serde(deserialize_with = "deserialize_union_array")]
            items: Vec<Known>,
        }
        let array: Array = serde_json::from_value(json!({"items": [known, {"type": "future"}]}))?;
        assert_eq!(array.items.len(), 1);
        assert!(serde_json::from_value::<Array>(json!({"items": [{"type": "message"}]})).is_err());
        Ok(())
    }

    #[test]
    fn lenient_fields_and_arrays_drop_invalid_values() -> Result<(), serde_json::Error> {
        #[derive(Serialize, Deserialize)]
        struct Fields {
            #[serde(default, skip_serializing_if = "Option::is_none", with = "lenient")]
            optional: Option<String>,
            #[serde(with = "lenient")]
            nullable: Option<String>,
            #[serde(deserialize_with = "deserialize_lenient_array")]
            array: Vec<String>,
        }
        let fields: Fields = serde_json::from_value(
            json!({"optional": 1, "nullable": false, "array": [1, "ok", null]}),
        )?;
        assert_eq!(
            serde_json::to_value(fields)?,
            json!({"nullable": null, "array": ["ok"]})
        );
        Ok(())
    }

    /// Fails when another crate unifies serde_json's `arbitrary_precision` into
    /// the build: serde then buffers non-integer numbers as a private map, and
    /// every `f64` inside an internally tagged or untagged enum stops decoding.
    #[test]
    fn buffered_unions_decode_fractional_numbers() -> Result<(), serde_json::Error> {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "type")]
        enum Tagged {
            A { x: f64 },
        }
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(untagged)]
        enum Untagged {
            A { x: f64 },
        }
        assert_eq!(
            serde_json::from_str::<Tagged>(r#"{"type":"A","x":1.5}"#)?,
            Tagged::A { x: 1.5 }
        );
        assert_eq!(
            serde_json::from_str::<Untagged>(r#"{"x":1.5}"#)?,
            Untagged::A { x: 1.5 }
        );
        Ok(())
    }

    #[test]
    fn dates_numbers_and_js_whitespace_keep_their_wire_meaning() -> Result<(), serde_json::Error> {
        for date in [
            "-251887-05-21T14:26:43.215Z",
            "+257176-08-18T05:50:06.455Z",
            "",
        ] {
            let value = json!(date);
            let decoded: WireDateTime = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(decoded)?, value);
        }
        for raw in [
            "9007199254740991",
            "-9007199254740991",
            "1.7976931348623157e308",
            "-8e-323",
        ] {
            let value: Value = serde_json::from_str(raw)?;
            let decoded: JsNumber = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(decoded)?, value);
        }
        // (JSON input text, what JSON.stringify(JSON.parse(input)) gives in JavaScript)
        for (input, javascript) in [
            ("-0.0", "0"),
            ("1.0", "1"),
            ("1e-7", "1e-7"),
            ("1e-6", "0.000001"),
            ("1.5e-6", "0.0000015"),
            ("0.00001", "0.00001"),
            ("1e20", "100000000000000000000"),
            ("1e21", "1e+21"),
            ("1.5e300", "1.5e+300"),
            ("123456789012345680000", "123456789012345680000"),
            ("1152921504606846976", "1152921504606847000"),
            ("9007199254740993", "9007199254740992"),
            ("1.0000000000000001e18", "1000000000000000100"),
            ("5.108443427076897e-154", "5.108443427076897e-154"),
            ("0.1", "0.1"),
            ("-2.5", "-2.5"),
            ("1.7976931348623157e308", "1.7976931348623157e+308"),
            ("5e-324", "5e-324"),
        ] {
            let number: JsNumber = serde_json::from_str(input)?;
            assert_eq!(serde_json::to_string(&number)?, javascript, "{input}");
            assert_eq!(number.to_string(), javascript, "{input}");
            let through_value: JsNumber = serde_json::from_value(serde_json::to_value(&number)?)?;
            assert_eq!(through_value, number, "{input}");
        }
        for rejected in ["1e999", "-1e400", "\"1\"", "null"] {
            assert!(
                serde_json::from_str::<JsNumber>(rejected).is_err(),
                "{rejected}"
            );
        }
        assert_eq!(trim_js("\u{feff}hello\u{00a0}"), "hello");
        assert_eq!(trim_js("\u{0085}hello\u{0085}"), "\u{0085}hello\u{0085}");
        Ok(())
    }
}
