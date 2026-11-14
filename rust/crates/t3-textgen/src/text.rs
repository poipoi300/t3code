use crate::{generation::GenerationError, prompts::BranchNaming};
use t3_contracts::wire::trim_js;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub fn head(value: &str, budget: usize) -> &str {
    let mut used = 0;
    let end = value.char_indices().find_map(|(at, ch)| {
        used += ch.len_utf16();
        (used > budget).then_some(at)
    });
    &value[..end.unwrap_or(value.len())]
}

pub fn tail(value: &str, budget: usize) -> &str {
    let mut used = 0;
    let start = value.char_indices().rev().find_map(|(at, ch)| {
        used += ch.len_utf16();
        (used > budget).then_some(at + ch.len_utf8())
    });
    &value[start.unwrap_or(0)..]
}

pub fn limit_section(value: &str, budget: usize) -> String {
    if utf16_len(value) <= budget {
        value.to_owned()
    } else {
        format!("{}\n\n[truncated]", head(value, budget))
    }
}

pub fn limit_title_message(value: &str, budget: usize) -> String {
    const MARKER: &str = "\n[Content truncated]\n";
    if utf16_len(value) <= budget {
        return value.to_owned();
    }
    if budget <= MARKER.len() {
        return String::new();
    }
    let available = budget - MARKER.len();
    format!(
        "{}{MARKER}{}",
        head(value, available.div_ceil(2)),
        tail(value, available / 2)
    )
}

/// JavaScript's `\s`, which `String.prototype.trim` also removes.
fn js_space(ch: char) -> bool {
    let mut buffer = [0; 4];
    trim_js(ch.encode_utf8(&mut buffer)).is_empty()
}
fn first_line(value: &str) -> &str {
    trim_js(trim_js(value).split('\n').next().unwrap_or(""))
}

pub fn sanitize_commit_subject(raw: &str) -> String {
    let line = trim_js(first_line(raw).trim_end_matches('.'));
    let mut used = 0;
    let subject = line
        .graphemes(true)
        .take_while(|cluster| {
            used += utf16_len(cluster);
            used <= 72
        })
        .collect::<String>();
    let subject = subject.trim_end_matches(js_space);
    if subject.is_empty() {
        "Update project files".to_owned()
    } else {
        subject.to_owned()
    }
}

pub fn sanitize_pr_title(raw: &str) -> String {
    let line = first_line(raw);
    if line.is_empty() {
        "Update project changes".to_owned()
    } else {
        line.to_owned()
    }
}

pub fn sanitize_thread_title(raw: &str) -> String {
    let decoded = serde_json::from_str::<serde_json::Value>(raw).ok();
    let title = decoded
        .as_ref()
        .and_then(|v| v.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or(raw);
    let unquoted = trim_js(first_line(title).trim_matches(['\'', '"', '`']));
    let mut normalized = String::with_capacity(unquoted.len());
    for ch in unquoted.chars() {
        if !js_space(ch) {
            normalized.push(ch);
        } else if !normalized.ends_with(' ') {
            normalized.push(' ');
        }
    }
    if normalized.is_empty() {
        return "New thread".to_owned();
    }
    if utf16_len(&normalized) <= 120 {
        normalized
    } else {
        format!("{}...", head(&normalized, 117).trim_end_matches(js_space))
    }
}

/// `sanitizeBranchFragment`: a lowercase `[a-z0-9/_-]` fragment of at most 64 units.
pub fn sanitize_branch(raw: &str) -> String {
    let normalized = raw
        .nfkd()
        .filter(|ch| !('\u{300}'..='\u{36f}').contains(ch))
        .collect::<String>();
    let normalized = trim_js(&normalized)
        .to_lowercase()
        .replace(['\'', '"', '`'], "");
    let normalized =
        normalized.trim_matches(|ch: char| matches!(ch, '.' | '/' | '_' | '-') || js_space(ch));
    let mut branch = String::new();
    for ch in normalized.chars() {
        let ch = if ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '/' | '_' | '-')
        {
            ch
        } else {
            '-'
        };
        if (ch == '-' || ch == '/') && branch.ends_with(ch) {
            continue;
        }
        branch.push(ch);
    }
    let separators = ['.', '/', '_', '-'];
    let fragment = head(branch.trim_matches(separators), 64).trim_end_matches(separators);
    if fragment.is_empty() {
        "update".to_owned()
    } else {
        fragment.to_owned()
    }
}

/// `sanitizeFeatureBranchName`: a fragment without a namespace goes under `feature/`.
pub fn sanitize_feature_branch(raw: &str) -> String {
    let branch = sanitize_branch(raw);
    if branch.contains('/') {
        branch
    } else {
        format!("feature/{branch}")
    }
}

/// Git's ref check (behind the rename's `--`) refuses malformed names but sets no length limit,
/// so a custom name longer than a file name may be is refused here.
const MAX_CUSTOM_BRANCH_BYTES: usize = 255;

/// `formatGeneratedBranchName`. Custom mode keeps the model's name as written, so it is refused
/// rather than shortened when it is longer than git can store as a loose ref.
pub fn format_generated_branch(
    raw: &str,
    naming: Option<&BranchNaming>,
) -> Result<String, GenerationError> {
    match naming {
        Some(BranchNaming::Custom { .. }) => {
            let name = trim_js(raw);
            if name.len() > MAX_CUSTOM_BRANCH_BYTES {
                Err(GenerationError::InvalidOutput)
            } else {
                Ok(name.to_owned())
            }
        }
        Some(BranchNaming::Static { prefix }) => {
            let prefix = prefix
                .split('/')
                .map(|part| {
                    let mut out = String::new();
                    for ch in part.chars() {
                        let ch = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                            ch
                        } else {
                            '-'
                        };
                        if ch != '-' || !out.ends_with('-') {
                            out.push(ch);
                        }
                    }
                    out.trim_matches('-').to_owned()
                })
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            let branch = sanitize_branch(raw);
            Ok(if prefix.is_empty() {
                branch
            } else {
                format!("{prefix}/{branch}")
            })
        }
        Some(BranchNaming::Semantic) | None => Ok(sanitize_branch(raw)),
    }
}

pub fn extract_json_object(raw: &str) -> &str {
    let trimmed = trim_js(raw);
    let Some(start) = trimmed.find('{') else {
        return trimmed;
    };
    let mut depth = 0;
    let mut in_string = false;
    let mut escaping = false;
    for (at, ch) in trimmed[start..].char_indices() {
        if in_string {
            if escaping {
                escaping = false;
            } else if ch == '\\' {
                escaping = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return &trimmed[start..start + at + 1];
            }
        }
    }
    &trimmed[start..]
}
