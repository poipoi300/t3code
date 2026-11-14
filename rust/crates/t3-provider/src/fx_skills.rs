use crate::environment::Environment;
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use t3_contracts::server::ServerProviderSkill;
const MAX_HEADER: usize = 65_536;
fn control(value: &str) -> bool {
    value
        .chars()
        .any(|character| character < ' ' || character == '\u{7f}')
}
fn trim(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !control(value)
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\'])
}
fn scalar(value: &str) -> Option<&str> {
    if value.starts_with(['|', '>']) {
        return None;
    }
    if value.starts_with(['\'', '"']) || value.ends_with(['\'', '"']) {
        if value.len() < 2 || value.as_bytes().first() != value.as_bytes().last() {
            return None;
        }
        Some(&value[1..value.len() - 1])
    } else {
        Some(value)
    }
}
fn decode(bytes: &[u8]) -> String {
    bytes
        .split(|byte| *byte == b'\n')
        .map(|line| match std::str::from_utf8(line) {
            Ok(line) => line.to_owned(),
            Err(_) => format!("{}\0", String::from_utf8_lossy(line)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn metadata(bytes: &[u8], folder: &str) -> Option<(String, Option<String>)> {
    let contents = decode(bytes);
    let opening = if contents.starts_with("---\r\n") {
        5
    } else if contents.starts_with("---\n") {
        4
    } else if contents == "---" {
        3
    } else {
        return valid_name(folder).then(|| (folder.into(), None));
    };
    let lines = contents[opening..].split('\n').collect::<Vec<_>>();
    let closing = lines.iter().enumerate().position(|(index, line)| {
        if index == lines.len() - 1 {
            *line == "---"
        } else {
            line.strip_suffix('\r').unwrap_or(line) == "---"
        }
    })?;
    let size = opening
        + lines[..=closing]
            .iter()
            .map(|line| line.len())
            .sum::<usize>()
        + closing
        + usize::from(closing < lines.len() - 1);
    if size > MAX_HEADER {
        return None;
    }
    let header = lines[..closing]
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    let mut name = None;
    let mut description = None;
    let mut saw_name = false;
    let mut saw_description = false;
    let mut previous = false;
    let mut index = 0;
    while index < header.len() {
        let line = header[index];
        let trimmed = trim(line);
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        if previous && line.starts_with([' ', '\t']) {
            return None;
        }
        previous = false;
        if let Some((key, raw)) = trimmed.split_once(':') {
            let (key, raw) = (trim(key), trim(raw));
            match key {
                "name" => {
                    if saw_name {
                        return None;
                    }
                    saw_name = true;
                    name = Some(scalar(raw)?.to_owned());
                    previous = true;
                }
                "description" => {
                    if saw_description {
                        return None;
                    }
                    saw_description = true;
                    if matches!(raw, ">" | ">-" | "|") {
                        let mut block: Vec<Option<&str>> = vec![];
                        let mut base = None;
                        while index + 1 < header.len() {
                            let next = header[index + 1];
                            if trim(next).is_empty() {
                                block.push(None);
                                index += 1;
                                continue;
                            }
                            let indent = next.bytes().take_while(|byte| *byte == b' ').count();
                            if next.as_bytes().get(indent) == Some(&b'\t') {
                                return None;
                            }
                            if indent == 0 {
                                break;
                            }
                            if base.is_some_and(|base| indent < base) {
                                return None;
                            }
                            let base = *base.get_or_insert(indent);
                            let content = &next[base..];
                            if control(content) {
                                return None;
                            }
                            block.push(Some(content));
                            index += 1;
                        }
                        let last = block.iter().rposition(Option::is_some);
                        let mut text = String::new();
                        if let Some(last) = last {
                            for index in 0..=last {
                                if index > 0 {
                                    text.push(
                                        if raw != "|"
                                            && block[index - 1].is_some()
                                            && block[index].is_some()
                                        {
                                            ' '
                                        } else {
                                            '\n'
                                        },
                                    );
                                }
                                text.push_str(block[index].unwrap_or_default());
                            }
                        }
                        description = Some(t3_contracts::wire::trim_js(&text).to_owned());
                    } else {
                        let value = scalar(raw)?;
                        if control(value) {
                            return None;
                        }
                        description = Some(value.to_owned());
                        previous = true;
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    let name = name.filter(|name| valid_name(name))?;
    Some((
        name,
        description.filter(|description| !description.is_empty()),
    ))
}
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .map(|path| {
            let text = path.to_string_lossy();
            if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
                PathBuf::from(format!("\\\\{rest}"))
            } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
                PathBuf::from(rest)
            } else {
                path
            }
        })
        .unwrap_or_else(|_| path.into())
}
struct Root {
    directory: PathBuf,
    scope: &'static str,
    rank: u8,
    links: bool,
}
fn roots(cwd: &Path, home: &Path) -> Vec<Root> {
    let mut roots = vec![];
    for ancestor in cwd.ancestors().take_while(|ancestor| *ancestor != home) {
        for (relative, rank) in [
            (".fx/skills", 1),
            ("skills", 1),
            (".opencode/skills", 2),
            (".codex/skills", 2),
            (".claude/skills", 2),
            (".agents/skills", 2),
            (".claw/skills", 2),
        ] {
            roots.push(Root {
                directory: ancestor.join(relative),
                scope: "project",
                rank,
                links: true,
            });
        }
    }
    for (relative, rank, links) in [
        (".fx/skills", 0, false),
        (".config/opencode/skills", 2, true),
        (".codex/skills", 2, true),
        (".claude/skills", 2, true),
        (".agents/skills", 2, true),
        (".claw/skills", 2, true),
    ] {
        roots.push(Root {
            directory: home.join(relative),
            scope: "user",
            rank,
            links,
        });
    }
    roots
}
pub fn discover(cwd: &Path, environment: &Environment) -> Vec<ServerProviderSkill> {
    let Some(home) = environment
        .get("HOME")
        .map(|home| t3_contracts::wire::trim_js(home))
        .filter(|home| !home.is_empty())
    else {
        return vec![];
    };
    let home = canonical(Path::new(home));
    let cwd = canonical(cwd);
    let mut folders = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut found = vec![];
    let link = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    };
    for root in roots(&cwd, &home) {
        if !root.links && canonical(&root.directory) != root.directory {
            continue;
        }
        let mut entries = fs::read_dir(&root.directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        });
        for entry in entries {
            let folder = entry.path();
            let path = folder.join("SKILL.md");
            if !root.links && (link(&folder) || link(&path))
                || !fs::metadata(&folder).is_ok_and(|metadata| metadata.is_dir())
                || !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file())
            {
                continue;
            }
            let Ok(file) = fs::File::open(&path) else {
                continue;
            };
            let mut prefix = vec![];
            if file
                .take((MAX_HEADER + 1) as u64)
                .read_to_end(&mut prefix)
                .is_err()
            {
                continue;
            }
            let Some((name, description)) = metadata(&prefix, &entry.file_name().to_string_lossy())
            else {
                continue;
            };
            let real = canonical(&folder);
            if !folders.insert(real) || !names.insert(name.clone()) {
                continue;
            }
            let mut value = json!({"name":name,"path":path.to_string_lossy(),"scope":root.scope,"enabled":true});
            if let Some(description) = description {
                value["description"] = json!(description);
            }
            if let Ok(skill) = serde_json::from_value(value) {
                found.push((root.rank, skill));
            }
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, skill)| skill).collect()
}
