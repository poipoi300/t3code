//! The `t3` command surface and foreground launcher.
pub mod bootstrap;
use crate::adapters::cli as local;
use crate::adapters::config::{ConfigInputs, RuntimeMode, ServerFlags};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{collections::BTreeMap, ffi::OsString, io::Write, path::PathBuf};

pub const UNAVAILABLE: u8 = 69;
/// `SERVER_EXIT_CODE_STATE_DIR_OWNED` in contracts' desktopBootstrap.ts.
pub const STATE_DIR_OWNED: u8 = 78;
/// `SERVER_EXIT_CODE_MANAGED_POLICY`: the desktop shell reports it at once instead of retrying.
pub const MANAGED_POLICY: u8 = 77;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("{0} is not yet available in the Rust server")]
    Unavailable(String),
    #[error(transparent)]
    Server(#[from] crate::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Unavailable(_) => UNAVAILABLE,
            Self::Server(crate::Error::StateDirOwned(_)) => STATE_DIR_OWNED,
            Self::Server(crate::Error::ManagedPolicy(_)) => MANAGED_POLICY,
            _ => 1,
        }
    }
}

fn value(name: &'static str) -> Arg {
    Arg::new(name).long(name)
}
fn boolean(name: &'static str) -> Arg {
    value(name)
        .value_parser(clap::builder::BoolishValueParser::new())
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value("true")
}
fn positional(name: &'static str) -> Arg {
    Arg::new(name).required(true)
}
fn integer(name: &'static str) -> Arg {
    value(name).value_parser(clap::value_parser!(i64))
}
fn location(command: Command) -> Command {
    command.arg(value("base-dir")).arg(value("home-dir"))
}
fn auth_location(command: Command) -> Command {
    location(command).arg(value("dev-url").value_parser(parse_url))
}
fn scope() -> Arg {
    value("scope").action(ArgAction::Append).value_parser([
        "orchestration:read",
        "orchestration:operate",
        "settings:write",
        "providers:manage",
        "environment:maintain",
        "preview:operate",
        "diagnostics:read",
        "terminal:read",
        "terminal:operate",
        "filesystem:read",
        "filesystem:write",
        "source-control:write",
        "access:read",
        "access:write",
        "relay:read",
        "relay:write",
    ])
}
fn duration(name: &'static str) -> Arg {
    value(name).value_parser(parse_duration)
}
fn server_command(name: &'static str, bootstrap: bool) -> Command {
    let command = location(Command::new(name))
        .arg(Arg::new("cwd"))
        .arg(value("mode").value_parser(["web", "desktop"]))
        .arg(value("port").value_parser(clap::value_parser!(u16)))
        .arg(value("host"))
        .arg(value("dev-url").value_parser(parse_url))
        .arg(boolean("no-browser"))
        .arg(integer("bootstrap-fd"))
        .arg(boolean("log-websocket-events").visible_alias("log-ws-events"))
        .arg(boolean("tailscale-serve"))
        .arg(value("tailscale-serve-port").value_parser(clap::value_parser!(u16)))
        .arg(boolean("lan"))
        .arg(boolean("no-lan").conflicts_with("lan"));
    if bootstrap {
        command.arg(boolean("auto-bootstrap-project-from-cwd"))
    } else {
        command
    }
}
fn yes() -> Arg {
    boolean("yes").short('y')
}

