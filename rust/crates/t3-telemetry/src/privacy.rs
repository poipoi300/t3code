use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

// After a string start, quote or space, or opening a query, fragment (`/pair#token=`) or `;` part.
static CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| {
    // A prefix is part of the name: `client_secret`, `private_token`, `auth_token`.
    Regex::new(r#"(?i)(^|[?&#;\"\s])([\w-]*(?:wsTicket|dpop|token|api_key|apikey|password|secret))=([^&#\"\\\s]*)"#)
        .expect("static credential pattern")
});
// OAuth `code` and `state` only inside a URL's query or fragment: `exit code=1` stays readable.
static OAUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([?&#;])(code|state)=([^&#\"\\\s]*)"#).expect("static OAuth pattern")
});
static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+").expect("static bearer pattern")
});
// A header written out in text, such as `x-api-key: …` or `Authorization: …`: the rest of the line.
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)\b((?:proxy-)?authorization|(?:set-)?cookie|[\w-]*api[-_]?key|[\w-]*token|[\w-]*secret)[ \t]*:[ \t]*[^\r\n]+")
        .expect("static header pattern")
});
// User info runs to the authority's last `@`, as WHATWG URL parsing splits it.
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([a-z][a-z0-9+.-]*://)[^/?#\s\"\\]*@"#).expect("static URL user-info pattern")
});
static CLI_PRIVATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(^|\s)(--?(?:api[-_]?key|access[-_]?token|refresh[-_]?token|id[-_]?token|token|password|secret|prompt|message|p))(=|\s+)(?:\"[^\"]*\"|'[^']*'|[^\s]+)"#)
        .expect("static private argument pattern")
});

pub fn redact_text(text: &str) -> String {
    let redacted = URL_USERINFO.replace_all(text, "$1<redacted>@");
    let redacted = CREDENTIALS.replace_all(&redacted, "$1$2=<redacted>");
    let redacted = OAUTH.replace_all(&redacted, "$1$2=<redacted>");
    let redacted = BEARER.replace_all(&redacted, "$1 <redacted>");
    let redacted = HEADER.replace_all(&redacted, "$1: <redacted>");
    CLI_PRIVATE
        .replace_all(&redacted, "$1$2$3<redacted>")
        .into_owned()
}

fn private_argument(argument: &str) -> bool {
    let Some(flag) = argument
        .strip_prefix("--")
        .or_else(|| argument.strip_prefix('-'))
    else {
        return false;
    };
    matches!(
        flag.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "token"
            | "password"
            | "secret"
            | "prompt"
            | "message"
            | "p"
    )
}

pub fn redact_arguments(arguments: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut private_value = false;
    arguments
        .into_iter()
        .map(|argument| {
            let argument = argument.as_ref();
            if std::mem::take(&mut private_value) {
                return "<redacted>".to_owned();
            }
            if let Some((flag, _)) = argument.split_once('=')
                && private_argument(flag)
            {
                return format!("{flag}=<redacted>");
            }
            private_value = private_argument(argument);
            redact_text(argument)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn private_field(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_', '.'], "");
    [
        "authorization",
        "cookie",
        "credential",
        "password",
        "secret",
        "token",
        "prompt",
        "filecontent",
        "messagecontent",
        "requestbody",
        "responsebody",
        "dbquerytext",
    ]
    .iter()
    .any(|fragment| normalized.contains(fragment))
}

/// Token counters a provider reports; numeric values are kept although the names contain "token".
pub(crate) fn usage_counter(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase()
            .replace(['-', '_', '.'], "")
            .as_str(),
        "inputtokens"
            | "outputtokens"
            | "cachedinputtokens"
            | "cachecreationtokens"
            | "reasoningtokens"
            | "totaltokens"
            | "otherinputtokens"
            | "otheroutputtokens"
            | "childinputtokens"
            | "childoutputtokens"
    )
}

pub fn sanitize(value: &mut Value) {
    fn visit(value: &mut Value, budget: &mut usize, depth: usize) {
        if *budget == 0 || depth > 64 {
            *value = Value::String("[Truncated]".into());
            return;
        }
        *budget -= 1;
        match value {
            Value::String(text) => {
                *text = redact_text(text);
                if text.chars().count() > 500 {
                    *text = text.chars().take(500).collect::<String>() + "…[truncated]";
                }
            }
            Value::Object(fields) => {
                for (key, entry) in fields {
                    if private_field(key) && !(usage_counter(key) && entry.is_number()) {
                        *entry = Value::String("<redacted>".into());
                    } else {
                        visit(entry, budget, depth + 1);
                    }
                }
            }
            Value::Array(items) => {
                items.truncate((*budget).min(10_000));
                for item in items {
                    visit(item, budget, depth + 1);
                }
            }
            _ => {}
        }
    }
    visit(value, &mut 10_000, 0);
}
