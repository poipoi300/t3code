//! Locale selection, ported from `packages/client-runtime/src/locales.ts`.
//!
//! The saved language setting is `"system"` or one of [`SUPPORTED_LOCALES`]; the host passes its
//! language list (`navigator.languages`, the device's preferred languages) in.

pub const DEFAULT_LOCALE: &str = "en";
pub const SYSTEM_LANGUAGE: &str = "system";

/// `[defaultLocale, ...locales]` of both apps' `gt.config.json`.
pub const SUPPORTED_LOCALES: [&str; 9] = ["en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "zh"];

/// A language in its own script, so a user can find theirs from any UI language.
pub fn native_language_name(locale: &str) -> &str {
    match locale {
        "en" => "English",
        "de" => "Deutsch",
        "es" => "Español",
        "fr" => "Français",
        "ja" => "日本語",
        "ko" => "한국어",
        "pt-BR" => "Português (Brasil)",
        "ru" => "Русский",
        "zh" => "中文（简体）",
        other => other,
    }
}

/// JavaScript's `String#trim` whitespace, which includes U+FEFF and excludes U+0085.
fn js_trim(tag: &str) -> &str {
    tag.trim_matches(crate::direction::is_js_space)
}

/// `/^zh[-_](hant|tw|hk|mo)\b/` on an already lower-cased tag. Our `zh` is Simplified, so
/// Traditional tags only match an exact locale.
fn is_traditional_chinese(normalized: &str) -> bool {
    let Some(rest) = normalized.strip_prefix("zh") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(['-', '_']) else {
        return false;
    };
    ["hant", "tw", "hk", "mo"].iter().any(|region| {
        rest.strip_prefix(region).is_some_and(|after| {
            after
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        })
    })
}

fn match_supported<'a>(tag: &str, supported: &[&'a str]) -> Option<&'a str> {
    let normalized = js_trim(tag).to_lowercase();
    let language = normalized.split(['-', '_']).next().unwrap_or_default();
    let exact = supported
        .iter()
        .copied()
        .find(|locale| locale.to_lowercase() == normalized);
    if exact.is_some() || is_traditional_chinese(&normalized) {
        return exact;
    }
    supported
        .iter()
        .copied()
        .find(|locale| locale.to_lowercase() == language)
}

/// Saved setting to active locale; `"system"` and unknown values follow the host's languages.
pub fn resolve_locale<'a>(
    language: &str,
    system_locales: &[impl AsRef<str>],
    supported: &[&'a str],
    fallback: &'a str,
) -> &'a str {
    if let Some(exact) = supported.iter().copied().find(|locale| *locale == language) {
        return exact;
    }
    system_locales
        .iter()
        .find_map(|tag| match_supported(tag.as_ref(), supported))
        .unwrap_or(fallback)
}

/// [`resolve_locale`] over the shipped languages and English fallback.
pub fn resolve_supported_locale(
    language: &str,
    system_locales: &[impl AsRef<str>],
) -> &'static str {
    resolve_locale(language, system_locales, &SUPPORTED_LOCALES, DEFAULT_LOCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(language: &str, system: &[&str]) -> &'static str {
        resolve_supported_locale(language, system)
    }

    #[test]
    fn saved_language_wins_over_the_system() {
        assert_eq!(resolve("fr", &["de-DE"]), "fr");
    }

    #[test]
    fn system_follows_the_first_matching_host_language() {
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["sv-SE", "de-AT", "fr"]), "de");
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["pt-br"]), "pt-BR");
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["pt-PT"]), "en");
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["  ES_mx "]), "es");
    }

    #[test]
    fn unknown_saved_values_follow_the_system() {
        assert_eq!(resolve("klingon", &["ja-JP"]), "ja");
        assert_eq!(resolve("", &[] as &[&str]), "en");
    }

    #[test]
    fn simplified_chinese_matches_by_language_and_traditional_only_exactly() {
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["zh-CN"]), "zh");
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["zh-Hans-CN"]), "zh");
        for traditional in ["zh-Hant", "zh-TW", "zh-HK", "zh-MO", "zh_tw", "zh-Hant-TW"] {
            assert_eq!(
                resolve(SYSTEM_LANGUAGE, &[traditional]),
                "en",
                "{traditional}"
            );
        }
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["zh-Hant", "ja"]), "ja");
        assert_eq!(resolve(SYSTEM_LANGUAGE, &["zh-moon"]), "zh");
    }

    #[test]
    fn an_exact_traditional_locale_still_matches_when_supported() {
        assert_eq!(
            resolve_locale("system", &["zh-TW"], &["en", "zh-TW"], "en"),
            "zh-TW"
        );
    }

    #[test]
    fn native_names_fall_back_to_the_tag() {
        assert_eq!(native_language_name("pt-BR"), "Português (Brasil)");
        assert_eq!(native_language_name("sv"), "sv");
    }
}