/// Declarations follow binCli.ts and cli/, including the hidden process helpers.
pub fn command() -> Command {
    server_command("t3", true)
        .version(env!("CARGO_PKG_VERSION"))
        .propagate_version(true)
        .about("Run T3 Code in this terminal. Use `t3 up` for remote access.")
        .subcommand_precedence_over_arg(true)
        .disable_help_subcommand(true)
        .arg(value("log-level").global(true).value_parser([
            "all", "trace", "debug", "info", "warn", "warning", "error", "fatal", "none", "All",
            "Trace", "Debug", "Info", "Warning", "Error", "Fatal", "None",
        ]))
        .arg(value("completions").global(true).value_parser([
            "bash",
            "zsh",
            "fish",
            "powershell",
            "sh",
            "pwsh",
        ]))
        .arg(boolean("wizard").global(true))
        .subcommand(Command::new("help"))
        .subcommand(server_command("start", true).hide(true))
        .subcommand(
            server_command("serve", false)
                .about("Run in the foreground and print pairing details."),
        )
        .subcommand(location(Command::new("app")).arg(Arg::new("path")))
        .subcommand(
            location(Command::new("pair"))
                .arg(scope())
                .arg(duration("ttl"))
                .arg(value("label"))
                .arg(boolean("tailscale"))
                .arg(
                    value("tailscale-serve-port")
                        .value_parser(clap::value_parser!(u16))
                        .default_value("443"),
                ),
        )
        .subcommand(
            Command::new("auth")
                .subcommand(
                    Command::new("pairing")
                        .subcommand(
                            auth_location(Command::new("create"))
                                .arg(scope())
                                .arg(duration("ttl"))
                                .arg(value("label"))
                                .arg(value("base-url").value_parser(parse_http_url))
                                .arg(boolean("json")),
                        )
                        .subcommand(auth_location(Command::new("list")).arg(boolean("json")))
                        .subcommand(auth_location(Command::new("revoke")).arg(positional("id"))),
                )
                .subcommand(
                    Command::new("session")
                        .subcommand(
                            auth_location(Command::new("issue"))
                                .arg(scope())
                                .arg(duration("ttl"))
                                .arg(value("label"))
                                .arg(value("subject"))
                                .arg(boolean("token-only"))
                                .arg(boolean("json")),
                        )
                        .subcommand(auth_location(Command::new("list")).arg(boolean("json")))
                        .subcommand(
                            auth_location(Command::new("revoke")).arg(positional("session-id")),
                        ),
                ),
        )
        .subcommand(
            Command::new("environment")
                .subcommand(
                    auth_location(Command::new("link"))
                        .arg(positional("target-url").value_name("url"))
                        .arg(value("url").action(ArgAction::Append))
                        .arg(value("pairing-code").required(true))
                        .arg(
                            value("access")
                                .value_parser([
                                    "read-only",
                                    "approval-required",
                                    "auto-accept-edits",
                                    "auto",
                                    "full-access",
                                ])
                                .default_value("approval-required"),
                        ),
                )
                .subcommand(auth_location(Command::new("list")).arg(boolean("json")))
                .subcommand(
                    auth_location(Command::new("unlink")).arg(positional("environment-id")),
                ),
        )
        .subcommand(Command::new("browser").subcommand(location(Command::new("setup"))))
        .subcommand(
            Command::new("project")
                .subcommand(
                    location(Command::new("add"))
                        .arg(positional("path"))
                        .arg(value("title")),
                )
                .subcommand(
                    location(Command::new("remove"))
                        .arg(positional("project"))
                        .arg(boolean("force")),
                )
                .subcommand(
                    location(Command::new("rename"))
                        .arg(positional("project"))
                        .arg(positional("title")),
                ),
        )
        .subcommand(
            Command::new("thread")
                .subcommand(
                    location(Command::new("list"))
                        .arg(value("project").default_value("."))
                        .arg(boolean("json")),
                )
                .subcommand(
                    location(Command::new("start"))
                        .arg(positional("project"))
                        .arg(positional("message"))
                        .arg(value("title"))
                        .arg(value("provider"))
                        .arg(value("model"))
                        .arg(value("worktree"))
                        .arg(value("branch"))
                        .arg(boolean("wait"))
                        .arg(boolean("json")),
                )
                .subcommand(
                    location(Command::new("send"))
                        .arg(positional("thread"))
                        .arg(positional("message"))
                        .arg(boolean("wait"))
                        .arg(boolean("json")),
                )
                .subcommand(
                    location(Command::new("show"))
                        .arg(positional("thread"))
                        .arg(integer("messages").default_value("10"))
                        .arg(boolean("json")),
                ),
        )
        .subcommand(
            location(Command::new("up"))
                .arg(boolean("connect"))
                .arg(boolean("no-connect").conflicts_with("connect"))
                .arg(boolean("lan"))
                .arg(boolean("no-lan").conflicts_with("lan"))
                .arg(boolean("yes"))
                .arg(boolean("headless")),
        )
        .subcommand(location(Command::new("status")).arg(boolean("json")))
        .subcommand(
            location(Command::new("connect"))
                .arg(boolean("headless"))
                .subcommand(location(Command::new("login")).arg(boolean("headless")))
                .subcommand(
                    location(Command::new("link"))
                        .arg(boolean("headless"))
                        .arg(boolean("publish-only")),
                )
                .subcommand(location(Command::new("status")).arg(boolean("json")))
                .subcommand(location(Command::new("publish")).arg(boolean("disable")))
                .subcommand(location(Command::new("unlink")))
                .subcommand(location(Command::new("logout"))),
        )
        .subcommand(
            Command::new("service")
                .subcommand(location(Command::new("install")).arg(boolean("allow-downgrade")))
                .subcommand(location(Command::new("update")).arg(boolean("allow-downgrade")))
                .subcommand(location(Command::new("restart")))
                .subcommand(location(Command::new("uninstall")))
                .subcommand(location(Command::new("status")))
                .subcommand(
                    location(Command::new("protect"))
                        .arg(boolean("undo"))
                        .arg(boolean("yes")),
                ),
        )
        .subcommand(
            location(Command::new("update"))
                .arg(Arg::new("release-version").value_name("version"))
                .arg(value("channel").value_parser(["stable", "nightly", "preview"]))
                .arg(boolean("allow-downgrade"))
                .arg(yes()),
        )
        .subcommand(Command::new("install"))
        .subcommand(location(Command::new("uninstall")).arg(yes()))
        .subcommand(
            location(Command::new("doctor"))
                .arg(boolean("fix"))
                .arg(boolean("dry-run")),
        )
        .subcommand(
            Command::new("policy")
                .subcommand(location(Command::new("show")))
                .subcommand(Command::new("check").arg(positional("file")))
                .subcommand(
                    Command::new("templates")
                        .arg(positional("directory"))
                        .arg(value("policy")),
                ),
        )
        .subcommand(
            Command::new("theme")
                .subcommand(
                    location(Command::new("set"))
                        .arg(positional("theme"))
                        .arg(value("id")),
                )
                .subcommand(location(Command::new("clear")))
                .subcommand(location(Command::new("show"))),
        )
        .subcommand(
            Command::new("trace").subcommand(
                location(Command::new("summary"))
                    .arg(duration("since"))
                    .arg(
                        integer("limit")
                            .value_parser(clap::value_parser!(i64).range(1..))
                            .default_value("25"),
                    ),
            ),
        )
        .subcommand(
            location(Command::new("triage"))
                .arg(value("agent").value_parser(["claude", "codex"]))
                .arg(value("model")),
        )
        .subcommand(Command::new("acp-mcp-bridge").hide(true))
        .subcommand(
            Command::new("acp-mcp-call")
                .hide(true)
                .arg(positional("tool"))
                .arg(positional("arguments-json")),
        )
        .subcommand(
            location(Command::new("__service-launcher").hide(true))
                .arg(value("log-file"))
                .arg(value("service-unit")),
        )
        .subcommand(
            Command::new("__managed-chatgpt-token")
                .hide(true)
                .arg(positional("endpoint"))
                .arg(positional("instance-id"))
                .arg(positional("environment-id"))
                .arg(positional("profile-key")),
        )
        .subcommand(
            Command::new("__document-text")
                .hide(true)
                .arg(positional("path"))
                .arg(positional("extension")),
        )
        .subcommand(
            Command::new("__service-preflight")
                .hide(true)
                .arg(value("database-path").required(true))
                .arg(integer("launcher-protocol").required(true)),
        )
        .subcommand(
            Command::new("__speech-worker")
                .hide(true)
                .arg(
                    positional("worker-mode")
                        .value_name("mode")
                        .value_parser(["discover", "model"]),
                )
                .arg(Arg::new("module-url")),
        )
        .subcommand(
            Command::new("__claude-history")
                .hide(true)
                .arg(positional("method"))
                .arg(positional("session-id"))
                .arg(Arg::new("options")),
        )
        .subcommand(
            Command::new("__ssh-helper")
                .hide(true)
                .subcommand(
                    Command::new("pick-port")
                        .arg(positional("port-file"))
                        .arg(positional("default-port").value_parser(clap::value_parser!(i64)))
                        .arg(positional("scan-window").value_parser(clap::value_parser!(i64))),
                )
                .subcommand(
                    Command::new("wait-ready")
                        .arg(positional("port").value_parser(clap::value_parser!(i64)))
                        .arg(positional("timeout-ms").value_parser(clap::value_parser!(i64)))
                        .arg(positional("probe-timeout-ms").value_parser(clap::value_parser!(i64))),
                )
                .subcommand(Command::new("runtime-port").arg(positional("runtime-file")))
                .subcommand(Command::new("update-remaining").arg(positional("marker-file"))),
        )
}

