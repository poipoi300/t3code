//! How a client presents itself when pairing, so the host can label its sessions. Ports
//! `apps/web/src/connection/clientMetadata.ts` (web, desktop) and
//! `apps/mobile/src/lib/authClientMetadata.ts` (mobile).
use t3_contracts::{
    auth::{AuthClientMetadataDeviceType as DeviceType, AuthClientPresentationMetadata},
    base_schemas::{ClientSurface, ClientWebDeployment, TrimmedNonEmptyString},
};

/// What a browser says about itself.
#[derive(Debug, Clone, Default)]
pub struct BrowserIdentity {
    pub user_agent: String,
    pub platform: String,
    pub max_touch_points: u32,
}

/// The desktop app. `platform` is the OS name as Rust's `std::env::consts::OS` gives it.
pub fn desktop(platform: &str, app_version: &str) -> AuthClientPresentationMetadata {
    let os = match platform {
        "macos" | "darwin" => "macOS",
        "windows" | "win32" => "Windows",
        "linux" => "Linux",
        "" => "unknown",
        _ => "other",
    };
    AuthClientPresentationMetadata {
        label: text("T3 Code Desktop"),
        device_type: Some(DeviceType::Desktop),
        os: text(os),
        surface: Some(ClientSurface::Desktop),
        app_version: version(app_version),
        ..empty()
    }
}

/// The web app, served by the environment (`hosted: false`) or from the hosted site.
pub fn web(
    identity: &BrowserIdentity,
    hosted: bool,
    app_version: &str,
) -> AuthClientPresentationMetadata {
    AuthClientPresentationMetadata {
        label: text("T3 Code Web"),
        device_type: Some(browser_device_type(identity)),
        os: text(browser_client_os(identity)),
        surface: Some(ClientSurface::Web),
        web_deployment: Some(if hosted {
            ClientWebDeployment::Hosted
        } else {
            ClientWebDeployment::Server
        }),
        browser: text(browser_family(&identity.user_agent)),
        app_version: version(app_version),
        ..empty()
    }
}

/// The mobile app. `os` is `"ios"` or `"android"`; `os_version` as the OS reports it.
pub fn mobile(
    os: &str,
    os_version: Option<&str>,
    device_model: Option<&str>,
    device_type: DeviceType,
    app_version: Option<&str>,
) -> AuthClientPresentationMetadata {
    let os_major_version = os_version
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.trim().parse::<i64>().ok())
        .filter(|major| *major > 0);
    AuthClientPresentationMetadata {
        label: text("T3 Code Mobile"),
        device_type: Some(match device_type {
            DeviceType::Tablet | DeviceType::Mobile => device_type,
            _ => DeviceType::Unknown,
        }),
        os: match os {
            "ios" => text("iOS"),
            "android" => text("Android"),
            _ => None,
        },
        os_major_version,
        device_model: device_model.and_then(text),
        surface: Some(ClientSurface::Mobile),
        app_version: app_version.and_then(text),
        ..empty()
    }
}

fn empty() -> AuthClientPresentationMetadata {
    AuthClientPresentationMetadata {
        label: None,
        device_type: None,
        os: None,
        os_major_version: None,
        device_model: None,
        surface: None,
        web_deployment: None,
        browser: None,
        app_version: None,
    }
}

fn text(value: &str) -> Option<TrimmedNonEmptyString> {
    let value = value.trim();
    (!value.is_empty()).then(|| TrimmedNonEmptyString(value.to_owned()))
}

/// Development builds report `0.0.0`, which the host should not show.
fn version(app_version: &str) -> Option<TrimmedNonEmptyString> {
    (app_version != "0.0.0")
        .then(|| text(app_version))
        .flatten()
}

fn has(haystack: &str, needles: &[&str]) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    needles.iter().any(|needle| haystack.contains(needle))
}

fn is_ipados_desktop(identity: &BrowserIdentity) -> bool {
    has(&identity.user_agent, &["macintosh"])
        && has(&identity.platform, &["mac"])
        && identity.max_touch_points > 1
}

pub fn browser_client_os(identity: &BrowserIdentity) -> &'static str {
    let agent = identity.user_agent.as_str();
    if agent.trim().is_empty() {
        return "unknown";
    }
    if has(agent, &["iphone", "ipad", "ipod"]) || is_ipados_desktop(identity) {
        "iOS"
    } else if has(agent, &["android"]) {
        "Android"
    } else if has(agent, &["cros"]) {
        "ChromeOS"
    } else if has(agent, &["windows"]) {
        "Windows"
    } else if has(agent, &["macintosh", "mac os x"]) {
        "macOS"
    } else if has(agent, &["linux", "x11"]) {
        "Linux"
    } else {
        "other"
    }
}

