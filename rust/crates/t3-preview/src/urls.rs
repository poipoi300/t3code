use crate::{Error, Result};
use t3_contracts::{preview::*, preview_automation::*};
use url::Url;

pub fn is_loopback_host(host: &str) -> bool {
    matches!(
        host,
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]"
    )
}

pub fn normalize_preview_url(raw: &str) -> Result<String> {
    let s = raw.trim();
    let fail = |reason: &str| Error::InvalidUrl {
        reason: reason.into(),
        input_length: raw.encode_utf16().count(),
    };
    if s.is_empty() {
        return Err(fail("empty"));
    }
    if let Some((scheme, _)) = s.split_once(':')
        && matches!(
            scheme.to_ascii_lowercase().as_str(),
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
        )
    {
        return Err(fail("unsupported-protocol"));
    }
    let scheme = s.split_once("://").map(|(scheme, _)| scheme);
    let candidate = if scheme.is_some_and(|scheme| {
        !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    }) {
        s.to_owned()
    } else {
        let lower = s.to_ascii_lowercase();
        let local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]
            .iter()
            .any(|host| {
                lower
                    .strip_prefix(host)
                    .is_some_and(|tail| tail.is_empty() || tail.starts_with([':', '/']))
            });
        format!("{}://{s}", if local { "http" } else { "https" })
    };
    let url = Url::parse(&candidate).map_err(|_| fail("parse"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(fail("unsupported-protocol"));
    }
    if url.host_str().is_none() {
        return Err(fail("parse"));
    }
    Ok(url.to_string())
}

pub fn configured_urls(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for s in raw.iter().take(CONFIGURED_LOCAL_SERVER_URLS_MAX_ITEMS) {
        if s.encode_utf16().count() > PREVIEW_URL_MAX_LENGTH {
            continue;
        }
        let Ok(mut url) = Url::parse(s) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || !url.host_str().is_some_and(is_loopback_host)
        {
            continue;
        }
        if url.as_str().encode_utf16().count() > PREVIEW_URL_MAX_LENGTH {
            continue;
        }
        if url.host_str() == Some("0.0.0.0") && url.set_host(Some("localhost")).is_err() {
            continue;
        }
        let s = url.to_string();
        if s.encode_utf16().count() <= PREVIEW_URL_MAX_LENGTH && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

pub fn validate_viewport(setting: &PreviewViewportSetting) -> Result<()> {
    let (w, h) = match setting {
        PreviewViewportSetting::Fill {} => return Ok(()),
        PreviewViewportSetting::Freeform { width, height }
        | PreviewViewportSetting::Preset { width, height, .. } => (*width, *height),
    };
    if !(240..=3840).contains(&w)
        || !(240..=3840).contains(&h)
        || w * h > PREVIEW_VIEWPORT_MAX_AREA as i64
    {
        return Err(Error::InvalidInput(
            "Viewport exceeds dimension or area limits".into(),
        ));
    }
    Ok(())
}

pub fn resolve_viewport(input: PreviewAutomationResizeInput) -> Result<PreviewViewportSetting> {
    if input.mode == PreviewAutomationResizeMode::Fill {
        return Ok(PreviewViewportSetting::Fill {});
    }
    let setting = if input.mode == PreviewAutomationResizeMode::Preset
        && let Some(preset) = input.preset.flatten()
    {
        let (mut width, mut height) = match preset {
            PreviewViewportPresetId::IphoneSe => (375, 667),
            PreviewViewportPresetId::IphoneXr => (414, 896),
            PreviewViewportPresetId::Iphone12Pro => (390, 844),
            PreviewViewportPresetId::Iphone14ProMax => (430, 932),
            PreviewViewportPresetId::Pixel7 | PreviewViewportPresetId::SamsungGalaxyS20Ultra => {
                (412, 915)
            }
            PreviewViewportPresetId::SamsungGalaxyS8Plus => (360, 740),
            PreviewViewportPresetId::IpadMini => (768, 1024),
            PreviewViewportPresetId::IpadAir => (820, 1180),
            PreviewViewportPresetId::IpadPro => (1024, 1366),
            PreviewViewportPresetId::SurfacePro7 => (912, 1368),
            PreviewViewportPresetId::SurfaceDuo => (540, 720),
            PreviewViewportPresetId::GalaxyZFold5 => (344, 882),
            PreviewViewportPresetId::AsusZenbookFold => (853, 1280),
            PreviewViewportPresetId::SamsungGalaxyA5171 => (412, 914),
            PreviewViewportPresetId::NestHub => (1024, 600),
            PreviewViewportPresetId::NestHubMax => (1280, 800),
        };
        if matches!(
            input.orientation.flatten(),
            Some(PreviewAutomationOrientation::Landscape)
        ) && height >= width
            || matches!(
                input.orientation.flatten(),
                Some(PreviewAutomationOrientation::Portrait)
            ) && height < width
        {
            std::mem::swap(&mut width, &mut height);
        }
        let preset_id = serde_json::from_value(serde_json::to_value(preset)?)?;
        PreviewViewportSetting::Preset {
            width,
            height,
            preset_id,
        }
    } else {
        PreviewViewportSetting::Freeform {
            width: input
                .width
                .flatten()
                .ok_or_else(|| Error::InvalidInput("Custom viewport requires width".into()))?,
            height: input
                .height
                .flatten()
                .ok_or_else(|| Error::InvalidInput("Custom viewport requires height".into()))?,
        }
    };
    validate_viewport(&setting)?;
    Ok(setting)
}

pub fn navigation_url(input: &PreviewAutomationNavigateInput) -> Result<String> {
    if let Some(url) = input.url.as_ref().and_then(Option::as_ref) {
        return normalize_preview_url(url);
    }
    match input
        .target
        .as_ref()
        .and_then(Option::as_ref)
        .ok_or_else(|| Error::InvalidInput("Expected URL or target".into()))?
    {
        BrowserNavigationTarget::Url { url } => normalize_preview_url(url),
        BrowserNavigationTarget::EnvironmentPort {
            port,
            protocol,
            path,
        } => {
            if *port == 0 {
                return Err(Error::InvalidInput("Port must be positive".into()));
            }
            let scheme = if protocol == &Some(Some(BrowserNavigationProtocol::Https)) {
                "https"
            } else {
                "http"
            };
            let path = path
                .as_ref()
                .and_then(Option::as_ref)
                .map(String::as_str)
                .unwrap_or("");
            Ok(format!(
                "{scheme}://localhost:{port}{}{path}",
                if path.is_empty() || path.starts_with('/') {
                    ""
                } else {
                    "/"
                }
            ))
        }
    }
}

pub fn profile_directory(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    let mut encoded = String::new();
    for b in id.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(b) {
            encoded.push(*b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    if id == "." || id == ".." {
        encoded = encoded.replace('.', "%2E");
    }
    root.join(encoded)
}
