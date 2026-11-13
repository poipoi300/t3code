use super::{
    GitHub,
    decode::{parse, text},
};
use crate::{FailureReason, Forge, Result, forge::CheckoutInput, model::SourceControlProviderInfo};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};
use t3_contracts::source_control::SourceControlProviderDiscoveryItem;

fn option(value: Option<&str>) -> Value {
    value
        .filter(|s| !s.trim().is_empty())
        .map(|value| json!({"_tag":"Some","value":value.trim()}))
        .unwrap_or_else(|| json!({"_tag":"None"}))
}
fn auth(status: &str, account: Option<&str>, host: Option<&str>, detail: Option<&str>) -> Value {
    json!({"status":status,"account":option(account),"host":option(host),"detail":option(detail)})
}
fn safe_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line
                    .to_lowercase()
                    .trim_start_matches('-')
                    .trim_start()
                    .starts_with("token:")
                && !line
                    .to_lowercase()
                    .trim_start_matches('-')
                    .trim_start()
                    .starts_with("token scopes:")
        })
        .map(crate::remote::transport_safe_error_value)
}
pub fn parse_auth(
    output: &super::cli::CliOutput,
    settings: &super::transport::CredentialSettings,
) -> Value {
    let parsed: Option<Value> = serde_json::from_str(&output.stdout)
        .ok()
        .filter(|v: &Value| v["hosts"].is_object());
    let Some(parsed) = parsed else {
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        let detail = if output.code != 0 && combined.contains("unknown flag: --json") {
            Some("GitHub CLI is too old to report sign-in status. Update `gh` to 2.81.0 or newer and rescan.".into())
        } else {
            safe_line(&combined)
        };
        return auth(
            if output.code != 0 && !combined.contains("unknown flag: --json") {
                "unauthenticated"
            } else {
                "unknown"
            },
            None,
            None,
            detail.as_deref(),
        );
    };
    let mut rows = Vec::new();
    for entries in parsed["hosts"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
    {
        for row in entries.as_array().into_iter().flatten() {
            let (Some(host), Some(login)) = (text(row, "host"), text(row, "login")) else {
                continue;
            };
            let variable = text(row, "tokenSource").filter(|v| {
                matches!(
                    *v,
                    "GH_TOKEN" | "GITHUB_TOKEN" | "GH_ENTERPRISE_TOKEN" | "GITHUB_ENTERPRISE_TOKEN"
                )
            });
            let mut value = json!({"host":host.to_lowercase(),"account":login,"active":row["active"]==true,"authenticated":row["state"]=="success"});
            if let Some(error) = text(row, "error") {
                value["error"] = json!(crate::remote::transport_safe_error_value(error));
            }
            if let Some(variable) = variable {
                value["environmentVariable"] = json!(variable);
            }
            rows.push(value);
        }
    }
    let fallback = rows
        .iter()
        .find(|r| r["authenticated"] == true && r["active"] == true)
        .or_else(|| rows.iter().find(|r| r["authenticated"] == true));
    let mut hosts = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(host) = fallback.and_then(|r| r["host"].as_str()) {
        hosts.push(host);
        seen.insert(host);
    }
    for row in &rows {
        if let Some(host) = row["host"].as_str()
            && seen.insert(host)
        {
            hosts.push(host);
        }
    }
    let chosen = hosts.into_iter().find_map(|host| {
        if settings.disabled_hosts.contains(host) {
            return None;
        }
        let usable: Vec<_> = rows
            .iter()
            .filter(|r| r["host"] == host && r["authenticated"] == true)
            .collect();
        usable
            .iter()
            .find(|r| r["environmentVariable"].is_string())
            .copied()
            .or_else(|| {
                settings.accounts.get(host).and_then(|account| {
                    usable
                        .iter()
                        .find(|r| r["account"] == account.as_str())
                        .copied()
                })
            })
            .or_else(|| usable.iter().find(|r| r["active"] == true).copied())
            .or_else(|| usable.first().copied())
    });
    let mut result = if let Some(chosen) = chosen {
        let detail=chosen["environmentVariable"].as_str().map(|v|format!("Using {v} from the server environment; it overrides the account chosen in Settings."));
        auth(
            "authenticated",
            chosen["account"].as_str(),
            chosen["host"].as_str(),
            detail.as_deref(),
        )
    } else if let Some(fallback) = fallback {
        auth(
            "unauthenticated",
            None,
            fallback["host"].as_str(),
            Some(
                "Every GitHub host gh is signed in to is turned off in Settings → Source Control.",
            ),
        )
    } else {
        let failed = rows
            .iter()
            .find(|r| r["active"] == true)
            .or_else(|| rows.first());
        auth(
            "unauthenticated",
            None,
            failed.and_then(|r| r["host"].as_str()),
            Some(failed.and_then(|r| r["error"].as_str()).unwrap_or(
                "Run `gh auth login` to authenticate GitHub CLI with an active account.",
            )),
        )
    };
    result["accounts"] = json!(rows);
    result
}
impl GitHub {
    pub(super) async fn discover_github(
        &self,
        cwd: &Path,
    ) -> Result<SourceControlProviderDiscoveryItem> {
        let settings = self.http.settings().await.unwrap_or_default();
        let probe = self.cli.run(cwd, &["--version".into()]).await;
        let mut item = json!({"kind":"github","label":"GitHub","executable":"gh","status":if probe.as_ref().is_ok_and(|o|o.code==0){"available"}else{"missing"},"version":option(probe.as_ref().ok().and_then(|o|o.stdout.lines().chain(o.stderr.lines()).map(str::trim).find(|s|!s.is_empty()))),"installHint":"Install the GitHub command-line tool (`gh`) via https://cli.github.com/ or your package manager (for example `brew install gh`).","detail":{"_tag":"None"},"auth":auth("unknown",None,None,None)});
        if probe.is_ok() {
            item["auth"] = match self
                .cli
                .run(
                    cwd,
                    &[
                        "auth".into(),
                        "status".into(),
                        "--json".into(),
                        "hosts".into(),
                    ],
                )
                .await
            {
                Ok(output) => parse_auth(&output, &settings),
                Err(_) => auth(
                    "unknown",
                    None,
                    None,
                    Some("Could not check GitHub CLI sign-in status."),
                ),
            };
        }
        if let Ok(credential) = self.http.credential("github.com", cwd).await
            && credential.source != "gh"
        {
            let previous_accounts = item["auth"].get("accounts").cloned();
            item["status"] = json!("available");
            let source = match credential.source {
                "settings" => "the token saved in Settings",
                "app" => "the T3 Code for GitHub sign-in",
                _ => {
                    if settings
                        .environment
                        .get("GH_TOKEN")
                        .is_some_and(|v| !v.trim().is_empty())
                    {
                        "GH_TOKEN from the server environment"
                    } else {
                        "GITHUB_TOKEN from the server environment"
                    }
                }
            };
            let read = self
                .rest_json(
                    "github.com",
                    &cwd.to_string_lossy(),
                    "discovery",
                    reqwest::Method::GET,
                    "user",
                    None,
                )
                .await;
            item["auth"] = match read {
                Ok(value) if text(&value, "login").is_some() => auth(
                    "authenticated",
                    text(&value, "login"),
                    Some("github.com"),
                    Some(&format!("Using {source}.")),
                ),
                Err(error) if error.reason() != Some(FailureReason::Unauthenticated) => auth(
                    "unknown",
                    None,
                    Some("github.com"),
                    Some(&format!("Could not check {source}.")),
                ),
                _ => auth(
                    "unauthenticated",
                    None,
                    Some("github.com"),
                    Some(&format!(
                        "GitHub refused {source}. Replace it or sign in again in Settings → Source Control."
                    )),
                ),
            };
            if let Some(accounts) = previous_accounts {
                item["auth"]["accounts"] = accounts;
            }
        }
        if self.app_sign_in.as_ref().is_some_and(|app| app.enabled()) {
            item["appSignIn"] = json!(true);
        }
        parse(item, "discovery")
    }
    pub(super) async fn refine_unknown(
        &self,
        input: CheckoutInput,
    ) -> Result<Option<SourceControlProviderInfo>> {
        let Some(context) = input.context else {
            return Ok(None);
        };
        let Some(url) = url::Url::parse(&context.provider.base_url)
            .ok()
            .filter(|u| {
                u.host_str().is_some() && u.username().is_empty() && u.password().is_none()
            })
        else {
            return Ok(None);
        };
        let host = super::repository::api_host_for_remote(&context.remote_url)
            .unwrap_or_else(|| url.authority().to_lowercase());
        if !super::transport::valid_host(&host) {
            return Ok(None);
        }
        let disabled = self
            .http
            .settings()
            .await
            .is_some_and(|s| s.disabled_hosts.contains(&host));
        if !disabled {
            match self.http.credential(&host, &input.cwd).await {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.reason(),
                        Some(FailureReason::MissingTool | FailureReason::Unauthenticated)
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(Some(SourceControlProviderInfo {
            kind: self.kind(),
            name: t3_contracts::base_schemas::TrimmedNonEmptyString(
                if host == "github.com" {
                    "GitHub"
                } else {
                    "GitHub Self-Hosted"
                }
                .into(),
            ),
            base_url: if host == url.authority() {
                context.provider.base_url
            } else {
                format!("https://{host}")
            },
        }))
    }
}
