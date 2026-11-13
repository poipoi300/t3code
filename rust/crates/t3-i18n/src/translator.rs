//! `gt("…", args)` for one UI language: catalogue lookup, ICU formatting, and the fallbacks
//! `gt-i18n` applies (`interpolateMessage` in `gt-i18n@1.0.40`).

use crate::catalog::{Catalogue, Catalogues, Surface};
use crate::format::Formats;
use crate::hash::{DataFormat, message_key};
use crate::locale::DEFAULT_LOCALE;
use crate::message::{Args, Message, MessageError, Runtime};

/// Translates English source messages into one UI language.
///
/// A message is looked up by its English text (GT's key is a hash of it, see [`message_key`]),
/// so the UI keeps writing English source in code, as it does with `gt("…")` today:
///
/// 1. In a language other than English, the language's catalogue entry is formatted with that
///    language's plural rules and number format.
/// 2. If the entry is missing, or fails to format, the English source is formatted instead
///    (with the language's rules in the second case, English's in the first).
/// 3. If that fails too, the English source comes back unformatted.
pub struct Translator {
    locale: String,
    catalogue: Option<&'static Catalogue>,
    formats: Formats,
    source_formats: Formats,
}

impl Translator {
    /// `locale` is a UI language (`"fr"`, `"pt-BR"`, …); anything the product does not ship
    /// translates to English.
    pub fn new(surface: Surface, locale: &str) -> Self {
        let catalogue = if locale == DEFAULT_LOCALE {
            None
        } else {
            Catalogues::get(surface).catalogue(locale)
        };
        Self {
            locale: locale.to_owned(),
            catalogue,
            formats: Formats::new(locale),
            source_formats: Formats::new(DEFAULT_LOCALE),
        }
    }

    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// The formatters of the UI language, for numbers and dates shown beside messages.
    pub fn formats(&self) -> &Formats {
        &self.formats
    }

    /// `gt(source)`.
    pub fn t(&self, source: &str) -> String {
        self.translate(source, &[])
    }

    /// `gt(source, args)`: an ICU message with variables.
    pub fn translate(&self, source: &str, args: &Args<'_>) -> String {
        if source.is_empty() {
            return String::new();
        }
        let entry = self
            .catalogue
            .and_then(|catalogue| catalogue.get(&message_key(source, DataFormat::Icu, None)));
        let Some(entry) = entry else {
            return format(source, &self.source_formats, args)
                .unwrap_or_else(|_| source.to_owned());
        };
        format(entry, &self.formats, args)
            .or_else(|_| format(source, &self.formats, args))
            .unwrap_or_else(|_| source.to_owned())
    }

    /// The text of a plain-text `<T>` child: the catalogue entry, or the source unchanged. JSX
    /// text is not ICU, so braces and apostrophes are left alone.
    pub fn text<'a>(&'a self, source: &'a str) -> &'a str {
        self.catalogue
            .and_then(|catalogue| catalogue.get(&message_key(source, DataFormat::Jsx, None)))
            .unwrap_or(source)
    }
}

fn format(message: &str, runtime: &dyn Runtime, args: &Args<'_>) -> Result<String, MessageError> {
    Message::parse(message)?.format(runtime, args)
}
