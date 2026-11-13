use super::unknown_field::text;
use serde_json::Value;
use std::error::Error;

pub fn sqlite_recovery_message(code: Option<i32>) -> &'static str {
    match code.map(|code| code & 0xff) {
        Some(5) => {
            "The T3 Code server database is busy (SQLITE_BUSY). Wait a moment, then retry. If this keeps happening, check for another T3 Code process using the same data directory on the server."
        }
        Some(6) => {
            "The T3 Code server database is locked (SQLITE_LOCKED). Wait a moment, then retry. If this keeps happening, check the server logs for the process holding the database lock."
        }
        Some(8) => {
            "The T3 Code server database is read-only (SQLITE_READONLY). On the machine running the server, check that its data directory is writable and its disk is mounted read-write, then retry."
        }
        Some(10) => {
            "The T3 Code server could not read or write its database (SQLITE_IOERR). On the machine running the server, check free disk space and disk health, then retry. If it still fails, check the server logs."
        }
        Some(11) => {
            "The T3 Code server database appears damaged (SQLITE_CORRUPT). Preserve the server data directory and check the server logs before restoring a database backup."
        }
        Some(13) => {
            "The T3 Code server could not save this change because its database or disk is full (SQLITE_FULL). Free space on the machine running the server, then retry. If space is available, check the server logs for a database size limit."
        }
        Some(14) => {
            "The T3 Code server could not open a database file (SQLITE_CANTOPEN). On the machine running the server, check free disk space and that its data and temporary directories exist and are writable, then retry. If it still fails, check the server logs."
        }
        Some(26) => {
            "The T3 Code server could not read its database file (SQLITE_NOTADB). Preserve the server data directory and check the configured database path and server logs before restoring a database backup."
        }
        _ => {
            "The T3 Code server could not complete this database request. Retry once; if it still fails, check the server logs for the database error."
        }
    }
}

pub fn user_facing_database_error_message(error: &(dyn Error + 'static)) -> Option<&'static str> {
    let mut current = Some(error);
    let mut seen = Vec::new();
    while let Some(error) = current {
        let pointer = std::ptr::from_ref(error).cast::<()>();
        if seen.contains(&pointer) {
            break;
        }
        seen.push(pointer);
        if let Some(crate::error::Error::Sql(sqlite)) = error.downcast_ref::<crate::error::Error>()
        {
            return Some(sqlite_recovery_message(
                sqlite.sqlite_error().map(|error| error.extended_code),
            ));
        }
        if let Some(crate::error::Error::Persistence(persistence)) =
            error.downcast_ref::<crate::error::Error>()
        {
            current = Some(persistence);
            continue;
        }
        if let Some(t3_persistence::Error::Sqlite(sqlite)) =
            error.downcast_ref::<t3_persistence::Error>()
        {
            return Some(sqlite_recovery_message(
                sqlite.sqlite_error().map(|error| error.extended_code),
            ));
        }
        if let Some(error) = error.downcast_ref::<t3_persistence::rusqlite::Error>() {
            return Some(sqlite_recovery_message(
                error.sqlite_error().map(|error| error.extended_code),
            ));
        }
        current = error.source();
    }
    None
}

fn database_error_message(value: &Value) -> Option<&'static str> {
    if value["_tag"] != "SqlError" {
        return None;
    }
    let mut cause = &value["cause"];
    while cause.is_object() {
        if let Some(code) = cause["errcode"]
            .as_f64()
            .filter(|code| code.is_finite() && *code > 0.0 && code.fract() == 0.0)
        {
            let code = (code % 256.0) as i32;
            let message = sqlite_recovery_message(Some(code));
            if message != sqlite_recovery_message(None) {
                return Some(message);
            }
        }
        cause = &cause["cause"];
    }
    Some(sqlite_recovery_message(None))
}

pub fn provider_display_name(instance: &str) -> String {
    let known = match instance {
        "codex" => Some("Codex"),
        "claudeAgent" => Some("Claude"),
        "cursor" => Some("Cursor"),
        "grok" => Some("Grok"),
        "opencode" => Some("OpenCode"),
        "antigravity" => Some("Antigravity"),
        "kiro" => Some("Kiro"),
        "pi" => Some("Pi"),
        "muse" => Some("Muse Code"),
        "fx" => Some("fx"),
        "acpRegistry" => Some("ACP Registry"),
        _ => None,
    };
    if let Some(known) = known {
        return known.into();
    }
    let trimmed = if instance
        .get(instance.len().saturating_sub(5)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("Agent"))
    {
        &instance[..instance.len() - 5]
    } else {
        instance
    };
    let trimmed = t3_contracts::wire::trim_js(trimmed);
    let mut characters = trimmed.chars();
    match characters.next() {
        None => instance.into(),
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
    }
}

pub fn capability_rejection_message(instance: &str, capability: &str) -> Option<String> {
    let provider = provider_display_name(instance);
    let message = match capability {
        "queued_messages" => "cannot queue messages behind an active run.",
        "active_steering" | "interrupt_restart_steering" => {
            "cannot redirect an active run. Stop it first, then send the message."
        }
        "interrupt" => "cannot stop a run once it has started.",
        "native_fork" => "cannot fork this thread natively.",
        "fork_from_turn" => "cannot fork from an earlier point in this thread.",
        "rollback" => {
            "cannot rewind its conversation, so this checkpoint cannot be restored on this thread."
        }
        "rollback_snapshot" => {
            "did not report its rewound conversation state, so the checkpoint was not restored."
        }
        "context_handoff" => "cannot receive the context handoff needed for this switch.",
        "strong_terminal_status" => "cannot confirm when its runs finish reliably enough for this.",
        _ => return None,
    };
    Some(format!("{provider} {message}"))
}

fn collect_messages(value: &Value, messages: &mut Vec<String>) {
    if let Some(values) = value.as_array() {
        for value in values {
            collect_messages(value, messages);
        }
        return;
    }
    if let Some(message) = database_error_message(value) {
        messages.push(message.into());
        return;
    }
    let friendly = (value["_tag"] == "CommandPolicyCapabilityUnsupportedError")
        .then(|| {
            capability_rejection_message(
                value["providerInstanceId"].as_str()?,
                value["capability"].as_str().unwrap_or_default(),
            )
        })
        .flatten();
    if let Some(message) = friendly.or_else(|| {
        text(value)
            .or_else(|| text(&value["detail"]))
            .or_else(|| text(&value["message"]))
            .map(str::to_owned)
    }) {
        messages.push(message);
    }
    if value.is_object() {
        for key in ["cause", "error", "errors"] {
            collect_messages(&value[key], messages);
        }
    }
}

pub fn user_facing_dispatch_error_message(cause: &Value) -> Option<String> {
    let mut messages = Vec::new();
    collect_messages(cause, &mut messages);
    let mut unique = Vec::new();
    for message in messages {
        if [
            "Failed to dispatch orchestration V2 command",
            "Failed to dispatch orchestration command ",
            "Provider adapter failed while dispatching orchestration command ",
        ]
        .iter()
        .any(|prefix| message.starts_with(prefix))
            || unique.contains(&message)
        {
            continue;
        }
        unique.push(message);
    }
    unique.pop()
}
