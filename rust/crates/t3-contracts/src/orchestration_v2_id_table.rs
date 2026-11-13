//! The `compactIds=1` record traversal from `orchestrationV2IdTable.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub enum RecordContainer<'a> {
    TopLevel,
    Key(&'a str),
    NoRecords,
}

/// Wire structs retain compact indices; expand the envelope before reading typed IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IdOrIndex<Id> {
    Id(Id),
    Index(u64),
}

// Value's sorted maps lose declaration order, which determines ID table indices.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum OrderedJson {
    Object(#[serde(with = "ordered_map")] Vec<(String, OrderedJson)>),
    Array(Vec<OrderedJson>),
    Scalar(Value),
}

mod ordered_map {
    use super::OrderedJson;
    use serde::{
        Deserializer, Serializer,
        de::{MapAccess, Visitor},
        ser::SerializeMap,
    };
    use std::fmt;

    pub fn serialize<S: Serializer>(
        fields: &[(String, OrderedJson)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(fields.len()))?;
        for (key, value) in fields {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(String, OrderedJson)>, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Vec<(String, OrderedJson)>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    fields.push(entry);
                }
                // ECMAScript enumerates array-index property names before other string keys.
                fields.sort_by_key(|(key, _): &(String, OrderedJson)| {
                    key.parse::<u32>()
                        .ok()
                        .filter(|index| *index != u32::MAX && index.to_string() == *key)
                        .map_or((1, 0), |index| (0, index))
                });
                Ok(fields)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum IdKey {
    String(String),
    Number(u64),
}

#[derive(Default)]
struct Intern {
    ids: Vec<Value>,
    index: HashMap<IdKey, usize>,
}

impl Intern {
    fn reference(&mut self, value: &mut OrderedJson) {
        let OrderedJson::Scalar(raw) = value else {
            return;
        };
        let key = match raw {
            Value::String(value) => IdKey::String(value.clone()),
            Value::Number(value) => {
                let Some(number) = value.as_f64() else {
                    return;
                };
                IdKey::Number(if number == 0.0 { 0 } else { number.to_bits() })
            }
            _ => return,
        };
        let position = *self.index.entry(key).or_insert_with(|| {
            let position = self.ids.len();
            self.ids.push(raw.clone());
            position
        });
        *raw = Value::from(position);
    }

    fn record(&mut self, record: &mut OrderedJson) {
        let OrderedJson::Object(fields) = record else {
            return;
        };
        for (key, value) in fields {
            if is_id_key(key) {
                self.reference(value);
            } else if key.ends_with("Ids") {
                if let OrderedJson::Array(values) = value {
                    for value in values {
                        self.reference(value);
                    }
                }
            } else if key == "item" {
                self.record(value);
            }
        }
    }

    fn container(&mut self, container: &mut OrderedJson) {
        let OrderedJson::Object(fields) = container else {
            return;
        };
        for (_, value) in fields {
            match value {
                OrderedJson::Array(records) => {
                    for record in records {
                        self.record(record);
                    }
                }
                OrderedJson::Object(_) => self.record(value),
                _ => {}
            }
        }
    }
}

fn is_id_key(key: &str) -> bool {
    key == "id" || key.ends_with("Id")
}

/// Pass the typed encoded envelope so serde's field order matches the TS declaration order.
/// No marker means plain JSON. `NoRecords` removes the marker without adding a table.
pub fn encode_record_id_table<T: Serialize>(
    encoded: &T,
    container: RecordContainer<'_>,
) -> Result<Value, serde_json::Error> {
    let mut ordered: OrderedJson = serde_json::from_str(&serde_json::to_string(encoded)?)?;
    let OrderedJson::Object(fields) = &mut ordered else {
        return serde_json::to_value(ordered);
    };
    if !fields.iter().any(|(key, value)| {
        key == "idTable" && matches!(value, OrderedJson::Scalar(Value::Bool(true)))
    }) {
        return serde_json::to_value(ordered);
    }
    fields.retain(|(key, _)| key != "idTable");
    let target = match container {
        RecordContainer::TopLevel => Some(&mut ordered),
        RecordContainer::Key(key) => fields
            .iter_mut()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value),
        RecordContainer::NoRecords => None,
    };
    let mut intern = Intern::default();
    let Some(target @ OrderedJson::Object(_)) = target else {
        return serde_json::to_value(ordered);
    };
    intern.container(target);
    let mut wire = serde_json::to_value(ordered)?;
    if let Value::Object(object) = &mut wire {
        object.insert("ids".to_owned(), Value::Array(intern.ids));
    }
    Ok(wire)
}

fn expand_reference(value: &mut Value, ids: &[Value], key: &str) -> Result<(), serde_json::Error> {
    let Value::Number(number) = value else {
        return Ok(());
    };
    let index = number
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .or_else(|| {
            let number = number.as_f64()?;
            (number >= 0.0 && number.fract() == 0.0 && number < ids.len() as f64)
                .then_some(number as usize)
        });
    let referenced = index.and_then(|index| ids.get(index)).ok_or_else(|| {
        <serde_json::Error as serde::de::Error>::custom(format!(
            "{key}: ID table index {number} is out of range"
        ))
    })?;
    *value = referenced.clone();
    Ok(())
}

fn expand_record(record: &mut Value, ids: &[Value]) -> Result<(), serde_json::Error> {
    let Value::Object(fields) = record else {
        return Ok(());
    };
    for (key, value) in fields {
        if is_id_key(key) {
            expand_reference(value, ids, key)?;
        } else if key.ends_with("Ids") {
            if let Value::Array(values) = value {
                for value in values {
                    expand_reference(value, ids, key)?;
                }
            }
        } else if key == "item" {
            expand_record(value, ids)?;
        }
    }
    Ok(())
}

/// Consumes `ids` once. Invalid references fail even in otherwise optional slots.
pub fn decode_record_id_table(
    raw: &mut Value,
    container: RecordContainer<'_>,
) -> Result<(), serde_json::Error> {
    let Some(object) = raw.as_object_mut() else {
        return Ok(());
    };
    if !object.get("ids").is_some_and(Value::is_array) {
        return Ok(());
    }
    let Some(Value::Array(ids)) = object.remove("ids") else {
        return Ok(());
    };
    let target = match container {
        RecordContainer::TopLevel => Some(raw),
        RecordContainer::Key(key) => object.get_mut(key),
        RecordContainer::NoRecords => None,
    };
    let Some(Value::Object(fields)) = target else {
        return Ok(());
    };
    for value in fields.values_mut() {
        match value {
            Value::Array(records) => {
                for record in records {
                    expand_record(record, &ids)?;
                }
            }
            Value::Object(_) => expand_record(value, &ids)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_wire_slots_round_trip_without_expanding() -> Result<(), serde_json::Error> {
        for value in [json!("thread:a"), json!(0)] {
            let slot: IdOrIndex<crate::base_schemas::ThreadId> =
                serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(slot)?, value);
        }
        Ok(())
    }

    #[test]
    fn table_order_and_traversal_match_typescript() -> Result<(), serde_json::Error> {
        // A raw JSON string deliberately supplies non-alphabetical field order.
        let encoded: OrderedJson = serde_json::from_str(
            r#"{"rows":[{"id":"row:a","count":7,"parentIds":["row:b"],"item":{"ownerId":"row:b","legacyId":3},"nested":{"id":"untouched"}},{"id":"row:b","count":3,"parentIds":[],"item":{"ownerId":null,"legacyId":0}}],"idTable":true}"#,
        )?;
        let mut wire = encode_record_id_table(&encoded, RecordContainer::TopLevel)?;
        assert_eq!(
            wire,
            json!({"rows": [
            {"id": 0, "count": 7, "parentIds": [1], "item": {"ownerId": 1, "legacyId": 2}, "nested": {"id": "untouched"}},
            {"id": 1, "count": 3, "parentIds": [], "item": {"ownerId": null, "legacyId": 3}}
        ], "ids": ["row:a", "row:b", 3, 0]})
        );
        decode_record_id_table(&mut wire, RecordContainer::TopLevel)?;
        let plain = wire.clone();
        decode_record_id_table(&mut wire, RecordContainer::TopLevel)?;
        assert_eq!(wire, plain);
        assert_eq!(
            encode_record_id_table(&plain, RecordContainer::TopLevel)?,
            plain
        );
        assert_eq!(plain["rows"][0]["item"]["legacyId"], 3);
        Ok(())
    }

    #[test]
    fn string_and_numeric_ids_are_distinct_but_equal_js_numbers_share_slots()
    -> Result<(), serde_json::Error> {
        let encoded = json!({"rows": [{"id": "3", "parentIds": [3, 3.0, -0.0, 0, null, true]}], "idTable": true});
        let wire = encode_record_id_table(&encoded, RecordContainer::TopLevel)?;
        assert_eq!(wire["ids"].as_array().map(Vec::len), Some(3));
        assert_eq!(
            wire["rows"][0]["parentIds"],
            json!([1, 1, 2, 2, null, true])
        );
        Ok(())
    }

    #[test]
    fn named_container_no_records_and_invalid_references() -> Result<(), serde_json::Error> {
        let encoded = json!({"payload": {"rows": [{"id": "x"}]}, "idTable": true});
        let mut wire = encode_record_id_table(&encoded, RecordContainer::Key("payload"))?;
        assert_eq!(
            wire,
            json!({"payload": {"rows": [{"id": 0}]}, "ids": ["x"]})
        );
        decode_record_id_table(&mut wire, RecordContainer::Key("payload"))?;
        assert_eq!(wire, json!({"payload": {"rows": [{"id": "x"}]}}));
        assert_eq!(
            encode_record_id_table(&encoded, RecordContainer::NoRecords)?,
            wire
        );
        for invalid in [json!(5), json!(-1), json!(0.5)] {
            let mut wire = json!({"rows": [{"mergedId": invalid}], "ids": ["x"]});
            let error = decode_record_id_table(&mut wire, RecordContainer::TopLevel)
                .expect_err("invalid optional reference must fail");
            assert!(error.to_string().contains("mergedId"));
        }
        Ok(())
    }
}
