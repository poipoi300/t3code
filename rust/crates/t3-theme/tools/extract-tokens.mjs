// Extracts the design-token declarations from apps/web/src/index.css, Tailwind's default
// theme and the diff-surface CSS into fixtures/tokens.json. The Rust crate resolves the
// cascade from that table; nothing is copied by hand.
//   node tools/extract-tokens.mjs            writes the fixture
//   node tools/extract-tokens.mjs --check    exits 1 when the checked-in fixture is stale
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, pnpm, repoRoot } from "./lib.mjs";
import { parseTerm, parseValue, splitTop, toShadowLayers } from "./css-value.mjs";

const postcss = (await import(pnpm("postcss@8.5.30", "postcss/lib/postcss.js"))).default;
const indexCssPath = join(repoRoot, "apps/web/src/index.css");
const tailwindThemePath = join(repoRoot, "node_modules/.pnpm/tailwindcss@4.3.3/node_modules/tailwindcss/theme.css");
const diffRendering = await import(pathToFileURL(join(repoRoot, "apps/web/src/lib/diffRendering.ts")).href).catch(() => null);

const indexCss = readFileSync(indexCssPath, "utf8");
const tailwindCss = readFileSync(tailwindThemePath, "utf8");

let order = 0;
const declarations = [];
const keyframes = [];
const properties = [];
const utilities = [];
const motion = [];
const zIndex = [];
const breakpointsUsed = new Set();
const variants = [];
const stats = { ignoredRules: 0, ignoredSelectors: new Set() };

const LAYER_ORDER = ["theme", "base", "clerk", "components", "utilities"];

// ---------------------------------------------------------------- selectors

