use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = std::env::var_os("T3_PROVIDER_FIXTURE").ok_or("missing fixture path")?;
    let config: Value = serde_json::from_slice(&fs::read(path)?)?;
    for (name, value) in config["expectedEnvironment"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if std::env::var(name).ok().as_deref() != value.as_str() {
            return Err("unexpected fixture environment".into());
        }
    }
    if let Some(log) = config["log"].as_str() {
        writeln!(
            OpenOptions::new().create(true).append(true).open(log)?,
            "{}",
            json!(args)
        )?;
    }
    if config["protocol"] == "opencode" && args.first().is_some_and(|argument| argument == "serve")
    {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(opencode_server(config, args));
    }
    if config["protocol"] == "pi" && args.windows(2).any(|args| args == ["--mode", "rpc"]) {
        return pi_protocol(&config, &args);
    }
    if config["protocol"] == "acp"
        || config["protocol"] == "grok" && args == ["agent", "stdio"]
        || config["protocol"] == "kiro" && args.first().is_some_and(|argument| argument == "acp")
    {
        return acp_protocol(&config);
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
        ["auth", "check", "--provider", _, "--json", "--no-refresh"] => {
            print!(
                "{}",
                config
                    .get("piAuthCheck")
                    .unwrap_or(&json!({"reason":"credentials_not_configured"}))
            );
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
        ["models"] => {
            let calls = config["log"]
                .as_str()
                .and_then(|path| fs::read_to_string(path).ok())
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<Vec<String>>(line).ok())
                .filter(|call| call == &["models"])
                .count();
            let output = config["grokModels"]
                .as_array()
                .and_then(|outputs| {
                    outputs
                        .get(calls.saturating_sub(1))
                        .or_else(|| outputs.last())
                })
                .or_else(|| config.get("grokModels"));
            print!(
                "{}",
                output
                    .and_then(Value::as_str)
                    .unwrap_or("You are logged in\n- grok-build (default)\n")
            );
            std::process::exit(config["modelsExit"].as_i64().unwrap_or(0) as i32);
        }
        ["inspect", "--json"] => {
            print!("{}", config.get("inspect").unwrap_or(&json!({"skills":[]})));
            std::process::exit(config["inspectExit"].as_i64().unwrap_or(0) as i32);
        }
        ["whoami", "--format", "json"] => {
            let value = config.get("whoami").cloned().unwrap_or_else(
                || json!({"accountType":"IdentityCenter","email":"kiro@example.invalid"}),
            );
            print!(
                "{}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            );
            std::process::exit(config["whoamiExit"].as_i64().unwrap_or(0) as i32);
        }
        ["chat", "--list-models", "--format", "json"] => {
            print!(
                "{}",
                config
                    .get("kiroModels")
                    .unwrap_or(&json!({"models":[{"model_id":"auto"}]}))
            );
            std::process::exit(config["kiroModelsExit"].as_i64().unwrap_or(0) as i32);
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
fn pi_protocol(config: &Value, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, BufRead};
    let text = args.iter().any(|arg| arg == "--no-extensions");
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let command = request["type"].as_str().ok_or("missing request type")?;
        if let Some(log) = config["rpcLog"].as_str() {
            writeln!(
                OpenOptions::new().create(true).append(true).open(log)?,
                "{}",
                json!({"method":command,"textGeneration":text})
            )?;
        }
        let data = match command {
            "get_state" => config.get("piState").cloned().unwrap_or(json!({})),
            "get_available_models" => config
                .get(if text { "piTextModels" } else { "piModels" })
                .cloned()
                .unwrap_or(json!({"models":[]})),
            "get_commands" => config
                .get("piCommands")
                .cloned()
                .unwrap_or(json!({"commands":[]})),
            _ => return Err("fixture refuses provider turns and unknown controls".into()),
        };
        println!(
            "{}",
            json!({"type":"response","id":request["id"],"command":command,"success":true,"data":data})
        );
        io::stdout().flush()?;
    }
    Ok(())
}
fn acp_protocol(config: &Value) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let mut output = std::io::stdout().lock();
    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    while let Some(line) = input.next() {
        let message: Value = serde_json::from_str(&line?)?;
        let Some(method) = message["method"].as_str() else {
            continue;
        };
        if let Some(log) = config["log"].as_str() {
            writeln!(
                OpenOptions::new().create(true).append(true).open(log)?,
                "{}",
                json!([method])
            )?;
        }
        if matches!(method, "session/prompt" | "prompt") {
            return Err("fixture forbids turns".into());
        }
        let mut error = None;
        if method == "initialize"
            && let Some(expected) = config.get("expectedInitializeMeta")
            && message["params"]["_meta"] != *expected
        {
            error = Some(json!({"code":-32602,"message":"unexpected initialize metadata"}));
        }
        let result=match method{
            "initialize"=>config.get("initialize").cloned().unwrap_or_else(||json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"delete":{},"resume":{}},"auth":{"logout":{}}},"authMethods":[{"id":"login","name":"Sign in"}]})),
            "session/new"=>{
                if config["authRequired"]==true{error=Some(json!({"code":-32000,"message":"authentication required; never include response secret"}));}
                else{writeln!(output,"{}",json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review"},{"name":"$skill","description":"Skill"}]}}}))?;}
                config.get("setup").cloned().unwrap_or_else(||json!({"sessionId":"fixture-session","models":{"currentModelId":"model","availableModels":[{"modelId":"model","name":"Model"}]},"configOptions":[]}))
            },
            "authenticate"|"auth/login"=>{
                if config["expectedAuthMethod"].is_string()&&message["params"]["methodId"]!=config["expectedAuthMethod"]{error=Some(json!({"code":-32602,"message":"wrong fixture auth method"}));}
                if let Some(url)=config["loginUrl"].as_str(){
                    writeln!(output,"{}",json!({"jsonrpc":"2.0","id":"fixture-auth-url","method":"session/elicitation","params":{"sessionId":"fixture-session","mode":"url","elicitationId":"auth-url","url":url,"message":"Sign in"}}))?;output.flush()?;
                    let reply:Value=serde_json::from_str(&input.next().ok_or("missing auth consent")??)?;
                    if reply["result"]["action"]["action"]!="accept"{error=Some(json!({"code":-32000,"message":"authentication declined"}));}
                }json!({})
            },
            "session/set_config_option"=>{
                let model = message["params"]["value"].as_str().unwrap_or_default();
                if let Some(options) = config["modelConfigs"].get(model) {
                    writeln!(output,"{}",json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture-session","update":{"sessionUpdate":"config_option_update","configOptions":options}}}))?;
                    json!({"configOptions":options})
                } else {error=Some(json!({"code":-32602,"message":"unknown catalog model"}));json!({})}
            },
            "session/delete"|"logout"|"auth/logout"|"providers/set"|"providers/disable"=>json!({}),
            "session/list"=>config.get("sessions").cloned().unwrap_or_else(||json!({"sessions":[{"sessionId":"one","cwd":"/fixture","title":"One"}],"nextCursor":null})),
            "providers/list"=>config.get("providers").cloned().unwrap_or_else(||json!({"providers":[{"providerId":"openai","supported":["openai"],"required":false,"current":null}]})),
            "_kiro/config/template"=>config.get("kiroTemplate").cloned().unwrap_or_else(||json!({"configOptions":[]})),
            _=>{error=Some(json!({"code":-32601,"message":"unsupported fixture method"}));json!({})}
        };
        if let Some(id) = message.get("id") {
            let response = if let Some(error) = error {
                json!({"jsonrpc":"2.0","id":id,"error":error})
            } else {
                json!({"jsonrpc":"2.0","id":id,"result":result})
            };
            writeln!(output, "{response}")?;
            output.flush()?;
        }
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

async fn opencode_server(
    config: Value,
    args: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port = args
        .iter()
        .find_map(|argument| argument.strip_prefix("--port="))
        .ok_or("missing fixture port")?
        .parse::<u16>()?;
    if !args
        .iter()
        .any(|argument| argument == "--hostname=127.0.0.1")
    {
        return Err("fixture requires loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let generation = config["generation"].as_u64().unwrap_or(2);
    let password = std::env::var(if generation == 2 {
        "OPENCODE_PASSWORD"
    } else {
        "OPENCODE_SERVER_PASSWORD"
    })
    .ok();
    if let Some(path) = config["environmentLog"].as_str() {
        fs::write(path,json!({"hasV2Password":std::env::var_os("OPENCODE_PASSWORD").is_some(),"hasV1Password":std::env::var_os("OPENCODE_SERVER_PASSWORD").is_some(),"emptyConfig":std::env::var("OPENCODE_CONFIG_CONTENT").is_ok_and(|value|value=="{}")}).to_string())?;
    }
    println!("opencode server listening on http://127.0.0.1:{port}");
    std::io::stdout().flush()?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let config = config.clone();
        let password = password.clone();
        tokio::spawn(async move {
            let work = async {
                let mut input = Vec::new();
                let mut bytes = [0; 2048];
                while input.len() < 8192 && !input.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut bytes).await?;
                    if count == 0 {
                        return Ok::<(), std::io::Error>(());
                    }
                    input.extend_from_slice(&bytes[..count]);
                }
                let text = String::from_utf8_lossy(&input);
                let target = text
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("GET "))
                    .and_then(|line| line.split_whitespace().next())
                    .unwrap_or("/unsupported");
                if let Some(log) = config["log"].as_str() {
                    writeln!(
                        OpenOptions::new().create(true).append(true).open(log)?,
                        "{}",
                        json!(["GET", target])
                    )?;
                }
                let authenticated = password.as_ref().is_none_or(|password| {
                    text.lines().any(|line| {
                        line.split_once(':').is_some_and(|(name, value)| {
                            name.eq_ignore_ascii_case("authorization")
                                && value.trim()
                                    == format!(
                                        "Basic {}",
                                        STANDARD.encode(format!("opencode:{password}"))
                                    )
                        })
                    })
                });
                let path = target.split('?').next().unwrap_or_default();
                let (status, body) = if !authenticated {
                    ("401 Unauthorized", json!({}))
                } else {
                    match path {
                        "/api/info" if generation == 2 => (
                            "200 OK",
                            json!({"version":config["version"].as_str().unwrap_or("2.0.28"),"pid":std::process::id(),"urls":[],"paths":{"tmp":"fixture"}}),
                        ),
                        "/global/health" if generation == 1 => (
                            "200 OK",
                            json!({"version":config["version"].as_str().unwrap_or("1.14.19"),"healthy":true}),
                        ),
                        "/api/provider" => {
                            ("200 OK", json!({"data":[{"id":"openai","name":"OpenAI"}]}))
                        }
                        "/api/model" => (
                            "200 OK",
                            json!({"data":[{"providerID":"openai","id":"model","name":"Model","family":"family","variants":[{"id":"medium"}],"time":{"released":1000},"status":"active"}]}),
                        ),
                        "/provider" => (
                            "200 OK",
                            json!({"connected":["openai"],"all":[{"id":"openai","name":"OpenAI","models":{"model":{"id":"model","name":"Model","variants":{"medium":{}},"release_date":"2026-01-01"}}}]}),
                        ),
                        "/agent" => ("200 OK", json!([{"name":"build","mode":"primary"}])),
                        "/api/skill" | "/api/command" => ("200 OK", json!({"data":[]})),
                        "/skill" | "/command" => ("200 OK", json!([])),
                        _ => ("404 Not Found", json!({})),
                    }
                };
                let body = body.to_string();
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await?;
                Ok(())
            };
            let _ = work.await;
        });
    }
}
