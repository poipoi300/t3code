// Generates tests/fixtures/*.json from the runtime's real Intl (and the GT packages).
// Run from the worktree root: node rust/crates/t3-i18n/tools/generate-fixtures.mjs
// Each fixture records the Node, ICU and CLDR versions it was generated with.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, "..", "tests", "fixtures");
const repo = process.env.T3_REPO ?? path.resolve(here, "../../../..");
const write = (name, value) => fs.writeFileSync(path.join(out, name), JSON.stringify(value));
const versions = { node: process.version, icu: process.versions.icu, cldr: process.versions.cldr, unicode: process.versions.unicode };

export const LOCALES = ["en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "zh", "en-US", "en-GB"];

// ---- numbers ------------------------------------------------------------------------------
const NUMBER_VALUES = [0, 1, -1, 12, 999, 1000, 1234, 9999, 12345, 123456, 999999, 1000000, 1234567.891, 1.5e9, 2.5e12, 0.5, 0.0005, 1.005, 2.5, 1234.5, -1234.5, 0.1234, 100.25, 1e15];
const NUMBER_FORMATS = {
  decimal: {},
  no_grouping: { useGrouping: false },
  max_fraction_1: { maximumFractionDigits: 1 },
  integer: { maximumFractionDigits: 0 },
  compact_1: { notation: "compact", maximumFractionDigits: 1 },
  usd_2: { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 },
  percent_0: { style: "percent", maximumFractionDigits: 0 },
};
const numbers = [];
for (const locale of LOCALES) {
  for (const [format, options] of Object.entries(NUMBER_FORMATS)) {
    const f = new Intl.NumberFormat(locale, options);
    for (const value of NUMBER_VALUES) numbers.push({ locale, format, value, expected: f.format(value) });
  }
}
write("numbers.json", { versions, cases: numbers });

// ---- plurals ------------------------------------------------------------------------------
const PLURAL_VALUES = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 20, 21, 22, 23, 24, 25, 100, 101, 102, 111, 112, 1000, 1000000, 1.5, 0.5, 2.5, 21.5];
const plurals = [];
for (const locale of LOCALES) {
  for (const type of ["cardinal", "ordinal"]) {
    const r = new Intl.PluralRules(locale, { type });
    for (const value of PLURAL_VALUES) plurals.push({ locale, type, value, expected: r.select(value) });
  }
}
write("plurals.json", { versions, cases: plurals });

