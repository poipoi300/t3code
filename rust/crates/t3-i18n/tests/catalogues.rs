use std::collections::BTreeSet;

use t3_i18n::{
    Catalogues, DEFAULT_LOCALE, DataFormat, Message, SUPPORTED_LOCALES, Surface, Translator,
    message_key,
};

const SURFACES: [Surface; 2] = [Surface::Web, Surface::Mobile];

/// Source keys a language is allowed to lack: `(surface, locale, key)`. Empty today; the
/// completeness test below fails when a key goes missing without being listed here.
const KNOWN_MISSING: &[(Surface, &str, &str)] = &[];

fn listed_missing(surface: Surface, locale: &str) -> BTreeSet<&'static str> {
    KNOWN_MISSING
        .iter()
        .filter(|(s, l, _)| *s == surface && *l == locale)
        .map(|(_, _, key)| *key)
        .collect()
}

#[test]
fn every_source_key_exists_in_every_language_or_is_listed_missing() {
    for surface in SURFACES {
        let catalogues = Catalogues::get(surface);
        for locale in SUPPORTED_LOCALES {
            let missing: BTreeSet<&str> = catalogues.missing_keys(locale).into_iter().collect();
            assert_eq!(
                missing,
                listed_missing(surface, locale),
                "{surface:?} {locale}"
            );
            assert!(
                catalogues.orphan_keys(locale).is_empty(),
                "{surface:?} {locale} has keys English lacks"
            );
        }
    }
}

#[test]
fn key_counts() {
    let count = |surface, locale: &str| {
        Catalogues::get(surface)
            .catalogue(locale)
            .map_or(0, |c| c.len())
    };
    for locale in SUPPORTED_LOCALES {
        assert_eq!(count(Surface::Web, locale), 72, "web {locale}");
        assert_eq!(count(Surface::Mobile, locale), 32, "mobile {locale}");
        assert_eq!(count(Surface::Unified, locale), 81, "unified {locale}");
    }
}

#[test]
fn every_message_parses_and_keeps_the_source_variables() {
    for surface in SURFACES {
        let catalogues = Catalogues::get(surface);
        for (key, source) in catalogues.source().iter() {
            let expected = Message::parse(source)
                .expect("source parses")
                .argument_names();
            for locale in SUPPORTED_LOCALES {
                let text = catalogues
                    .catalogue(locale)
                    .and_then(|c| c.get(key))
                    .expect("translated");
                let parsed = Message::parse(text)
                    .unwrap_or_else(|e| panic!("{surface:?} {locale} {key}: {e}"));
                assert_eq!(
                    parsed.argument_names(),
                    expected,
                    "{surface:?} {locale} {key}"
                );
            }
        }
    }
}

#[test]
fn keys_are_the_hash_of_their_english_source() {
    let mut formats = (0, 0);
    for surface in SURFACES {
        let source = Catalogues::get(surface).source();
        for (key, text) in source.iter() {
            let icu = message_key(text, DataFormat::Icu, None) == key;
            let jsx = message_key(text, DataFormat::Jsx, None) == key;
            assert!(icu ^ jsx, "{surface:?} {key} {text:?}");
            assert_eq!(source.lookup(text), Some(text));
            formats.0 += usize::from(icu);
            formats.1 += usize::from(jsx);
        }
    }
    assert_eq!(formats, (93, 11));
}

#[test]
fn gt_keys_match_generaltranslation() {
    #[derive(serde::Deserialize)]
    struct Case {
        source: String,
        #[serde(rename = "dataFormat")]
        data_format: String,
        context: Option<String>,
        key: String,
    }
    #[derive(serde::Deserialize)]
    struct File {
        cases: Vec<Case>,
    }
    let file: File = serde_json::from_str(include_str!("fixtures/keys.json")).expect("fixture");
    for case in file.cases {
        let format = if case.data_format == "ICU" {
            DataFormat::Icu
        } else {
            DataFormat::Jsx
        };
        assert_eq!(
            message_key(&case.source, format, case.context.as_deref()),
            case.key,
            "{:?}",
            case.source
        );
    }
}

/// The checked-in copies must equal the apps' catalogues; skipped where `apps/` is absent.
#[test]
fn data_files_equal_the_apps_catalogues() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../..");
    let mut compared = 0;
    for (surface, dir) in [
        ("web", "apps/web/src/i18n/translations"),
        ("mobile", "apps/mobile/src/i18n/translations"),
    ] {
        for locale in SUPPORTED_LOCALES {
            let app = root.join(dir).join(format!("{locale}.json"));
            let Ok(expected) = std::fs::read(&app) else {
                continue;
            };
            let copy = std::fs::read(manifest.join(format!("data/{surface}/{locale}.json")))
                .expect("data file");
            assert_eq!(
                copy,
                expected,
                "{surface} {locale} drifted from {}",
                app.display()
            );
            compared += 1;
        }
    }
    eprintln!("compared {compared} catalogue files with the apps");
}

#[test]
fn unified_catalogue_prefers_web_and_reports_drift() {
    let drift = Catalogues::drift("fr");
    assert_eq!(drift.len(), 5);
    let unified = Catalogues::get(Surface::Unified)
        .catalogue("fr")
        .expect("fr");
    for entry in &drift {
        assert_eq!(unified.get(&entry.key), Some(entry.web.as_str()));
    }
    let totals: Vec<usize> = SUPPORTED_LOCALES
        .iter()
        .map(|locale| Catalogues::drift(locale).len())
        .collect();
    assert_eq!(totals, vec![0, 1, 1, 5, 1, 1, 3, 5, 2]);
}

#[test]
fn translator_follows_gts_fallback_chain() {
    let fr = Translator::new(Surface::Mobile, "fr");
    assert_eq!(fr.t("Settings"), "Réglages");
    assert_eq!(
        fr.translate("No settings match “{query}”.", &[("query", "abc".into())]),
        "Aucun réglage ne correspond à «\u{a0}abc\u{a0}».",
    );
    // Not in any catalogue: the English source, formatted with English rules.
    assert_eq!(
        fr.translate("Hello {name}", &[("name", "Ada".into())]),
        "Hello Ada"
    );
    // A missing variable fails the catalogue entry and then the source: the raw source returns.
    assert_eq!(
        fr.t("No settings match “{query}”."),
        "No settings match “{query}”."
    );
    // English and unshipped languages never read a catalogue.
    for locale in ["en", "sv", ""] {
        assert_eq!(
            Translator::new(Surface::Web, locale).t("Settings"),
            "Settings",
            "{locale:?}"
        );
    }
    assert_eq!(Translator::new(Surface::Web, "ja").t(""), "");
}

#[test]
fn translator_text_reads_jsx_entries_verbatim() {
    let de = Translator::new(Surface::Web, "de");
    let source = "System default follows your browser or OS language.";
    assert_ne!(de.text(source), source);
    assert_eq!(de.text("Untranslated {braces}"), "Untranslated {braces}");
    assert_eq!(
        Translator::new(Surface::Web, DEFAULT_LOCALE).text(source),
        source
    );
}
