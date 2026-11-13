//! Transport errors belong to connection status rather than the thread's error banner.
use crate::view_logic::JsTrim;
pub const NETWORK_BLOCKING_HINT: &str = "Your DNS or firewall may be blocking T3 Connect. Try another network, such as a different Wi-Fi or mobile data.";
pub const UNRESPONSIVE_HOST_HINT: &str = "It may be overloaded, asleep, or off the network.";

fn word_start(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(index, _)| {
        index == 0
            || !text.as_bytes()[index - 1].is_ascii_alphanumeric()
                && text.as_bytes()[index - 1] != b'_'
    })
}

fn word(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(index, _)| {
        let start = index == 0
            || !text.as_bytes()[index - 1].is_ascii_alphanumeric()
                && text.as_bytes()[index - 1] != b'_';
        let end = text
            .as_bytes()
            .get(index + needle.len())
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        start && end
    })
}

pub fn is_transport_connection_error_message(message: Option<&str>) -> bool {
    let Some(message) = message
        .map(|message| message.js_trim())
        .filter(|m| !m.is_empty())
    else {
        return false;
    };
    let full_text = message.to_lowercase();
    let text = full_text.as_str();
    if [
        "socketcloseerror",
        "socketopenerror",
        "socket is not connected",
        "clientprotocolerror",
        "rpcclienterror",
        "ping timeout",
    ]
    .iter()
    .any(|p| word(text, p))
        || text.contains("unable to connect to the t3 server websocket.")
    {
        return true;
    }
    let network_hint = format!(" {}", NETWORK_BLOCKING_HINT.to_lowercase());
    let text = text.strip_suffix(&network_hint).unwrap_or(text);
    for suffix in [
        "is not connected.",
        "disconnected.",
        "stopped responding.",
        "could not establish a websocket connection.",
    ] {
        if text.ends_with(suffix)
            && (text.len() == suffix.len()
                || text
                    .as_bytes()
                    .get(text.len() - suffix.len() - 1)
                    .is_some_and(|b| !b.is_ascii_alphanumeric() && *b != b'_'))
        {
            return true;
        }
    }
    let suffix = format!("s. {}", UNRESPONSIVE_HOST_HINT.to_lowercase());
    let Some(prefix) = full_text.strip_suffix(&suffix) else {
        return false;
    };
    let digits_start = prefix
        .char_indices()
        .rfind(|(_, ch)| !ch.is_ascii_digit())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    digits_start < prefix.len()
        && prefix[..digits_start].ends_with("stopped responding for ")
        && word_start(prefix, "stopped responding for ")
}

pub fn sanitize_thread_error_message(message: Option<&str>) -> Option<&str> {
    if is_transport_connection_error_message(message) {
        None
    } else {
        message
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn native_disconnected_socket_errors_are_transport_errors() {
        assert!(is_transport_connection_error_message(Some(
            "The operation couldn't be completed. Socket is not connected"
        )));
    }
    #[test]
    fn relay_network_hint_is_recognized_without_hiding_sign_in_errors() {
        for sentence in [
            "Relay environment disconnected.",
            "Relay environment stopped responding.",
            "Relay environment could not establish a WebSocket connection.",
        ] {
            assert!(is_transport_connection_error_message(Some(&format!(
                "{sentence} {NETWORK_BLOCKING_HINT}"
            ))));
        }
        assert!(!is_transport_connection_error_message(Some(
            "Your ChatGPT connection expired or was disconnected. Sign in again."
        )));
    }
    #[test]
    fn sanitization_preserves_other_errors_and_absence() {
        assert_eq!(
            sanitize_thread_error_message(Some("SocketCloseError: oops")),
            None
        );
        assert_eq!(
            sanitize_thread_error_message(Some("failure")),
            Some("failure")
        );
        assert_eq!(sanitize_thread_error_message(None), None);
        assert_eq!(sanitize_thread_error_message(Some("")), Some(""));
        assert!(!is_transport_connection_error_message(Some(
            "NotSocketCloseError: oops"
        )));
        assert!(is_transport_connection_error_message(Some(&format!(
            "Host stopped responding for 12s. {UNRESPONSIVE_HOST_HINT}"
        ))));
        assert!(!is_transport_connection_error_message(Some(&format!(
            "Host stopped responding for és. {UNRESPONSIVE_HOST_HINT}"
        ))));
        assert!(!is_transport_connection_error_message(Some(&format!(
            "Host stopped responding for 12s. {UNRESPONSIVE_HOST_HINT} {NETWORK_BLOCKING_HINT}"
        ))));
    }
}
