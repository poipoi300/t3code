use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

pub fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn skip_filler(mut source: &str) -> &str {
    loop {
        source = source.trim_start();
        if let Some(rest) = source.strip_prefix("//") {
            source = rest
                .find(['\n', '\r', '\u{2028}', '\u{2029}'])
                .map(|at| &rest[at..])
                .unwrap_or("");
        } else if let Some(rest) = source.strip_prefix("/*") {
            let Some(at) = rest.find("*/") else { return source };
            source = &rest[at + 2..];
        } else {
            return source;
        }
    }
}
fn spread_filler(mut source:&str)->&str{loop{source=skip_filler(source);if let Some(rest)=source.strip_prefix('('){source=rest;}else{return source;}}}
pub fn literal_ids(source: &str) -> Option<Vec<String>> {
    if source.encode_utf16().count() > 256 * 1024 {
        return None;
    }
    let mentions = regex::Regex::new(r"(?i-u:\bprojectId\b)").ok()?;
    let positions = mentions.find_iter(source).collect::<Vec<_>>();
    if positions.len() > 32 {
        return None;
    }
    let assignment=regex::Regex::new(r#"\bObject\s*\.\s*assign\b|\bassign\s*\(|(?:\.\s*(?:extra|eas)|\[\s*["'`](?:extra|eas)["'`]\s*\])\s*=[^=]|\bdelete\b|\brequire\s*\(|\bimport\s*\("#).ok()?;
    if assignment.is_match(source) {
        return None;
    }
    let mut ids = Vec::new();
    for position in positions {
        let mut rest = source[position.end()..].trim_start();
        if rest.starts_with(['"', '\'']) {
            rest = &rest[1..];
        }
        rest = rest.trim_start().strip_prefix(':')?.trim_start();
        let quote = rest
            .chars()
            .next()
            .filter(|c| ['\'', '"', '`'].contains(c))?;
        rest = &rest[quote.len_utf8()..];
        let end = rest.find(quote)?;
        let value = &rest[..end];
        if value.contains(['\n', '\r']) || value.contains("${") {
            return None;
        }
        let tail = skip_filler(&rest[end + quote.len_utf8()..]);
        if !tail.is_empty() && !tail.starts_with([',', ';', '}', ')', ']']) {
            return None;
        }
        if rest[end + quote.len_utf8()..].contains("...") {
            return None;
        }
        if is_uuid(value) {
            ids.push(value.to_lowercase());
        }
    }
    if ids.is_empty() {
        let config=regex::Regex::new(r#"^config(?:(?:\s*\??\.\s*extra|\s*(?:\?\.)?\s*\[\s*["'`]extra["'`]\s*\])(?:\s*\??\.\s*eas|\s*(?:\?\.)?\s*\[\s*["'`]eas["'`]\s*\])?)?"#).ok()?;
        let mut spreads = 0;
        let continuation=regex::Regex::new(r"^[\p{XID_Continue}$\u200c\u200d\\]").ok()?;
        for (at, _) in source.match_indices("...") {
            let after = spread_filler(&source[at+3..]);
            let found = config.find(after)?;
            let tail = &after[found.end()..];
            if continuation.is_match(tail)
                || tail.trim_start().starts_with(['.', '[', '('])
            {
                return None;
            }
            spreads += 1;
        }
        if spreads == 0 {
            return None;
        }
        let holders =
            regex::Regex::new(r#"(?:\b(extra|eas)|["'`](extra|eas)["'`]\s*\]?)\s*:\s*"#).ok()?;
        let mut count = 0;
        for entry in holders.captures_iter(source) {
            count += 1;
            if count > 32 {
                return None;
            }
            let whole = entry.get(0)?;
            let name = entry.get(1).or(entry.get(2))?.as_str();
            let tail = skip_filler(source[whole.end()..].strip_prefix('{')?);
            let spread = spread_filler(tail.strip_prefix("...")?);
            let found = config.find(spread)?;
            let head = &spread[..found.end()];
            if name == "extra" && (!head.contains("extra")||head.contains("eas")) || name == "eas" && !head.contains("eas")
            {
                return None;
            }
        }
        if regex::Regex::new(r"(?:^|[^.\p{XID_Continue}$])(?:extra|eas)\s*[,}]")
            .ok()?
            .is_match(source)
        {
            return None;
        }
    }
    Some(ids)
}
async fn ids_in(directory: &Path) -> Option<Vec<String>> {
    let mut ids = Vec::new();
    for name in [
        "app.config.ts",
        "app.config.mts",
        "app.config.js",
        "app.config.mjs",
        "app.config.cjs",
    ] {
        let file = directory.join(name);
        match tokio::fs::metadata(&file).await {
            Ok(_) => {
                let source = tokio::fs::read_to_string(file).await.ok()?;
                ids.extend(literal_ids(&source)?);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return None,
        }
    }
    for name in ["app.config.json", "app.json"] {
        match tokio::fs::read(directory.join(name)).await {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes).ok()?;
                if !value.is_object() {
                    return None;
                }
                let config = value.get("expo").unwrap_or(&value);
                if !config.is_object(){return None;}
                if let Some(extra)=config.get("extra"){
                    if !extra.is_object(){return None;}
                    if extra.get("eas").is_some_and(|eas|!eas.is_object()){return None;}
                }
                if let Some(id) = config.pointer("/extra/eas/projectId") {
                    let id = id.as_str()?;
                    if is_uuid(id) {
                        ids.push(id.to_lowercase());
                    }
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return None,
        }
    }
    Some(ids)
}
pub async fn find_eas_project_id(root: &Path) -> Option<String> {
    let mut directories = vec![root.to_path_buf()];
    for parent in ["apps", "packages"] {
        if let Ok(mut entries) = tokio::fs::read_dir(root.join(parent)).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if tokio::fs::metadata(entry.path()).await.is_ok_and(|m| m.is_dir()) {
                    directories.push(entry.path());
                }
            }
        }
    }
    directories.sort();
    let mut ids = BTreeSet::new();
    for directory in directories {
        ids.extend(ids_in(&directory).await?);
    }
    if ids.len() == 1 {
        ids.into_iter().next()
    } else {
        None
    }
}
