// Parses CSS values into the expression trees `src/tokens/expr.rs` evaluates.
import { culori } from "./lib.mjs";

const { parse: parseColor } = await culori();

const MODE_TO_SPACE = {
  rgb: "srgb", lrgb: "srgb-linear", p3: "display-p3", a98: "a98-rgb", prophoto: "prophoto-rgb", rec2020: "rec2020",
  xyz50: "xyz-d50", xyz65: "xyz-d65", lab: "lab", lch: "lch", oklab: "oklab", oklch: "oklch", hsl: "hsl", hwb: "hwb",
};
const COMPONENTS = {
  rgb: ["r", "g", "b"], lrgb: ["r", "g", "b"], p3: ["r", "g", "b"], a98: ["r", "g", "b"], prophoto: ["r", "g", "b"],
  rec2020: ["r", "g", "b"], xyz50: ["x", "y", "z"], xyz65: ["x", "y", "z"], lab: ["l", "a", "b"], lch: ["l", "c", "h"],
  oklab: ["l", "a", "b"], oklch: ["l", "c", "h"], hsl: ["h", "s", "l"], hwb: ["h", "w", "b"],
};

export function literalColor(text) {
  const parsed = parseColor(text);
  if (!parsed) return null;
  const names = COMPONENTS[parsed.mode];
  const space = MODE_TO_SPACE[parsed.mode];
  if (!names || !space) return null;
  return { space, c: names.map((n) => parsed[n] ?? 0), alpha: parsed.alpha ?? 1 };
}

/** Splits at top-level `sep` (a single character), ignoring text inside parentheses and quotes. */
export function splitTop(text, sep) {
  const parts = [];
  let depth = 0;
  let quote = null;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === "\\") i++;
      else if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") quote = ch;
    else if (ch === "(") depth++;
    else if (ch === ")") depth--;
    else if (depth === 0 && (sep === " " ? /\s/.test(ch) : ch === sep)) {
      if (sep === " ") {
        if (i > start) parts.push(text.slice(start, i));
        start = i + 1;
      } else {
        parts.push(text.slice(start, i));
        start = i + 1;
      }
    }
  }
  if (sep === " ") {
    if (start < text.length) parts.push(text.slice(start));
  } else parts.push(text.slice(start));
  return parts.map((p) => p.trim()).filter((p, i, all) => sep !== "," || p !== "" || all.length === 1);
}

function fnCall(term) {
  const m = /^(--?[a-zA-Z][\w-]*|[a-zA-Z][\w-]*)\(([\s\S]*)\)$/.exec(term);
  if (!m) return null;
  // The closing paren must match the first open one.
  let depth = 0;
  for (let i = m[1].length; i < term.length; i++) {
    if (term[i] === "(") depth++;
    else if (term[i] === ")") {
      depth--;
      if (depth === 0 && i !== term.length - 1) return null;
    }
  }
  return { name: m[1].toLowerCase(), args: m[2] };
}

const isPercentLiteral = (t) => /^[+-]?(\d+\.?\d*|\.\d+)%$/.test(t);

function parseDimension(term) {
  const m = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)$/i.exec(term);
  if (!m) return null;
  const v = Number(m[1]);
  const unit = m[2].toLowerCase();
  if (unit === "") return { k: "number", v };
  if (unit === "%") return { k: "percent", v };
  if (unit === "px") return { k: "length", px: v, rem: 0 };
  if (unit === "rem") return { k: "length", px: 0, rem: v };
  if (unit === "vw") return { k: "length", px: 0, rem: 0, vw: v };
  if (unit === "ms") return { k: "duration", ms: v };
  if (unit === "s") return { k: "duration", ms: v * 1000 };
  if (["deg", "rad", "grad", "turn"].includes(unit)) return { k: "angle", deg: unit === "deg" ? v : unit === "turn" ? v * 360 : unit === "grad" ? v * 0.9 : (v * 180) / Math.PI };
  return { k: "dimension", v, unit };
}

