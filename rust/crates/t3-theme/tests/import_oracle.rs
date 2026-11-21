//! VS Code and Open VSX import against the real TypeScript (fixtures/import-oracle.json,
//! written by tools/import-oracle.mjs), plus ports of the scenarios in
//! `vscodeThemeImport.test.ts` and `openVsxThemes.test.ts`.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use serde::Deserialize;
use serde_json::{Value, json};
use t3_theme::import::{
    Cancel, HttpError, HttpMethod, HttpResponse, NoCancel, OpenVsxClient, OpenVsxError,
    OpenVsxHttp, OpenVsxThemeExtension, OpenVsxThemeSort, humanize_theme_name,
    is_vscode_theme_file, pair_vscode_themes, parse_theme_import, parse_vscode_theme_file,
    resolve_theme_label_collisions,
};
use t3_theme::palette::{Appearance, ThemeDefinition, theme_color_to_hex};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}

#[derive(Deserialize)]
struct Outcome {
    ok: Option<Value>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct VsCodeCase {
    input: Value,
    #[serde(rename = "isVsCode")]
    is_vscode: bool,
    parsed: Outcome,
}

#[derive(Deserialize)]
struct TextCase {
    source: String,
    parsed: Outcome,
}

#[derive(Deserialize)]
struct PairingCase {
    themes: Vec<ThemeDefinition>,
    plain: Vec<ThemeDefinition>,
    #[serde(rename = "withId")]
    with_id: Vec<ThemeDefinition>,
}

#[derive(Deserialize)]
struct CollisionEntry {
    theme: ThemeDefinition,
    #[serde(rename = "sourceName")]
    source_name: Option<String>,
}

#[derive(Deserialize)]
struct CollisionCase {
    entries: Vec<CollisionEntry>,
    out: Vec<ThemeDefinition>,
}

#[derive(Deserialize)]
struct HumanizeCase {
    raw: String,
    out: String,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    extension: OpenVsxThemeExtension,
    package: String,
    checksum: String,
    advertised: Value,
    result: Outcome,
}

#[derive(Deserialize)]
struct Oracle {
    #[serde(rename = "vscodeFiles")]
    vscode_files: Vec<VsCodeCase>,
    #[serde(rename = "importTexts")]
    import_texts: Vec<TextCase>,
    pairing: Vec<PairingCase>,
    collisions: Vec<CollisionCase>,
    humanize: Vec<HumanizeCase>,
    scenarios: Vec<Scenario>,
}

fn oracle() -> Oracle {
    serde_json::from_str(include_str!("../fixtures/import-oracle.json")).expect("fixture parses")
}

fn compare(
    label: &str,
    rust: Result<ThemeDefinition, t3_theme::palette::ThemeError>,
    expected: &Outcome,
) {
    match (rust, &expected.ok, &expected.error) {
        (Ok(theme), Some(ts), None) => assert_eq!(
            &serde_json::to_value(theme).expect("serialises"),
            ts,
            "{label}"
        ),
        (Err(error), None, Some(message)) => {
            // JSON.parse reports engine-specific syntax errors; the port reports one stable message.
            let expected =
                if message.contains(" in JSON ") || message.starts_with("Unexpected token") {
                    "That theme file is not valid JSON."
                } else {
                    message.as_str()
                };
            assert_eq!(error.0, expected, "{label}");
        }
        (rust, ts, error) => panic!("{label}: rust {rust:?} ts {ts:?} {error:?}"),
    }
}

#[test]
fn vscode_themes_convert_like_typescript() {
    let cases = oracle().vscode_files;
    assert!(cases.len() > 150);
    for case in cases {
        assert_eq!(
            is_vscode_theme_file(&case.input),
            case.is_vscode,
            "{}",
            case.input
        );
        compare(
            &case.input.to_string(),
            parse_vscode_theme_file(&case.input),
            &case.parsed,
        );
    }
}

#[test]
fn theme_import_text_matches_typescript() {
    for case in oracle().import_texts {
        compare(&case.source, parse_theme_import(&case.source), &case.parsed);
    }
}

#[test]
fn pairing_collisions_and_names_match_typescript() {
    let o = oracle();
    for case in o.pairing {
        assert_eq!(pair_vscode_themes(&case.themes, None), case.plain);
        let make_id = |light: &ThemeDefinition, dark: &ThemeDefinition| {
            format!("paired-{}-{}", light.id, dark.id)
        };
        assert_eq!(
            pair_vscode_themes(&case.themes, Some(&make_id)),
            case.with_id
        );
    }
    for case in o.collisions {
        let entries: Vec<(ThemeDefinition, Option<&str>)> = case
            .entries
            .iter()
            .map(|e| (e.theme.clone(), e.source_name.as_deref()))
            .collect();
        assert_eq!(resolve_theme_label_collisions(&entries), case.out);
    }
    for case in o.humanize {
        assert_eq!(humanize_theme_name(&case.raw), case.out, "{:?}", case.raw);
    }
}

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        _ => 63,
    };
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|c| *c != b'=') {
        buffer = (buffer << 6) | value(c);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}

