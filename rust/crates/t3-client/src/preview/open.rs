use t3_contracts::{
    browser_profile::BrowserProfileId,
    preview::{
        PreviewAppearancePreference, PreviewOpenInput, PreviewViewportSetting, PreviewZoomFactor,
    },
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDefaults {
    pub viewport: PreviewViewportSetting,
    pub profile_id: BrowserProfileId,
    pub zoom_factor: PreviewZoomFactor,
    pub appearance: PreviewAppearancePreference,
}

#[derive(Clone, Copy)]
pub enum EntryPoint {
    Session,
    File,
    TerminalLink,
}

pub fn apply_defaults(
    mut input: PreviewOpenInput,
    defaults: &BrowserDefaults,
    entry: EntryPoint,
) -> PreviewOpenInput {
    use t3_contracts::preview::PreviewRuntime;
    if input.viewport.as_ref().and_then(Option::as_ref).is_none() {
        input.viewport = Some(Some(defaults.viewport.clone()));
    }
    if input.profile_id.as_ref().and_then(Option::as_ref).is_none() {
        input.profile_id = Some(Some(defaults.profile_id.clone()));
    }
    if matches!(entry, EntryPoint::Session) && input.runtime == Some(Some(PreviewRuntime::Desktop))
    {
        input.runtime = None;
    }
    if input.runtime.as_ref().and_then(Option::as_ref).is_some() {
        input.zoom_factor = Some(Some(defaults.zoom_factor.clone()));
        input.color_scheme = Some(Some(defaults.appearance));
    }
    input
}

/// Port of shared preview address handling, including the selected search engine.
pub fn resolve_address(
    raw: &str,
    settings: &t3_contracts::settings::ClientSettingsSchema,
) -> Option<String> {
    use crate::view_logic::JsTrim;
    use t3_contracts::settings::BrowserSearchEngine;
    let input = raw.js_trim();
    if input.is_empty() {
        return None;
    }
    let protocol = input.split_once(':').filter(|(scheme, _)| {
        scheme
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && scheme
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
    });
    let authority = input.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let (host, has_port) = authority
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|c| c.is_ascii_digit()))
        .map_or((authority, false), |(host, _)| (host, true));
    let lower = host.to_ascii_lowercase();
    let loopback = matches!(
        lower.as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "[::]"
    );
    let explicit = protocol.and_then(|(scheme, tail)| {
        let scheme = scheme.to_ascii_lowercase();
        let non_web = matches!(
            scheme.as_str(),
            "about"
                | "blob"
                | "chrome"
                | "data"
                | "file"
                | "ftp"
                | "javascript"
                | "mailto"
                | "sms"
                | "ssh"
                | "tel"
                | "view-source"
                | "ws"
                | "wss"
        );
        (tail.starts_with("//") || (!input.contains(char::is_whitespace) && (non_web || !has_port)))
            .then_some((scheme, tail))
    });
    if let Some((scheme, tail)) = explicit {
        if !matches!(scheme.as_str(), "http" | "https") {
            return None;
        }
        if tail.starts_with("//") {
            return web_url(input);
        }
        return web_url(&format!("https://{input}"));
    }
    if !input.contains(char::is_whitespace)
        && (has_port || loopback || host.starts_with('[') || dotted_host(host))
    {
        let scheme = if loopback { "http" } else { "https" };
        if let Some(address) = web_url(&format!("{scheme}://{input}")) {
            return Some(address);
        }
    }
    const DEFAULT_SEARCH: &str = "https://duckduckgo.com/?q=%s";
    let template = match settings.browser_search_engine {
        BrowserSearchEngine::Duckduckgo => DEFAULT_SEARCH,
        BrowserSearchEngine::Google => "https://www.google.com/search?q=%s",
        BrowserSearchEngine::Bing => "https://www.bing.com/search?q=%s",
        BrowserSearchEngine::Brave => "https://search.brave.com/search?q=%s",
        BrowserSearchEngine::Kagi => "https://kagi.com/search?q=%s",
        BrowserSearchEngine::Ecosia => "https://www.ecosia.org/search?q=%s",
        BrowserSearchEngine::Custom => {
            let custom = settings.browser_custom_search_url.trim();
            if valid_search_template(custom) {
                custom
            } else {
                DEFAULT_SEARCH
            }
        }
    };
    let encoded = crate::persistence::encode_uri_component(input);
    Some(template.replace("%s", &encoded))
}

fn web_url(input: &str) -> Option<String> {
    let url = url::Url::parse(input).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.into())
}

fn valid_search_template(template: &str) -> bool {
    if !template.contains("%s") {
        return false;
    }
    let authority = |query| {
        let address = web_url(&template.replace("%s", query))?;
        let url = url::Url::parse(&address).ok()?;
        Some((
            url.username().to_owned(),
            url.password().map(str::to_owned),
            url.host_str()?.to_owned(),
            url.port(),
        ))
    };
    authority("a").is_some_and(|first| Some(first) == authority("b"))
}

fn dotted_host(host: &str) -> bool {
    let name = host.strip_suffix('.').unwrap_or(host);
    name.contains('.') && name.split('.').all(|label| !label.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::settings::{BrowserSearchEngine, ClientSettingsSchema};

    #[test]
    fn address_bar_keeps_local_urls_and_rejects_explicit_unhandled_schemes() {
        let settings = ClientSettingsSchema::default();
        for (input, expected) in [
            (
                " localhost:5173/page?q=1 ",
                "http://localhost:5173/page?q=1",
            ),
            ("devbox:8080", "https://devbox:8080/"),
            ("[::1]:4000", "http://[::1]:4000/"),
            ("example.com", "https://example.com/"),
            (
                "how to center a div",
                "https://duckduckgo.com/?q=how%20to%20center%20a%20div",
            ),
        ] {
            assert_eq!(resolve_address(input, &settings).as_deref(), Some(expected));
        }
        for input in [
            "",
            "ftp:21",
            "file:///tmp/example",
            "mailto:alice@example.com",
            "javascript:alert(1)",
            "data:text/plain,a",
            "view-source:https://example.test/",
            "chrome://settings/",
        ] {
            assert_eq!(resolve_address(input, &settings), None);
        }
    }

    #[test]
    fn custom_search_validates_the_template_authority() {
        let mut settings = ClientSettingsSchema {
            browser_search_engine: BrowserSearchEngine::Custom,
            browser_custom_search_url: "https://search.example/?q=%s&lang=en".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_address("a&b", &settings).as_deref(),
            Some("https://search.example/?q=a%26b&lang=en")
        );
        settings.browser_custom_search_url = "https://%s@search.example/?q=%s".into();
        assert_eq!(
            resolve_address("a&b", &settings).as_deref(),
            Some("https://duckduckgo.com/?q=a%26b")
        );
    }
}