function parseCalc(text) {
  // Recursive descent over + - * / with parentheses.
  const tokens = [];
  let i = 0;
  const s = text.trim();
  while (i < s.length) {
    const ch = s[i];
    if (/\s/.test(ch)) i++;
    else if ("+-*/".includes(ch) && !(ch === "-" && tokens.length === 0) && !((ch === "+" || ch === "-") && /[\d.]/.test(s[i + 1] ?? "") && (tokens.length === 0 || ["op", "("].includes(tokens.at(-1).t)))) {
      tokens.push({ t: "op", v: ch });
      i++;
    } else if (ch === "(") {
      tokens.push({ t: "(" });
      i++;
    } else if (ch === ")") {
      tokens.push({ t: ")" });
      i++;
    } else {
      let depth = 0;
      let j = i;
      while (j < s.length) {
        const c = s[j];
        if (c === "(") depth++;
        else if (c === ")") {
          if (depth === 0) break;
          depth--;
        } else if (depth === 0 && (/\s/.test(c) || "*/".includes(c))) break;
        else if (depth === 0 && (c === "+" || c === "-") && j > i && !/[eE]/.test(s[j - 1])) break;
        j++;
      }
      tokens.push({ t: "term", v: s.slice(i, j) });
      i = j;
    }
  }
  let pos = 0;
  const primary = () => {
    const tok = tokens[pos++];
    if (!tok) throw new Error(`calc: unexpected end in ${text}`);
    if (tok.t === "(") {
      const inner = sum();
      pos++;
      return inner;
    }
    return parseTerm(tok.v);
  };
  const product = () => {
    let left = primary();
    while (tokens[pos]?.t === "op" && "*/".includes(tokens[pos].v)) {
      const op = tokens[pos++].v;
      left = { k: "calc", op, l: left, r: primary() };
    }
    return left;
  };
  const sum = () => {
    let left = product();
    while (tokens[pos]?.t === "op" && "+-".includes(tokens[pos].v)) {
      const op = tokens[pos++].v;
      left = { k: "calc", op, l: left, r: product() };
    }
    return left;
  };
  return sum();
}

function parseMixOperand(text) {
  const terms = splitTop(text, " ");
  if (terms.length === 1) return [parseTerm(terms[0]), null];
  const looksLikePercent = (t) => isPercentLiteral(t) || (fnCall(t)?.name === "calc" && t.includes("%"));
  if (terms.length === 2) {
    return looksLikePercent(terms[0]) ? [parseTerm(terms[1]), parseTerm(terms[0])] : [parseTerm(terms[0]), parseTerm(terms[1])];
  }
  throw new Error(`color-mix operand: ${text}`);
}

