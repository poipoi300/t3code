use crate::sensitive::{ServiceError, ServiceResult};
use serde_json::json;
use std::{collections::BTreeSet, fs, io::Read, path::Path};
use t3_contracts::{server::ServerProviderSkill, wire::trim_js};
struct Budget {
    bytes: usize,
    entries: usize,
}
fn read(path: &Path, budget: &mut Budget) -> ServiceResult<Option<String>> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ServiceError::Storage),
    };
    if !file
        .metadata()
        .map_err(|_| ServiceError::Storage)?
        .is_file()
    {
        return Ok(None);
    }
    let limit = 1_000_000.min(budget.bytes);
    let mut bytes = vec![];
    let (mut dashes, mut markers) = (0, 0);
    let mut end = None;
    loop {
        let mut buffer = vec![0; 1024.min(limit + 1 - bytes.len())];
        if buffer.is_empty() {
            break;
        }
        let count = file.read(&mut buffer).map_err(|_| ServiceError::Storage)?;
        if count == 0 {
            break;
        }
        buffer.truncate(count);
        let offset = bytes.len();
        for (index, byte) in buffer.iter().enumerate() {
            dashes = if *byte == b'-' { dashes + 1 } else { 0 };
            if dashes == 3 {
                dashes = 0;
                markers += 1;
                if markers == 2 {
                    end = Some(offset + index + 1);
                    break;
                }
            }
        }
        bytes.extend(buffer);
        if end.is_some() {
            break;
        }
    }
    budget.bytes = budget.bytes.saturating_sub(bytes.len());
    if end.unwrap_or(bytes.len()) > limit {
        return Err(ServiceError::Response);
    }
    bytes.truncate(end.unwrap_or(bytes.len()));
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}
fn key(name: &str) -> String {
    let mut output = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~;/@&=+$,:".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}
fn metadata(contents: &str, file: &str) -> Option<(String, Option<String>)> {
    let start = contents.find("---")? + 3;
    let end = start + contents[start..].find("---")?;
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(contents[start..end].trim()).ok()?;
    if !yaml.is_null() && !yaml.is_mapping() {
        return None;
    }
    let field = |key: &str| {
        let value = &yaml[key];
        if value.is_null() {
            Some(None)
        } else {
            value.as_str().map(Some)
        }
    };
    let name = field("name")?
        .filter(|name| !name.is_empty())
        .unwrap_or(file.strip_suffix(".md")?);
    let description = field("description")?
        .map(trim_js)
        .filter(|description| !description.is_empty())
        .map(str::to_owned);
    (!name.is_empty() && trim_js(name) == name).then(|| (name.into(), description))
}
fn scan(
    root: &Path,
    scope: &str,
    children: bool,
    budget: &mut Budget,
    seen: &mut BTreeSet<String>,
    skills: &mut Vec<ServerProviderSkill>,
) -> ServiceResult<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(_) => return Err(ServiceError::Storage),
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ServiceError::Storage)?;
    if entries.len() > budget.entries {
        return Err(ServiceError::Response);
    }
    budget.entries -= entries.len();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().into_owned());
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.file_name().to_string_lossy().to_lowercase() == "skill.md")
    {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !file.ends_with(".md") {
            return Ok(());
        }
        let Some(contents) = read(&entry.path(), budget)? else {
            return Ok(());
        };
        let Some((name, description)) = metadata(&contents, &file) else {
            return Ok(());
        };
        if !seen.insert(name.clone()) {
            return Ok(());
        }
        let mut value = json!({"name":name,"path":entry.path(),"scope":scope,"enabled":true});
        if let Some(description) = description {
            value["description"] = json!(description);
        }
        skills.push(serde_json::from_value(value).map_err(|_| ServiceError::Response)?);
        return Ok(());
    }
    if children {
        entries.sort_by_key(|entry| key(&entry.file_name().to_string_lossy()));
        for entry in entries {
            scan(&entry.path(), scope, false, budget, seen, skills)?;
        }
    }
    Ok(())
}
pub fn discover(cwd: &Path, home: &Path) -> ServiceResult<Vec<ServerProviderSkill>> {
    let mut budget = Budget {
        bytes: 8_000_000,
        entries: 10_000,
    };
    let mut seen = BTreeSet::new();
    let mut skills = vec![];
    for (root, scope) in [
        (home.join(".gemini/config/skills"), "user"),
        (cwd.join(".gemini/skills"), "project"),
        (home.join(".gemini/antigravity-cli/skills"), "user"),
        (cwd.join(".agents/skills"), "project"),
        (cwd.join(".agent/skills"), "project"),
    ] {
        scan(&root, scope, true, &mut budget, &mut seen, &mut skills)?;
    }
    skills.sort_by(|left, right| left.name.0.cmp(&right.name.0));
    Ok(skills)
}
