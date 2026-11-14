#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_contracts::{model::*, server::*};
use t3_provider::{catalog::*, versions::*};

fn manifest() -> Manifest {
    Manifest::parse(&json!({"version":1,"currentModels":{},"providers":{"test":{"defaults":{"chat":"new"},"profiles":{"p":{"capabilities":{"optionDescriptors":[]}}},"models":[{"slug":"old","name":"Old","status":"legacy"},{"slug":"new","name":"New","status":"current","profile":"p","releasedAt":"2026-01-01"}]}}}).to_string()).unwrap()
}

#[test]
fn bundled_manifest_and_provider_catalogs_validate() {
    let bundled = Manifest::bundled().unwrap();
    for (driver, catalog) in bundled.providers.as_ref().unwrap() {
        assert_eq!(
            bundled.catalog(driver, 0).unwrap().models.len(),
            catalog.models.len()
        );
    }
    assert!(bundled.catalog("missing", 0).is_none());
}

#[test]
fn badges_expire_at_seven_days_without_refresh_and_never_mark_legacy() {
    let manifest = manifest();
    for (now, badge) in [
        ("2025-12-31T23:59:59Z", false),
        ("2026-01-01T00:00:00Z", true),
        ("2026-01-07T23:59:59.999Z", true),
        ("2026-01-08T00:00:00Z", false),
    ] {
        let catalog = manifest.catalog("test", date_millis(now).unwrap()).unwrap();
        assert_eq!(catalog.models[1].model.badge.is_some(), badge);
        assert!(catalog.models[0].model.badge.is_none());
    }
}

#[test]
fn manifest_rejects_duplicate_slugs_missing_profiles_and_nonexistent_defaults() {
    let base = serde_json::to_value(manifest()).unwrap();
    for change in ["duplicate", "profile", "default"] {
        let mut bad = base.clone();
        match change {
            "duplicate" => bad["providers"]["test"]["models"][1]["slug"] = json!("old"),
            "profile" => bad["providers"]["test"]["models"][1]["profile"] = json!("missing"),
            _ => bad["providers"]["test"]["defaults"]["chat"] = json!("missing"),
        }
        assert!(Manifest::parse(&bad.to_string()).is_err());
    }
}

#[test]
fn manifest_rejects_invalid_claude_runtime_metadata_and_reversed_version_bounds() {
    for adapter in [
        json!({"claudeCode":{"minVersion":"invalid"}}),
        json!({"claudeCode":{"minVersion":"2.0.0","maxVersionExclusive":"1.0.0"}}),
    ] {
        let value = json!({"version":1,"currentModels":{},"providers":{"claudeAgent":{"profiles":{},"models":[{"slug":"m","name":"Model","status":"current","adapter":adapter}]}}});
        assert!(Manifest::parse(&value.to_string()).is_err());
    }
    let value = json!({"version":1,"currentModels":{},"providers":{"claudeAgent":{"profiles":{"p":{"adapter":{"claudeCode":{"effortMap":{"high":4}}}}},"models":[]}}});
    assert!(Manifest::parse(&value.to_string()).is_err());
}

#[test]
fn manifest_normalizes_trimmed_fields_and_rejects_empty_optional_names_and_aliases() {
    let value = json!({"version":1,"currentModels":{},"providers":{"test":{"defaults":{"chat":" m "},"profiles":{},"models":[{"slug":" m ","name":" Model ","status":"current","shortName":" M ","aliases":[" alias "]}]}}});
    let manifest = Manifest::parse(&value.to_string()).unwrap();
    let catalog = manifest.catalog("test", 0).unwrap();
    assert_eq!(catalog.models[0].model.slug.0, "m");
    assert_eq!(catalog.models[0].model.name.0, "Model");
    let mut bad = value.clone();
    bad["providers"]["test"]["models"][0]["aliases"] = json!([" "]);
    assert!(Manifest::parse(&bad.to_string()).is_err());
}