/// Serves fixed bodies by URL, like the `fetch` stubs in the TypeScript tests.
struct StubHttp {
    routes: Vec<(String, Option<HttpMethod>, HttpResponse)>,
    seen: RefCell<Vec<(HttpMethod, String)>>,
    on_request: Box<dyn Fn(&str)>,
}

impl StubHttp {
    fn new(routes: Vec<(String, Option<HttpMethod>, HttpResponse)>) -> Self {
        Self {
            routes,
            seen: RefCell::default(),
            on_request: Box::new(|_| {}),
        }
    }
}

fn body(status: u16, text: &str) -> HttpResponse {
    HttpResponse {
        status,
        content_length: None,
        body: text.as_bytes().to_vec(),
    }
}

impl OpenVsxHttp for StubHttp {
    fn request(
        &self,
        method: HttpMethod,
        url: &str,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> {
        self.seen.borrow_mut().push((method, url.to_owned()));
        (self.on_request)(url);
        let found = self
            .routes
            .iter()
            .find(|(route, m, _)| {
                (url == route
                    || url.ends_with(route.as_str())
                    || (route.ends_with('?') && url.starts_with(route.as_str())))
                    && m.is_none_or(|m| m == method)
            })
            .map(|(_, _, response)| response.clone());
        async move { found.ok_or_else(|| HttpError(format!("no route for {url}"))) }
    }
}

#[test]
fn extension_imports_match_typescript() {
    let scenarios = oracle().scenarios;
    assert!(scenarios.len() >= 20);
    for scenario in scenarios {
        let package = base64_decode(&scenario.package);
        let ext = &scenario.extension;
        let http = StubHttp::new(vec![
            (
                ext.manifest_url.clone(),
                None,
                body(200, &scenario.advertised.to_string()),
            ),
            (
                ext.sha256_url.clone(),
                None,
                body(200, &format!("{}  demo.vsix", scenario.checksum)),
            ),
            (
                ext.vsix_url.clone(),
                None,
                HttpResponse {
                    status: 200,
                    content_length: Some(package.len() as u64),
                    body: package,
                },
            ),
        ]);
        let result = block_on(OpenVsxClient::new(http).import(ext, &NoCancel));
        match (&result, &scenario.result.ok, &scenario.result.error) {
            (Ok(themes), Some(expected), None) => {
                assert_eq!(
                    &serde_json::to_value(themes).expect("serialises"),
                    expected,
                    "{}",
                    scenario.name
                );
            }
            (Err(error), None, Some(message)) => {
                assert_eq!(&error.to_string(), message, "{}", scenario.name);
            }
            _ => panic!(
                "{}: rust {result:?} ts {:?}",
                scenario.name, scenario.result.error
            ),
        }
    }
}

#[test]
fn imported_pairs_keep_search_colours_and_readable_ids() {
    let scenario = oracle()
        .scenarios
        .into_iter()
        .find(|s| s.name == "ok")
        .expect("ok scenario");
    let themes: Vec<ThemeDefinition> =
        serde_json::from_value(scenario.result.ok.expect("themes")).expect("themes parse");
    assert_eq!(themes.len(), 2);
    assert!(themes.iter().all(|t| {
        t.collection
            .as_ref()
            .is_some_and(|c| c.id == "open-vsx:demo.theme")
    }));
    assert!(themes.iter().all(|t| {
        let rest = t.id.strip_prefix("ovx-theme-").expect("prefix");
        rest.len() == 12 && rest.bytes().all(|b| b.is_ascii_hexdigit())
    }));
    let paired = themes
        .iter()
        .find(|t| t.modes().len() == 2)
        .expect("paired theme");
    let dark = paired.colors_for(Appearance::Dark).expect("dark");
    let hex = |value: &str| theme_color_to_hex(value).expect("colour");
    assert_eq!(paired.label, "Demo");
    assert_eq!(
        hex(paired.colors.get(t3_theme::palette::ThemeColorRole::Canvas)),
        "#fafafa"
    );
    assert_eq!(
        hex(dark.get(t3_theme::palette::ThemeColorRole::Canvas)),
        "#111111"
    );
    assert_eq!(
        hex(dark.get(t3_theme::palette::ThemeColorRole::SearchMatchActiveBackground)),
        "#663399"
    );
    assert_eq!(
        hex(dark.get(t3_theme::palette::ThemeColorRole::SearchMatchBackground)),
        "#2f2f2f"
    );
}

const ASSET_ROOT: &str = "https://open-vsx.org/api/demo/theme/1.0.0/file";

fn extension_detail(namespace: &str, license: &str) -> Value {
    let root = ASSET_ROOT.replace("demo/theme", &format!("{namespace}/theme"));
    json!({
        "namespace": namespace, "name": "theme", "displayName": "Demo Theme", "description": "A nice theme",
        "version": "1.0.0", "downloadCount": 123456, "license": license, "repository": "https://github.com/demo/theme",
        "files": {
            "icon": format!("{root}/icon.png"),
            "manifest": format!("{root}/package.json"),
            "sha256": format!("{root}/demo.theme-1.0.0.sha256"),
            "download": format!("{root}/demo.theme-1.0.0.vsix"),
        }
    })
}

fn route(
    url: &str,
    method: Option<HttpMethod>,
    response: HttpResponse,
) -> (String, Option<HttpMethod>, HttpResponse) {
    (url.to_owned(), method, response)
}

#[test]
fn search_keeps_only_supported_open_source_licences() {
    let theme_manifest =
        json!({ "license": "MIT", "contributes": { "themes": [{ "path": "./theme.json" }] } })
            .to_string();
    let head = |length: Option<u64>, status: u16| HttpResponse {
        status,
        content_length: length,
        body: Vec::new(),
    };
    let detail_of = |namespace: &str, license: &str, download: &str| {
        let mut value = extension_detail(namespace, license);
        let root = ASSET_ROOT.replace("demo/theme", &format!("{namespace}/theme"));
        value["files"]["download"] = json!(format!("{root}/{download}"));
        value.to_string()
    };
    let search = json!({ "extensions": [
        {"namespace":"demo","name":"theme"},{"namespace":"icons","name":"theme"},{"namespace":"oversized","name":"theme"},
        {"namespace":"unlicensed","name":"theme"},{"namespace":"unavailable","name":"theme"},{"namespace":"closed","name":"theme"},
        {"namespace":"huge","name":"theme"}
    ]})
    .to_string();
    let huge = json!({ "padding": "x".repeat(256 * 1024) }).to_string();
    let routes = vec![
        route(
            "https://open-vsx.org/api/icons/theme",
            None,
            body(200, &detail_of("icons", "MIT", "demo.theme-1.0.0.vsix")),
        ),
        route(
            "https://open-vsx.org/api/oversized/theme",
            None,
            body(200, &detail_of("oversized", "MIT", "oversized.vsix")),
        ),
        route(
            "https://open-vsx.org/api/unlicensed/theme",
            None,
            body(
                200,
                &detail_of("unlicensed", "MIT", "demo.theme-1.0.0.vsix"),
            ),
        ),
        route(
            "https://open-vsx.org/api/unavailable/theme",
            None,
            body(200, &detail_of("unavailable", "MIT", "unavailable.vsix")),
        ),
        route(
            "https://open-vsx.org/api/closed/theme",
            None,
            body(
                200,
                &extension_detail("closed", "All Rights Reserved").to_string(),
            ),
        ),
        route(
            "https://open-vsx.org/api/huge/theme",
            None,
            body(200, &huge),
        ),
        route(
            "https://open-vsx.org/api/demo/theme",
            None,
            body(200, &detail_of("demo", "MIT", "demo.theme-1.0.0.vsix")),
        ),
        route(
            "/oversized/theme/1.0.0/file/oversized.vsix",
            Some(HttpMethod::Head),
            head(Some(20 * 1024 * 1024 + 1), 200),
        ),
        route(
            "/unavailable/theme/1.0.0/file/unavailable.vsix",
            Some(HttpMethod::Head),
            head(None, 404),
        ),
        route(".vsix", Some(HttpMethod::Head), head(Some(1024), 200)),
        route(
            "/icons/theme/1.0.0/file/package.json",
            None,
            body(200, &json!({"contributes":{"iconThemes":[{}]}}).to_string()),
        ),
        route(
            "/unlicensed/theme/1.0.0/file/package.json",
            None,
            body(
                200,
                &json!({"contributes":{"themes":[{"path":"./theme.json"}]}}).to_string(),
            ),
        ),
        route(
            "/unavailable/theme/1.0.0/file/package.json",
            None,
            body(200, &theme_manifest),
        ),
        route(
            "/demo/theme/1.0.0/file/package.json",
            None,
            body(200, &theme_manifest),
        ),
        route(
            "https://open-vsx.org/api/-/search?",
            None,
            body(200, &search),
        ),
    ];
    let http = StubHttp::new(routes);
    let client = OpenVsxClient::new(&http);
    let results =
        block_on(client.search("  dracula  ", OpenVsxThemeSort::DownloadCount, &NoCancel))
            .expect("search");
    assert_eq!(results.len(), 1);
    let found = &results[0];
    assert_eq!(found.id, "demo.theme");
    assert_eq!(found.name, "Demo Theme");
    assert_eq!(found.publisher, "demo");
    assert_eq!(found.download_count, 123_456.0);
    assert_eq!(
        found.icon_url.as_deref(),
        Some(format!("{ASSET_ROOT}/icon.png").as_str())
    );
    assert_eq!(
        found.source_url.as_deref(),
        Some("https://github.com/demo/theme")
    );
    assert_eq!(found.license, "MIT");
    assert_eq!(found.collection_id, "open-vsx:demo.theme");
    let first = http.seen.borrow()[0].1.clone();
    let url = url::Url::parse(&first).expect("url");
    let query: std::collections::BTreeMap<String, String> =
        url.query_pairs().into_owned().collect();
    assert_eq!(query["query"], "dracula");
    assert_eq!(query["category"], "Themes");
    assert_eq!(query["sortBy"], "downloadCount");
    assert_eq!(query["sortOrder"], "desc");
    assert_eq!(query["size"], "16");
}

#[test]
fn search_reports_unavailable_when_every_detail_request_fails() {
    let http = StubHttp::new(vec![
        route("https://open-vsx.org/api/demo/theme", None, body(429, "")),
        route(
            "https://open-vsx.org/api/-/search?",
            None,
            body(
                200,
                &json!({"extensions":[{"namespace":"demo","name":"theme"}]}).to_string(),
            ),
        ),
    ]);
    let error = block_on(OpenVsxClient::new(&http).search(
        "dracula",
        OpenVsxThemeSort::DownloadCount,
        &NoCancel,
    ))
    .expect_err("unavailable");
    assert!(
        error
            .to_string()
            .contains("Open VSX theme details are unavailable"),
        "{error}"
    );
}

struct CancelAfterChecksum<'a>(&'a Cell<bool>);

