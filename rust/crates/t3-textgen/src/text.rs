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

pub fn sanitize_commit_subject(raw: &str) -> String {
    let line = raw
        .trim()
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('.')
        .trim();
    let mut used = 0;
    let subject = line
        .graphemes(true)
        .take_while(|cluster| {
            used += utf16_len(cluster);
            used <= 72
        })
        .collect::<String>();
    if subject.trim_end().is_empty() {
        "Update project files".to_owned()
    } else {
        subject.trim_end().to_owned()
    }
}

pub fn sanitize_pr_title(raw: &str) -> String {
    let line = raw.trim().split('\n').next().unwrap_or("").trim();
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
    let normalized = title
        .trim()
        .split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(['\'', '"', '`'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return "New thread".to_owned();
    }
    if utf16_len(&normalized) <= 120 {
        normalized
    } else {
        format!("{}...", head(&normalized, 117).trim_end())
    }
}

pub fn sanitize_branch(raw: &str) -> String {
    let normalized = raw
        .nfkd()
        .filter(|ch| !('\u{300}'..='\u{36f}').contains(ch))
        .collect::<String>()
        .trim()
        .to_lowercase()
        .replace(['\'', '"', '`'], "");
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
    let fragment = branch.trim_matches(['/', '_', '-']);
    let fragment = head(fragment, 64).trim_end_matches(['/', '_', '-']);
    if fragment.is_empty() {
        "update".to_owned()
    } else {
        fragment.to_owned()
    }
}

pub fn extract_json_object(raw: &str) -> &str {
    let trimmed = raw.trim();
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
