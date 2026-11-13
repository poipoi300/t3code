use crate::{Error, Result};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone)]
pub struct ServicePlan {
    pub program: Vec<String>,
    pub base_dir: String,
    pub log_path: String,
    pub install_bin_dir: Option<String>,
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn systemd(value: &str) -> String {
    let escaped = value.replace('%', "%%");
    if escaped
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}
pub fn render_systemd(plan: &ServicePlan) -> Result<String> {
    if plan.program.is_empty()
        || plan.program[0]
            .chars()
            .any(|c| c.is_control() || matches!(c, '\'' | '"' | '\\'))
    {
        return Err(Error::Refused("unsupported service executable path"));
    }
    let mut lines = vec![
        "[Unit]".into(),
        "Description=T3 Code server".into(),
        "StartLimitIntervalSec=300".into(),
        "StartLimitBurst=5".into(),
        "".into(),
        "[Service]".into(),
        "Type=simple".into(),
        "WorkingDirectory=%h".into(),
        format!("Environment=T3CODE_HOME={}", systemd(&plan.base_dir)),
    ];
    if let Some(bin) = &plan.install_bin_dir {
        lines.push(format!(
            "Environment=T3CODE_INSTALL_BIN_DIR={}",
            systemd(bin)
        ));
    }
    lines.extend([
        "Environment=T3_BOOT_SERVICE_UNIT=t3code.service".into(),
        format!(
            "ExecStart={}",
            plan.program
                .iter()
                .map(|s| systemd(s))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "KillMode=mixed".into(),
        "OOMPolicy=continue".into(),
        "Restart=always".into(),
        "RestartSec=5".into(),
        "CPUWeight=1000".into(),
        "IOWeight=1000".into(),
        "MemoryLow=1536M".into(),
        "OOMScoreAdjust=100".into(),
        "ManagedOOMPreference=avoid".into(),
        format!("StandardOutput=append:{}", plan.log_path.replace('%', "%%")),
        format!("StandardError=append:{}", plan.log_path.replace('%', "%%")),
        "".into(),
        "[Install]".into(),
        "WantedBy=default.target".into(),
        "".into(),
    ]);
    Ok(lines.join("\n"))
}
pub fn render_launchd(plan: &ServicePlan, home: &str, path: &str) -> String {
    let mut lines=vec!["<?xml version=\"1.0\" encoding=\"UTF-8\"?>".into(),"<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".into(),"<plist version=\"1.0\">".into(),"<dict>".into(),"  <key>Label</key>".into(),"  <string>com.t3tools.t3code.service</string>".into(),"  <key>ProgramArguments</key>".into(),"  <array>".into()];
    lines.extend(
        plan.program
            .iter()
            .map(|a| format!("    <string>{}</string>", xml(a))),
    );
    lines.extend([
        "  </array>".into(),
        "  <key>EnvironmentVariables</key>".into(),
        "  <dict>".into(),
        "    <key>PATH</key>".into(),
        format!("    <string>{}</string>", xml(path)),
        "    <key>T3CODE_HOME</key>".into(),
        format!("    <string>{}</string>", xml(&plan.base_dir)),
    ]);
    if let Some(bin) = &plan.install_bin_dir {
        lines.extend([
            "    <key>T3CODE_INSTALL_BIN_DIR</key>".into(),
            format!("    <string>{}</string>", xml(bin)),
        ]);
    }
    lines.extend([
        "    <key>T3_BOOT_SERVICE_UNIT</key>".into(),
        "    <string>com.t3tools.t3code.service.plist</string>".into(),
        "  </dict>".into(),
        "  <key>WorkingDirectory</key>".into(),
        format!("  <string>{}</string>", xml(home)),
        "  <key>RunAtLoad</key>".into(),
        "  <true/>".into(),
        "  <key>KeepAlive</key>".into(),
        "  <true/>".into(),
        "  <key>ThrottleInterval</key>".into(),
        "  <integer>5</integer>".into(),
        "  <key>ExitTimeOut</key>".into(),
        "  <integer>90</integer>".into(),
        "  <key>ProcessType</key>".into(),
        "  <string>Interactive</string>".into(),
        "  <key>StandardOutPath</key>".into(),
        format!("  <string>{}</string>", xml(&plan.log_path)),
        "  <key>StandardErrorPath</key>".into(),
        format!("  <string>{}</string>", xml(&plan.log_path)),
        "</dict>".into(),
        "</plist>".into(),
        "".into(),
    ]);
    lines.join("\n")
}
fn windows_argument(value: &str) -> Result<String> {
    if value.contains(['"', '\0', '\r', '\n']) {
        return Err(Error::Invalid);
    }
    let trailing = value.chars().rev().take_while(|c| *c == '\\').count();
    Ok(format!("\"{value}{}\"", "\\".repeat(trailing)))
}
pub fn render_windows_task(
    plan: &ServicePlan,
    user: &str,
    home: &str,
    conhost: &str,
) -> Result<String> {
    let mut arguments = plan.program.clone();
    arguments.extend([
        "--base-dir".into(),
        plan.base_dir.clone(),
        "--log-file".into(),
        plan.log_path.clone(),
        "--service-unit".into(),
        "t3code-task.xml".into(),
    ]);
    let arguments = arguments
        .iter()
        .map(|a| windows_argument(a))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    Ok(format!(
        "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n  <RegistrationInfo>\n    <Description>T3 Code server</Description>\n  </RegistrationInfo>\n  <Triggers>\n    <LogonTrigger>\n      <UserId>{user}</UserId>\n    </LogonTrigger>\n    <TimeTrigger>\n      <StartBoundary>2000-01-01T00:00:00</StartBoundary>\n      <Repetition>\n        <Interval>PT1M</Interval>\n      </Repetition>\n    </TimeTrigger>\n  </Triggers>\n  <Principals>\n    <Principal id=\"Author\">\n      <UserId>{user}</UserId>\n      <LogonType>InteractiveToken</LogonType>\n      <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n  </Principals>\n  <Settings>\n    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n    <Priority>5</Priority>\n  </Settings>\n  <Actions Context=\"Author\">\n    <Exec>\n      <Command>{conhost}</Command>\n      <Arguments>--headless {arguments}</Arguments>\n      <WorkingDirectory>{home}</WorkingDirectory>\n    </Exec>\n  </Actions>\n</Task>\n",
        user = xml(user),
        conhost = xml(conhost),
        arguments = xml(&arguments),
        home = xml(home)
    ))
}
pub fn service_task_name(domain: &str, user: &str) -> String {
    format!("t3code-{domain}-{user}")
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceState {
    pub protocol: u8,
    pub active_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<ServiceUpdate>,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ServiceUpdate {
    #[serde(rename_all = "camelCase")]
    Pending {
        id: String,
        from_version: String,
        target_version: String,
        db_path: String,
    },
    #[serde(rename_all = "camelCase")]
    Committed {
        id: String,
        from_version: String,
        target_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RolledBack {
        id: String,
        from_version: String,
        target_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        id: String,
        from_version: String,
        target_version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}
pub fn is_exact_service_version(version: &str) -> bool {
    static PATTERN: std::sync::LazyLock<std::result::Result<regex::Regex, regex::Error>> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:0|[1-9]\d*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
            )
        });
    PATTERN
        .as_ref()
        .is_ok_and(|pattern| pattern.is_match(version))
}
pub fn compare_exact_service_versions(left: &str, right: &str) -> Result<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if !is_exact_service_version(left) || !is_exact_service_version(right) {
        return Err(Error::Invalid);
    }
    fn parts(version: &str) -> (&str, Option<&str>) {
        let plain = version.split('+').next().unwrap_or(version);
        plain
            .split_once('-')
            .map_or((plain, None), |(core, pre)| (core, Some(pre)))
    }
    fn numeric(a: &str, b: &str) -> Ordering {
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    }
    let (a, ap) = parts(left);
    let (b, bp) = parts(right);
    for (a, b) in a.split('.').zip(b.split('.')) {
        let order = numeric(a, b);
        if order != Ordering::Equal {
            return Ok(order);
        }
    }
    let (ap, bp) = match (ap, bp) {
        (None, None) => return Ok(Ordering::Equal),
        (None, Some(_)) => return Ok(Ordering::Greater),
        (Some(_), None) => return Ok(Ordering::Less),
        (Some(a), Some(b)) => (a, b),
    };
    let mut a = ap.split('.');
    let mut b = bp.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ok(Ordering::Equal),
            (None, Some(_)) => return Ok(Ordering::Less),
            (Some(_), None) => return Ok(Ordering::Greater),
            (Some(a), Some(b)) => {
                let an = a.bytes().all(|b| b.is_ascii_digit());
                let bn = b.bytes().all(|b| b.is_ascii_digit());
                let order = match (an, bn) {
                    (true, true) => numeric(a, b),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    _ => a.cmp(b),
                };
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
        }
    }
}
pub fn parse_service_state(raw: &str) -> Result<ServiceState> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|_| Error::Refused("service state is invalid or unsupported"))?;
    let mut state: ServiceState = serde_json::from_value(value.clone())
        .map_err(|_| Error::Refused("service state is invalid or unsupported"))?;
    if state.protocol == 2
        && value.as_object().is_some_and(|m| m.len() == 2)
        && state.update.is_none()
        && is_exact_service_version(&state.active_version)
    {
        state.protocol = 4;
        return Ok(state);
    }
    if !matches!(state.protocol, 3 | 4)
        || !is_exact_service_version(&state.active_version)
        || value.get("update").is_some_and(serde_json::Value::is_null)
    {
        return Err(Error::Refused("service state is invalid or unsupported"));
    }
    if let Some(update) = &state.update {
        let (id, from, to, expected, reason) = match update {
            ServiceUpdate::Pending {
                id,
                from_version,
                target_version,
                db_path,
            } => {
                if db_path.trim().is_empty() {
                    return Err(Error::Invalid);
                }
                (id, from_version, target_version, from_version, None)
            }
            ServiceUpdate::Committed {
                id,
                from_version,
                target_version,
                reason,
            } => (
                id,
                from_version,
                target_version,
                target_version,
                reason.as_ref(),
            ),
            ServiceUpdate::RolledBack {
                id,
                from_version,
                target_version,
                reason,
            }
            | ServiceUpdate::Failed {
                id,
                from_version,
                target_version,
                reason,
            } => (
                id,
                from_version,
                target_version,
                from_version,
                reason.as_ref(),
            ),
        };
        if id.trim().is_empty()
            || reason.is_some_and(|s| s.trim().is_empty())
            || expected != &state.active_version
            || compare_exact_service_versions(to, from)? != std::cmp::Ordering::Greater
        {
            return Err(Error::Refused("service state is contradictory"));
        }
    }
    state.protocol = 4;
    Ok(state)
}
pub fn read_service_state(path: &Path) -> Result<ServiceState> {
    parse_service_state(&fs::read_to_string(path).map_err(|_| Error::Io)?)
}
pub fn write_service_state(path: &Path, state: &ServiceState) -> Result<()> {
    use std::io::Write;
    let encoded = t3_contracts::stored::to_value(state).map_err(|_| Error::Invalid)?;
    let contents = serde_json::to_string_pretty(&encoded).map_err(|_| Error::Invalid)? + "\n";
    parse_service_state(&contents)?;
    let parent = path.parent().ok_or(Error::Invalid)?;
    fs::create_dir_all(parent).map_err(|_| Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|_| Error::Io)?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Io)?;
    temp.write_all(contents.as_bytes()).map_err(|_| Error::Io)?;
    temp.as_file().sync_all().map_err(|_| Error::Io)?;
    temp.persist(path).map_err(|_| Error::Io)?;
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .map_err(|_| Error::Io)?
            .sync_all()
            .map_err(|_| Error::Io)?;
    }
    Ok(())
}
pub fn service_task_is_disabled(xml: &str) -> bool {
    xml.split_once("<Settings>")
        .and_then(|(_, tail)| tail.split_once("</Settings>"))
        .is_some_and(|(settings, _)| settings.contains("<Enabled>false</Enabled>"))
}
pub fn disabled_service_task(xml: &str) -> String {
    if service_task_is_disabled(xml) {
        xml.into()
    } else {
        xml.replacen(
            "  </Settings>",
            "    <Enabled>false</Enabled>\n  </Settings>",
            1,
        )
    }
}
pub fn read_service_environment(
    base: &Path,
    inherited: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let raw = match fs::read_to_string(base.join("service.env")) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(inherited.clone()),
        Err(_) => return Err(Error::Io),
    };
    let mut result = inherited.clone();
    for (name, value) in parse_environment(&raw) {
        let name = name.trim();
        if name.is_empty()
            || !name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
            || [
                "PATH",
                "T3CODE_HOME",
                "T3_BOOT_SERVICE_UNIT",
                "T3_SERVICE_LAUNCHER_CONTEXT",
            ]
            .contains(&name.to_ascii_uppercase().as_str())
        {
            continue;
        }
        result.insert(name.into(), value);
    }
    Ok(result)
}
fn parse_environment(raw: &str) -> Vec<(String, String)> {
    let raw = raw.replace("\r\n", "\n");
    let mut remaining = raw.as_str();
    let mut entries = Vec::new();
    while !remaining.is_empty() {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }
        if remaining.starts_with('#') {
            remaining = remaining.split_once('\n').map_or("", |(_, rest)| rest);
            continue;
        }
        let line_end = remaining.find('\n').unwrap_or(remaining.len());
        let line = &remaining[..line_end];
        let Some(equals) = line.find('=') else {
            remaining = remaining.get(line_end + 1..).unwrap_or("");
            continue;
        };
        let name = line[..equals]
            .trim()
            .strip_prefix("export ")
            .unwrap_or(line[..equals].trim())
            .trim()
            .to_owned();
        let value = remaining[equals + 1..].trim_start_matches([' ', '\t']);
        let quote = value
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"' | '`'));
        if let Some(quote) = quote
            && let Some(end) = value[1..].find(quote)
        {
            let contents = &value[1..end + 1];
            let contents = if quote == '"' {
                contents.replace("\\n", "\n")
            } else {
                contents.to_owned()
            };
            entries.push((name, contents));
            remaining = value[end + 2..]
                .split_once('\n')
                .map_or("", |(_, rest)| rest);
        } else {
            let (end, rest) = value.split_once('\n').unwrap_or((value, ""));
            entries.push((name, end.split('#').next().unwrap_or("").trim().to_owned()));
            remaining = rest;
        }
    }
    entries
}
