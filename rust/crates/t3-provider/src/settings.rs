use crate::{Result, discovery::Driver, invalid};
use serde_json::{Value, json};
use t3_contracts::{model::CustomModelSetting, secret_reference::SecretReference};

/// Decodes the packaged driver schemas absent from t3-contracts, retaining their wire field names.
pub fn decode_packaged(driver: Driver, raw: Option<&Value>) -> Result<Value> {
    let empty = json!({});
    let input = raw.unwrap_or(&empty);
    let input = input
        .as_object()
        .ok_or_else(|| invalid("provider settings", "expected an object"))?;
    let text = |name: &str, default: &str| -> Result<String> {
        match input.get(name) {
            None => Ok(default.into()),
            Some(Value::String(s)) => Ok(s.trim().into()),
            _ => Err(invalid(
                "provider settings",
                format!("{name} must be a string"),
            )),
        }
    };
    let enabled = match input.get("enabled") {
        None => driver == Driver::AcpRegistry,
        Some(Value::Bool(enabled)) => *enabled,
        _ => return Err(invalid("provider settings", "enabled must be a boolean")),
    };
    let custom: Vec<Value> = match input.get("customModels") {
        None => vec![],
        Some(Value::Array(a)) => {
            if driver == Driver::AcpRegistry {
                a.iter()
                    .map(|v| {
                        v.as_str().map(|s| json!(s)).ok_or_else(|| {
                            invalid(
                                "provider settings",
                                "registry custom models must be strings",
                            )
                        })
                    })
                    .collect::<Result<_>>()?
            } else {
                a.iter()
                    .map(|v| {
                        serde_json::from_value::<CustomModelSetting>(v.clone())
                            .and_then(serde_json::to_value)
                            .map_err(Into::into)
                    })
                    .collect::<Result<_>>()?
            }
        }
        _ => {
            return Err(invalid(
                "provider settings",
                "customModels must be an array",
            ));
        }
    };
    let mut config = json!({"enabled":enabled,"customModels":custom});
    match driver {
        Driver::Cursor => {
            for field in ["binaryPath", "apiEndpoint"] {
                if input.contains_key(field) {
                    config[field] = json!(text(field, "")?);
                }
            }
        }
        Driver::AcpRegistry => {
            let literal = |name: &str, default: &str| -> Result<String> {
                match input.get(name) {
                    None => Ok(default.into()),
                    Some(Value::String(s)) => Ok(s.clone()),
                    _ => Err(invalid(
                        "provider settings",
                        format!("{name} must be a string"),
                    )),
                }
            };
            let source = literal("source", "registry")?;
            if !["local", "registry"].contains(&source.as_str()) {
                return Err(invalid("provider settings", "invalid ACP source"));
            }
            let distribution = literal("distribution", "auto")?;
            if !["auto", "binary", "npx", "uvx"].contains(&distribution.as_str()) {
                return Err(invalid("provider settings", "invalid ACP distribution"));
            }
            config["source"] = json!(source);
            config["distribution"] = json!(distribution);
            for field in ["agentId", "commandPath", "authMethodId"] {
                config[field] = json!(text(field, "")?);
            }
            config["commandArgs"] = match input.get("commandArgs") {
                None => json!([]),
                Some(Value::Array(args)) if args.iter().all(Value::is_string) => json!(args),
                _ => return Err(invalid("provider settings", "commandArgs must be strings")),
            };
        }
        Driver::Grok | Driver::Kiro | Driver::Pi | Driver::Muse | Driver::Fx | Driver::OpenCode => {
            let binary = text("binaryPath", driver.binary().unwrap_or_default())?;
            config["binaryPath"] = json!(if binary.is_empty() {
                driver.binary().unwrap_or_default()
            } else {
                &binary
            });
            if driver == Driver::Pi {
                config["launchArgs"] = json!(text("launchArgs", "")?);
            }
            if driver == Driver::OpenCode {
                config["serverUrl"] = json!(text("serverUrl", "")?);
                config["serverPassword"] = match input.get("serverPassword") {
                    None => json!(""),
                    Some(Value::String(s)) => json!(s),
                    Some(value)
                        if serde_json::from_value::<SecretReference>(value.clone()).is_ok() =>
                    {
                        value.clone()
                    }
                    _ => return Err(invalid("provider settings", "invalid serverPassword")),
                };
            }
        }
        _ => {
            return Err(invalid(
                "provider settings",
                "use the contract settings codec for this driver",
            ));
        }
    }
    Ok(config)
}
