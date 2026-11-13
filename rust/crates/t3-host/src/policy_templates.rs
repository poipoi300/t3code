use serde_json::{Value, json};
use t3_contracts::managed_policy::ManagedPolicyDocument;

pub fn windows_admx() -> &'static str {
    include_str!("policy-windows.admx")
}
pub fn windows_adml() -> &'static str {
    include_str!("policy-windows.adml")
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProfileError(String);
pub fn macos_profile(
    document: &ManagedPolicyDocument,
    profile_uuid: &str,
    payload_uuid: &str,
) -> Result<String, ProfileError> {
    let document = t3_contracts::stored::to_value(document)
        .map_err(|_| ProfileError("Policy could not be encoded.".into()))?;
    if let Some(mut path) = null_path(&document, Vec::new()) {
        if path.first().is_some_and(|p| p == "settings")
            && path.last().is_some_and(|p| p == "value")
        {
            path.pop();
        }
        return Err(ProfileError(format!(
            "{} is null, which a configuration profile cannot hold; deploy this policy as a file.",
            path.join(".")
        )));
    }
    let mut payload = json!({"PayloadDisplayName":"T3 Code policy","PayloadIdentifier":format!("com.t3tools.t3code.policy.{payload_uuid}"),"PayloadType":"com.t3tools.t3code","PayloadUUID":payload_uuid,"PayloadVersion":1});
    if let (Some(target), Some(source)) = (payload.as_object_mut(), document.as_object()) {
        target.extend(source.clone());
    }
    let profile = json!({"PayloadContent":[payload],"PayloadDescription":"Configures T3 Code for every user on this Mac.","PayloadDisplayName":format!("T3 Code policy for {}",document["organization"].as_str().unwrap_or("")),"PayloadIdentifier":"com.t3tools.t3code.policy","PayloadOrganization":document["organization"],"PayloadScope":"System","PayloadType":"Configuration","PayloadUUID":profile_uuid,"PayloadVersion":1});
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n{}\n</plist>\n",
        plist_value(&profile, "")
    ))
}
fn null_path(value: &Value, path: Vec<String>) -> Option<Vec<String>> {
    match value {
        Value::Null => Some(path),
        Value::Object(map) => map.iter().find_map(|(key, value)| {
            let mut next = path.clone();
            next.push(key.clone());
            null_path(value, next)
        }),
        Value::Array(items) => items.iter().enumerate().find_map(|(i, value)| {
            let mut next = path.clone();
            next.push(i.to_string());
            null_path(value, next)
        }),
        _ => None,
    }
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn plist_value(value: &Value, indent: &str) -> String {
    let inner = format!("{indent}  ");
    match value {
        Value::String(text) => format!("{indent}<string>{}</string>", xml(text)),
        Value::Bool(value) => format!("{indent}<{value} />"),
        Value::Number(number) => {
            let tag = if number.as_f64().is_some_and(|n| n.fract() == 0.0) {
                "integer"
            } else {
                "real"
            };
            let number = t3_contracts::wire::JsNumber::try_from(number.clone())
                .map(|n| n.to_string())
                .unwrap_or_else(|_| number.to_string());
            format!("{indent}<{tag}>{number}</{tag}>")
        }
        Value::Array(items) => format!(
            "{indent}<array>\n{}\n{indent}</array>",
            items
                .iter()
                .map(|v| plist_value(v, &inner))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Value::Object(map) => format!(
            "{indent}<dict>\n{}\n{indent}</dict>",
            map.iter()
                .map(|(key, value)| format!(
                    "{inner}<key>{}</key>\n{}",
                    xml(key),
                    plist_value(value, &inner)
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Value::Null => String::new(),
    }
}
