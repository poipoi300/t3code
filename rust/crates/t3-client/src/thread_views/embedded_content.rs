//! Port of packages/shared/src/{htmlRender,mcpApp}.ts and apps/web/src/components/chat/HtmlRenderFrame.tsx helpers.

use crate::view_logic::{JsTrim, is_js_whitespace};
use serde_json::Value;
use t3_contracts::assets::{AssetDisposition, AssetRenderIntent, AssetResource};
use t3_contracts::base_schemas::TrimmedNonEmptyString;

pub const HTML_RENDER_COLUMN_WIDTH: f64 = 728.0;
pub const HTML_RENDER_MIN_HEIGHT: f64 = 80.0;
pub const HTML_RENDER_MAX_HEIGHT: f64 = 2000.0;
pub const HTML_RENDER_MAX_FIT_HEIGHT: f64 = 20_000.0;
pub const MIN_URL_LIFE_MS: f64 = 300_000.0;
pub const MCP_APP_DEFAULT_HEIGHT: f64 = 320.0;
pub const MCP_APP_MAX_HEIGHT: f64 = 2000.0;
pub const MAX_DOWNLOAD_BYTES: usize = 25 * 1024 * 1024;

/// A reference already normalized by the timeline's resource projection.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlRenderReference {
    pub attachment_id: String,
    pub title: String,
    pub height: f64,
    pub heights: Vec<(f64, f64)>,
}

impl HtmlRenderReference {
    pub fn from_projection(value: &Value) -> Option<Self> {
        let attachment_id = value.get("attachmentId")?.as_str()?;
        let title = value.get("title")?.as_str()?;
        let height = value.get("height")?.as_f64()?;
        if attachment_id.is_empty() || !height.is_finite() {
            return None;
        }
        let heights = value
            .get("heights")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        let pair = entry.as_array().filter(|pair| pair.len() == 2)?;
                        Some((pair[0].as_f64()?, pair[1].as_f64()?))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or(Some(Vec::new()))?;
        Some(Self {
            attachment_id: attachment_id.into(),
            title: title.into(),
            height,
            heights,
        })
    }

    pub fn asset_resource(&self) -> AssetResource {
        AssetResource::Attachment {
            attachment_id: TrimmedNonEmptyString(self.attachment_id.clone()),
            file_name: Some(TrimmedNonEmptyString(html_render_file_name(&self.title))),
            mime_type: Some(TrimmedNonEmptyString("text/html".into())),
            disposition: Some(AssetDisposition::Inline),
            render_intent: None,
        }
    }
}

fn measured_height(heights: &[(f64, f64)], width: f64) -> f64 {
    let high = heights
        .iter()
        .position(|(measured, _)| *measured >= width)
        .unwrap_or(heights.len() - 1);
    let low = if heights[high].0 == width {
        high
    } else {
        high.saturating_sub(1)
    };
    heights[low].1.max(heights[high].1)
}

pub fn html_render_frame_height(
    reference: &HtmlRenderReference,
    width: f64,
    content_height: Option<f64>,
) -> f64 {
    if let Some(height) = content_height {
        return height
            .ceil()
            .clamp(HTML_RENDER_MIN_HEIGHT, HTML_RENDER_MAX_FIT_HEIGHT);
    }
    if reference.heights.is_empty() {
        return clamp_html_render_height(reference.height);
    }
    let cap = if measured_height(&reference.heights, HTML_RENDER_COLUMN_WIDTH) > reference.height {
        reference.height
    } else {
        HTML_RENDER_MAX_HEIGHT
    };
    clamp_html_render_height(cap.min(measured_height(&reference.heights, width)))
}

pub fn clamp_html_render_height(height: f64) -> f64 {
    height
        .round()
        .clamp(HTML_RENDER_MIN_HEIGHT, HTML_RENDER_MAX_HEIGHT)
}

pub fn html_render_file_name(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = replaced
        .split(is_js_whitespace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // Rust strings cannot retain the lone surrogate that JS slice can leave at this boundary.
    let mut units = 0;
    let name: String = collapsed
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 120
        })
        .collect();
    let name = name.js_trim();
    format!("{}.html", if name.is_empty() { "Page" } else { name })
}

pub fn read_html_render_content_height(message: &Value) -> Option<f64> {
    if message.get("jsonrpc")?.as_str()? != "2.0"
        || message.get("method")?.as_str()? != "ui/notifications/size-changed"
    {
        return None;
    }
    message
        .get("params")?
        .get("height")?
        .as_f64()
        .filter(|h| h.is_finite() && *h > 0.0)
}

/// The inline frame never follows a re-minted URL; a full-screen document may need a fresh one.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedHtmlRender {
    pub src: String,
    pub expires_at: f64,
}

