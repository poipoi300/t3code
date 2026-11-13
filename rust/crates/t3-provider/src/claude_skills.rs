use crate::environment::{Environment, Platform, absolute_path, expand_home_path};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};
use t3_contracts::server::ServerProviderSkill;
use unicode_normalization::UnicodeNormalization;

static COMMENTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"("(?:[^"\\]|\\.)*")|//[^\r\n]*|/\*[\s\S]*?\*/"#).expect("JSON comments")
});
static COMMAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"("(?:[^"\\]|\\.)*")|,(\s*[}\]])"#).expect("JSON commas"));
static FRONT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)").expect("frontmatter"));
static SCALAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([\w-]+:[ \t]*)([^\r\n]*)").expect("YAML scalar"));
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$").expect("UUID")
});
pub fn parse_lenient_json(text: &str) -> Option<Value> {
    let stripped = COMMENTS.replace_all(text, |c: &regex::Captures<'_>| {
        c.get(1).map_or(" ", |m| m.as_str()).to_owned()
    });
    let stripped = COMMAS.replace_all(&stripped, |c: &regex::Captures<'_>| {
        c.get(1)
            .or_else(|| c.get(2))
            .map_or("", |m| m.as_str())
            .to_owned()
    });
    serde_json::from_str(&stripped).ok()
}
fn read(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return None;
    }
    Some(String::from_utf8_lossy(&fs::read(path).ok()?).into_owned())
}
fn strict(path: &Path) -> Option<Value> {
    serde_json::from_str(&read(path)?).ok()
}
fn lenient(path: &Path) -> Option<Value> {
    parse_lenient_json(&read(path)?)
}
fn text(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(number) => match number.as_f64() {
            Some(1.) => Some(true),
            Some(0.) => Some(false),
            _ => None,
        },
        Value::String(value) => match value.trim().to_lowercase().as_str() {
            "true" | "yes" | "on" | "y" => Some(true),
            "false" | "no" | "off" | "n" => Some(false),
            _ => None,
        },
        _ => None,
    }
}
pub fn frontmatter(contents: &str) -> Option<Value> {
    let Some(captures) = FRONT.captures(contents) else {
        return Some(json!({}));
    };
    let raw = &captures[1];
    let parse = |raw: &str| {
        serde_yaml_ng::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object)
    };
    if let Some(value) = parse(raw) {
        return Some(value);
    }
    let repaired = raw
        .lines()
        .map(|line| {
            let Some(captures) = SCALAR.captures(line) else {
                return line.to_owned();
            };
            let value = &captures[2];
            let comment = value
                .char_indices()
                .find(|(i, c)| {
                    *c == '#'
                        && *i > 0
                        && value[..*i].chars().last().is_some_and(char::is_whitespace)
                })
                .map(|(i, _)| i);
            let end = comment
                .map(|i| value[..i].trim_end().len())
                .unwrap_or(value.len());
            let scalar = &value[..end];
            let reserved = scalar
                .chars()
                .next()
                .is_some_and(|c| "\"'[]{}|>&*!#%@`".contains(c))
                || scalar.starts_with(['-', '?', ':'])
                    && scalar.chars().nth(1).is_none_or(char::is_whitespace);
            if reserved || !scalar.contains(": ") && !scalar.contains(":\t") {
                return line.to_owned();
            }
            format!(
                "{}{}{}",
                &captures[1],
                serde_json::to_string(scalar).unwrap_or_default(),
                &value[end..]
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    parse(&repaired)
}
pub fn launch_directories(args: &str) -> Vec<String> {
    let mut collecting = false;
    let mut output = vec![];
    for token in t3_codex_app_server::launch::tokenize_cli_args(args) {
        if token == "--" {
            break;
        }
        if token == "--add-dir" {
            collecting = true;
        } else if let Some(directory) = token.strip_prefix("--add-dir=") {
            output.push(directory.into());
            collecting = false;
        } else if token.starts_with('-') {
            collecting = false;
        } else if collecting {
            output.push(token);
        }
    }
    output
}
fn canonical(path: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
            return Some(PathBuf::from(format!("\\\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix("\\\\?\\") {
            return Some(PathBuf::from(rest));
        }
    }
    Some(path)
}
fn repository(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_path_buf)
}
fn plugin_repository(path: &Path) -> Option<PathBuf> {
    let root = repository(path)?;
    let git = root.join(".git");
    let Some(contents) = read(&git).filter(|s| s.trim().starts_with("gitdir:")) else {
        return Some(root);
    };
    let directory = absolute_path(
        Path::new(contents.trim().trim_start_matches("gitdir:").trim()),
        &root,
    );
    let common = read(&directory.join("commondir")).unwrap_or_default();
    let backlink = read(&directory.join("gitdir")).unwrap_or_default();
    if common.trim().is_empty() || backlink.trim().is_empty() {
        return Some(root);
    }
    let common = absolute_path(Path::new(common.trim()), &directory);
    if directory.parent() != Some(common.join("worktrees").as_path())
        || canonical(&git).is_none()
        || canonical(&absolute_path(Path::new(backlink.trim()), &directory)) != canonical(&git)
    {
        return Some(root);
    }
    Some(if common.file_name().is_some_and(|s| s == ".git") {
        common.parent()?.to_path_buf()
    } else {
        common
    })
}
fn managed_settings(platform: Platform, environment: &Environment) -> Option<PathBuf> {
    match platform {
        Platform::Macos => {
            Some("/Library/Application Support/ClaudeCode/managed-settings.json".into())
        }
        Platform::Windows => environment
            .get("PROGRAMDATA")
            .map(|v| PathBuf::from(v).join("ClaudeCode/managed-settings.json")),
        _ => Some("/etc/claude-code/managed-settings.json".into()),
    }
}
fn settings_paths(
    config: &Path,
    cwd: Option<&Path>,
    platform: Platform,
    environment: &Environment,
) -> Vec<PathBuf> {
    let mut paths = vec![config.join("settings.json")];
    if let Some(cwd) = cwd {
        paths.extend([
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
        ]);
        if let Some(root) = repository(cwd).filter(|p| p != cwd) {
            paths.push(root.join(".claude/settings.local.json"));
        }
    }
    paths.extend(managed_settings(platform, environment));
    paths
}
pub fn settings_environment(
    config: &Path,
    platform: Platform,
    environment: &Environment,
) -> BTreeMap<String, Value> {
    let mut output = BTreeMap::new();
    for path in settings_paths(config, None, platform, environment) {
        if let Some(env) = lenient(&path).and_then(|v| v["env"].as_object().cloned()) {
            if env.iter().any(|(k, v)| {
                matches!(k.as_str(), "ANTHROPIC_BASE_URL" | "CLAUDE_CODE_ENTRYPOINT")
                    && !v.is_string()
            }) {
                continue;
            }
            for (k, v) in env {
                if matches!(
                    k.as_str(),
                    "ANTHROPIC_BASE_URL"
                        | "CLAUDE_CODE_ENTRYPOINT"
                        | "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"
                        | "CLAUDE_CODE_GATEWAY_MODEL_DISCOVERY_TIMEOUT_MS"
                ) {
                    output.insert(k, v);
                }
            }
        }
    }
    output
}
#[derive(Clone)]
struct Root {
    directory: PathBuf,
    scope: &'static str,
    prefix: Option<String>,
}
fn plugin_roots(
    path: &Path,
    scope: &'static str,
    prefix: String,
    manifest: Option<&Value>,
) -> Vec<Root> {
    let mut output = vec![];
    let default = path.join("skills");
    if default.exists() {
        output.push(Root {
            directory: default.clone(),
            scope,
            prefix: Some(prefix.clone()),
        });
    }
    let declared = manifest.and_then(|m| m.get("skills"));
    if declared.is_none() && !default.exists() && path.join("SKILL.md").exists() {
        output.push(Root {
            directory: path.into(),
            scope,
            prefix: Some(prefix.clone()),
        });
    }
    let values: Vec<_> = match declared {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    for relative in values {
        let directory = absolute_path(Path::new(relative), path);
        if !directory.starts_with(path) {
            continue;
        }
        if let (Some(install), Some(real)) = (canonical(path), canonical(&directory))
            && real.starts_with(install)
        {
                output.push(Root {
                    directory,
                    scope,
                    prefix: Some(prefix.clone()),
                });
        }
    }
    output
}
fn plugin_manifest(path: &Path) -> Option<Value> {
    let value = strict(&path.join(".claude-plugin/plugin.json"))?;
    if value
        .get("name")
        .is_some_and(|v| v.as_str().is_none_or(str::is_empty))
        || value.get("skills").is_some_and(|v| {
            !v.is_string() && !v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
        })
    {
        return None;
    }
    Some(value)
}
fn key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}
fn plugin_roots_for(
    config: &Path,
    cwd: Option<&Path>,
    home: &Path,
    environment: &Environment,
    enabled: &BTreeMap<String, bool>,
    bucket: Option<&str>,
) -> Vec<Root> {
    let plugins = environment
        .get("CLAUDE_CODE_PLUGIN_CACHE_DIR")
        .filter(|v| !v.is_empty())
        .map(|v| absolute_path(Path::new(&expand_home_path(v, home)), cwd.unwrap_or(home)))
        .unwrap_or_else(|| config.join("plugins"));
    let installed = strict(&plugins.join("installed_plugins.json")).unwrap_or(Value::Null);
    let mut output = vec![];
    let mut claimed = BTreeSet::new();
    let workspace = cwd.map(|p| canonical(p).unwrap_or_else(|| p.into()));
    let repository = workspace.as_deref().and_then(plugin_repository);
    let mut records: Vec<_> = installed["plugins"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    records.sort_by_key(|(k, _)| *k);
    for (id, entries) in records {
        if enabled.get(id) != Some(&true) {
            continue;
        }
        let applicable: Vec<_> = entries
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| {
                let scope = r["scope"].as_str().unwrap_or_default();
                if !matches!(scope, "user" | "managed" | "project" | "local")
                    || r["installPath"].as_str().is_none_or(str::is_empty)
                {
                    return false;
                }
                if matches!(scope, "user" | "managed") {
                    return true;
                }
                let (Some(project), Some(workspace)) =
                    (text(&r["projectPath"]), workspace.as_ref())
                else {
                    return false;
                };
                let project = absolute_path(Path::new(project), cwd.unwrap_or(home));
                project == *workspace
                    || repository
                        .as_ref()
                        .is_some_and(|root| plugin_repository(&project).as_ref() == Some(root))
            })
            .collect();
        let existing = applicable.iter().find(|r| {
            absolute_path(
                Path::new(r["installPath"].as_str().unwrap_or_default()),
                cwd.unwrap_or(home),
            )
            .exists()
        });
        let Some(record) = existing.copied().or_else(|| applicable.first().copied()) else {
            continue;
        };
        let path = absolute_path(
            Path::new(record["installPath"].as_str().unwrap_or_default()),
            cwd.unwrap_or(home),
        );
        let manifest = plugin_manifest(&path);
        let prefix = manifest
            .as_ref()
            .and_then(|v| v["name"].as_str())
            .or_else(|| id.split('@').next())
            .unwrap_or_default();
        if prefix.is_empty() {
            continue;
        }
        if existing.is_some() {
            claimed.insert(key(prefix));
        }
        output.extend(plugin_roots(
            &path,
            if matches!(record["scope"].as_str(), Some("user" | "managed")) {
                "user"
            } else {
                "project"
            },
            prefix.into(),
            manifest.as_ref(),
        ));
    }
    let Some(bucket) = bucket else {
        return output;
    };
    let synced = config.join("plugins/synced");
    if !synced.join(format!(".bucket-{bucket}")).exists() {
        return output;
    }
    let directory = synced.join(bucket);
    let manifest = strict(&directory.join("manifest.json")).unwrap_or(Value::Null);
    let disabled: BTreeSet<_> = enabled
        .iter()
        .filter_map(|(name, enabled)| {
            (!enabled)
                .then(|| name.strip_suffix("@synced").map(key))
                .flatten()
        })
        .collect();
    for row in manifest["plugins"].as_array().into_iter().flatten() {
        let (Some(_), Some(name)) = (row["pluginId"].as_str(), row["name"].as_str()) else {
            continue;
        };
        if row.get("generation").is_some_and(|v| !v.is_number()) {
            continue;
        }
        let name = name
            .chars()
            .map(|c| if "<>:\"|?*\\/".contains(c) { '_' } else { c })
            .collect::<String>()
            .trim_end_matches(['.', ' '])
            .to_owned();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let generation = row["generation"].as_u64().filter(|v| *v >= 2);
        let path = directory.join(
            generation
                .map(|g| format!("{name}~g{g}"))
                .unwrap_or_else(|| name.clone()),
        );
        let manifest = plugin_manifest(&path);
        let prefix = manifest
            .as_ref()
            .and_then(|m| m["name"].as_str())
            .unwrap_or(&name);
        let key = key(prefix);
        if claimed.contains(&key) || disabled.contains(&key) {
            continue;
        }
        claimed.insert(key);
        output.extend(plugin_roots(
            &path,
            "user",
            prefix.into(),
            manifest.as_ref(),
        ));
    }
    output
}
pub struct ClaudeSkillContext<'a> {
    pub config: &'a Path,
    pub custom_home: bool,
    pub cwd: Option<&'a Path>,
    pub home: &'a Path,
    pub environment: &'a Environment,
    pub platform: Platform,
    pub launch_args: &'a str,
}
pub fn discover(context: ClaudeSkillContext<'_>) -> Vec<ServerProviderSkill> {
    let ClaudeSkillContext {
        config,
        custom_home,
        cwd,
        home,
        environment,
        platform,
        launch_args,
    } = context;
    let mut overrides = BTreeMap::<String, String>::new();
    let mut plugins = BTreeMap::new();
    let mut hidden = false;
    for path in settings_paths(config, cwd, platform, environment) {
        let Some(value) = lenient(&path) else {
            continue;
        };
        if value.get("skillOverrides").is_some_and(|v| {
            !v.as_object().is_some_and(|m| {
                m.values().all(|v| {
                    matches!(
                        v.as_str(),
                        Some("on" | "name-only" | "user-invocable-only" | "off")
                    )
                })
            })
        }) || value.get("enabledPlugins").is_some_and(|v| !v.is_object())
        {
            continue;
        }
        for (k, v) in value["skillOverrides"].as_object().into_iter().flatten() {
            if let Some(v) = v.as_str() {
                overrides.insert(k.clone(), v.into());
            }
        }
        for (k, v) in value["enabledPlugins"].as_object().into_iter().flatten() {
            if let Some(v) = v.as_bool() {
                plugins.insert(k.clone(), v);
            } else {
                plugins.remove(k);
            }
        }
        if value["syncClaudeAiPlugins"] == false
            && cwd.is_none_or(|cwd| path != cwd.join(".claude/settings.json"))
        {
            hidden = true;
        }
    }
    let bucket = if !hidden && config.join("plugins/synced").exists() {
        let account_path = if custom_home
            || environment
                .get("CLAUDE_CONFIG_DIR")
                .is_some_and(|s| !s.trim().is_empty())
        {
            config.join(".claude.json")
        } else {
            home.join(".claude.json")
        };
        let account = strict(&account_path).and_then(|v| v.get("oauthAccount").cloned());
        let organization = environment
            .get("CLAUDE_CODE_ORGANIZATION_UUID")
            .map(String::as_str)
            .or_else(|| {
                account
                    .as_ref()
                    .and_then(|a| a["organizationUuid"].as_str())
            });
        let account_id = if account.is_some()
            && !environment
                .get("CLAUDE_CODE_OAUTH_TOKEN")
                .is_some_and(|s| !s.is_empty())
        {
            account.as_ref().and_then(|a| a["accountUuid"].as_str())
        } else {
            Some(
                environment
                    .get("CLAUDE_CODE_ACCOUNT_UUID")
                    .filter(|s| UUID.is_match(s))
                    .map_or("unbound", String::as_str),
            )
        };
        match (organization, account_id) {
            (Some(org), Some(account))
                if UUID.is_match(org) && (account == "unbound" || UUID.is_match(account)) =>
            {
                Some(format!("{}_{}", org.to_lowercase(), account.to_lowercase()))
            }
            _ => None,
        }
    } else {
        None
    };
    let mut roots = vec![Root {
        directory: config.join("skills"),
        scope: "user",
        prefix: None,
    }];
    if let Some(cwd) = cwd {
        roots.push(Root {
            directory: cwd.join(".claude/skills"),
            scope: "project",
            prefix: None,
        });
    }
    for directory in launch_directories(launch_args) {
        if !directory.is_empty() && (cwd.is_some() || Path::new(&directory).is_absolute()) {
            roots.push(Root {
                directory: absolute_path(Path::new(&directory), cwd.unwrap_or(home))
                    .join(".claude/skills"),
                scope: "project",
                prefix: None,
            });
        }
    }
    roots.extend(plugin_roots_for(
        config,
        cwd,
        home,
        environment,
        &plugins,
        bucket.as_deref(),
    ));
    let mut skills = BTreeMap::new();
    for root in roots {
        let single = root.prefix.is_some() && root.directory.join("SKILL.md").exists();
        let mut entries: Vec<String> = if single {
            root.directory
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .into_iter()
                .collect()
        } else {
            fs::read_dir(&root.directory)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect()
        };
        entries.sort();
        for entry in entries {
            let path = if single {
                root.directory.join("SKILL.md")
            } else {
                root.directory.join(&entry).join("SKILL.md")
            };
            let Some(contents) = read(&path) else {
                continue;
            };
            let Some(front) = frontmatter(&contents) else {
                continue;
            };
            if entry.trim().is_empty() {
                continue;
            }
            let mut name = entry.trim().to_owned();
            if let Some(prefix) = root.prefix.as_ref() {
                let declared = if single {
                    text(&front["name"]).unwrap_or_default()
                } else {
                    ""
                };
                let declared = declared
                    .strip_prefix(&format!("{prefix}:"))
                    .unwrap_or(declared);
                let tail = if declared.is_empty() {
                    entry.as_str()
                } else {
                    declared
                };
                name = format!(
                    "{prefix}:{}",
                    tail.chars()
                        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                            c
                        } else {
                            '-'
                        })
                        .collect::<String>()
                );
                if !name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
                    || !name.chars().any(|c| c.is_ascii_alphabetic())
                {
                    continue;
                }
            }
            if skills.contains_key(&name) {
                continue;
            }
            let setting = if root.prefix.is_some() {
                None
            } else {
                overrides.get(&name).map(String::as_str)
            };
            let mut skill = json!({"name":name,"path":path.to_string_lossy(),"scope":root.scope,"enabled":setting!=Some("off")});
            if let Some(description) = text(&front["description"]) {
                skill["description"] = json!(description);
            }
            if boolean(&front["disable-model-invocation"]) == Some(true)
                || setting == Some("user-invocable-only")
            {
                skill["userInvocationOnly"] = json!(true);
            }
            if boolean(&front["user-invocable"]) == Some(false) {
                skill["userInvocable"] = json!(false);
            }
            if let Ok(skill) = serde_json::from_value(skill) {
                skills.insert(name, skill);
            }
        }
    }
    skills.into_values().collect()
}
