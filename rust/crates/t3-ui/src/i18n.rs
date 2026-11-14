//! Every user-facing string goes through here, with the English source text as the key.
//!
//! `t("Settings")` and `tr("Sent {count} files", &[("count", Value::Int(3))])` read the UI
//! language reactively, so a binding that calls them follows a language change. Strings the
//! client core returns in English (status texts, failure details) are passed through `t` at
//! display, so the catalogue can translate them too. Numbers and dates use [`formats`].

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;
pub use t3_i18n::Value;
use t3_i18n::{Formats, Surface, Translator, resolve_supported_locale};

#[derive(Clone, Debug, PartialEq)]
struct UiLanguage(&'static str);

impl Default for UiLanguage {
    fn default() -> Self {
        Self(t3_i18n::DEFAULT_LOCALE)
    }
}

thread_local! {
    static TRANSLATOR: RefCell<Option<Rc<Translator>>> = const { RefCell::new(None) };
}

/// Applies the saved language setting (`"system"` or a language) against the host's languages.
pub fn set_language(setting: &str, host_languages: &[String]) {
    let locale = resolve_supported_locale(setting, host_languages);
    let language = global::<UiLanguage>();
    if language.get_untracked().0 != locale {
        language.set(UiLanguage(locale));
    }
}

fn translator() -> Rc<Translator> {
    let locale =
        try_global::<UiLanguage>().map_or(t3_i18n::DEFAULT_LOCALE, |language| language.get().0);
    TRANSLATOR.with_borrow_mut(|current| match current {
        Some(translator) if translator.locale() == locale => translator.clone(),
        _ => current
            .insert(Rc::new(Translator::new(Surface::Unified, locale)))
            .clone(),
    })
}

/// `gt("…")`: the catalogue entry for `source` in the UI language, or `source`.
pub fn t(source: &str) -> String {
    translator().t(source)
}

/// `gt("…", args)`: an ICU message with variables.
pub fn tr(source: &str, args: &[(&str, Value)]) -> String {
    translator().translate(source, args)
}

/// `t` as a `SharedString`, for props that take one.
pub fn ts(source: &str) -> SharedString {
    t(source).into()
}

/// Number and date formatting in the UI language.
pub fn formats() -> Formats {
    Formats::new(translator().locale())
}

/// What a component's text slot holds (ui-package-addendum.md, "Translation keys and content").
/// A key is English source text the code owns, translated when shown. Content is shown as it
/// is: user and agent text, names, paths, formatted dates, times, durations and counts, server
/// details, and text a caller already translated. A literal in our code converts to a key; a
/// runtime string converts to content, so a slot cannot translate content by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Text {
    Key(SharedString),
    Content(SharedString),
}

impl Text {
    /// An English sentence the client returns for display (its message is a catalogue key).
    pub fn client_key(source: impl Into<SharedString>) -> Self {
        Self::Key(source.into())
    }

    pub fn content(text: impl Into<SharedString>) -> Self {
        Self::Content(text.into())
    }

    /// The text to show in the UI language.
    pub fn resolve(&self) -> SharedString {
        match self {
            Self::Key(source) => ts(source),
            Self::Content(text) => text.clone(),
        }
    }
}

impl From<&'static str> for Text {
    fn from(source: &'static str) -> Self {
        Self::Key(source.into())
    }
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        Self::Content(text.into())
    }
}

impl From<SharedString> for Text {
    fn from(text: SharedString) -> Self {
        Self::Content(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_keys_are_translated_and_runtime_strings_are_content() {
        assert!(matches!(Text::from("Copy"), Text::Key(_)));
        let formatted = String::from("Nov 13, 9:41 PM");
        assert_eq!(
            Text::from(formatted.clone()),
            Text::Content(formatted.into())
        );
        let already = SharedString::from(t("Delete thread"));
        assert_eq!(Text::from(already.clone()).resolve(), already);
        assert!(matches!(
            Text::client_key("Enter a message."),
            Text::Key(_)
        ));
    }

    #[test]
    fn outside_an_engine_the_source_text_is_used() {
        assert_eq!(t("Settings"), "Settings");
        assert_eq!(
            tr(
                "{count, plural, one {# file} other {# files}}",
                &[("count", Value::Int(2))]
            ),
            "2 files"
        );
    }

    /// The rule from the first screen: user-facing text reaches the screen through `t`/`tr`.
    #[test]
    fn no_user_facing_literal_bypasses_the_translator() {
        const CALLS: [&str; 8] = [
            "text(\"",
            ".placeholder(\"",
            ".label(\"",
            ".tooltip(\"",
            ".hint(\"",
            ".title(\"",
            "accessibility_label(\"",
            "ButtonContent::Label(\"",
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offences = Vec::new();
        let mut stack = vec![root.join("screens"), root.join("ui")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap_or_default();
                    // A file's trailing test module builds fixture text, not user-facing copy.
                    let lines: Vec<&str> = source.lines().collect();
                    let end = lines
                        .windows(2)
                        .position(|pair| {
                            pair[0].trim() == "#[cfg(test)]" && pair[1].trim_start().starts_with("mod ")
                        })
                        .unwrap_or(lines.len());
                    for (number, line) in lines[..end].iter().enumerate() {
                        let code = line.split("//").next().unwrap_or("");
                        if CALLS.iter().any(|call| code.contains(call)) {
                            offences.push(format!(
                                "{}:{}: {}",
                                path.display(),
                                number + 1,
                                line.trim()
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            offences.is_empty(),
            "wrap these in i18n::t:\n{}",
            offences.join("\n")
        );
    }

    #[test]
    fn a_shipped_language_translates_by_english_key() {
        let translator = Translator::new(Surface::Unified, "fr");
        assert_eq!(translator.t("Settings"), "Param\u{e8}tres");
    }
}
