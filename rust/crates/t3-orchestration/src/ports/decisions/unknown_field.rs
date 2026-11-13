use serde_json::Value;

pub fn field(source: &Value, key: &str) -> Option<Value> {
    match source {
        Value::Object(fields) => fields.get(key).cloned(),
        Value::Array(values) if key == "length" => Some(Value::from(values.len())),
        Value::Array(values) => key
            .parse::<usize>()
            .ok()
            .filter(|index| index.to_string() == key)
            .and_then(|index| values.get(index))
            .cloned(),
        _ => None,
    }
}
pub fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(t3_contracts::wire::trim_js)
        .filter(|text| !text.is_empty())
}
