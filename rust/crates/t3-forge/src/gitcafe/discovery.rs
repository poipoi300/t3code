use super::*;
use crate::cli::*;
impl GitCafe {
    pub(super) async fn discover_cli(
        &self,
        cwd: std::path::PathBuf,
    ) -> Result<t3_contracts::source_control::SourceControlProviderDiscoveryItem> {
        let probe = |args: Vec<String>| {
            self.cli.run(CliRequest {
                program: "cafe",
                provider: "gitcafe",
                operation: "discover".into(),
                cwd: cwd.clone(),
                args,
                stdin: None,
                environment: self.credentials.cli_environment("git.cafe"),
                max_bytes: 16_000,
                timeout: std::time::Duration::from_secs(10),
                cancellation: self.stop.clone(),
            })
        };
        let version = probe(vec!["--version".into()]).await;
        let available = version.as_ref().is_ok_and(|v| v.code == 0);
        let mut result = json!({"kind":"gitcafe","label":"GitCafe","executable":"cafe","status":if available{"available"}else{"missing"},"version":{"_tag":"None"},"installHint":"Install the GitCafe CLI, save a token in Settings, or set CAFE_TOKEN on the server.","detail":{"_tag":"None"},"auth":{"status":"unknown","account":{"_tag":"None"},"host":{"_tag":"Some","value":"git.cafe"},"detail":{"_tag":"None"}}});
        if available {
            if let Ok(version) = version
                && let Some(line) = String::from_utf8_lossy(&version.stdout)
                    .lines()
                    .find(|s| !s.trim().is_empty())
            {
                result["version"] =
                    json!({"_tag":"Some","value":crate::remote::transport_safe_error_value(line)});
            }
            if let Ok(auth) = probe(vec![
                "--host=https://git.cafe/api".into(),
                "--no-input".into(),
                "--no-update-check".into(),
                "auth".into(),
                "status".into(),
                "--json".into(),
            ])
            .await
            {
                let raw = serde_json::from_slice::<Value>(&auth.stdout).unwrap_or(Value::Null);
                if auth.code == 0
                    && raw["schemaVersion"] == 1
                    && let Some(username) = text(&raw["data"], "username")
                {
                    result["auth"]["status"] = json!("authenticated");
                    result["auth"]["account"] = json!({"_tag":"Some","value":username});
                } else {
                    let failure = [&auth.stderr, &auth.stdout].into_iter().find_map(|bytes| {
                        let combined = String::from_utf8_lossy(bytes);
                        std::iter::once(combined.as_ref())
                            .chain(combined.lines().rev())
                            .find_map(|s| {
                                let value = serde_json::from_str::<Value>(s).ok()?;
                                (value["schemaVersion"] == 1 && value["error"].is_object())
                                    .then(|| value["error"].clone())
                            })
                    });
                    if let Some(failure) = failure {
                        result["auth"]["status"] = json!(if failure["code"]
                            == "AUTHENTICATION_REQUIRED"
                            || failure["status"] == 401
                        {
                            "unauthenticated"
                        } else {
                            "unknown"
                        });
                    }
                }
            }
        }
        parse(result, "discover")
    }
}
