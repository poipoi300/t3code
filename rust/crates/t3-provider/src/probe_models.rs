use crate::{
    catalog::{self, capabilities, model, select_option},
    sensitive::{ServiceError, ServiceResult},
};
use serde_json::{Value, json};
use t3_contracts::{
    model::{ModelCapabilities, ProviderOptionDescriptor},
    server::{ServerProviderAuth, ServerProviderModel},
};

fn codex_route_base_url(config: &Value) -> Option<Option<&str>> {
    let config = config.as_object()?;
    for field in ["model_provider", "openai_base_url"] {
        if config
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return None;
        }
    }
    let providers = if let Some(value) = config.get("model_providers") {
        let providers = value.as_object()?;
        for provider in providers.values() {
            let provider = provider.as_object()?;
            if provider
                .get("base_url")
                .is_some_and(|value| !value.is_null() && !value.is_string())
            {
                return None;
            }
        }
        Some(providers)
    } else {
        None
    };
    let provider = config
        .get("model_provider")
        .and_then(Value::as_str)
        .unwrap_or("openai");
    if provider == "openai" {
        Some(
            config
                .get("openai_base_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty()),
        )
    } else {
        Some(
            providers?
                .get(provider)?
                .get("base_url")
                .and_then(Value::as_str),
        )
    }
}
fn routes_to_native_openai(config: &Value) -> bool {
    match codex_route_base_url(config) {
        None => false,
        Some(None) => true,
        Some(Some(value)) => url::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && matches!(url.host_str(), Some("api.openai.com" | "chatgpt.com"))
        }),
    }
}
pub fn codex_custom_endpoint(config: &Value) -> Option<String> {
    if routes_to_native_openai(config) {
        return None;
    }
    let url = url::Url::parse(codex_route_base_url(config)??).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}