impl Cancel for CancelAfterChecksum<'_> {
    fn is_cancelled(&self) -> bool {
        self.0.get()
    }
}

#[test]
fn import_stops_when_cancelled() {
    let package = vec![1u8, 2, 3];
    let flag = Cell::new(false);
    let extension = OpenVsxThemeExtension {
        collection_id: "open-vsx:demo.theme".into(),
        id: "demo.theme".into(),
        name: "Demo Theme".into(),
        publisher: "demo".into(),
        description: String::new(),
        download_count: 1.0,
        icon_url: None,
        source_url: None,
        license: "MIT".into(),
        manifest_url: format!("{ASSET_ROOT}/package.json"),
        sha256_url: format!("{ASSET_ROOT}/demo.theme-1.0.0.sha256"),
        version: "1.0.0".into(),
        vsix_url: format!("{ASSET_ROOT}/demo.theme-1.0.0.vsix"),
    };
    let mut http = StubHttp::new(vec![
        route(
            &extension.manifest_url,
            None,
            body(
                200,
                &json!({"contributes":{"themes":[{"path":"./theme.json"}]}}).to_string(),
            ),
        ),
        route(&extension.sha256_url, None, body(200, &"0".repeat(64))),
        route(
            &extension.vsix_url,
            None,
            HttpResponse {
                status: 200,
                content_length: None,
                body: package,
            },
        ),
    ]);
    let sha_url = extension.sha256_url.clone();
    let flag_ref: &'static Cell<bool> = Box::leak(Box::new(Cell::new(false)));
    http.on_request = Box::new(move |url| {
        if url == sha_url {
            flag_ref.set(true);
        }
    });
    let _ = &flag;
    let result =
        block_on(OpenVsxClient::new(http).import(&extension, &CancelAfterChecksum(flag_ref)));
    assert_eq!(result.expect_err("cancelled"), OpenVsxError::Aborted);
}