fn parse_url(raw: &str) -> Result<String, String> {
    url::Url::parse(raw)
        .map(|_| raw.to_owned())
        .map_err(|e| e.to_string())
}
fn parse_http_url(raw: &str) -> Result<String, String> {
    let url = url::Url::parse(raw).map_err(|e| e.to_string())?;
    if matches!(url.scheme(), "http" | "https") {
        Ok(raw.to_owned())
    } else {
        Err("expected an http:// or https:// URL".into())
    }
}
fn parse_duration(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let amount: f64 = raw[..split]
        .parse()
        .map_err(|_| "expected a duration, for example 5m or 15 minutes")?;
    let multiplier = match raw[split..].trim() {
        "ms" | "milli" | "millis" | "millisecond" | "milliseconds" => 1.0,
        "s" | "sec" | "second" | "seconds" => 1_000.0,
        "m" | "min" | "minute" | "minutes" => 60_000.0,
        "h" | "hour" | "hours" => 3_600_000.0,
        "d" | "day" | "days" => 86_400_000.0,
        "w" | "week" | "weeks" => 604_800_000.0,
        _ => return Err("expected a duration unit, for example 5m or 15 minutes".into()),
    };
    let milliseconds = amount * multiplier;
    if !milliseconds.is_finite() || milliseconds < 0.0 || milliseconds >= i64::MAX as f64 {
        return Err("duration is out of range".into());
    }
    Ok(milliseconds as i64)
}
fn text<'a>(matches: &'a ArgMatches, name: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(name)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn flag(matches: &ArgMatches, name: &str) -> Option<bool> {
    matches.try_get_one::<bool>(name).ok().flatten().copied()
}
fn number<T: Clone + Send + Sync + 'static>(matches: &ArgMatches, name: &str) -> Option<T> {
    matches.try_get_one::<T>(name).ok().flatten().cloned()
}

