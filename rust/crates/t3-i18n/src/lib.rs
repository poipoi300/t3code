//! Localisation catalogues, message formatting and locale-aware formatting for the shared UI.
//!
//! Must compile to `wasm32-unknown-unknown`: no I/O, threads or clocks. The current time, the
//! user's locale and the time zone are arguments.
//!
//! # What the product does today
//!
//! Both apps use General Translation (`gt-react`, `gt-react-native`). English text stays in the
//! source; a catalogue is a JSON file per language mapping a hash of the English text to the
//! translation. Messages are ICU MessageFormat. There are nine languages (`en` and eight
//! translations) and only 72 web and 32 mobile messages (23 shared), because almost all
//! user-facing text is hard-coded English in components: across the 1,133 component files
//! about 3,400 text nodes and props are literals against 78 GT uses in 5 files. The Rust UI has
//! to route its text through [`Translator`] from the start; this crate cannot translate what
//! the UI does not pass to it.
//!
//! # Using it
//!
//! ```
//! use t3_i18n::{DateFormat, Formats, Surface, TimeZone, Translator, resolve_supported_locale};
//!
//! // The saved setting is "system" or a language; the host passes its language list in.
//! let ui_language = resolve_supported_locale("system", &["fr-CA", "en"]);
//! let translator = Translator::new(Surface::Unified, ui_language);
//! assert_eq!(translator.t("Settings"), "Param\u{e8}tres");
//!
//! // Numbers and dates use the host's regional locale, which need not be the UI language.
//! let host = Formats::new("de-DE");
//! assert_eq!(host.decimal(1234567.891), "1.234.567,891");
//! let noon_utc = 1_780_056_000_000;
//! assert_eq!(host.date(DateFormat::TimeHm, noon_utc, &TimeZone::Fixed(3600), None), "13:00");
//! ```
//!
//! - [`Translator::t`] and [`Translator::translate`] are `gt("…")`: the English text is the key.
//!   [`Translator::text`] is a plain-text `<T>` child.
//! - [`resolve_locale`] is `resolveLocale`; [`native_language_name`] fills the language picker.
//! - [`text_direction`] and [`markdown_prose_direction`] give the `dir` of text, and
//!   [`composer_block_directions`] applies the composer's per-block rules to blocks the caller
//!   extracted from its document.
//! - [`Formats`] formats numbers, plural categories, lists, week starts and the sixteen
//!   [`DateFormat`] shapes the apps pass to `Intl.DateTimeFormat`.
//! - [`display`] holds the apps' own byte, token, USD and percent helpers, ported exactly.
//!
//! # Where the line with `t3-client` falls
//!
//! `t3-client` owns the decisions of `timestampFormat.ts`: which day label a message gets
//! (today, "yesterday at", a numeric date), when a relative label next changes, "5m ago",
//! "Expires in 2h 3m". Those strings are English literals in the TypeScript, not `Intl` output,
//! and the product uses no `Intl.RelativeTimeFormat` or `Intl.DurationFormat`. This crate owns
//! what `Intl` produced: [`Formats::date`] for the time and the numeric date, the civil-day
//! arithmetic ([`format::civil`]) in a time zone, and [`Formats::week_starts_on`]. When those
//! English literals are localised they become catalogue messages (with plural rules this
//! engine applies) rather than new formatters.
//!
//! # Decisions
//!
//! **Messages.** [`message`] is a port of `@generaltranslation/icu` 0.1.2, the formatter behind
//! `useGT`: literals and quoting, arguments, `select`, `plural`, `selectordinal` (with `offset`
//! and `=N`), `#`, the parse errors, and `gt-i18n`'s fallback chain. Percent, currency, date
//! and time arguments, skeletons and rich-text tags parse but do not format; no catalogue uses
//! them. `tests/messages.rs` runs 858 cases through the real formatter.
//!
//! **ICU4X against narrower crates.** ICU4X (`icu` 2.x) is the standard Rust CLDR
//! implementation, compiles to wasm, and is the only option that covers an arbitrary host
//! locale, so it is the base for numbers, plurals, lists and week data. Alone it matches `Intl`
//! exactly for plurals, lists, week starts and most number shapes, but not dates (72.9% of
//! 9,856 cases for the shipped locales, 66.5% of 2,688 for twelve others) and not compact
//! numbers (77.7%): `Intl` honours requested field widths (`hour: "numeric"` is `7:30` in
//! German, ICU4X's pattern is `07:30`), and ICU4X's compact formatter ignores
//! `maximumFractionDigits`. Narrower crates (`num-format`, `chrono` locales) cover fewer
//! locales and fewer shapes. So the shipped locales use patterns and compact units read back
//! from `Intl` (`tools/generate-intl-tables.mjs` writes `intl_tables.rs`; 100% parity bar one
//! V8 quirk) and ICU4X covers the other locales behind the `icu-fallback` feature.
//!
//! **Size.** Release wasm, `opt-level = "s"`, LTO, raw and gzip, from a `cdylib` using each
//! part (ICU4X's plural and decimal data for every locale is in every build that formats):
//!
//! | Build | Raw | Gzip |
//! | --- | --- | --- |
//! | text direction | 83 KB | 18 KB |
//! | messages: catalogues, engine, plurals, decimals, key hashing | 274 KB | 115 KB |
//! | numbers: decimal, compact, currency, percent (no `icu-fallback`) | 341 KB | 77 KB |
//! | numbers with `icu-fallback` | 365 KB | 85 KB |
//! | dates: generated tables only | 73 KB | 23 KB |
//! | dates with `iana` (bundled tz database) | 375 KB | 119 KB |
//! | dates with `icu-fallback` and `iana` | 1,197 KB | 357 KB |
//! | everything, no `icu-fallback`, no `iana` | 651 KB | 177 KB |
//! | everything, default features | 1,744 KB | 497 KB |
//!
//! The ICU4X date data costs 822 KB raw and the bundled time zone database 302 KB. A web shell
//! that can show English-pattern dates for unshipped host locales builds with
//! `default-features = false, features = ["iana"]`. Trimming ICU4X's data to the shipped
//! locales needs `icu4x-datagen`, which downloads CLDR from GitHub, so it was not run here.
//!
//! **Parity** with the runtime's `Intl` (Node 26, ICU 78.3, CLDR 48) is tested on 1,848
//! number, 704 plural, 9,856 date, 24 week and 88 list cases; `tests/parity.rs` lists the one
//! accepted difference. Browsers differ from Node in small ways; V8 replaces CLDR's narrow
//! no-break space in date output with a plain space, which this crate does too.
//!
//! # Regenerating
//!
//! From the repository root: `node rust/crates/t3-i18n/tools/generate-fixtures.mjs` rewrites
//! `tests/fixtures`, and `node rust/crates/t3-i18n/tools/generate-intl-tables.mjs` rewrites
//! `src/intl_tables.rs`. The catalogues in `data/` are copies of the apps' files and
//! `tests/catalogues.rs` fails when they drift.

pub mod catalog;
pub mod direction;
pub mod display;
pub mod format;
pub mod hash;
mod intl_pattern;
mod intl_tables;
pub mod locale;
pub mod message;
pub mod translator;

pub use catalog::{Catalogue, Catalogues, Drift, Surface};
pub use direction::{
    DirectionBlock, TextDirection, composer_block_directions, markdown_prose_direction,
    text_direction,
};
pub use format::{CalendarDay, Civil, DateFormat, Formats, ListKind, NumberOptions, TimeZone};
pub use hash::{DataFormat, message_key};
pub use locale::{
    DEFAULT_LOCALE, SUPPORTED_LOCALES, SYSTEM_LANGUAGE, native_language_name, resolve_locale,
    resolve_supported_locale,
};
pub use message::{Message, MessageError, Value};
pub use translator::Translator;