#[tokio::test]
async fn failed_manifest_refresh_keeps_current_catalog_and_backs_off() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        assert_ne!(socket.read(&mut bytes).await.unwrap(), 0);
        socket.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
    });
    let service = ModelManifest::new(reqwest::Client::new(), None)
        .unwrap()
        .with_url(format!("http://{address}"));
    let before = service.current().await;
    assert!(
        service
            .refresh(0, false, false, true)
            .await
            .unwrap()
            .is_none()
    );
    server.await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &service.current().await));
    assert!(
        service
            .refresh(1, false, false, true)
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn catalog_moves_default_and_aliases_but_preserves_custom_legacy_flags() {
    let catalog = manifest().catalog("test", 0).unwrap();
    let models = vec![serde_json::from_value(json!({"slug":"old","name":"Old","isCustom":false,"isDefault":true,"aliases":["default"],"capabilities":null})).unwrap(),model("new","New",None),serde_json::from_value(json!({"slug":"mine","name":"Mine","isCustom":true,"isLegacy":true,"capabilities":null})).unwrap()];
    let next = apply_catalog(&models, Some(&catalog), |s| s);
    assert_eq!(next[0].is_legacy, Some(Some(true)));
    assert!(next[0].is_default.is_none());
    assert!(next[0].aliases.is_none());
    assert_eq!(next[1].is_default, Some(Some(true)));
    assert_eq!(
        next[1].aliases.as_ref().unwrap().as_ref().unwrap()[0].0,
        "default"
    );
    assert_eq!(next[2], models[2]);
    let cleared = apply_catalog(&next, None, |s| s);
    assert!(cleared[0].is_legacy.is_none());
    assert_eq!(cleared[2], models[2]);
}

#[test]
fn qualified_discovered_models_keep_native_ids_when_catalog_default_moves() {
    let catalog = Manifest::parse(&json!({"version":1,"currentModels":{},"providers":{"codex":{"defaults":{"chat":"gpt-new"},"profiles":{},"models":[{"slug":"gpt-old","name":"Old","status":"legacy"},{"slug":"gpt-new","name":"New","status":"current","adapter":{"codex":{"minVersion":"1.2.0"}}}]}}}).to_string()).unwrap().catalog("codex",0).unwrap();
    let mut old = model("openai.gpt-old", "Old", None);
    old.is_default = Some(Some(true));
    let discovered = vec![old, model("openai.gpt-new", "New", None)];
    let next = apply_catalog(&discovered, Some(&catalog), codex_family);
    assert_eq!(next[1].slug.0, "openai.gpt-new");
    assert_eq!(next[1].is_default, Some(Some(true)));
    assert!(codex_update_required(&next, Some(&catalog), Some("1.1.0")).is_empty());
    let required = codex_update_required(&next[..1], Some(&catalog), Some("1.1.0"));
    assert_eq!(required[0].min_version.0, "1.2.0");
    assert!(codex_update_required(&next[..1], Some(&catalog), Some("1.2.0")).is_empty());
}

#[test]
fn custom_models_trim_deduplicate_rename_builtins_and_do_not_inherit_aliases() {
    let mut built = model("gpt-real", "Original", None);
    built.short_name = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "Short".into(),
    )));
    let caps = capabilities(vec![boolean_option("fastMode", "Fast", Some(true))]);
    let result = custom_models(
        &[built],
        &[
            json!({"slug":"gpt-real","name":"Mine"}),
            json!("  alias  "),
            json!({"slug":"alias","name":"ignored"}),
            json!("  "),
            json!({"slug":"custom","capabilities":{"optionDescriptors":[]}}),
        ],
        Some(&caps),
    );
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].name.0, "Mine");
    assert!(result[0].short_name.is_none());
    assert!(!result[0].is_custom);
    assert_eq!(result[1].slug.0, "alias");
    assert_eq!(result[1].capabilities, Some(caps));
    assert!(result[1].is_custom);
    assert_eq!(
        descriptors(result[2].capabilities.as_ref().unwrap()).len(),
        0
    );
}

