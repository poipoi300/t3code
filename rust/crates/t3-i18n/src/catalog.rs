//! The message catalogues the web and mobile apps ship.
//!
//! The JSON files under `data/` are byte-for-byte copies of
//! `apps/{web,mobile}/src/i18n/translations/*.json` (the `gt` CLI output); `tests/catalogues.rs`
//! fails when a copy drifts from the app's file. Each file maps a GT key (see [`crate::hash`]) to
//! the message in that language; `en.json` maps keys to the English source.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::hash::{DataFormat, message_key};
use crate::locale::{DEFAULT_LOCALE, SUPPORTED_LOCALES};

/// Which app's catalogue. The two have separate files and have drifted (see
/// [`Catalogues::drift`]); the shared Rust UI will want [`Surface::Unified`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Web,
    Mobile,
    /// Web and mobile merged. A key in both takes the web translation.
    Unified,
}

/// One language's catalogue: GT key to message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    entries: BTreeMap<String, String>,
}

impl Catalogue {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// The message for English source text, which GT keys as an ICU string and, for plain-text
    /// `<T>` children, as JSX.
    pub fn lookup(&self, source: &str) -> Option<&str> {
        [DataFormat::Icu, DataFormat::Jsx]
            .into_iter()
            .find_map(|format| self.get(&message_key(source, format, None)))
    }
}

fn parse(json: &str) -> Catalogue {
    // The files are embedded and checked by tests; an unreadable one is an empty catalogue, which
    // falls back to English like a failed load does in the TypeScript.
    Catalogue {
        entries: serde_json::from_str(json).unwrap_or_default(),
    }
}

fn source_json(locale: &str, surface: Surface) -> Option<&'static str> {
    Some(match (locale, surface) {
        ("en", Surface::Web) => include_str!("../data/web/en.json"),
        ("de", Surface::Web) => include_str!("../data/web/de.json"),
        ("es", Surface::Web) => include_str!("../data/web/es.json"),
        ("fr", Surface::Web) => include_str!("../data/web/fr.json"),
        ("ja", Surface::Web) => include_str!("../data/web/ja.json"),
        ("ko", Surface::Web) => include_str!("../data/web/ko.json"),
        ("pt-BR", Surface::Web) => include_str!("../data/web/pt-BR.json"),
        ("ru", Surface::Web) => include_str!("../data/web/ru.json"),
        ("zh", Surface::Web) => include_str!("../data/web/zh.json"),
        ("en", Surface::Mobile) => include_str!("../data/mobile/en.json"),
        ("de", Surface::Mobile) => include_str!("../data/mobile/de.json"),
        ("es", Surface::Mobile) => include_str!("../data/mobile/es.json"),
        ("fr", Surface::Mobile) => include_str!("../data/mobile/fr.json"),
        ("ja", Surface::Mobile) => include_str!("../data/mobile/ja.json"),
        ("ko", Surface::Mobile) => include_str!("../data/mobile/ko.json"),
        ("pt-BR", Surface::Mobile) => include_str!("../data/mobile/pt-BR.json"),
        ("ru", Surface::Mobile) => include_str!("../data/mobile/ru.json"),
        ("zh", Surface::Mobile) => include_str!("../data/mobile/zh.json"),
        _ => return None,
    })
}

/// All catalogues of one surface, parsed once.
#[derive(Debug)]
pub struct Catalogues {
    by_locale: BTreeMap<&'static str, Catalogue>,
}

impl Catalogues {
    pub fn get(surface: Surface) -> &'static Catalogues {
        static WEB: OnceLock<Catalogues> = OnceLock::new();
        static MOBILE: OnceLock<Catalogues> = OnceLock::new();
        static UNIFIED: OnceLock<Catalogues> = OnceLock::new();
        match surface {
            Surface::Web => WEB.get_or_init(|| Self::load(Surface::Web)),
            Surface::Mobile => MOBILE.get_or_init(|| Self::load(Surface::Mobile)),
            Surface::Unified => UNIFIED.get_or_init(|| {
                let (web, mobile) = (Self::get(Surface::Web), Self::get(Surface::Mobile));
                let by_locale = SUPPORTED_LOCALES
                    .iter()
                    .map(|locale| {
                        let mut merged = mobile.catalogue(locale).cloned().unwrap_or_default();
                        if let Some(web) = web.catalogue(locale) {
                            merged.entries.extend(web.entries.clone());
                        }
                        (*locale, merged)
                    })
                    .collect();
                Catalogues { by_locale }
            }),
        }
    }

    fn load(surface: Surface) -> Self {
        let by_locale = SUPPORTED_LOCALES
            .iter()
            .filter_map(|locale| Some((*locale, parse(source_json(locale, surface)?))))
            .collect();
        Self { by_locale }
    }

    /// The catalogue of a shipped locale, `None` for any other tag.
    pub fn catalogue(&self, locale: &str) -> Option<&Catalogue> {
        self.by_locale.get(locale)
    }

    /// The English source: key to source text.
    pub fn source(&self) -> &Catalogue {
        self.by_locale.get(DEFAULT_LOCALE).unwrap_or_else(|| {
            static EMPTY: Catalogue = Catalogue {
                entries: BTreeMap::new(),
            };
            &EMPTY
        })
    }

    /// Source keys with no entry in `locale`, in key order. Empty when the locale is complete.
    pub fn missing_keys(&self, locale: &str) -> Vec<&str> {
        let translated = self.catalogue(locale);
        self.source()
            .iter()
            .map(|(key, _)| key)
            .filter(|key| translated.is_none_or(|catalogue| catalogue.get(key).is_none()))
            .collect()
    }

    /// Keys in `locale` that the English source lacks, which no message can ever request.
    pub fn orphan_keys(&self, locale: &str) -> Vec<&str> {
        self.catalogue(locale)
            .into_iter()
            .flat_map(Catalogue::iter)
            .map(|(key, _)| key)
            .filter(|key| self.source().get(key).is_none())
            .collect()
    }
}

/// A message both apps carry, translated differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub key: String,
    pub source: String,
    pub web: String,
    pub mobile: String,
}

impl Catalogues {
    /// Messages present in both the web and mobile catalogues of `locale` whose translations
    /// differ. A shared key has identical English by construction (the key hashes it).
    pub fn drift(locale: &str) -> Vec<Drift> {
        let (web, mobile) = (Self::get(Surface::Web), Self::get(Surface::Mobile));
        let (Some(web_locale), Some(mobile_locale)) =
            (web.catalogue(locale), mobile.catalogue(locale))
        else {
            return Vec::new();
        };
        mobile_locale
            .iter()
            .filter_map(|(key, mobile_text)| {
                let web_text = web_locale.get(key)?;
                (web_text != mobile_text).then(|| Drift {
                    key: key.to_owned(),
                    source: web.source().get(key).unwrap_or_default().to_owned(),
                    web: web_text.to_owned(),
                    mobile: mobile_text.to_owned(),
                })
            })
            .collect()
    }
}
