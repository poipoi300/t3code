use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = std::env::var_os("T3_PROVIDER_FIXTURE").ok_or("missing fixture path")?;
    let config: Value = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(log) = config["log"].as_str() {
        writeln!(
            OpenOptions::new().create(true).append(true).open(log)?,
            "{}",
            json!(args)
        )?;
    }
    if config["protocol"] == "codex" && args.first().is_some_and(|s| s == "app-server")
        || config["protocol"] == "claude" && args.iter().any(|s| s == "--input-format")
    {
        return protocol(&config);
    }
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] => {
            print!("{}", config["version"].as_str().unwrap_or("fx 0.0.13"));
            eprint!("{}", config["versionStderr"].as_str().unwrap_or(""));
            std::process::exit(config["versionExit"].as_i64().unwrap_or(0) as i32);
        }
        ["status", "--json"] => {
            print!(
                "{}",
                config
                    .get("status")
                    .unwrap_or(&json!({"auth":"oauth","model":"meta/muse"}))
            );
            std::process::exit(config["statusExit"].as_i64().unwrap_or(0) as i32);
        }
        ["models", "--json"] => {
            print!(
                "{}",
                config
                    .get("models")
                    .unwrap_or(&json!({"ids":["meta/muse","other"]}))
            );
            std::process::exit(config["modelsExit"].as_i64().unwrap_or(0) as i32);
        }
        ["--cache=false", "read", _, "--no-newline", reference] => {
            if let Some(value) = config["secrets"][reference].as_str() {
                print!("{value}");
            } else {
                std::process::exit(1);
            }
        }
        ["--cache=false", "inject", _, "-i", file] => {
            if let Some(exit) = config["injectExit"].as_i64().filter(|e| *e != 0) {
                std::process::exit(exit as i32);
            }
            let template = fs::read_to_string(file)?;
            let mut rest = template.as_str();
            while let Some(index) = rest.find("{{ ") {
                print!("{}", &rest[..index]);
                let after = &rest[index + 3..];
                let end = after.find(" }}").ok_or("invalid template")?;
                let reference = &after[..end];
                let value = config["secrets"][reference]
                    .as_str()
                    .ok_or("unreadable secret")?;
                print!("{value}");
                rest = &after[end + 3..];
            }
            print!("{rest}");
        }
        _ => return Err("fixture only supports status, version, models and secret reads".into()),
    }
    Ok(())
}

fn protocol(config: &Value) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let mut output = std::io::stdout().lock();
    let mut model_requests = 0;
    for line in std::io::stdin().lock().lines() {
        let message: Value = serde_json::from_str(&line?)?;
        let claude = config["protocol"] == "claude";
        let method = if claude {
            message["request"]["subtype"].as_str()
        } else {
            message["method"].as_str()
        }
        .ok_or("fixture received an unrecognized protocol frame")?;
        if let Some(log) = config["log"].as_str() {
            writeln!(
                OpenOptions::new().create(true).append(true).open(log)?,
                "{}",
                json!([method])
            )?;
        }
        let response=match method {
            "initialized"=>continue,
            "initialize" if claude=>json!({"commands":[{"name":"review","description":"Review changes","argumentHint":"target"}],"account":config.get("nativeAccount").cloned().unwrap_or(json!({"email":"ada@example.com","subscriptionType":"max","tokenSource":"oauth","apiProvider":"firstParty"})),"models":config.get("nativeModels").cloned().unwrap_or(json!([])),"claude_code_version":"2.1.200"}),
            "initialize"=>json!({"codexHome":std::env::var("CODEX_HOME").unwrap_or_else(|_|"fixture-home".into()),"platformFamily":"fixture","platformOs":"fixture","userAgent":"codex/0.166.0"}),
            "account/read"=>config.get("nativeAccount").cloned().unwrap_or(json!({"account":{"type":"chatgpt","email":"ada@example.com","planType":"pro"},"requiresOpenaiAuth":true})),
            "model/list"=> {
                model_requests += 1;
                if let Ok(home) = std::env::var("CODEX_HOME") {
                    fs::create_dir_all(&home)?;
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
                    let fetched = chrono::DateTime::from_timestamp_millis(now).ok_or("invalid fixture clock")?.to_rfc3339();
                    fs::write(std::path::Path::new(&home).join("models_cache.json"), json!({"fetched_at":fetched}).to_string())?;
                }
                json!({"data":if model_requests>1 {config.get("refreshedModels").or(config.get("nativeModels"))} else {config.get("nativeModels")}.cloned().unwrap_or(json!([{ "id":"gpt-6-luna","model":"gpt-6-luna","displayName":"GPT-6 Luna","description":"","hidden":false,"isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"low","description":""}],"defaultReasoningEffort":"low"}])),"nextCursor":null})
            },
            "config/read"=>json!({"config":config.get("routing").cloned().unwrap_or(json!({}))}),
            "skills/list"=>json!({"data":[]}),
            "account/rateLimits/read"=>config.get("nativeUsage").cloned().unwrap_or(json!({"rateLimits":{"planType":"pro","primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1800000000}},"rateLimitsByLimitId":null,"rateLimitResetCredits":null,"accountId":"workspace"})),
            "account/rateLimitResetCredit/consume" => {
                if message["params"]["idempotencyKey"] != config["resetKey"] { return Err("unexpected reset key".into()); }
                json!({"outcome":"reset"})
            },
            "get_usage"=>config.get("nativeUsage").cloned().unwrap_or(json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":42,"resets_at":"2026-11-13T12:00:00Z"}}})),
            "get_settings"=>config.get("nativeSettings").cloned().unwrap_or(json!({"effective":{}})),
            _=>return Err("fixture refuses provider turns and unsupported controls".into()),
        };
        let frame = if claude {
            json!({"type":"control_response","response":{"subtype":"success","request_id":message["request_id"],"response":response}})
        } else {
            json!({"jsonrpc":"2.0","id":message["id"],"result":response})
        };
        writeln!(output, "{frame}")?;
        output.flush()?;
    }
    Ok(())
}
