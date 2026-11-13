// Generates src/intl_tables.rs: the date patterns and compact-number suffixes the runtime's Intl
// uses for the locales the product ships, read back from Intl itself with formatToParts.
//
// Why: Intl honours the requested field widths (`hour: "numeric"` gives "7:30" in German) while
// ICU4X's semantic field sets use each locale's standard pattern ("07:30"), and ICU4X's compact
// formatter ignores maximumFractionDigits. The table makes the shipped locales byte-identical.
//
// Run from the worktree root: node rust/crates/t3-i18n/tools/generate-intl-tables.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outFile = path.join(here, "..", "src", "intl_tables.rs");

export const LOCALES = ["en-US", "en-GB", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "zh"];

const FORMATS = {
  TimeHm: { hour: "numeric", minute: "2-digit" },
  TimeHms: { hour: "numeric", minute: "2-digit", second: "2-digit" },
  MonthDayNumeric: { month: "numeric", day: "numeric" },
  MonthDayYearNumeric: { month: "numeric", day: "numeric", year: "numeric" },
  DateMedium: { dateStyle: "medium" },
  DateMediumTimeShort: { dateStyle: "medium", timeStyle: "short" },
  MonthShortDay: { month: "short", day: "numeric" },
  MonthShortDayYear: { month: "short", day: "numeric", year: "numeric" },
  MonthShortDayTime: { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" },
  WeekdayLong: { weekday: "long" },
  WeekdayShort: { weekday: "short" },
  MonthShort: { month: "short" },
  MonthLong: { month: "long" },
  DefaultDateTime: "toLocaleString",
  DefaultDate: "toLocaleDateString",
  DefaultTime: "toLocaleTimeString",
};
const HAS_HOUR = new Set(["TimeHm", "TimeHms", "DateMediumTimeShort", "MonthShortDayTime", "DefaultDateTime", "DefaultTime"]);

const at = (iso) => new Date(iso);
const P_SINGLE = at("2026-03-08T07:05:09Z");
const P_DOUBLE = at("2026-11-22T15:45:30Z");
const P_MIDNIGHT = at("2026-01-02T00:05:09Z");
const P_NOON = at("2026-06-04T12:00:00Z");

function formatterFor(locale, spec, hour12) {
  const options = { timeZone: "UTC", ...(hour12 === undefined ? {} : { hour12 }) };
  if (typeof spec === "string") return (date) => date[spec](locale, options);
  const f = new Intl.DateTimeFormat(locale, { ...spec, ...options });
  return (date) => f.format(date);
}

function partsFor(locale, spec, hour12, date) {
  const options = { timeZone: "UTC", ...(hour12 === undefined ? {} : { hour12 }) };
  if (typeof spec === "string") {
    const probe = { toLocaleString: { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric" },
      toLocaleDateString: { year: "numeric", month: "numeric", day: "numeric" },
      toLocaleTimeString: { hour: "numeric", minute: "numeric", second: "numeric" } }[spec];
    // The toLocale* defaults are these field sets; check against the real call below.
    return new Intl.DateTimeFormat(locale, { ...probe, ...options }).formatToParts(date);
  }
  return new Intl.DateTimeFormat(locale, { ...spec, ...options }).formatToParts(date);
}

function rustString(value) {
  return JSON.stringify(value).replace(/\\u([0-9a-fA-F]{4})/g, (_, hex) => `\\u{${hex}}`);
}

function names(locale, spec, hour12, type, dates) {
  return dates.map((date) => {
    const part = partsFor(locale, spec, hour12, date).find((p) => p.type === type);
    return part?.value ?? "";
  });
}

const MONTH_DATES = Array.from({ length: 12 }, (_, m) => new Date(Date.UTC(2026, m, 15, 12)));
const WEEKDAY_DATES = Array.from({ length: 7 }, (_, d) => new Date(Date.UTC(2026, 2, 1 + d, 12))); // Sunday first

const lit = (value) => ({ t: "Lit", value });

function tokensFor(locale, format, spec, hour12) {
  const single = partsFor(locale, spec, hour12, P_SINGLE);
  const double = partsFor(locale, spec, hour12, P_DOUBLE);
  const midnight = partsFor(locale, spec, hour12, P_MIDNIGHT);
  const find = (parts, type) => parts.find((p) => p.type === type)?.value;
  const tokens = [];
  for (const part of single) {
    switch (part.type) {
      case "literal":
        tokens.push(lit(part.value));
        break;
      case "year":
        tokens.push({ t: "Year", two_digit: part.value.length === 2 });
        break;
      case "month":
        if (/^\d+$/.test(part.value)) tokens.push({ t: "Month", pad: part.value.length === 2 });
        else tokens.push({ t: "MonthName", names: names(locale, spec, hour12, "month", MONTH_DATES) });
        break;
      case "day":
        tokens.push({ t: "Day", pad: part.value.length === 2 });
        break;
      case "weekday":
        tokens.push({ t: "Weekday", names: names(locale, spec, hour12, "weekday", WEEKDAY_DATES) });
        break;
      case "hour": {
        const afternoon = find(double, "hour"); // 15:45
        const twelve = afternoon === "3" || afternoon === "03";
        const zero = find(midnight, "hour"); // 00:05
        let cycle;
        if (twelve) cycle = zero === "12" ? "H12" : zero === "0" || zero === "00" ? "H11" : null;
        else cycle = zero === "24" ? "H24" : "H23";
        if (!cycle) throw new Error(`unknown hour cycle ${locale} ${format} ${zero}/${afternoon}`);
        tokens.push({ t: "Hour", pad: part.value.length === 2, cycle });
        break;
      }
      case "minute":
        tokens.push({ t: "Minute", pad: part.value.length === 2 });
        break;
      case "second":
        tokens.push({ t: "Second", pad: part.value.length === 2 });
        break;
      case "dayPeriod":
        tokens.push({ t: "DayPeriod", am: find(single, "dayPeriod"), pm: find(double, "dayPeriod") });
        break;
      default:
        throw new Error(`unhandled part ${part.type} in ${locale} ${format}`);
    }
  }
  return tokens;
}

function renderTokens(tokens, date) {
  const pad = (n, on) => (on && n < 10 ? "0" : "") + n;
  const hour = date.getUTCHours();
  return tokens
    .map((token) => {
      switch (token.t) {
        case "Lit": return token.value;
        case "Year": return token.two_digit ? pad(date.getUTCFullYear() % 100, true) : String(date.getUTCFullYear());
        case "Month": return pad(date.getUTCMonth() + 1, token.pad);
        case "MonthName": return token.names[date.getUTCMonth()];
        case "Day": return pad(date.getUTCDate(), token.pad);
        case "Weekday": return token.names[date.getUTCDay()];
        case "Hour": {
          const h = { H23: hour, H11: hour % 12, H12: hour % 12 === 0 ? 12 : hour % 12, H24: hour === 0 ? 24 : hour }[token.cycle];
          return pad(h, token.pad);
        }
        case "Minute": return pad(date.getUTCMinutes(), token.pad);
        case "Second": return pad(date.getUTCSeconds(), token.pad);
        case "DayPeriod": return hour < 12 ? token.am : token.pm;
      }
      throw new Error(token.t);
    })
    .join("");
}

const PROBES = [P_SINGLE, P_DOUBLE, P_MIDNIGHT, P_NOON];

// formatToParts and the toLocale* calls disagree about the hour cycle in some locales (Japanese
// with hour12: true gives 0:05 from toLocaleTimeString but 12:05 from formatToParts), so the
// tokens must reproduce the real call; try the other hour cycles when they do not.
function fitted(locale, format, spec, hour12) {
  const real = formatterFor(locale, spec, hour12);
  const matches = (tokens) => PROBES.every((date) => renderTokens(tokens, date) === real(date));
  let tokens = tokensFor(locale, format, spec, hour12);
  if (matches(tokens)) return tokens;
  const hourIndex = tokens.findIndex((token) => token.t === "Hour");
  if (hourIndex >= 0) {
    for (const cycle of ["H11", "H12", "H23", "H24"]) {
      tokens = tokens.map((token, i) => (i === hourIndex ? { ...token, cycle } : token));
      if (matches(tokens)) return tokens;
    }
  }
  throw new Error(`cannot reproduce ${locale} ${format} hour12=${hour12}: ${PROBES.map((d) => `${renderTokens(tokens, d)} vs ${real(d)}`).join("; ")}`);
}

function toRust(token) {
  const q = rustString;
  switch (token.t) {
    case "Lit": return `Token::Lit(${q(token.value)})`;
    case "Year": return `Token::Year { two_digit: ${token.two_digit} }`;
    case "Month": return `Token::Month { pad: ${token.pad} }`;
    case "MonthName": return `Token::MonthName(&[${token.names.map(q).join(", ")}])`;
    case "Day": return `Token::Day { pad: ${token.pad} }`;
    case "Weekday": return `Token::Weekday(&[${token.names.map(q).join(", ")}])`;
    case "Hour": return `Token::Hour { pad: ${token.pad}, cycle: Cycle::${token.cycle} }`;
    case "Minute": return `Token::Minute { pad: ${token.pad} }`;
    case "Second": return `Token::Second { pad: ${token.pad} }`;
    case "DayPeriod": return `Token::DayPeriod { am: ${q(token.am)}, pm: ${q(token.pm)} }`;
  }
  throw new Error(token.t);
}

const entries = [];
for (const locale of LOCALES) {
  for (const [format, spec] of Object.entries(FORMATS)) {
    const variants = HAS_HOUR.has(format) ? [["Locale", undefined], ["H12", true], ["H23", false]] : [["Locale", undefined]];
    for (const [clock, hour12] of variants) {
      const tokens = fitted(locale, format, spec, hour12);
      entries.push(`    Pattern { locale: ${rustString(locale)}, format: DateFormat::${format}, clock: Clock::${clock}, tokens: &[${tokens.map(toRust).join(", ")}] },`);
    }
  }
}

// ---- compact notation ---------------------------------------------------------------------
// For each locale: the units (power of ten, suffix with its separator) Intl uses for short compact.
const compact = [];
for (const locale of LOCALES) {
  const f = new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 });
  const rows = [];
  for (let k = 1; k <= 15; k += 1) {
    const parts = f.formatToParts(10 ** k);
    const suffixParts = [];
    let seenInteger = false;
    for (const part of parts) {
      if (part.type === "integer") seenInteger = true;
      else if (seenInteger && (part.type === "compact" || part.type === "literal")) suffixParts.push(part.value);
    }
    const suffix = suffixParts.join("");
    if (suffix === "") continue;
    const integer = parts.find((p) => p.type === "integer").value;
    rows.push(`(${k}, ${k - (integer.length - 1)}, ${rustString(suffix)})`);
  }
  compact.push(`    Compact { locale: ${rustString(locale)}, units: &[${rows.join(", ")}] },`);
}

const header = `// @generated by tools/generate-intl-tables.mjs from the runtime's Intl. Do not edit.
// ${JSON.stringify({ node: process.version, icu: process.versions.icu, cldr: process.versions.cldr })}
use crate::format::DateFormat;
use crate::intl_pattern::{Clock, Compact, Cycle, Pattern, Token};

`;
fs.writeFileSync(
  outFile,
  `${header}#[rustfmt::skip]\npub static PATTERNS: &[Pattern] = &[\n${entries.join("\n")}\n];\n\n#[rustfmt::skip]\npub static COMPACT: &[Compact] = &[\n${compact.join("\n")}\n];\n`,
);
console.log(`${entries.length} patterns, ${compact.length} compact rows -> ${path.relative(process.cwd(), outFile)}`);