fn normalize_booleans(args: impl Iterator<Item = OsString>) -> Result<Vec<OsString>, CliError> {
    const FLAGS: &[&str] = &[
        "no-browser",
        "auto-bootstrap-project-from-cwd",
        "log-websocket-events",
        "log-ws-events",
        "tailscale-serve",
        "lan",
        "json",
        "tailscale",
        "token-only",
        "force",
        "wait",
        "connect",
        "yes",
        "headless",
        "publish-only",
        "disable",
        "allow-downgrade",
        "undo",
        "fix",
        "dry-run",
        "wizard",
    ];
    let mut args = args.peekable();
    let mut result = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--" {
            result.push(arg);
            result.extend(args);
            break;
        }
        let raw = arg.to_str().unwrap_or("");
        if raw == "-v" {
            result.push(OsString::from("--version"));
            continue;
        }
        let name = raw
            .strip_prefix("--")
            .or_else(|| (raw == "-y").then_some("yes"));
        if let Some(name) = name {
            if let Some(positive) = name.strip_prefix("no-").filter(|name| FLAGS.contains(name)) {
                if args
                    .peek()
                    .and_then(|a| a.to_str())
                    .is_some_and(|value| local::parse_bool(value).is_some())
                {
                    return Err(CliError::Usage(format!(
                        "Negated flag --{name} does not accept a value"
                    )));
                }
                result.push(format!("--{positive}=false").into());
                continue;
            }
            if FLAGS.contains(&name) {
                let value = args
                    .peek()
                    .and_then(|a| a.to_str())
                    .and_then(local::parse_bool);
                if let Some(value) = value {
                    args.next();
                    result.push(format!("--{name}={value}").into());
                    continue;
                }
            }
        }
        result.push(arg);
    }
    Ok(result)
}

pub enum Invocation {
    Serve(Box<ConfigInputs>),
    Pair {
        inputs: Box<ConfigInputs>,
        scopes: Vec<t3_contracts::auth::AuthEnvironmentScope>,
        ttl: Option<i64>,
        label: String,
    },
    Status {
        inputs: Box<ConfigInputs>,
        json: bool,
    },
    Printed,
}