export function parseTerm(term) {
  const t = term.trim();
  if (t === "") return { k: "empty" };
  const call = fnCall(t);
  if (call) {
    const { name, args } = call;
    if (name === "var") {
      const parts = splitTop(args, ",");
      const fallback = parts.length > 1 ? parseValue(args.slice(args.indexOf(",") + 1)) : null;
      return { k: "var", name: parts[0], fallback };
    }
    if (name === "calc") return parseCalc(args);
    if (name === "light-dark") {
      const [light, dark] = splitTop(args, ",");
      return { k: "lightDark", light: parseValue(light), dark: parseValue(dark) };
    }
    if (name === "color-mix") {
      const parts = splitTop(args, ",");
      const space = /^in\s+([\w-]+)/.exec(parts[0])?.[1];
      if (!space || parts.length !== 3) throw new Error(`color-mix: ${t}`);
      const [a, pa] = parseMixOperand(parts[1]);
      const [b, pb] = parseMixOperand(parts[2]);
      return { k: "mix", space, a, pa, b, pb };
    }
    if (name === "--alpha") {
      const [color, pct] = args.split("/").map((p) => p.trim());
      return { k: "alpha", color: parseTerm(color), pct: parseTerm(pct) };
    }
    if (["rgb", "rgba"].includes(name) && /^from\s/.test(args.trim())) {
      const m = /^from\s+([\s\S]+?)\s+r\s+g\s+b(?:\s*\/\s*([\s\S]+))?$/.exec(args.trim());
      if (!m) throw new Error(`relative colour: ${t}`);
      return { k: "relative", from: parseTerm(m[1]), alpha: m[2] ? parseValue(m[2]) : null };
    }
    if (["rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color"].includes(name)) {
      if (/var\(|calc\(|\bfrom\b/.test(args)) return { k: "raw", text: t };
      const color = literalColor(t);
      if (color) return { k: "color", c: color };
      return { k: "raw", text: t };
    }
    if (name === "cubic-bezier") return { k: "cubicBezier", p: splitTop(args, ",").map(Number) };
    if (name === "steps") {
      const [n, jump] = splitTop(args, ",");
      return { k: "steps", n: Number(n), jump: jump ?? "end" };
    }
    if (name === "env") {
      const parts = splitTop(args, ",");
      return { k: "env", name: parts[0], fallback: parts.length > 1 ? parseValue(args.slice(args.indexOf(",") + 1)) : null };
    }
    if (["blur", "saturate", "brightness", "contrast", "grayscale", "hue-rotate", "invert", "opacity", "sepia"].includes(name)) {
      return { k: "filter", name, arg: parseValue(args) };
    }
    if (["max", "min", "clamp"].includes(name)) return { k: "fn", name, args: splitTop(args, ",").map(parseValue) };
    return { k: "raw", text: t };
  }
  if (/^#[0-9a-f]{3,8}$/i.test(t)) {
    const color = literalColor(t);
    if (color) return { k: "color", c: color };
  }
  const dimension = parseDimension(t);
  if (dimension) return dimension;
  if (/^["'][\s\S]*["']$/.test(t)) return { k: "string", v: t.slice(1, -1) };
  if (/^[a-zA-Z-]+$/.test(t)) {
    const lower = t.toLowerCase();
    if (lower === "currentcolor") return { k: "currentColor" };
    if (lower === "initial" || lower === "unset" || lower === "inherit") return { k: "initial", v: lower };
    const color = literalColor(lower);
    if (color && lower !== "none") return { k: "color", c: color, name: lower };
    return { k: "keyword", v: t };
  }
  return { k: "raw", text: t };
}

export function parseValue(text) {
  const value = text.trim();
  const commas = splitTop(value, ",");
  if (commas.length > 1) return { k: "list", sep: ",", items: commas.map(parseSpaceList) };
  return parseSpaceList(value);
}

function parseSpaceList(text) {
  const terms = splitTop(text, " ");
  if (terms.length === 0) return { k: "empty" };
  if (terms.length === 1) return parseTerm(terms[0]);
  // A slash splits `a / b` pairs; keep them as raw to stay faithful.
  return { k: "list", sep: " ", items: terms.map(parseTerm) };
}

/** Typed shadow layers from a parsed comma list (or single layer). */
export function toShadowLayers(expr) {
  const layers = expr.k === "list" && expr.sep === "," ? expr.items : [expr];
  return layers.map((layer) => {
    const items = layer.k === "list" && layer.sep === " " ? layer.items : [layer];
    const inset = items.some((i) => i.k === "keyword" && i.v === "inset");
    const lengths = items.filter((i) => i.k === "length" || (i.k === "number" && i.v === 0) || i.k === "calc");
    const color = items.find((i) => ["color", "mix", "var", "alpha", "relative", "lightDark"].includes(i.k)) ?? null;
    const asLength = (i) => (i ? (i.k === "number" ? { k: "length", px: 0, rem: 0 } : i) : { k: "length", px: 0, rem: 0 });
    return { inset, x: asLength(lengths[0]), y: asLength(lengths[1]), blur: asLength(lengths[2]), spread: asLength(lengths[3]), color };
  });
}