/// A stored-entries ZIP writer for building oversized archives.
fn zip_of(entries: &[(&str, &[u8])], comment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let directory_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend_from_slice(comment);
    out
}

#[test]
fn zip_reader_enforces_its_limits() {
    use t3_theme::import::zip::{MAX_ZIP_ENTRIES, ZipArchive};
    let many: Vec<String> = (0..=MAX_ZIP_ENTRIES)
        .map(|i| format!("extension/n/{i}.js"))
        .collect();
    let entries: Vec<(&str, &[u8])> = many.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let error = ZipArchive::open(&zip_of(&entries, b""))
        .expect_err("too many")
        .0;
    assert_eq!(error, "That extension package has too many files.");

    // Many entries under the cap, a comment that imitates a directory record, and a theme file.
    let junk: Vec<String> = (0..3_000)
        .map(|i| format!("extension/node_modules/package-{i}.js"))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = junk.iter().map(|n| (n.as_str(), &b""[..])).collect();
    entries.push(("extension/themes/a.json", b"{\"colors\":{}}"));
    let comment = [b"PK\x05\x06".as_slice(), &[b'x'; 26]].concat();
    let zip = ZipArchive::open(&zip_of(&entries, &comment)).expect("opens despite the comment");
    assert_eq!(
        zip.read_text("extension/themes/a.json", "a").expect("text"),
        "{\"colors\":{}}"
    );
    assert_eq!(
        zip.read_text("extension/missing.json", "Theme")
            .expect_err("missing")
            .0,
        "Theme is missing from the extension package."
    );

    let big = vec![b'x'; 256 * 1024 + 1];
    let zip = ZipArchive::open(&zip_of(&[("extension/big.json", &big)], b"")).expect("opens");
    assert_eq!(
        zip.read_text("extension/big.json", "Big")
            .expect_err("large")
            .0,
        "Big is too large."
    );

    let slip = zip_of(&[("extension/../../evil.json", b"{}")], b"");
    assert!(ZipArchive::open(&slip).is_err());
    assert!(ZipArchive::open(b"short").is_err());
}

