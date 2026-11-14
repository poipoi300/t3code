//! Text-generator settings rules that servers and clients apply alike.
use t3_contracts::settings::{TextGeneratorConfig, TextGeneratorKind};
use t3_contracts::settings_secret_value::SettingsSecretValue;

/// What a supplied, unsaved configuration may do when its models are listed or it is tested:
/// the only time a server contacts an address a client chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppliedGenerator {
    /// Contact the supplied address with the supplied key.
    UseSupplied,
    /// The key is the redaction placeholder: contact the saved address with the saved key.
    UseSavedKey,
    /// The key stands for a stored secret that may not go to this address or kind of API.
    KeyRequired,
    /// The address is malformed, not plain http(s), carries user info, a query or a fragment,
    /// or is neither the saved address nor an official endpoint.
    AddressRefused,
}

/// The one rule for a supplied configuration (`redacted` is the server's placeholder for a
/// saved key):
/// - A placeholder or secret reference reuses a stored secret, so it is allowed only for the
///   saved kind at the saved address, and a reference only when it is the saved one.
/// - Any other key may go to the saved address or to an official endpoint, and nowhere else.
///
/// Addresses are equal when their scheme, host, port and path are, with trailing `/` ignored
/// (requests append to the address without its trailing `/`). TypeScript passes any supplied
/// address and reference through; the server adapter enforced this before it was shared.
pub fn check_supplied_generator(
    supplied: &TextGeneratorConfig,
    saved: Option<&TextGeneratorConfig>,
    redacted: &str,
) -> SuppliedGenerator {
    let Some(address) = endpoint(&supplied.base_url.0) else {
        return SuppliedGenerator::AddressRefused;
    };
    let at_saved_address =
        saved.is_some_and(|saved| endpoint(&saved.base_url.0).as_ref() == Some(&address));
    let placeholder =
        matches!(&supplied.api_key, SettingsSecretValue::Literal(key) if key == redacted);
    if placeholder || matches!(&supplied.api_key, SettingsSecretValue::Reference(_)) {
        let reusable = at_saved_address
            && saved.is_some_and(|saved| {
                saved.kind == supplied.kind && (placeholder || saved.api_key == supplied.api_key)
            });
        return match (reusable, placeholder) {
            (false, _) => SuppliedGenerator::KeyRequired,
            (true, true) => SuppliedGenerator::UseSavedKey,
            (true, false) => SuppliedGenerator::UseSupplied,
        };
    }
    if at_saved_address || official(&supplied.kind, &address) {
        SuppliedGenerator::UseSupplied
    } else {
        SuppliedGenerator::AddressRefused
    }
}

/// An address in compared form: `(scheme, host, port, path without trailing /)`.
type Endpoint = (String, String, u16, String);

fn official(kind: &TextGeneratorKind, address: &Endpoint) -> bool {
    let at = |host: &str, path: &str| {
        address.0 == "https" && address.1 == host && address.2 == 443 && address.3 == path
    };
    (*kind == TextGeneratorKind::Anthropic && at("api.anthropic.com", ""))
        || at("api.openai.com", "/v1")
}

/// `http(s)://host[:port][/path]` with nothing a URL parser could read another way: no user
/// info, query, fragment, percent escapes, dot segments or unusual characters. Anything else has
/// no endpoint, so the rule fails closed.
fn endpoint(url: &str) -> Option<Endpoint> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
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
    let plain = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:/".contains(&b));
    if !plain
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    Some((scheme, host, port, path.trim_end_matches('/').to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{SuppliedGenerator::*, *};

    const PLACEHOLDER: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
    /// Built directly: decoding would already refuse some of these addresses.
    fn config(kind: &str, url: &str, key: serde_json::Value) -> TextGeneratorConfig {
        TextGeneratorConfig {
            kind: serde_json::from_value(kind.into()).expect("kind"),
            label: None,
            base_url: t3_contracts::base_schemas::TrimmedNonEmptyString(url.into()),
            api_key: serde_json::from_value(key).expect("key"),
            enabled: true,
        }
    }

    #[test]
    fn addresses_compare_as_requests_reach_them_and_fail_closed() {
        let saved = config(
            "openaiCompatible",
            "https://gateway.example/v1/acct/a",
            "k".into(),
        );
        let check = |url: &str| {
            check_supplied_generator(
                &config("openaiCompatible", url, PLACEHOLDER.into()),
                Some(&saved),
                PLACEHOLDER,
            )
        };
        for same in [
            "https://gateway.example/v1/acct/a",
            "https://gateway.example/v1/acct/a/",
            "HTTPS://Gateway.Example:443/v1/acct/a",
        ] {
            assert_eq!(check(same), UseSavedKey, "{same}");
        }
        // Another tenant behind the same origin is another address.
        for elsewhere in [
            "https://gateway.example/v1/acct/b",
            "https://gateway.example/v1/ACCT/a",
            "http://gateway.example/v1/acct/a",
            "https://gateway.example:8443/v1/acct/a",
        ] {
            assert_eq!(check(elsewhere), KeyRequired, "{elsewhere}");
        }
        for refused in [
            "https://user:p@gateway.example/v1/acct/a",
            "https://gateway.example/v1/acct/a#x",
            "https://gateway.example/v1/acct/a?x",
            "https://gateway.example/v1/acct/b/../a",
            "https://gateway.example/v1/acct/%61",
            "https://gateway.example\\v1/acct/a",
            "https:gateway.example/v1/acct/a",
            "ftp://gateway.example/v1/acct/a",
            "not a url",
        ] {
            assert_eq!(check(refused), AddressRefused, "{refused}");
        }
    }

    #[test]
    fn fresh_keys_go_only_to_the_saved_address_or_an_official_endpoint() {
        let saved = config("openaiCompatible", "http://localhost:11434/v1", "".into());
        let fresh = |kind: &str, url: &str| {
            check_supplied_generator(
                &config(kind, url, "typed".into()),
                Some(&saved),
                PLACEHOLDER,
            )
        };
        assert_eq!(
            fresh("openaiCompatible", "http://localhost:11434/v1/"),
            UseSupplied
        );
        assert_eq!(
            fresh("openaiCompatible", "https://api.openai.com/v1/"),
            UseSupplied
        );
        assert_eq!(fresh("anthropic", "https://api.anthropic.com"), UseSupplied);
        assert_eq!(
            fresh("openaiCompatible", "https://api.anthropic.com"),
            AddressRefused
        );
        assert_eq!(
            fresh("openaiCompatible", "https://attacker.example/v1"),
            AddressRefused
        );
        assert_eq!(
            fresh("openaiCompatible", "http://10.0.0.5:8080/v1"),
            AddressRefused
        );
    }
}