/** Parses one compound selector made of the forms the stylesheet uses for token scopes. */
function parseCompound(text) {
  const attrs = [];
  const classes = [];
  let rest = text.trim();
  let tag = null;
  let specificity = [0, 0, 0];
  // A bare class (.wco, .electron-windows) styles the root element when it carries the class.
  if (/^\.[\w-]/.test(rest)) tag = "html";
  if (rest.startsWith(":root")) {
    tag = "html";
    rest = rest.slice(5);
    specificity[1] += 1;
  } else if (rest.startsWith("html")) {
    tag = "html";
    rest = rest.slice(4);
    specificity[2] += 1;
  }
  while (rest.length > 0) {
    let m;
    if ((m = /^\[([\w-]+)(?:=(?:"([^"]*)"|'([^']*)'))?\]/.exec(rest))) {
      attrs.push({ name: m[1], op: m[2] === undefined && m[3] === undefined ? "present" : "equals", value: m[2] ?? m[3] ?? null });
      specificity[1] += 1;
    } else if ((m = /^:not\(\[([\w-]+)(?:=(?:"([^"]*)"|'([^']*)'))?\]\)/.exec(rest))) {
      attrs.push({ name: m[1], op: m[2] === undefined && m[3] === undefined ? "absent" : "notEquals", value: m[2] ?? m[3] ?? null });
      specificity[1] += 1;
    } else if ((m = /^\.([\w-]+)/.exec(rest))) {
      classes.push(m[1]);
      specificity[1] += 1;
    } else return null;
    rest = rest.slice(m[0].length);
  }
  return { tag, attrs, classes, specificity };
}

/** Expands a selector list into target alternatives with their specificity, or null if unsupported. */
function parseSelector(selector) {
  const alts = [];
  const list = (text) => splitTop(text.replace(/\s+/g, " "), ",");
  for (const part of list(selector)) {
    const isMatch = /^:is\(([\s\S]*)\)$/.exec(part);
    const inner = isMatch ? list(isMatch[1]) : [part];
    const group = [];
    for (const item of inner) {
      // `html[...] [data-app-sidebar]` or `[data-app-sidebar]` or a plain html compound.
      const sidebar = /^(.*?)\s*\[data-app-sidebar\]$/.exec(item);
      if (sidebar) {
        const html = sidebar[1] ? parseCompound(sidebar[1]) : { tag: "html", attrs: [], classes: [], specificity: [0, 0, 0] };
        if (!html) return null;
        group.push({ target: "sidebar", html: { attrs: html.attrs, classes: html.classes }, specificity: [html.specificity[0], html.specificity[1] + 1, html.specificity[2]] });
        continue;
      }
      const compound = parseCompound(item);
      if (!compound || compound.tag !== "html") return null;
      group.push({ target: "root", html: { attrs: compound.attrs, classes: compound.classes }, specificity: compound.specificity });
    }
    if (isMatch) {
      const max = group.reduce((best, g) => (cmp(g.specificity, best) > 0 ? g.specificity : best), [0, 0, 0]);
      for (const g of group) alts.push({ ...g, specificity: max });
    } else alts.push(...group);
  }
  return alts;
}
const cmp = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];

// ---------------------------------------------------------------- walking

const isCustom = (prop) => prop.startsWith("--");

function recordDeclarations(node, base) {
  for (const child of node.nodes ?? []) {
    if (child.type === "decl" && (isCustom(child.prop) || child.prop === "color-scheme")) {
      let value;
      try {
        value = parseValue(child.value);
      } catch (error) {
        value = { k: "raw", text: child.value, error: String(error.message) };
      }
      for (const alt of base.alts) {
        declarations.push({
          name: child.prop,
          value,
          important: child.important === true,
          layer: base.layer,
          order: order++,
          specificity: [base.extra[0] + alt.specificity[0], base.extra[1] + alt.specificity[1], base.extra[2] + alt.specificity[2]],
          target: alt.target,
          html: alt.html,
          when: base.when,
          origin: base.origin,
          inline: base.inline ?? false,
        });
      }
    } else if (child.type === "atrule" && child.name === "variant") {
      const variant = child.params.trim();
      const when = { ...base.when };
      let extra = [0, 0, 0];
      if (variant === "dark") {
        when.dark = true;
        extra = [0, 1, 0];
      } else if (variant === "light") {
        when.dark = false;
        extra = [0, 1, 0];
      } else if (variant === "sm") when.minWidthRem = 40;
      else if (variant === "wco") {
        when.wco = true;
        extra = [0, 1, 0];
      } else {
        variants.push(variant);
        continue;
      }
      recordDeclarations(child, { ...base, when, extra: [base.extra[0] + extra[0], base.extra[1] + extra[1], base.extra[2] + extra[2]] });
    }
  }
}

function recordUses(node, selectorText, context) {
  for (const child of node.nodes ?? []) {
    if (child.type === "decl") {
      const prop = child.prop;
      if (prop === "z-index") zIndex.push({ selector: selectorText, value: child.value.trim(), ...context });
      if (/^(transition|animation)(-(duration|timing-function|delay|iteration-count|name))?$/.test(prop)) {
        motion.push({ selector: selectorText, property: prop, value: child.value.replace(/\s+/g, " ").trim(), ...context });
      }
    } else if (child.type === "rule") {
      recordUses(child, `${selectorText} ${child.selector}`.replace(/\s+/g, " "), context);
    } else if (child.type === "atrule" && !["keyframes"].includes(child.name)) {
      recordUses(child, selectorText, { ...context, nested: child.name === "variant" ? `@variant ${child.params}` : `@${child.name} ${child.params}`.trim() });
    }
  }
}

function keyframesOf(atrule, origin) {
  const frames = [];
  for (const stop of atrule.nodes ?? []) {
    if (stop.type !== "rule") continue;
    const props = {};
    for (const d of stop.nodes ?? []) if (d.type === "decl") props[d.prop] = d.value.replace(/\s+/g, " ").trim();
    frames.push({ stops: stop.selector.split(",").map((s) => s.trim()), properties: props });
  }
  keyframes.push({ name: atrule.params.trim(), origin, frames });
}

function walk(container, state) {
  for (const node of container.nodes ?? []) {
    if (node.type === "atrule") {
      switch (node.name) {
        case "layer":
          if (node.nodes) walk(node, { ...state, layer: node.params.trim() });
          break;
        case "theme": {
          // `reference` blocks only feed utilities that use them; Tailwind emits no variables for them.
          if (/\breference\b/.test(node.params)) break;
          const inline = /\binline\b/.test(node.params);
          // `@theme inline` values are substituted where they are used, so they resolve
          // against whichever element carries the utility: the root or the sidebar.
          recordDeclarations(node, {
            alts: [
              { target: "root", html: { attrs: [], classes: [] }, specificity: [0, 1, 0] },
              ...(inline ? [{ target: "sidebar", html: { attrs: [], classes: [] }, specificity: [0, 1, 0] }] : []),
            ],
            layer: "theme", extra: [0, 0, 0], when: {}, origin: state.origin, inline,
          });
          for (const child of node.nodes ?? []) if (child.type === "atrule" && child.name === "keyframes") keyframesOf(child, state.origin);
          break;
        }
        case "utility": {
          const declarationsOfUtility = [];
          const collect = (n, variant) => {
            for (const child of n.nodes ?? []) {
              if (child.type === "decl") {
                let value;
                try {
                  value = parseValue(child.value);
                } catch {
                  value = { k: "raw", text: child.value };
                }
                declarationsOfUtility.push({ prop: child.prop, value, variant, important: child.important === true });
              } else if (child.type === "atrule" && child.name === "variant") collect(child, child.params.trim());
              else if (child.type === "rule") collect(child, `${variant ?? ""}${child.selector}`);
            }
          };
          collect(node, null);
          utilities.push({ name: node.params.trim(), declarations: declarationsOfUtility });
          recordUses(node, `@utility ${node.params}`, { layer: state.layer ?? null });
          break;
        }
        case "keyframes":
          keyframesOf(node, state.origin);
          break;
        case "property": {
          const props = {};
          for (const d of node.nodes ?? []) if (d.type === "decl") props[d.prop] = d.value;
          properties.push({ name: node.params.trim(), ...props });
          break;
        }
        case "custom-variant":
          variants.push(`custom-variant ${node.params}`);
          break;
        case "media":
        case "supports":
          recordUses(node, "", { layer: state.layer ?? null, condition: `@${node.name} ${node.params}` });
          for (const m of node.params.matchAll(/min-width:\s*(\d+)px/g)) breakpointsUsed.add(Number(m[1]));
          break;
        default:
          break;
      }
    } else if (node.type === "rule") {
      const alts = parseSelector(node.selector);
      recordUses(node, node.selector.replace(/\s+/g, " "), { layer: state.layer ?? null });
      const hasCustom = (node.nodes ?? []).some((d) => (d.type === "decl" && (isCustom(d.prop) || d.prop === "color-scheme")) || (d.type === "atrule" && d.name === "variant"));
      if (!alts) {
        if (hasCustom && (node.nodes ?? []).some((d) => d.type === "decl" && isCustom(d.prop))) {
          stats.ignoredRules += 1;
          stats.ignoredSelectors.add(node.selector.replace(/\s+/g, " "));
        }
        continue;
      }
      recordDeclarations(node, { alts, layer: state.layer ?? null, extra: [0, 0, 0], when: {}, origin: state.origin });
    }
  }
}

postcss.parse(tailwindCss).walkAtRules("theme", () => {});
walk(postcss.parse(tailwindCss), { layer: null, origin: "tailwind" });
const tailwindDeclarations = declarations.length;
walk(postcss.parse(indexCss), { layer: null, origin: "index.css" });

// ---------------------------------------------------------------- the diff surface css

const surfaces = [];
if (diffRendering?.DIFF_SURFACE_THEME_UNSAFE_CSS) {
  const root = postcss.parse(diffRendering.DIFF_SURFACE_THEME_UNSAFE_CSS);
  root.walkRules((rule) => {
    const selectors = splitTop(rule.selector.replace(/\s+/g, " "), ",");
    const decls = [];
    rule.walkDecls((d) => {
      let value;
      try {
        value = parseValue(d.value);
      } catch {
        value = { k: "raw", text: d.value };
      }
      decls.push({ prop: d.prop, value, important: d.important === true });
    });
    surfaces.push({ selectors, declarations: decls });
  });
}

// ---------------------------------------------------------------- classification

const lastValue = new Map();
for (const d of declarations) lastValue.set(d.name, d.value);

const typeCache = new Map();
function typeOf(name, seen = new Set()) {
  if (typeCache.has(name)) return typeCache.get(name);
  // Inputs the theme layer supplies at runtime (applyThemePalette) and the diff highlighter's bases.
  if (name.startsWith("--app-theme-") || /^--diffs-(addition|deletion|modified)-base$/.test(name)) return "color";
  if (seen.has(name)) return "unknown";
  seen.add(name);
  const value = lastValue.get(name);
  const type = value ? typeOfExpr(value, seen) : "unknown";
  typeCache.set(name, type);
  return type;
}
function typeOfExpr(expr, seen) {
  switch (expr.k) {
    case "color": case "mix": case "alpha": case "relative": case "currentColor": return "color";
    case "length": return "length";
    case "number": return "number";
    case "percent": return "percent";
    case "duration": return "duration";
    case "cubicBezier": case "steps": return "easing";
    case "lightDark": return typeOfExpr(expr.light, seen);
    case "var": return typeOf(expr.name, seen);
    case "calc": {
      const l = typeOfExpr(expr.l, seen);
      const r = typeOfExpr(expr.r, seen);
      if (expr.op === "*" ) return l === "length" || r === "length" ? "length" : l;
      return l === "unknown" ? r : l;
    }
    case "list": return expr.sep === "," ? "list" : "compound";
    case "keyword": return "keyword";
    case "raw": return /gradient|url\(/.test(expr.text) ? "image" : "raw";
    default: return "unknown";
  }
}

function categoryOf(name) {
  const type = typeOf(name);
  const has = (re) => re.test(name);
  if (has(/^--(font-sans|font-serif|font-mono|font-composer|diffs-font-family|diffs-header-font-family)$/)) return "typography.family";
  if (has(/^--text-[\w]+--line-height$/) || has(/^--leading-/)) return "typography.line-height";
  if (has(/^--tracking-/)) return "typography.letter-spacing";
  if (has(/^--font-weight-/)) return "typography.weight";
  if (has(/^--(text-|font-size)/)) return "typography.size";
  if (has(/^--(shadow|inset-shadow|drop-shadow|text-shadow)/)) return "shadow";
  if (has(/^--ease-|^--animate-|^--default-transition|transition-duration$/)) return "motion";
  if (has(/^--z-/)) return "z-index";
  if (has(/^--breakpoint-/)) return "breakpoint";
  if (has(/^--blur-|^--glass-blur$/)) return "blur";
  if (has(/^--glass-|^--assistant-citation-highlight-opacity$|opacity$/)) return "effect";
  if (has(/^--radius|-radius$/)) return "radius";
  if (has(/^--container-|^--perspective-|^--aspect-/)) return "layout";
  if (has(/^--(spacing|workspace-gutter)|inset$|-gap$|gutter/) && type === "length") return "spacing";
  if (has(/^--appearance-contrast/)) return "appearance";
  if (has(/^--surface-grain/)) return "texture";
  if (has(/^--stage-/)) return "stage-art";
  if (type === "length" || type === "number" && has(/size|height|width|max-width/)) return "layout";
  if (type === "color") {
    if (has(/^--color-(red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone|taupe|mauve|olive|mist)-\d+$/) || has(/^--color-(white|black|zinc-25)$/)) return "color.palette";
    if (has(/border|^--input$|^--contrast-input$|^--ring$/)) return "border";
    if (has(/^--color-/)) return "color.alias";
    if (has(/^--contrast-/)) return "color.contrast";
    return "color";
  }
  if (type === "image") return "image";
  return `other.${type}`;
}

const names = [...new Set(declarations.map((d) => d.name))];
const categories = {};
for (const n of names) categories[n] = categoryOf(n);

// typed shadows for the shadow category
const shadows = {};
for (const n of names) {
  if (categories[n] === "shadow") {
    const v = lastValue.get(n);
    // `inset-shadow-*` utilities paint their token inside the box even when the value omits `inset`.
    if (v) shadows[n] = toShadowLayers(v).map((layer) => (n.startsWith("--inset-shadow") ? { ...layer, inset: true } : layer));
  }
}

const counts = {};
for (const c of Object.values(categories)) counts[c] = (counts[c] ?? 0) + 1;

const fixture = {
  source: {
    indexCss: createHash("sha256").update(indexCss).digest("hex"),
    tailwindTheme: createHash("sha256").update(tailwindCss).digest("hex"),
    tailwindVersion: "4.3.3",
    declarations: { tailwind: tailwindDeclarations, indexCss: declarations.length - tailwindDeclarations },
  },
  layers: LAYER_ORDER,
  declarations,
  categories,
  shadows,
  keyframes,
  properties,
  utilities,
  surfaces,
  motion,
  zIndex,
  breakpointsUsedPx: [...breakpointsUsed].sort((a, b) => a - b),
  ignored: { rules: stats.ignoredRules, selectors: [...stats.ignoredSelectors].sort() },
  variants: [...new Set(variants)].sort(),
};
const text = `${JSON.stringify(fixture)}\n`;
const target = join(crateDir, "fixtures/tokens.json");
if (process.argv.includes("--check")) {
  if (!existsSync(target) || readFileSync(target, "utf8") !== text) {
    console.error("fixtures/tokens.json is stale; run node tools/extract-tokens.mjs");
    process.exit(1);
  }
  process.exit(0);
}
writeFileSync(target, text);
console.log("declarations", declarations.length, "names", names.length, "keyframes", keyframes.length, "surfaces", surfaces.length);
console.log(JSON.stringify(counts, null, 1));
console.log("ignored selectors:", fixture.ignored.selectors.length, "ignored rules", fixture.ignored.rules);