/// Help, syntax errors and unavailable commands do not open a home or start services.
pub fn parse(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Invocation, CliError> {
    let args = normalize_booleans(args.into_iter().map(Into::into))?;
    let matches =
        match command().try_get_matches_from(std::iter::once(OsString::from("t3")).chain(args)) {
            Ok(matches) => matches,
            Err(error)
                if matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
            {
                error.print()?;
                return Ok(Invocation::Printed);
            }
            Err(error) => return Err(CliError::Usage(error.to_string())),
        };
    if text(&matches, "completions").is_some() {
        return Err(CliError::Unavailable("Shell completions".into()));
    }
    if flag(&matches, "wizard") == Some(true) {
        return Err(CliError::Unavailable("Interactive CLI wizard".into()));
    }
    let (name, leaf) = matches.subcommand().unwrap_or(("t3", &matches));
    if name == "help" {
        command().print_long_help()?;
        println!();
        return Ok(Invocation::Printed);
    }
    if !matches!(name, "t3" | "start" | "serve" | "pair" | "status") {
        let mut path = format!("t3 {name}");
        let mut nested = leaf;
        while let Some((name, child)) = nested.subcommand() {
            path.push(' ');
            path.push_str(name);
            nested = child;
        }
        return Err(CliError::Unavailable(path));
    }
    if name == "pair" && flag(leaf, "tailscale") == Some(true) {
        return Err(CliError::Unavailable("Tailscale pairing".into()));
    }
    let pick = |key| {
        text(leaf, key)
            .or_else(|| text(&matches, key))
            .map(str::to_owned)
    };
    let pick_flag = |key| flag(leaf, key).or_else(|| flag(&matches, key));
    let flags = ServerFlags {
        mode: pick("mode").map(|s| {
            if s == "desktop" {
                RuntimeMode::Desktop
            } else {
                RuntimeMode::Web
            }
        }),
        port: number(leaf, "port").or_else(|| number(&matches, "port")),
        host: pick("host"),
        base_dir: pick("base-dir"),
        cwd: pick("cwd"),
        dev_url: pick("dev-url"),
        no_browser: pick_flag("no-browser"),
        bootstrap_fd: number(leaf, "bootstrap-fd").or_else(|| number(&matches, "bootstrap-fd")),
        auto_bootstrap_project_from_cwd: pick_flag("auto-bootstrap-project-from-cwd"),
        log_websocket_events: pick_flag("log-websocket-events"),
        tailscale_serve_enabled: pick_flag("tailscale-serve"),
        tailscale_serve_port: number(leaf, "tailscale-serve-port")
            .or_else(|| number(&matches, "tailscale-serve-port")),
        lan: if pick_flag("no-lan") == Some(true) {
            Some(false)
        } else {
            pick_flag("lan")
        },
    };
    let inputs = inputs(flags, pick("home-dir"), name, pick("log-level"))?;
    if name == "t3"
        && let Some(cwd) = &inputs.flags.cwd
    {
        let cwd = cwd.trim();
        let explicit = cwd == "."
            || cwd == ".."
            || cwd == "~"
            || cwd.contains(['/', '\\'])
            || (cfg!(windows) && cwd.as_bytes().get(1) == Some(&b':'));
        if !explicit && !inputs.process_cwd.join(cwd).is_dir() {
            return Err(CliError::Usage(format!(
                "Unknown command {cwd:?}. Use \"t3 --help\" for commands or an explicit path such as \"t3 ./my-project\" for a new directory."
            )));
        }
    }
    let resolved = crate::adapters::config::resolve(&inputs)?;
    match name {
        "pair" => {
            let mut scopes = leaf
                .get_many::<String>("scope")
                .map(|values| {
                    values
                        .map(|s| serde_json::from_value(serde_json::json!(s)))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(|e| CliError::Usage(e.to_string()))?
                .unwrap_or_else(local::standard_scopes);
            let mut unique = Vec::new();
            scopes.retain(|scope| {
                if unique.contains(scope) {
                    false
                } else {
                    unique.push(*scope);
                    true
                }
            });
            Ok(Invocation::Pair {
                inputs: Box::new(inputs),
                scopes,
                ttl: number(leaf, "ttl"),
                label: pick("label").unwrap_or_else(|| "t3 pair".into()),
            })
        }
        "status" => Ok(Invocation::Status {
            inputs: Box::new(inputs),
            json: flag(leaf, "json").unwrap_or(false),
        }),
        _ => {
            if resolved.private_network_only || resolved.tailscale_serve_enabled {
                return Err(CliError::Unavailable(
                    "LAN fencing and Tailscale Serve".into(),
                ));
            }
            Ok(Invocation::Serve(Box::new(inputs)))
        }
    }
}

fn inputs(
    flags: ServerFlags,
    home_dir: Option<String>,
    command: &str,
    cli_log_level: Option<String>,
) -> Result<ConfigInputs, CliError> {
    let headless = matches!(command, "serve" | "pair" | "status");
    let env: BTreeMap<_, _> = std::env::vars().collect();
    let home = if cfg!(windows) {
        env.get("USERPROFILE").or_else(|| env.get("HOME"))
    } else {
        env.get("HOME")
    }
    .map(PathBuf::from)
    .ok_or_else(|| CliError::Usage("account home unavailable".into()))?;
    let mut inputs = ConfigInputs {
        flags,
        env,
        home,
        process_cwd: std::env::current_dir()?,
        bootstrap: None,
        headless,
        force_auto_bootstrap: headless.then_some(false),
        service_managed: false,
        cli_log_level,
        automatic_port: None,
        persisted_observability: BTreeMap::new(),
        static_dir: crate::adapters::config::discover_static_dir(),
    };
    if command != "pair" {
        hydrate_bootstrap(&mut inputs)?;
    }
    if inputs.bootstrap.is_some() {
        if let Some(home) = home_dir.filter(|home| !home.trim().is_empty()) {
            inputs.flags.base_dir = Some(home);
        }
    } else {
        local::select_home(&mut inputs, home_dir.as_deref());
    }
    if command == "pair" {
        inputs.flags = ServerFlags {
            base_dir: inputs.flags.base_dir.take(),
            ..Default::default()
        };
        inputs.env.clear();
    } else {
        local::load_observability(&mut inputs)?;
    }
    Ok(inputs)
}

/// Consume the descriptor once, before resolving paths or opening a T3 home.
pub(crate) fn hydrate_bootstrap(inputs: &mut ConfigInputs) -> crate::Result<()> {
    let fd = inputs
        .flags
        .bootstrap_fd
        .or_else(|| inputs.env.get("T3CODE_BOOTSTRAP_FD")?.parse().ok());
    if let Some(fd) = fd {
        if let Some(envelope) = bootstrap::read(fd)? {
            inputs.bootstrap = Some(envelope);
        }
        inputs.flags.bootstrap_fd = None;
        inputs.env.remove("T3CODE_BOOTSTRAP_FD");
    }
    Ok(())
}

pub async fn run(invocation: Invocation) -> Result<(), CliError> {
    match invocation {
        Invocation::Printed => Ok(()),
        Invocation::Serve(inputs) => {
            let config = crate::adapters::config::resolve(&inputs)?;
            let headless = inputs.headless;
            let server = crate::runtime::start_with_global_tracing(*inputs).await?;
            if !server.wait_ready().await {
                server.wait().await?;
                return Err(CliError::Usage(
                    "Server stopped before becoming ready".into(),
                ));
            }
            if !headless && config.mode == RuntimeMode::Desktop {
                server.run_until_signal().await?;
                return Ok(());
            }
            let result = headless_output(
                config.host.as_deref(),
                server.address.port(),
                &server.pairing_credential,
            )
            .and_then(|output| {
                let mut stdout = std::io::stdout().lock();
                writeln!(stdout, "{output}")?;
                // `announceStartupPairingUrl`: a browser-presentation dev server pairs through
                // the dev URL, which the dev runner and its scripts read from this line.
                if !headless && let Some(dev) = &config.dev_url {
                    let pairing = pairing_url(dev.as_str(), &server.pairing_credential)?;
                    writeln!(stdout, "  pairingUrl: {pairing}")?;
                }
                stdout.flush()?;
                Ok(())
            });
            if let Err(error) = result {
                server.shutdown().await?;
                return Err(error);
            }
            server.run_until_signal().await?;
            Ok(())
        }
        Invocation::Pair {
            inputs,
            scopes,
            ttl,
            label,
        } => {
            let paired = local::pair(*inputs, scopes, ttl, label).await?;
            let server_origin = connection_string(paired.host.as_deref(), paired.port);
            let base = paired
                .dev_url
                .clone()
                .unwrap_or_else(|| server_origin.clone());
            let pairing = pairing_url(&base, &paired.credential)?;
            println!(
                "Pairing with {} ({}).\n\n{}\n\nPairing URL: {pairing}\nApp link: {}\nToken: {}\nExpires: {}",
                paired.label,
                paired.origin,
                qr(&pairing)?,
                app_link(&pairing),
                paired.credential,
                paired.expires_at
            );
            // TypeScript pairs a dev server through the dev URL only, which works for clients
            // that reach the server through the dev URL's proxy. The desktop and mobile apps pair
            // with the server itself, so a dev server also prints its own origin.
            if paired.dev_url.is_some() {
                println!(
                    "Server pairing URL: {}",
                    pairing_url(&server_origin, &paired.credential)?
                );
            }
            if url::Url::parse(&base)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .is_some_and(|host| {
                    host == "localhost"
                        || host.starts_with("127.")
                        || host == "[::1]"
                        || host == "::1"
                })
            {
                println!(
                    "\nNote: This URL is only reachable from this machine. To reach it from your other devices, run `t3 up`, or re-run with --tailscale."
                );
            }
            println!();
            Ok(())
        }
        Invocation::Status { inputs, json } => {
            let status = local::status(*inputs)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status)
                        .map_err(|e| CliError::Usage(e.to_string()))?
                );
            } else {
                let access = status
                    .lan
                    .host_override
                    .as_ref()
                    .map(|host| format!("overridden · T3CODE_HOST={host}"))
                    .unwrap_or_else(|| {
                        if status.lan.enabled {
                            "on · pairing code only".into()
                        } else {
                            "off".into()
                        }
                    });
                println!(
                    "This machine\n  {:15}unavailable\n  {:15}{access}",
                    "Service", "Local network"
                );
                if status.lan.host_override.is_none() && status.lan.enabled {
                    println!(
                        "\nAnyone on your LAN or tailnet can reach the port. A pairing code is the only lock."
                    );
                }
                println!("\nChange access: `t3 up`.");
            }
            Ok(())
        }
    }
}