#[test]
fn descriptors_preserve_explicit_false_fall_back_invalid_choices_and_handle_prompt_injection() {
    let mut effort = select_option(
        "effort",
        "Effort",
        &[("low", "Low"), ("high", "High"), ("ultrathink", "Ultra")],
        Some("low"),
    );
    if let ProviderOptionDescriptor::Select(effort) = &mut effort {
        effort.prompt_injected_values = Some(Some(vec!["ultrathink".into()]));
    }
    let caps = capabilities(vec![effort, boolean_option("fastMode", "Fast", Some(true))]);
    for value in ["invalid", "ultrathink", " "] {
        let selected = option_descriptors(
            &caps,
            &[
                ProviderOptionSelection {
                    id: "effort".into(),
                    value: ProviderOptionSelectionValue::String(value.into()),
                },
                ProviderOptionSelection {
                    id: "fastMode".into(),
                    value: ProviderOptionSelectionValue::Boolean(false),
                },
            ],
        );
        assert!(
            matches!(&selected[0],ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("low".into())))
        );
        assert!(
            matches!(&selected[1],ProviderOptionDescriptor::Boolean(d) if d.current_value == Some(Some(false)))
        );
    }
    let implicit = implicit_fast_mode(&caps, &[]);
    assert_eq!(implicit.len(), 1);
    assert_eq!(
        implicit[0].value,
        ProviderOptionSelectionValue::Boolean(false)
    );
}

#[test]
fn legacy_ultracode_effort_migrates_to_xhigh_and_toggle() {
    let caps = capabilities(vec![
        select_option(
            "effort",
            "Effort",
            &[("high", "High"), ("xhigh", "Extra High")],
            Some("high"),
        ),
        boolean_option("ultracode", "Ultracode", None),
    ]);
    let options = option_descriptors(
        &caps,
        &[ProviderOptionSelection {
            id: "effort".into(),
            value: ProviderOptionSelectionValue::String("ultracode".into()),
        }],
    );
    assert!(
        matches!(&options[0],ProviderOptionDescriptor::Select(d) if d.current_value == Some(Some("xhigh".into())))
    );
    assert!(
        matches!(&options[1],ProviderOptionDescriptor::Boolean(d) if d.current_value == Some(Some(true)))
    );
}

#[test]
fn compatibility_preserves_first_matching_range_and_unknown_prereleases() {
    let policy:CompatibilityPolicy = serde_json::from_value(json!({"driver":"codex","t3CodeRange":">=0.0.42 <0.1.0","recommendedVersion":"2.0.0","ranges":[{"range":"<1.0.0","status":"broken"},{"range":">=1.0.0 <1.5.0","status":"unsupported"},{"range":">=1.5.0 <2.0.0","status":"graceful"},{"range":">=2.0.0 <3.0.0","status":"supported"}]})).unwrap();
    validate_compatibility_policy(&policy).unwrap();
    for (version, expected) in [
        ("0.9.9", ServerProviderCompatibilityStatus::Broken),
        ("1.0.0", ServerProviderCompatibilityStatus::Unsupported),
        ("1.5.0", ServerProviderCompatibilityStatus::Graceful),
        ("v2.0.0", ServerProviderCompatibilityStatus::Supported),
        ("2.0.0-beta.1", ServerProviderCompatibilityStatus::Unknown),
    ] {
        assert_eq!(
            resolve_compatibility(
                std::slice::from_ref(&policy),
                "codex",
                Some(version),
                "0.0.45"
            )
            .unwrap()
            .status,
            expected
        );
    }
    assert!(resolve_compatibility(&[policy], "codex", Some("2.0.0"), "0.1.0").is_none());
}