impl LoadedHtmlRender {
    pub fn expansion_src(&self, now: f64) -> Option<&str> {
        (self.expires_at - now > MIN_URL_LIFE_MS).then_some(self.src.as_str())
    }
}

/// The timeline has already authenticated this reference against the owning tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct McpAppReference {
    pub attachment_id: String,
    pub server: String,
    pub tool: String,
    pub resource_uri: String,
    pub csp: Option<Value>,
    pub permissions: Option<Value>,
    pub prefers_border: bool,
}

impl McpAppReference {
    pub fn from_projection(value: &Value) -> Option<Self> {
        Some(Self {
            attachment_id: value.get("attachmentId")?.as_str()?.into(),
            server: value.get("server")?.as_str()?.into(),
            tool: value.get("tool")?.as_str()?.into(),
            resource_uri: value.get("resourceUri")?.as_str()?.into(),
            csp: value.get("csp").cloned(),
            permissions: value.get("permissions").cloned(),
            prefers_border: value.get("prefersBorder").and_then(Value::as_bool) == Some(true),
        })
    }

    pub fn asset_resource(&self) -> AssetResource {
        AssetResource::Attachment {
            attachment_id: TrimmedNonEmptyString(self.attachment_id.clone()),
            file_name: Some(TrimmedNonEmptyString(mcp_app_file_name(&self.tool))),
            mime_type: Some(TrimmedNonEmptyString("text/html".into())),
            disposition: Some(AssetDisposition::Inline),
            render_intent: Some(AssetRenderIntent::McpApp),
        }
    }

    pub fn allow_attribute(&self) -> String {
        [
            ("camera", "camera"),
            ("microphone", "microphone"),
            ("geolocation", "geolocation"),
            ("clipboardWrite", "clipboard-write"),
        ]
        .into_iter()
        .filter_map(|(key, feature)| {
            self.permissions
                .as_ref()?
                .get(key)?
                .is_object()
                .then_some(feature)
        })
        .collect::<Vec<_>>()
        .join("; ")
    }
}

pub fn clamp_mcp_app_height(height: f64) -> f64 {
    if !height.is_finite() {
        return MCP_APP_DEFAULT_HEIGHT;
    }
    height.clamp(80.0, MCP_APP_MAX_HEIGHT).round()
}

pub fn mcp_app_file_name(tool: &str) -> String {
    let mut name = String::new();
    let mut replacing = false;
    for c in tool.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            name.push(c);
            replacing = false;
        } else if !replacing {
            name.push('-');
            replacing = true;
        }
        if name.len() == 80 {
            break;
        }
    }
    format!("{}.html", if name.is_empty() { "app" } else { &name })
}