fn pairing_url(base: &str, token: &str) -> Result<String, CliError> {
    let mut url = url::Url::parse(base).map_err(|e| CliError::Usage(e.to_string()))?;
    url.set_path("/pair");
    let query: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "token")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.set_fragment(Some(
        &url::form_urlencoded::Serializer::new(String::new())
            .append_pair("token", token)
            .finish(),
    ));
    Ok(url.to_string())
}
fn app_link(pairing_url: &str) -> String {
    format!(
        "t3code://pair?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("pairingUrl", pairing_url)
            .finish()
    )
}
fn qr(value: &str) -> Result<String, CliError> {
    let code = qrcodegen::QrCode::encode_text(value, qrcodegen::QrCodeEcc::Medium)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let mut rows = Vec::new();
    for y in (-2..code.size() + 2).step_by(2) {
        let mut row = String::new();
        for x in -2..code.size() + 2 {
            row.push(match (code.get_module(x, y), code.get_module(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        rows.push(row);
    }
    Ok(rows.join("\n"))
}
fn connection_string(host: Option<&str>, port: u16) -> String {
    let host = match host {
        None | Some("") => "localhost".to_owned(),
        Some("0.0.0.0" | "::" | "[::]") => {
            let addresses = if_addrs::get_if_addrs().unwrap_or_default();
            addresses
                .iter()
                .find(|a| !a.is_loopback() && a.ip().is_ipv4())
                .or_else(|| addresses.iter().find(|a| !a.is_loopback()))
                .map_or_else(|| "localhost".into(), |a| a.ip().to_string())
        }
        Some(host) => host.trim_matches(['[', ']']).to_owned(),
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    format!("http://{host}:{port}")
}
pub fn headless_output(host: Option<&str>, port: u16, token: &str) -> Result<String, CliError> {
    let connection = connection_string(host, port);
    let pairing = pairing_url(&connection, token)?;
    Ok(format!(
        "T3 Code server is ready.\nConnection string: {connection}\nToken: {token}\nPairing URL: {pairing}\nApp link: {}\n\n{}\n",
        app_link(&pairing),
        qr(&pairing)?
    ))
}