// ---- dates and times ----------------------------------------------------------------------
const INSTANTS = [
  "2026-01-02T00:05:09Z", "2026-03-08T07:30:00Z", "2026-06-04T12:00:00Z", "2026-06-04T15:45:30Z",
  "2026-11-01T05:30:00Z", "2026-12-31T23:59:59Z", "2024-02-29T09:07:03Z", "2025-10-26T01:30:00Z",
];
const ZONES = ["UTC", "America/New_York", "Asia/Kolkata", "Europe/Berlin"];
const DATE_FORMATS = {
  time_hm: { hour: "numeric", minute: "2-digit" },
  time_hms: { hour: "numeric", minute: "2-digit", second: "2-digit" },
  month_day_numeric: { month: "numeric", day: "numeric" },
  month_day_year_numeric: { month: "numeric", day: "numeric", year: "numeric" },
  date_medium: { dateStyle: "medium" },
  date_medium_time_short: { dateStyle: "medium", timeStyle: "short" },
  month_short_day: { month: "short", day: "numeric" },
  month_short_day_year: { month: "short", day: "numeric", year: "numeric" },
  month_short_day_time: { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" },
  weekday_long: { weekday: "long" },
  weekday_short: { weekday: "short" },
  month_short: { month: "short" },
  month_long: { month: "long" },
  default_date_time: null,
  default_date: null,
  default_time: null,
};
const HOUR12_APPLIES = new Set(["time_hm", "time_hms", "date_medium_time_short", "month_short_day_time", "default_date_time", "default_time"]);
const dates = [];
for (const locale of LOCALES) {
  for (const [format, options] of Object.entries(DATE_FORMATS)) {
    for (const zone of ZONES) {
      const variants = HOUR12_APPLIES.has(format) ? [null, true, false] : [null];
      for (const hour12 of variants) {
        const withZone = { timeZone: zone, ...(hour12 === null ? {} : { hour12 }) };
        let call;
        if (format === "default_date_time") call = (d) => d.toLocaleString(locale, withZone);
        else if (format === "default_date") call = (d) => d.toLocaleDateString(locale, withZone);
        else if (format === "default_time") call = (d) => d.toLocaleTimeString(locale, withZone);
        else {
          const f = new Intl.DateTimeFormat(locale, { ...options, ...withZone });
          call = (d) => f.format(d);
        }
        for (const iso of INSTANTS) dates.push({ locale, format, zone, hour12, instant: iso, expected: call(new Date(iso)) });
      }
    }
  }
}
write("dates.json", { versions, cases: dates });

// ---- week info and lists ------------------------------------------------------------------
const weekLocales = LOCALES.concat(["ar", "he", "fa", "de-AT", "fr-CA", "pt-PT", "en-AU", "es-MX", "zh-TW", "ja-JP", "ko-KR", "sv-SE", "pl-PL"]);
const weeks = weekLocales.map((locale) => {
  const l = new Intl.Locale(locale);
  const info = l.getWeekInfo?.() ?? l.weekInfo;
  return { locale, firstDay: info.firstDay };
});
write("weeks.json", { versions, cases: weeks });

const LISTS = [["Chrome"], ["Chrome", "Arc"], ["Chrome", "Arc", "Brave"], ["Chrome", "Arc", "Brave", "Edge"]];
const lists = [];
for (const locale of LOCALES) {
  for (const type of ["conjunction", "disjunction"]) {
    const f = new Intl.ListFormat(locale, { type });
    for (const items of LISTS) lists.push({ locale, type, items, expected: f.format(items) });
  }
}
write("lists.json", { versions, cases: lists });

// ---- GT keys and the real GT ICU formatter ------------------------------------------------
const gtRequire = createRequire(path.join(repo, "node_modules/.pnpm/generaltranslation@9.5.9/node_modules/generaltranslation/package.json"));
const { hashSource } = gtRequire("generaltranslation/id");
const icuRequire = createRequire(path.join(repo, "node_modules/.pnpm/@generaltranslation+icu@0.1.2/node_modules/@generaltranslation/icu/package.json"));
const { formatMessage } = icuRequire("@generaltranslation/icu");

const KEY_SOURCES = ["Settings", "", " ", "No settings match “{query}”.", "Quote \" back\\slash\nnewline\ttab \u0001 \u007f  ", "日本語 \u{1f600} é", "{n, plural, one {# file} other {# files}}"];
const keys = [];
for (const source of KEY_SOURCES) {
  for (const dataFormat of ["ICU", "JSX"]) {
    for (const context of [undefined, "button label"]) {
      keys.push({ source, dataFormat, context: context ?? null, key: hashSource({ source, dataFormat, ...(context ? { context } : {}) }) });
    }
  }
}
write("keys.json", { cases: keys });

const nums = (...values) => values.map((n) => ({ n }));
const MESSAGES = [
  ["No settings match “{query}”.", [{ query: "abc" }, { query: "" }, { query: "a{b}" }]],
  ["Notifications dans l'app", [{}]],
  ["it''s '{literal}' and '#'", [{}]],
  ["{n}", nums(0, 1, 1234567, 1.5, -2, "x")],
  ["{n, number}", nums(1234567.891, 0, -1234.5)],
  ["{n, number, integer}", nums(2.5, 1234567.891, -0.4)],
  ["{n, plural, =0 {none} one {# file} other {# files}}", nums(0, 1, 2, 5, 11, 21, 1000, 1234567, 1.5)],
  ["{n, plural, one {# fichier} many {# de fichiers} other {# fichiers}}", nums(0, 1, 2, 1000000, 1.5)],
  ["{n, plural, few {# few} many {# many} one {# one} other {# other}}", nums(0, 1, 2, 3, 5, 11, 21, 22, 25, 101)],
  ["{n, plural, offset:1 =0 {nobody} =1 {you} one {you and # other} other {you and # others}}", nums(0, 1, 2, 3, 10)],
  ["{n, selectordinal, one {#st} two {#nd} few {#rd} other {#th}}", nums(1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111)],
  ["{g, select, female {She} male {He} other {They}} left", [{ g: "female" }, { g: "male" }, { g: "x" }, { g: 3 }]],
  ["{count, plural, one {{g, select, a {# A} other {# other}}} other {many # {g}}}", [{ count: 1, g: "a" }, { count: 1, g: "b" }, { count: 3, g: "a" }]],
  ["{x}", [{}]],
  ["{", [{}]],
  ["{}", [{}]],
  ["{a b}", [{ a: 1 }]],
  ["{a, wat}", [{ a: 1 }]],
  ["{a, select, x {y}}", [{ a: 1 }]],
  ["<b>x</b>", [{}]],
  ["a <br/> b", [{}]],
  ["{n, plural, other {x}}", nums("abc", "", " 5 ")],
  ["{n, number, percent}", nums(0.5)],
  ["{d, date, short}", [{ d: 0 }]],
];
const messages = [];
for (const locale of LOCALES) {
  for (const [message, argSets] of MESSAGES) {
    for (const args of argSets) {
      let expected = null;
      let error = null;
      try {
        expected = String(formatMessage(message, locale, args));
      } catch (e) {
        error = String(e.message ?? e);
      }
      messages.push({ locale, message, args, expected, error });
    }
  }
}
write("messages.json", { versions, cases: messages });

console.log(JSON.stringify({ numbers: numbers.length, plurals: plurals.length, dates: dates.length, weeks: weeks.length, lists: lists.length, keys: keys.length, messages: messages.length, versions }));

// ---- the product's own display helpers ----------------------------------------------------
const usage = await import(pathToFileURL(path.join(repo, "packages/shared/src/usageFormat.ts")).href);
const { formatBytes } = await import(pathToFileURL(path.join(repo, "apps/web/src/lib/formatBytes.ts")).href);
const DISPLAY_VALUES = [0, 0.4, 0.5, 1.5, 2.5, -0.5, -1.5, -2.5, 0.05, 0.005, 1.005, 12.345, 99.995, 100, 999, 999.5, 1000, 1023.4, 1023.5, 1023.6, 1024, 1535, 1536, 10239, 10240, 1048575, 1048576, 999499, 999500, 999999, 1e6, 1.23e6, 12.5e6, 1e9, 123456789, 1e12, 1e15, 1.0995e11, -1234.5678, 1234.5678];
const display = [];
for (const value of DISPLAY_VALUES) {
  display.push({ fn: "bytes", value, digits: null, expected: formatBytes(value) });
  display.push({ fn: "tokens", value, digits: null, expected: usage.formatTokens(value) });
  display.push({ fn: "count", value, digits: null, expected: usage.formatCount(value) });
  display.push({ fn: "usd", value, digits: null, expected: usage.formatUsd(value) });
  for (const digits of [0, 1, 2, 3]) display.push({ fn: "percent", value, digits, expected: usage.formatPercent(value, digits) });
}
write("display.json", { cases: display });

// ---- locales without a table: what ICU4X alone is measured against -------------------------
const OTHER_LOCALES = ["es-MX", "de-AT", "pt-PT", "en-AU", "sv-SE", "it", "nl", "pl", "tr", "hi", "ar", "th"];
const otherDates = [];
for (const locale of OTHER_LOCALES) {
  for (const [format, options] of Object.entries(DATE_FORMATS)) {
    const variants = HOUR12_APPLIES.has(format) ? [null, true, false] : [null];
    for (const hour12 of variants) {
      const withZone = { timeZone: "UTC", ...(hour12 === null ? {} : { hour12 }) };
      let call;
      if (format === "default_date_time") call = (d) => d.toLocaleString(locale, withZone);
      else if (format === "default_date") call = (d) => d.toLocaleDateString(locale, withZone);
      else if (format === "default_time") call = (d) => d.toLocaleTimeString(locale, withZone);
      else {
        const f = new Intl.DateTimeFormat(locale, { ...options, ...withZone });
        call = (d) => f.format(d);
      }
      for (const iso of INSTANTS) otherDates.push({ locale, format, zone: "UTC", hour12, instant: iso, expected: call(new Date(iso)) });
    }
  }
}
write("dates-other.json", { versions, cases: otherDates });
