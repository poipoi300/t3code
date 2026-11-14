//! `TextGeneratorBaseUrl` decodes as TypeScript's schema decides. Each case was recorded by
//! decoding the URL with the real `TextGeneratorConfig` of `packages/contracts/src/settings.ts`.
use t3_contracts::settings::TextGeneratorConfig;

#[test]
fn base_urls_decode_as_typescript_decides() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("text_generator_base_urls.json")).expect("cases");
    assert_eq!(cases.len(), 49);
    let mismatches: Vec<_> = cases
        .iter()
        .filter(|case| {
            let decoded = serde_json::from_value::<TextGeneratorConfig>(serde_json::json!({
                "kind": "openaiCompatible",
                "baseUrl": case["baseUrl"],
            }));
            decoded.is_ok() != case["valid"].as_bool().expect("valid")
        })
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