fn browser_family(agent: &str) -> &'static str {
    let lower = agent.to_ascii_lowercase();
    if agent.trim().is_empty() {
        "unknown"
    } else if ["edg/", "edge/", "edga/", "edgios/"]
        .iter()
        .any(|n| lower.contains(n))
    {
        "Edge"
    } else if has(agent, &["opr/", "opios/"]) {
        "Opera"
    } else if has(agent, &["samsungbrowser/"]) {
        "Samsung Internet"
    } else if has(agent, &["firefox/", "fxios/"]) {
        "Firefox"
    } else if has(agent, &["chrome/", "crios/"]) {
        "Chrome"
    } else if has(agent, &["safari/"]) {
        "Safari"
    } else {
        "other"
    }
}

fn browser_device_type(identity: &BrowserIdentity) -> DeviceType {
    let agent = identity.user_agent.as_str();
    if agent.trim().is_empty() {
        return DeviceType::Unknown;
    }
    let lower = agent.to_ascii_lowercase();
    if has(agent, &["ipad", "tablet", "kindle", "silk"])
        || (lower.contains("android") && !lower.contains("mobile"))
        || is_ipados_desktop(identity)
    {
        DeviceType::Tablet
    } else if has(agent, &["iphone", "ipod", "mobile"]) {
        // `android.+mobile` is covered by `mobile`.
        DeviceType::Mobile
    } else {
        DeviceType::Desktop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(user_agent: &str, platform: &str, touch: u32) -> BrowserIdentity {
        BrowserIdentity {
            user_agent: user_agent.into(),
            platform: platform.into(),
            max_touch_points: touch,
        }
    }

    #[test]
    fn each_surface_presents_itself_as_the_typescript_clients_do() {
        let desktop = serde_json::to_value(super::desktop("windows", "1.4.0")).unwrap();
        assert_eq!(
            desktop,
            serde_json::json!({"label":"T3 Code Desktop","deviceType":"desktop","os":"Windows","surface":"desktop","appVersion":"1.4.0"})
        );
        assert!(
            super::desktop("linux", "0.0.0").app_version.is_none(),
            "dev builds hide their version"
        );

        let chrome = identity(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36",
            "Win32",
            0,
        );
        assert_eq!(
            serde_json::to_value(super::web(&chrome, true, "1.4.0")).unwrap(),
            serde_json::json!({"label":"T3 Code Web","deviceType":"desktop","os":"Windows","surface":"web","webDeployment":"hosted","browser":"Chrome","appVersion":"1.4.0"})
        );

        let phone = super::mobile(
            "ios",
            Some("18.2.1"),
            Some(" iPhone 16 "),
            DeviceType::Mobile,
            Some("1.4.0"),
        );
        assert_eq!(
            serde_json::to_value(phone).unwrap(),
            serde_json::json!({"label":"T3 Code Mobile","deviceType":"mobile","os":"iOS","osMajorVersion":18,"deviceModel":"iPhone 16","surface":"mobile","appVersion":"1.4.0"})
        );
        let unknown = super::mobile("android", Some("beta"), None, DeviceType::Desktop, None);
        assert_eq!(unknown.device_type, Some(DeviceType::Unknown));
        assert_eq!(unknown.os_major_version, None);
    }

    #[test]
    fn browsers_are_told_apart_by_their_user_agent() {
        for (agent, platform, touch, os, family, device) in [
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.1 Safari/605.1.15",
                "MacIntel",
                5,
                "iOS",
                "Safari",
                DeviceType::Tablet,
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.1 Safari/605.1.15",
                "MacIntel",
                0,
                "macOS",
                "Safari",
                DeviceType::Desktop,
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/131.0 Mobile Safari/537.36",
                "Linux armv8l",
                5,
                "Android",
                "Chrome",
                DeviceType::Mobile,
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SM-X710) Chrome/131.0 Safari/537.36",
                "Linux",
                5,
                "Android",
                "Chrome",
                DeviceType::Tablet,
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0) Chrome/131.0 Safari/537.36 Edg/131.0",
                "Win32",
                0,
                "Windows",
                "Edge",
                DeviceType::Desktop,
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) Chrome/131.0 Safari/537.36",
                "Linux",
                0,
                "ChromeOS",
                "Chrome",
                DeviceType::Desktop,
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
                "Linux x86_64",
                0,
                "Linux",
                "Firefox",
                DeviceType::Desktop,
            ),
            ("", "", 0, "unknown", "unknown", DeviceType::Unknown),
        ] {
            let identity = identity(agent, platform, touch);
            let presented = super::web(&identity, false, "1.0.0");
            assert_eq!(browser_client_os(&identity), os, "{agent}");
            assert_eq!(browser_family(agent), family, "{agent}");
            assert_eq!(presented.device_type, Some(device), "{agent}");
        }
    }
}