impl OpenVsxHttp for &StubHttp {
    fn request(
        &self,
        method: HttpMethod,
        url: &str,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> {
        StubHttp::request(self, method, url)
    }
}

#[test]
fn zip_checked_offsets_and_lengths() {
    use t3_theme::import::zip::ZipArchive;
    let original = zip_of(&[("a", b"ok")], b"");
    let end = original.len() - 22;
    let central = end - 47;
    for (at, field) in [
        (end + 16, u32::MAX.to_le_bytes().to_vec()),
        (end + 12, u32::MAX.to_le_bytes().to_vec()),
        (central + 42, (u32::MAX - 1).to_le_bytes().to_vec()),
        (central + 30, u16::MAX.to_le_bytes().to_vec()),
        (28, u16::MAX.to_le_bytes().to_vec()),
        (end + 20, u16::MAX.to_le_bytes().to_vec()),
    ] {
        let mut bytes = original.clone();
        bytes[at..at + field.len()].copy_from_slice(&field);
        assert!(ZipArchive::open(&bytes).is_err(), "invalid field at {at}");
    }
}

#[test]
fn zip_checked_truncated_directory_and_entry_counts() {
    use t3_theme::import::zip::ZipArchive;
    let original = zip_of(&[("a", b"")], b"");
    let end = original.len() - 22;
    let mut truncated = original.clone();
    truncated.remove(end - 1);
    truncated[end - 1 + 12..end - 1 + 16].copy_from_slice(&46u32.to_le_bytes());
    assert!(ZipArchive::open(&truncated).is_err());
    for count in [0u16, 2, u16::MAX] {
        let mut bytes = original.clone();
        bytes[end + 8..end + 10].copy_from_slice(&count.to_le_bytes());
        bytes[end + 10..end + 12].copy_from_slice(&count.to_le_bytes());
        assert!(ZipArchive::open(&bytes).is_err(), "count {count}");
    }
}

#[test]
fn zip_checked_member_names() {
    use t3_theme::import::zip::ZipArchive;
    for path in ["/a", "C:/a", r"\a", r"extension\..\..\a"] {
        assert!(
            ZipArchive::open(&zip_of(&[(path, b"")], b"")).is_err(),
            "{path}"
        );
    }
}

#[test]
fn zip_checked_inflated_data_is_bounded_despite_underreported_size() {
    use t3_theme::import::zip::{MAX_THEME_BYTES, ZipArchive};
    let expanded = vec![0; MAX_THEME_BYTES + 1];
    let compressed = miniz_oxide::deflate::compress_to_vec(&expanded, 1);
    let mut bytes = zip_of(&[("a", &compressed)], b"");
    let central = bytes.len() - 22 - 47;
    for at in [8, central + 10] {
        bytes[at..at + 2].copy_from_slice(&8u16.to_le_bytes());
    }
    let archive = ZipArchive::open(&bytes).expect("metadata fits");
    assert_eq!(
        archive.read_text("a", "Theme").unwrap_err().0,
        "Theme is too large."
    );
}

#[test]
fn zip_checked_random_input_and_mutated_records_return_without_panicking() {
    use t3_theme::import::zip::ZipArchive;
    let seed = zip_of(&[("a", b"hello")], b"comment");
    let mut state = 20261121u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for n in 0..4096 {
        let mut bytes = if n % 2 == 0 {
            seed.clone()
        } else {
            vec![0; (next() % 256) as usize]
        };
        let changes = if n % 2 == 0 {
            1 + (next() % 8) as usize
        } else {
            bytes.len()
        };
        for i in 0..changes {
            let index = if n % 2 == 0 {
                next() as usize % bytes.len()
            } else {
                i
            };
            bytes[index] = next() as u8;
        }
        if let Ok(archive) = ZipArchive::open(&bytes) {
            let _ = archive.read_text("a", "Theme");
        }
    }
}

#[test]
fn zip_checked_extra_fields_cannot_exceed_their_header() {
    use t3_theme::import::zip::ZipArchive;
    for local in [false, true] {
        let mut bytes = zip_of(&[("a", b"")], b"");
        let central = 31;
        let end = central + 47;
        let (length, insertion) = if local {
            bytes[end + 16..end + 20].copy_from_slice(&35u32.to_le_bytes());
            (28, central)
        } else {
            bytes[end + 12..end + 16].copy_from_slice(&51u32.to_le_bytes());
            (central + 30, end)
        };
        bytes[length..length + 2].copy_from_slice(&4u16.to_le_bytes());
        bytes.splice(insertion..insertion, [0xfe, 0xca, 0, 0]);
        assert!(ZipArchive::open(&bytes).is_ok());
        bytes[insertion + 2] = 1;
        assert!(ZipArchive::open(&bytes).is_err());
    }
}