pub fn claude_custom_endpoint(base_url: Option<&str>) -> Option<String> {
    let url = url::Url::parse(t3_contracts::wire::trim_js(base_url?)).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|host| host != "api.anthropic.com"))
    .then(|| url.origin().ascii_serialization())
}
pub fn codex_native_quota(account: &Value, config: &Value) -> bool {
    account["requiresOpenaiAuth"].as_bool() == Some(true)
        && account["account"]["type"].as_str() != Some("apiKey")
        && routes_to_native_openai(config)
}
pub fn codex_plan(plan: &str) -> Option<&'static str> {
    Some(match plan {
        "free" => "ChatGPT Free Subscription",
        "go" => "ChatGPT Go Subscription",
        "plus" => "ChatGPT Plus Subscription",
        "pro" => "ChatGPT Pro 200 Subscription",
        "prolite" => "ChatGPT Pro 100 Subscription",
        "promax" => "ChatGPT Pro 500 Subscription",
        "self_serve_business_prolite" => "ChatGPT Business Premium Subscription",
        "team" | "self_serve_business_usage_based" | "business" => "ChatGPT Business Subscription",
        "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" | "enterprise" => {
            "ChatGPT Enterprise Subscription"
        }
        "edu" | "edu_plus" | "edu_pro" => "ChatGPT Edu Subscription",
        "unknown" => "ChatGPT Subscription",
        _ => return None,
    })
}
fn effort_label(id: &str) -> &str {
    match id {
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        _ => id,
    }
}
pub fn codex_capabilities(raw: &Value) -> ServiceResult<ModelCapabilities> {
    let mut options = vec![];
    let efforts: Vec<_> = raw
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| o["reasoningEffort"].as_str())
        .map(|s| (s, effort_label(s)))
        .collect();
    let default =
        if catalog::codex_family(raw["model"].as_str().unwrap_or_default()) == "gpt-6-astra" {
            Some("medium")
        } else {
            raw["defaultReasoningEffort"].as_str()
        };
    if !efforts.is_empty() {
        options.push(select_option(
            "reasoningEffort",
            "Reasoning",
            &efforts,
            default.filter(|s| efforts.iter().any(|(id, _)| id == s)),
        ));
    }
    let tiers:Vec<Value> = raw.get("serviceTiers").and_then(Value::as_array).filter(|a|!a.is_empty()).cloned().unwrap_or_else(||raw.get("additionalSpeedTiers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|id|json!({"id":id,"name":if id == "fast" {"Fast"} else {id},"description":""})).collect());
    if !tiers.is_empty() {
        let default = raw["defaultServiceTier"]
            .as_str()
            .filter(|s| tiers.iter().any(|t| t["id"].as_str() == Some(s)))
            .unwrap_or("default");
        let mut standard = json!({"id":"default","label":"Standard"});
        if default == "default" {
            standard["isDefault"] = json!(true);
        }
        let mut choices = vec![standard];
        for tier in tiers {
            let id = tier["id"].as_str().ok_or(ServiceError::Response)?;
            let mut choice = json!({"id":id,"label":tier["name"]});
            if id == default {
                choice["isDefault"] = json!(true);
            }
            let description = if id == "ultrafast" {
                Some("Even faster, more expensive")
            } else {
                tier["description"].as_str().filter(|s| !s.is_empty())
            };
            if let Some(description) = description {
                choice["description"] = json!(description);
            }
            choices.push(choice);
        }
        options.push(serde_json::from_value(json!({"id":"serviceTier","label":"Service Tier","type":"select","options":choices,"currentValue":default})).map_err(|_|ServiceError::Response)?);
    }
    if let Some(programs) = raw["availableAccessPrograms"]
        .get("cyber")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
    {
        let mut ids: Vec<_> = programs.iter().filter_map(Value::as_str).collect();
        let default = if ids.contains(&"standard") {
            "standard"
        } else {
            *ids.first().ok_or(ServiceError::Response)?
        };
        if !ids.contains(&"standard") {
            ids.insert(0, "standard");
        }
        let choices: Vec<_> = ids
            .iter()
            .map(|id| {
                (
                    *id,
                    match *id {
                        "standard" => "Standard",
                        "daybreakBlue" => "Daybreak Blue",
                        "daybreakRed" => "Daybreak Red",
                        _ => id,
                    },
                )
            })
            .collect();
        options.push(select_option(
            "cyberAccessProgram",
            "Cyber Access",
            &choices,
            Some(default),
        ));
    }
    Ok(capabilities(options))
}
pub fn codex_models(raw: &[Value]) -> ServiceResult<Vec<ServerProviderModel>> {
    let mut models = vec![];
    for entry in raw {
        let slug = entry["model"].as_str().ok_or(ServiceError::Response)?;
        let name = entry["displayName"].as_str().unwrap_or(slug);
        let mut next = model(
            slug,
            &format_codex_name(name),
            Some(codex_capabilities(entry)?),
        );
        if entry["isDefault"].as_bool() == Some(true) {
            next.is_default = Some(Some(true));
        }
        models.push(next);
    }
    let preferred = t3_contracts::model::PREFERRED_DEFAULT_CODEX_MODELS
        .iter()
        .find_map(|slug| {
            models
                .iter()
                .find(|m| !m.is_custom && catalog::codex_family(&m.slug.0) == *slug)
                .map(|m| m.slug.0.clone())
        });
    if let Some(preferred) = preferred {
        for model in &mut models {
            model.is_default = (model.slug.0 == preferred).then_some(Some(true));
        }
    }
    Ok(models)
}
pub fn format_codex_name(name: &str) -> String {
    let mut value = name.to_owned();
    if value
        .get(..3)
        .is_some_and(|s| s.eq_ignore_ascii_case("gpt"))
    {
        value.replace_range(..3, "GPT");
    }
    let mut after_hyphen = false;
    value
        .chars()
        .map(|c| {
            let next = if after_hyphen && c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            };
            after_hyphen = c == '-';
            next
        })
        .collect()
}
pub fn codex_skills(
    response: &Value,
    cwd: &str,
) -> ServiceResult<Vec<t3_contracts::server::ServerProviderSkill>> {
    let data = response["data"].as_array().ok_or(ServiceError::Response)?;
    let entries: Vec<_> = if let Some(entry) = data.iter().find(|e| e["cwd"].as_str() == Some(cwd))
    {
        vec![entry]
    } else {
        data.iter().collect()
    };
    let mut output = vec![];
    for entry in entries {
        for skill in entry["skills"].as_array().ok_or(ServiceError::Response)? {
            let mut value =
                json!({"name":skill["name"],"path":skill["path"],"enabled":skill["enabled"]});
            for key in ["description", "scope"] {
                if skill[key].as_str().is_some_and(|s| !s.is_empty()) {
                    value[key] = skill[key].clone();
                }
            }
            if let Some(name) = skill["interface"]["displayName"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                value["displayName"] = json!(name);
            }
            if let Some(description) = skill["shortDescription"]
                .as_str()
                .or_else(|| skill["interface"]["shortDescription"].as_str())
                .filter(|s| !s.is_empty())
            {
                value["shortDescription"] = json!(description);
            }
            output.push(serde_json::from_value(value).map_err(|_| ServiceError::Response)?);
        }
    }
    Ok(output)
}
pub fn codex_configured_defaults(
    models: &mut [ServerProviderModel],
    config: Option<&Value>,
    account: &Value,
) {
    let tier = config.and_then(|v| v["service_tier"].as_str());
    let unavailable = account["account"]["type"] == "apiKey"
        && config.is_none_or(|v| v["features"]["api_key_cyber_access_programs"] == false);
    for model in models {
        let Some(options) = model
            .capabilities
            .as_mut()
            .and_then(|c| c.option_descriptors.as_mut())
            .and_then(Option::as_mut)
        else {
            continue;
        };
        options.retain(|d|!matches!(d,ProviderOptionDescriptor::Select(d) if d.id=="cyberAccessProgram" && (model.is_custom || unavailable)));
        for descriptor in options {
            let ProviderOptionDescriptor::Select(d) = descriptor else {
                continue;
            };
            let selected = if d.id == "serviceTier" {
                tier.and_then(|tier| {
                    d.options
                        .iter()
                        .find(|o| o.id == tier || tier == "fast" && o.id == "priority")
                        .map(|o| o.id.clone())
                })
            } else if d.id == "cyberAccessProgram" {
                if config.is_none() {
                    d.current_value = None;
                    for option in &mut d.options {
                        option.is_default = None;
                    }
                }
                if config.is_some_and(|v| v["daybreak"] == true) {
                    ["daybreakBlue", "daybreakRed"]
                        .into_iter()
                        .find(|id| d.options.iter().any(|o| o.id == *id))
                        .map(str::to_owned)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(selected) = selected {
                d.current_value = Some(Some(selected.clone()));
                for option in &mut d.options {
                    option.is_default = (option.id == selected).then_some(Some(true));
                }
            }
        }
    }
}
pub fn codex_auth(
    account: &Value,
    limits: Option<&Value>,
) -> ServiceResult<(ServerProviderAuth, &'static str)> {
    let mut auth = json!({"status":"unknown"});
    let current = &account["account"];
    let status = if current.is_object() {
        auth["status"] = json!("authenticated");
        auth["type"] = current["type"].clone();
        let label = match current["type"].as_str() {
            Some("apiKey") => Some("OpenAI API Key"),
            Some("amazonBedrock") => Some("Amazon Bedrock"),
            Some("chatgpt") => limits
                .and_then(|l| l["rateLimits"]["planType"].as_str())
                .filter(|s| *s != "unknown")
                .and_then(codex_plan)
                .or_else(|| current["planType"].as_str().and_then(codex_plan)),
            _ => None,
        };
        if let Some(label) = label {
            auth["label"] = json!(label);
        }
        if current["type"].as_str() == Some("chatgpt")
            && let Some(email) = current["email"].as_str().filter(|s| !s.trim().is_empty())
        {
            auth["email"] = json!(email);
        }
        if let Some(id) = limits
            .and_then(|l| l["accountId"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            auth["workspaceId"] = json!(id);
        }
        "ready"
    } else if account["requiresOpenaiAuth"].as_bool() == Some(true) {
        auth["status"] = json!("unauthenticated");
        "error"
    } else {
        "warning"
    };
    Ok((
        serde_json::from_value(auth).map_err(|_| ServiceError::Response)?,
        status,
    ))
}
fn title(value: &str) -> String {
    value
        .split([' ', '_', '-'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().to_string() + &chars.as_str().to_lowercase())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn claude_auth(
    account: &Value,
    custom_endpoint: bool,
) -> ServiceResult<(ServerProviderAuth, &'static str)> {
    let provider = account["apiProvider"].as_str();
    let authenticated = custom_endpoint
        || provider.is_some_and(|s| s != "firstParty")
        || account["tokenSource"].as_str() != Some("none")
        || account["apiKeySource"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s != "none")
        || account["email"].as_str().is_some_and(|s| !s.is_empty())
        || account["subscriptionType"]
            .as_str()
            .is_some_and(|s| !s.is_empty());
    let mut auth = json!({"status":if authenticated {"authenticated"} else {"unauthenticated"}});
    if authenticated {
        for field in ["email", "workspaceId"] {
            if let Some(value) = account[field].as_str().filter(|s| !s.trim().is_empty()) {
                auth[field] = json!(value);
            }
        }
        let method = account["tokenSource"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .replace([' ', '_', '-'], "");
        let metadata: Option<(String, String)> = if provider == Some("bedrock") {
            Some(("bedrock".into(), "Amazon Bedrock".into()))
        } else if custom_endpoint {
            Some(("customEndpoint".into(), "Custom API endpoint".into()))
        } else if ["apikey", "anthropicapikey", "anthropicauthtoken"].contains(&method.as_str()) {
            Some(("apiKey".into(), "Claude API Key".into()))
        } else {
            account["subscriptionType"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|raw| {
                    let normalized = raw.to_lowercase().replace([' ', '_', '-'], "");
                    let label = match normalized.as_str() {
                        "claudemaxsubscription"
                        | "claudemax5xsubscription"
                        | "max"
                        | "maxplan"
                        | "max5" => "Max".into(),
                        "claudemax20xsubscription" | "max20" => "Ultra".into(),
                        "claudeenterprisesubscription" | "enterprise" => "Enterprise".into(),
                        "claudeteamsubscription" | "team" => "Team".into(),
                        "claudeprosubscription" | "pro" => "Pro".into(),
                        "claudefreesubscription" | "free" => "Free".into(),
                        _ => title(raw),
                    };
                    let normalized = label.to_lowercase().replace([' ', '_', '-'], "");
                    let label = match (
                        normalized.starts_with("claude"),
                        normalized.ends_with("subscription"),
                    ) {
                        (true, true) => label,
                        (true, false) => format!("{label} Subscription"),
                        (false, true) => format!("Claude {label}"),
                        (false, false) => format!("Claude {label} Subscription"),
                    };
                    (raw.into(), label)
                })
        };
        if let Some((kind, label)) = metadata {
            auth["type"] = json!(kind);
            auth["label"] = json!(label);
        }
    }
    Ok((
        serde_json::from_value(auth).map_err(|_| ServiceError::Response)?,
        if authenticated { "ready" } else { "error" },
    ))
}
pub fn option_values(caps: &ModelCapabilities, id: &str) -> Vec<String> {
    catalog::descriptors(caps)
        .iter()
        .find_map(|d| match d {
            ProviderOptionDescriptor::Select(d) if d.id == id => {
                Some(d.options.iter().map(|o| o.id.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}
