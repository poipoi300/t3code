//! Text-generator settings rules that servers and clients apply alike.
use t3_contracts::settings::TextGeneratorConfig;

/// Whether the saved API key may stand in for the redacted key of a supplied, unsaved
/// configuration: only for the same kind of API at the same origin (scheme, host and port), so a
/// key is never sent to an address it was not saved for. Otherwise the key must be entered again.
/// TypeScript substitutes it for any address; this is a deliberate hardening.
pub fn saved_api_key_applies(supplied: &TextGeneratorConfig, saved: &TextGeneratorConfig) -> bool {
    supplied.kind == saved.kind
        && origin(&supplied.base_url.0)
            .is_some_and(|origin| self::origin(&saved.base_url.0) == Some(origin))
}

/// The `(scheme, host, port)` origin of a plain `http(s)://host[:port]` URL. Anything a URL
/// parser could read another way (user info, missing slashes, escapes, unusual host characters)
/// has no origin here, so the comparison fails closed and the key must be entered again.
fn origin(url: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (address, after) = bracketed.split_once(']')?;
        if address.is_empty()
            || !address
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        {
            return None;
        }
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (format!("[{}]", address.to_ascii_lowercase()), port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    let port = match port {
        None => default_port,
        Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            port.parse().ok()?
        }
        Some(_) => return None,
    };
    Some((scheme, host, port))
}
