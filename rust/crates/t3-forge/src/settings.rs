//! Inputs must come from the host's secret resolver, never from a client settings snapshot.
use crate::{
    FailureReason, ForgeError, Result, gitcafe::credentials::GitCafeSettings,
    github::transport::CredentialSettings, http::ApiCredential,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use t3_contracts::settings::SourceControlHostSettings;
fn literal(value: Option<&Value>, provider: &str) -> Result<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) if s != "••••••" => Ok(s.clone()),
        _ => Err(ForgeError::provider(
            provider,
            "credential",
            FailureReason::Unauthenticated,
            "The configured secret could not be resolved. Check the server's secret provider.",
        )),
    }
}
fn record(
    raw: &SourceControlHostSettings,
    key: &str,
    provider: &str,
) -> Result<BTreeMap<String, String>> {
    let Some(value) = raw.get(key) else {
        return Ok(BTreeMap::new());
    };
    let values = value.as_object().ok_or_else(|| {
        ForgeError::provider(
            provider,
            "credential",
            FailureReason::Failed,
            "The source-control credential settings are invalid.",
        )
    })?;
    values
        .iter()
        .map(|(host, value)| {
            Ok((
                host.trim().to_ascii_lowercase(),
                literal(Some(value), provider)?,
            ))
        })
        .collect()
}
pub fn github(
    raw: &SourceControlHostSettings,
    environment: &t3_process::Environment,
) -> Result<CredentialSettings> {
    Ok(CredentialSettings {
        tokens: record(raw, "tokens", "github")?,
        accounts: record(raw, "accounts", "github")?,
        disabled_hosts: raw
            .get("disabledHosts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|v| v.trim().to_ascii_lowercase())
            .collect::<HashSet<_>>(),
        environment: environment
            .iter()
            .filter_map(|(k, v)| Some((k.to_str()?.into(), v.as_ref()?.to_str()?.into())))
            .collect(),
    })
}
fn env(environment: &t3_process::Environment, key: &str) -> String {
    environment
        .get(std::ffi::OsStr::new(key))
        .and_then(|v| v.as_ref())
        .map(|v| v.to_string_lossy().into())
        .unwrap_or_default()
}
fn header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (33..=126).contains(&b))
}
pub fn bitbucket(
    raw: &SourceControlHostSettings,
    environment: &t3_process::Environment,
) -> Result<Option<ApiCredential>> {
    let access = literal(raw.get("accessToken"), "bitbucket")?;
    if header_safe(&access) {
        return Ok(Some(ApiCredential::Bearer(access)));
    }
    let api = literal(raw.get("apiToken"), "bitbucket")?;
    let email = raw.get("email").and_then(Value::as_str).unwrap_or_default();
    if header_safe(email) && header_safe(&api) {
        return Ok(Some(ApiCredential::Basic {
            username: email.into(),
            password: api,
        }));
    }
    let access = env(environment, "T3CODE_BITBUCKET_ACCESS_TOKEN");
    if header_safe(&access) {
        return Ok(Some(ApiCredential::Bearer(access)));
    }
    let email = env(environment, "T3CODE_BITBUCKET_EMAIL");
    let api = env(environment, "T3CODE_BITBUCKET_API_TOKEN");
    Ok(
        (header_safe(&email) && header_safe(&api)).then_some(ApiCredential::Basic {
            username: email,
            password: api,
        }),
    )
}
pub fn gitcafe(
    raw: &SourceControlHostSettings,
    environment: t3_process::Environment,
) -> Result<GitCafeSettings> {
    Ok(GitCafeSettings {
        token: Some(literal(raw.get("token"), "gitcafe")?).filter(|s| !s.is_empty()),
        environment,
    })
}
