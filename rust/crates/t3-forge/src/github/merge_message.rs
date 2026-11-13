use regex::Regex;
use std::sync::LazyLock;
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})(.*)$").expect("static regex"));
static AUTHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^co-authored-by:\s*(.*?)\s*<([^<>]+)>\s*$").expect("static regex")
});
static GENERATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:🤖\s*)?generated (?:with|by)\s+(?:claude(?: code)?|codex|cursor|github copilot|copilot|opencode|grok|antigravity)[.!]?$").expect("static regex")
});
static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\[([^\]]+)\]\(https://(?:claude\.ai|claude\.com|www\.anthropic\.com|chatgpt\.com|openai\.com|cursor\.com|github\.com|opencode\.ai|x\.ai|antigravity\.google)(?:/[^\s)]*)?\)").expect("static regex")
});
pub fn remove_agent_credits(message: &str) -> String {
    let mut fence = None::<String>;
    let mut removed = false;
    let newline = if message.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = Vec::new();
    for line in message
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
    {
        if line.starts_with("    ") || line.starts_with('\t') {
            lines.push(line);
            continue;
        }
        if let Some(c) = FENCE.captures(line) {
            let marker = &c[1];
            let rest = &c[2];
            if let Some(open) = &fence {
                if marker.as_bytes()[0] == open.as_bytes()[0]
                    && marker.len() >= open.len()
                    && rest.chars().all(|c| c == ' ' || c == '\t')
                {
                    fence = None;
                }
            } else if marker.starts_with('~') || !rest.contains('`') {
                fence = Some(marker.into());
            }
            lines.push(line);
            continue;
        }
        if fence.is_some() {
            lines.push(line);
            continue;
        }
        let text = line.trim();
        let text = if text.starts_with(['-', '*'])
            && text.chars().nth(1).is_some_and(char::is_whitespace)
        {
            text[1..].trim_start()
        } else {
            text
        };
        let agent = AUTHOR.captures(text).is_some_and(|c| {
            let email = c[2].to_lowercase();
            matches!(
                email.as_str(),
                "noreply@anthropic.com"
                    | "claude@anthropic.com"
                    | "noreply@openai.com"
                    | "codex@openai.com"
                    | "cursoragent@cursor.com"
                    | "copilot@github.com"
            ) || (email == "noreply@github.com"
                && matches!(c[1].to_lowercase().as_str(), "github copilot" | "copilot"))
        });
        if agent || GENERATED.is_match(&LINK.replace_all(text, "$1")) {
            removed = true;
        } else {
            lines.push(line);
        }
    }
    if removed {
        lines
            .join(newline)
            .trim_end_matches(['\r', '\n', ' ', '\t'])
            .into()
    } else {
        message.into()
    }
}