pub fn mcp_app_initial_url(src: &str, appearance: &str) -> Option<String> {
    if !matches!(appearance, "dark" | "light") {
        return None;
    }
    let mut url = url::Url::parse(src).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let mut replaced = false;
    let mut pairs: Vec<_> = url
        .query_pairs()
        .filter_map(|(key, value)| {
            if key == "t3-color-scheme" {
                if replaced {
                    return None;
                }
                replaced = true;
                return Some((key.into_owned(), appearance.to_owned()));
            }
            Some((key.into_owned(), value.into_owned()))
        })
        .collect();
    if !replaced {
        pairs.push(("t3-color-scheme".into(), appearance.into()));
    }
    url.query_pairs_mut().clear().extend_pairs(pairs);
    Some(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reference() -> HtmlRenderReference {
        HtmlRenderReference::from_projection(&json!({
            "attachmentId": "page", "title": "Chart", "height": 400,
            "heights": [[320, 900], [728, 400], [1144, 300]]
        }))
        .expect("client reference")
    }

    #[test]
    fn frame_fits_live_page_before_measured_breakpoints_and_caps() {
        let mut page = reference();
        for (width, expected) in [
            (200.0, 900.0),
            (320.0, 900.0),
            (500.0, 900.0),
            (728.0, 400.0),
            (900.0, 400.0),
            (1500.0, 400.0),
        ] {
            assert_eq!(html_render_frame_height(&page, width, None), expected);
        }
        page.height = 250.0;
        assert_eq!(html_render_frame_height(&page, 320.0, None), 250.0);
        for (height, expected) in [
            (20.0, 80.0),
            (300.1, 301.0),
            (5000.0, 5000.0),
            (30_000.0, 20_000.0),
        ] {
            assert_eq!(
                html_render_frame_height(&page, 320.0, Some(height)),
                expected
            );
        }
        page.heights.clear();
        page.height = 3000.0;
        assert_eq!(html_render_frame_height(&page, 390.0, None), 2000.0);
    }

    #[test]
    fn height_messages_require_the_protocol_method_and_a_positive_number() {
        let valid = json!({"jsonrpc":"2.0", "method":"ui/notifications/size-changed", "params":{"height":5000}});
        assert_eq!(read_html_render_content_height(&valid), Some(5000.0));
        for patch in [
            json!({"jsonrpc":"1.0"}),
            json!({"method":"other"}),
            json!({"params":{"height":"5000"}}),
            json!({"params":{"height":0}}),
            json!({"params":{"height":-1}}),
        ] {
            let mut value = valid.clone();
            value
                .as_object_mut()
                .expect("object")
                .extend(patch.as_object().expect("patch").clone());
            assert_eq!(read_html_render_content_height(&value), None);
        }
    }

    #[test]
    fn attachment_scope_and_file_names_match_the_frame() {
        for (title, expected) in [
            ("  Chart / Q4 : \"Sales\"  ", "Chart Q4 Sales.html"),
            ("/\u{0000}:?", "Page.html"),
            ("résumé", "résumé.html"),
        ] {
            assert_eq!(html_render_file_name(title), expected);
        }
        assert_eq!(
            html_render_file_name(&"x".repeat(121)),
            format!("{}.html", "x".repeat(120))
        );
        let resource = serde_json::to_value(reference().asset_resource()).expect("resource");
        assert_eq!(
            resource,
            json!({"_tag":"attachment", "attachmentId":"page", "fileName":"Chart.html", "mimeType":"text/html", "disposition":"inline"})
        );
    }

    #[test]
    fn expanding_requires_strictly_more_than_five_minutes_of_url_life() {
        let loaded = LoadedHtmlRender {
            src: "https://environment.test/signed".into(),
            expires_at: 600_000.0,
        };
        assert_eq!(loaded.expansion_src(299_999.0), Some(loaded.src.as_str()));
        assert_eq!(loaded.expansion_src(300_000.0), None);
        assert_eq!(loaded.expansion_src(600_001.0), None);
    }

    #[test]
    fn mcp_assets_require_the_sandboxed_app_render_intent() {
        let app = McpAppReference::from_projection(&json!({"attachmentId":"app", "server":"Settings", "tool":"get weather/now", "resourceUri":"ui://weather/card", "permissions":{"camera":{}, "clipboardWrite":{}, "microphone":false}, "prefersBorder":true})).expect("projected app");
        assert_eq!(app.server, "Settings");
        assert_eq!(app.allow_attribute(), "camera; clipboard-write");
        assert!(app.prefers_border);
        assert_eq!(
            serde_json::to_value(app.asset_resource()).expect("resource"),
            json!({"_tag":"attachment", "attachmentId":"app", "fileName":"get-weather-now.html", "mimeType":"text/html", "disposition":"inline", "renderIntent":"mcp-app"})
        );
    }

    #[test]
    fn mcp_height_and_file_name_match_the_web_limits() {
        for (height, expected) in [
            (f64::NAN, 320.0),
            (f64::INFINITY, 320.0),
            (-1.0, 80.0),
            (80.5, 81.0),
            (20_000.0, 2000.0),
        ] {
            assert_eq!(clamp_mcp_app_height(height), expected);
        }
        for (tool, expected) in [
            ("", "app.html"),
            ("___a.b-c", "___a.b-c.html"),
            ("é☁ /weather", "-weather.html"),
        ] {
            assert_eq!(mcp_app_file_name(tool), expected);
        }
        assert_eq!(
            mcp_app_file_name(&"x".repeat(100)),
            format!("{}.html", "x".repeat(80))
        );
    }

    #[test]
    fn initial_scheme_preserves_the_asset_grant_and_fragment() {
        let src =
            "https://host.test/page?grant=a%2Fb&t3-color-scheme=dark&t3-color-scheme=light#anchor";
        let next = mcp_app_initial_url(src, "light").expect("asset URL");
        let next = url::Url::parse(&next).expect("URL");
        assert_eq!(next.fragment(), Some("anchor"));
        assert_eq!(
            next.query_pairs().collect::<Vec<_>>(),
            [
                ("grant".into(), "a/b".into()),
                ("t3-color-scheme".into(), "light".into())
            ]
        );
        assert!(mcp_app_initial_url("javascript:alert(1)", "dark").is_none());
        assert!(mcp_app_initial_url(src, "normal").is_none());
        assert_eq!(
            mcp_app_initial_url(
                "https://host.test/page?t3-color-scheme=dark&grant=a&t3-color-scheme=dark",
                "light"
            )
            .as_deref(),
            Some("https://host.test/page?t3-color-scheme=light&grant=a")
        );
    }
}