#[test]
fn provider_release_tags_and_muse_revision_comparison_follow_typescript() {
    for (driver, version, expected) in [
        (
            "cursor",
            "2026.05.09-a1b2c3d",
            ServerProviderCompatibilityStatus::Supported,
        ),
        (
            "cursor",
            "2026.05.09-beta.1",
            ServerProviderCompatibilityStatus::Unknown,
        ),
        (
            "antigravity",
            "agy_acp_server_1.5.7",
            ServerProviderCompatibilityStatus::Supported,
        ),
        (
            "muse",
            "1.4.6-R12.2",
            ServerProviderCompatibilityStatus::Supported,
        ),
    ] {
        let policy:CompatibilityPolicy = serde_json::from_value(json!({"driver":driver,"t3CodeRange":">=0.0.0","ranges":[{"range":">=0.0.0","status":"supported"}]})).unwrap();
        assert_eq!(
            resolve_compatibility(&[policy], driver, Some(version), "0.0.99")
                .unwrap()
                .status,
            expected
        );
    }
    assert!(compare_muse_versions("1.4.6-R9", "1.4.6-R10").is_lt());
    assert!(compare_muse_versions("1.4.6-R1", "1.4.6").is_eq());
    assert_eq!(
        parse_muse_version("muse version 1.4.6-R10.2").as_deref(),
        Some("1.4.6-R10.2")
    );
    assert_eq!(
        parse_codex_version("codex-cli 1.2.3-beta.2").as_deref(),
        Some("1.2.3-beta.2")
    );
    assert!(parse_codex_version("wrapper1.2.3").is_none());
}

#[test]
fn capability_listing_keeps_custom_and_pinned_models_with_stable_preferences_and_search() {
    let mut provider: ServerProvider = serde_json::from_value(json!({"instanceId":"opencode-work","driver":"opencode","displayName":"Work","enabled":true,"installed":true,"version":"1.0.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-04-10T00:00:00.000Z","models":[],"skills":[],"slashCommands":[]})).unwrap();
    provider.models = (1..=12)
        .map(|index| {
            let mut model = model(
                &format!("oc/m{index:02}"),
                &format!("Model {index:02}"),
                None,
            );
            model.is_legacy = (index == 3).then_some(Some(true));
            model
        })
        .collect();
    let mut custom = model("custom", "Custom", None);
    custom.is_custom = true;
    custom.is_legacy = Some(Some(true));
    provider.models.push(custom);
    let preferences =
        serde_json::from_value(json!({"hiddenModels":["oc/m02","custom"],"modelOrder":["oc/m12"]}))
            .unwrap();
    let pinned = |model: &ServerProviderModel| model.is_custom || model.slug.0 == "oc/m10";
    let slugs = |listing: CapabilityModels<'_>| {
        (
            listing
                .listed
                .iter()
                .map(|model| model.slug.0.clone())
                .collect::<Vec<_>>(),
            listing.omitted,
        )
    };
    assert_eq!(
        slugs(list_capability_models(
            &provider,
            Some(&preferences),
            &[],
            pinned
        )),
        (
            [
                "oc/m12", "oc/m01", "oc/m04", "oc/m05", "oc/m06", "oc/m07", "oc/m08", "oc/m09",
                "oc/m10", "custom"
            ]
            .map(str::to_owned)
            .to_vec(),
            2
        )
    );
    assert_eq!(
        slugs(list_capability_models(
            &provider,
            Some(&preferences),
            &["oc/m".into()],
            pinned
        )),
        (
            [
                "oc/m12", "oc/m01", "oc/m03", "oc/m04", "oc/m05", "oc/m06", "oc/m07", "oc/m08"
            ]
            .map(str::to_owned)
            .to_vec(),
            3
        )
    );
    assert_eq!(
        slugs(list_capability_models(
            &provider,
            Some(&preferences),
            &["open".into()],
            pinned
        )),
        slugs(list_capability_models(
            &provider,
            Some(&preferences),
            &[],
            pinned
        ))
    );
    assert_eq!(
        slugs(list_capability_models(
            &provider,
            Some(&preferences),
            &["work".into(), "03".into()],
            pinned
        )),
        (vec!["oc/m03".into()], 0)
    );
    provider.driver = "codex".into();
    assert_eq!(
        list_capability_models(&provider, Some(&preferences), &[], pinned).omitted,
        1
    );
    let duplicate =
        serde_json::from_value(json!({"modelOrder":["oc/m01","oc/m12","oc/m01"]})).unwrap();
    assert_eq!(
        list_capability_models(&provider, Some(&duplicate), &[], pinned).listed[0]
            .slug
            .0,
        "oc/m12"
    );
}
