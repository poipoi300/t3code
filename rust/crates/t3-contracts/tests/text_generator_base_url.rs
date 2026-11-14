//! `TextGeneratorBaseUrl` decodes as TypeScript's schema decides. Each case was recorded by
//! decoding the URL with the real `TextGeneratorConfig` of `packages/contracts/src/settings.ts`.
use t3_contracts::settings::TextGeneratorConfig;

#[test]
fn base_urls_decode_as_typescript_decides() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("text_generator_base_urls.json")).expect("cases");
    assert_eq!(cases.len(), 49);
    for case in &cases {
        let url = case["baseUrl"].as_str().expect("url");
        let decoded = serde_json::from_value::<TextGeneratorConfig>(
            serde_json::json!({"kind": "openaiCompatible", "baseUrl": url}),
        );
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().expect("valid"),
            "{url:?}"
        );
    }
}
