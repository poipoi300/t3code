use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFileTier {
    Source,
    Test,
    Generated,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub addition_lines: Vec<String>,
    pub deletion_lines: Vec<String>,
}
pub fn diff_file_tier(path: &str) -> DiffFileTier {
    let mut segments: Vec<_> = path.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    if [
        "pnpm-lock.yaml",
        "yarn.lock",
        "package-lock.json",
        "bun.lockb",
        "Cargo.lock",
        "go.sum",
        "composer.lock",
        "Gemfile.lock",
    ]
    .contains(&name)
        || [".snap", ".min.js", ".min.css"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || name.contains(".generated.")
        || segments
            .iter()
            .any(|s| ["__snapshots__", "__generated__", "dist", "build", "vendor"].contains(s))
    {
        DiffFileTier::Generated
    } else if name.contains(".test.")
        || name.contains(".spec.")
        || segments
            .iter()
            .any(|s| ["__tests__", "tests", "test"].contains(s))
    {
        DiffFileTier::Test
    } else {
        DiffFileTier::Source
    }
}
fn strip_extension(path: &str) -> &str {
    path.rfind('.')
        .filter(|dot| path.rfind('/').is_none_or(|slash| *dot > slash))
        .map_or(path, |dot| &path[..dot])
}
fn base_name(path: &str) -> &str {
    strip_extension(path.rsplit('/').next().unwrap_or_default())
}
fn relative(from: &str, specifier: &str) -> String {
    let mut parts: Vec<_> = from.split('/').collect();
    parts.pop();
    for part in specifier.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}
fn specifiers(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let bytes = line.as_bytes();
    for (start, _) in line.char_indices() {
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        for keyword in ["from", "import", "require"] {
            let Some(rest) = line[start..].strip_prefix(keyword) else {
                continue;
            };
            let rest = rest.trim_start();
            let rest = if keyword == "require" {
                let Some(rest) = rest.strip_prefix('(') else {
                    continue;
                };
                rest.trim_start()
            } else {
                rest
            };
            let rest = rest.strip_prefix('(').unwrap_or(rest).trim_start();
            let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
                continue;
            };
            let tail = &rest[quote.len_utf8()..];
            if let Some(end) = tail.find(['\'', '"'])
                && end > 0
            {
                found.push(&tail[..end]);
            }
        }
    }
    found
}
fn tested_position(path: &str, positions: &BTreeMap<String, usize>) -> usize {
    let name = path.rsplit('/').next().unwrap_or_default();
    let cut = [".test.", ".spec."]
        .iter()
        .filter_map(|s| name.find(s))
        .min()
        .unwrap_or(name.len());
    let mut name = &name[..cut];
    loop {
        if let Some(position) = positions.get(name) {
            return *position;
        }
        let Some(dot) = name.rfind('.').filter(|dot| *dot > 0) else {
            return usize::MAX;
        };
        name = &name[..dot];
    }
}
pub fn order_diff_files(files: &[DiffFile]) -> Vec<&DiffFile> {
    let by_path: BTreeMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let source: Vec<_> = by_path
        .keys()
        .copied()
        .filter(|path| diff_file_tier(path) == DiffFileTier::Source)
        .collect();
    let mut by_module = BTreeMap::new();
    let mut by_base = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    for path in &source {
        by_module.insert(strip_extension(path), *path);
        if by_base.insert(base_name(path), *path).is_some() {
            ambiguous.insert(base_name(path));
        }
    }
    for name in ambiguous {
        by_base.remove(name);
    }
    let imports: BTreeMap<_, BTreeSet<_>> = source
        .iter()
        .map(|path| {
            let file = by_path[path];
            let imports = file
                .addition_lines
                .iter()
                .chain(&file.deletion_lines)
                .flat_map(|line| specifiers(line))
                .filter_map(|specifier| {
                    let resolved = if specifier.starts_with('.') {
                        relative(path, specifier)
                    } else {
                        specifier.into()
                    };
                    let module = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
                        .iter()
                        .find_map(|ext| resolved.strip_suffix(ext))
                        .unwrap_or(&resolved);
                    by_module
                        .get(module)
                        .or_else(|| by_module.get(format!("{module}/index").as_str()))
                        .or_else(|| by_base.get(base_name(module)))
                        .copied()
                        .filter(|target| target != path)
                })
                .collect();
            (*path, imports)
        })
        .collect();
    let mut remaining: BTreeSet<_> = source.iter().copied().collect();
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .find(|path| {
                imports
                    .get(**path)
                    .is_none_or(|deps| deps.iter().all(|dep| !remaining.contains(dep)))
            })
            .or_else(|| remaining.first())
            .copied();
        let Some(next) = next else { break };
        remaining.remove(next);
        ordered.push(next);
    }
    let mut positions = BTreeMap::new();
    for (index, path) in ordered.iter().enumerate() {
        positions.entry(base_name(path).to_owned()).or_insert(index);
    }
    let mut tests: Vec<_> = by_path
        .keys()
        .copied()
        .filter(|p| diff_file_tier(p) == DiffFileTier::Test)
        .collect();
    tests.sort_by(|a, b| {
        tested_position(a, &positions)
            .cmp(&tested_position(b, &positions))
            .then(a.cmp(b))
    });
    ordered.extend(tests);
    ordered.extend(
        by_path
            .keys()
            .copied()
            .filter(|p| diff_file_tier(p) == DiffFileTier::Generated),
    );
    ordered.into_iter().map(|p| by_path[p]).collect()
}
