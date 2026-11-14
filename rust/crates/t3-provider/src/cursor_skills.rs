use crate::{
    environment::Environment,
    sensitive::{ServiceError, ServiceResult},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use t3_contracts::server::ServerProviderSkill;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFailure {
    Budget,
    Filesystem,
}
pub struct SkillInspection {
    pub skills: Vec<ServerProviderSkill>,
    pub failure: Option<ScanFailure>,
}
struct Budget {
    directories: usize,
    stats: usize,
    bytes: u64,
    exhausted: bool,
    incomplete: bool,
}
impl Budget {
    fn read<T>(&mut self, result: std::io::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    self.incomplete = true;
                }
                None
            }
        }
    }
    fn stat(&mut self, path: &Path) -> Option<fs::Metadata> {
        if self.stats == 0 {
            self.exhausted = true;
            return None;
        }
        self.stats -= 1;
        self.read(fs::metadata(path))
    }
}
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) if value.as_f64() == Some(1.) => Some(true),
        Value::Number(value) if value.as_f64() == Some(0.) => Some(false),
        Value::String(value) => match value.trim().to_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}
fn frontmatter(text: &str) -> Option<Value> {
    let text = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"));
    let Some(text) = text else {
        return Some(json!({}));
    };
    let lines: Vec<_> = text.lines().collect();
    let end = lines.iter().position(|line| *line == "---");
    let Some(end) = end else {
        return Some(json!({}));
    };
    serde_yaml_ng::from_str::<Value>(&lines[..end].join("\n"))
        .ok()
        .filter(Value::is_object)
}
fn skill(directory: &Path, scope: &str, metadata: Value) -> Option<ServerProviderSkill> {
    let surfaces = &metadata["metadata"]["surfaces"];
    let surfaces: Vec<_> = surfaces
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .or_else(|| surfaces.as_str().map(|value| value.split(',').collect()))
        .unwrap_or_default();
    if !surfaces.is_empty()
        && !surfaces
            .iter()
            .any(|surface| surface.trim().eq_ignore_ascii_case("cli"))
    {
        return None;
    }
    let name = directory.file_name()?.to_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let mut value =
        json!({"name":name,"path":directory.join("SKILL.md"),"scope":scope,"enabled":true});
    if let Some(display) = metadata["name"]
        .as_str()
        .map(str::trim)
        .filter(|display| !display.is_empty() && *display != name)
    {
        value["displayName"] = json!(display);
    }
    if let Some(description) = metadata["description"]
        .as_str()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        value["description"] = json!(description);
    }
    if boolean(&metadata["disable-model-invocation"]) == Some(true) {
        value["userInvocationOnly"] = json!(true);
    }
    if boolean(&metadata["user-invocable"]) == Some(false) {
        value["userInvocable"] = json!(false);
    }
    serde_json::from_value(value).ok()
}
fn visit(
    directory: &Path,
    root: &Path,
    scope: &str,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
    budget: &mut Budget,
    output: &mut Vec<ServerProviderSkill>,
) {
    if budget.exhausted {
        return;
    }
    let Some(resolved) = budget.read(fs::canonicalize(directory)) else {
        return;
    };
    if !visited.insert(resolved.clone()) {
        return;
    }
    let path = directory.join("SKILL.md");
    if let Some(info) = budget.stat(&path).filter(|info| info.is_file()) {
        let metadata = if info.len() <= 1_000_000 && info.len() <= budget.bytes {
            match fs::read(&path) {
                Ok(bytes) => {
                    budget.bytes -= info.len();
                    frontmatter(&String::from_utf8_lossy(&bytes))
                }
                Err(_) => Some(json!({})),
            }
        } else {
            Some(json!({}))
        };
        if let Some(skill) = metadata.and_then(|metadata| skill(directory, scope, metadata)) {
            output.push(skill);
        }
    }
    if !resolved.starts_with(root) {
        return;
    }
    let Some(entries) = budget.read(fs::read_dir(directory)) else {
        return;
    };
    let mut entries: Vec<_> = entries
        .filter_map(|entry| budget.read(entry).map(|entry| entry.file_name()))
        .collect();
    entries.sort();
    for name in entries {
        if name == "node_modules" {
            continue;
        }
        let child = directory.join(name);
        let Some(info) = budget.stat(&child) else {
            if budget.exhausted {
                return;
            }
            continue;
        };
        if !info.is_dir() || depth >= 10 {
            continue;
        }
        if budget.directories == 0 {
            budget.exhausted = true;
            return;
        }
        budget.directories -= 1;
        visit(&child, root, scope, depth + 1, visited, budget, output);
    }
}
fn inspect_with_budget(
    cwd: Option<&Path>,
    home: &Path,
    environment: &Environment,
    mut budget: Budget,
) -> SkillInspection {
    let home = environment
        .get("HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            environment
                .get("USERPROFILE")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
        })
        .map(Path::new)
        .unwrap_or(home);
    let mut roots = vec![];
    for (base, scope) in cwd
        .map(|cwd| (cwd, "project"))
        .into_iter()
        .chain([(home, "user")])
    {
        for directory in [
            ".cursor/skills",
            ".agents/skills",
            ".codex/skills",
            ".claude/skills",
        ] {
            roots.push((base.join(directory), scope));
        }
    }
    roots.push((home.join(".cursor/skills-cursor"), "system"));
    let mut scanned = BTreeSet::new();
    let mut skills = BTreeMap::new();
    for (path, scope) in roots {
        if budget.exhausted {
            break;
        }
        let Some(root) = budget.read(fs::canonicalize(path)) else {
            continue;
        };
        if !scanned.insert(root.clone()) {
            continue;
        }
        let mut found = vec![];
        visit(
            &root,
            &root,
            scope,
            0,
            &mut BTreeSet::new(),
            &mut budget,
            &mut found,
        );
        for skill in found {
            skills.entry(skill.name.clone()).or_insert(skill);
        }
    }
    SkillInspection {
        skills: skills.into_values().collect(),
        failure: if budget.exhausted {
            Some(ScanFailure::Budget)
        } else if budget.incomplete {
            Some(ScanFailure::Filesystem)
        } else {
            None
        },
    }
}
pub fn inspect(cwd: Option<&Path>, home: &Path, environment: &Environment) -> SkillInspection {
    inspect_with_budget(
        cwd,
        home,
        environment,
        Budget {
            directories: 10_000,
            stats: 100_000,
            bytes: 8_000_000,
            exhausted: false,
            incomplete: false,
        },
    )
}
pub fn probe(
    cwd: Option<&Path>,
    home: &Path,
    environment: &Environment,
) -> ServiceResult<Vec<ServerProviderSkill>> {
    let inspection = inspect(cwd, home, environment);
    if inspection.failure.is_some() {
        Err(ServiceError::Response)
    } else {
        Ok(inspection.skills)
    }
}
