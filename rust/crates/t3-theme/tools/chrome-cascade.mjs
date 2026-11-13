// Loads the real index.css (Tailwind v4 syntax lowered to native CSS) in Chrome, applies
// page states the way the app does, and records what the browser computes for every
// colour and length token. Writes fixtures/chrome-tokens.json for the Rust cascade tests.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, pnpm, repoRoot, withChrome } from "./lib.mjs";

const postcss = (await import(pnpm("postcss@8.5.30", "postcss/lib/postcss.js"))).default;
const palettes = JSON.parse(readFileSync(join(crateDir, "fixtures/palettes.json"), "utf8"));
const tokens = JSON.parse(readFileSync(join(crateDir, "fixtures/tokens.json"), "utf8"));

const indexCss = readFileSync(join(repoRoot, "apps/web/src/index.css"), "utf8");
const tailwindCss = readFileSync(join(repoRoot, "node_modules/.pnpm/tailwindcss@4.3.3/node_modules/tailwindcss/theme.css"), "utf8");

function lowerValue(value) {
  // --alpha(color / N%) is Tailwind sugar for a colour mixed with transparent in oklab.
  let out = value;
  for (;;) {
    const at = out.indexOf("--alpha(");
    if (at < 0) return out;
    let depth = 0;
    let end = at + 7;
    for (; end < out.length; end++) {
      if (out[end] === "(") depth++;
      else if (out[end] === ")" && --depth === 0) break;
    }
    const [color, pct] = out.slice(at + 8, end).split("/").map((s) => s.trim());
    out = `${out.slice(0, at)}color-mix(in oklab, ${color} ${pct}, transparent)${out.slice(end + 1)}`;
  }
}

function lower(root) {
  const themeDecls = [];
  const inlineDecls = [];
  root.walkAtRules((rule) => {
    if (["import", "custom-variant", "apply"].includes(rule.name)) rule.remove();
  });
  root.walkAtRules("utility", (rule) => rule.remove());
  root.walkAtRules("theme", (rule) => {
    const inline = /\binline\b/.test(rule.params);
    for (const node of rule.nodes ?? []) if (node.type === "decl") (inline ? inlineDecls : themeDecls).push([node.prop, node.value]);
    rule.remove();
  });
  root.walkAtRules("variant", (rule) => {
    const name = rule.params.trim();
    const wrapper = { dark: "&:is(.dark, .dark *)", light: "&:not(.dark, .dark *)", wco: "&:is(.wco, .wco *)" }[name];
    if (wrapper) rule.replaceWith(postcss.rule({ selector: wrapper, nodes: rule.nodes }));
    else if (name === "sm") rule.replaceWith(postcss.atRule({ name: "media", params: "(min-width: 40rem)", nodes: rule.nodes }));
    else rule.remove();
  });
  root.walkDecls((decl) => {
    if (decl.value.includes("--alpha(")) decl.value = lowerValue(decl.value);
  });
  return { themeDecls, inlineDecls };
}

const tw = postcss.parse(tailwindCss);
const twLowered = lower(tw);
const ix = postcss.parse(indexCss);
const ixLowered = lower(ix);
const decl = ([prop, value]) => `${prop}: ${lowerValue(value)};`;
const themeCss = `@layer theme, base, clerk, components, utilities;
@layer theme {
  :root { ${[...twLowered.themeDecls, ...ixLowered.themeDecls].map(decl).join("\n")} }
  :root, [data-app-sidebar] { ${ixLowered.inlineDecls.map(decl).join("\n")} }
}`;
const css = `${themeCss}\n${ix.toString()}`;

// ------------------------------------------------------------ contexts

const roleVars = Object.fromEntries(palettes.roles.map((r) => [r.name, r.cssVariable]));
const themeColorsFor = (theme, mode) => (mode === theme.appearance ? theme.colors : theme.variants?.[mode]);
const contexts = [
  { name: "standard-light", dark: false, themeId: null, colors: null },
  { name: "standard-dark", dark: true, themeId: null, colors: null },
  { name: "standard-light-wide", dark: false, themeId: null, colors: null, viewportRem: 80 },
  { name: "standard-dark-contrast-130", dark: true, themeId: null, colors: null, contrast: 130 },
  { name: "standard-light-contrast-70", dark: false, themeId: null, colors: null, contrast: 70 },
  { name: "standard-light-blue-orange", dark: false, themeId: null, colors: null, diffScheme: "blue-orange" },
  { name: "standard-dark-blue-orange", dark: true, themeId: null, colors: null, diffScheme: "blue-orange" },
  { name: "standard-light-wco-wide-chat", dark: false, themeId: null, colors: null, wco: true, chatWidth: "wide" },
];
for (const theme of palettes.builtInThemes) {
  for (const mode of ["light", "dark"]) {
    const colors = themeColorsFor(theme, mode);
    if (!colors) continue;
    contexts.push({ name: `${theme.id}-${mode}`, dark: mode === "dark", themeId: theme.id, colors });
  }
}
contexts.push({
  name: "grove-dark-contrast-140",
  dark: true,
  themeId: "grove",
  colors: themeColorsFor(palettes.builtInThemes.find((t) => t.id === "grove"), "dark") ?? palettes.builtInThemes[1].variants.dark,
  contrast: 140,
});

const syntax = JSON.parse(readFileSync(join(crateDir, "fixtures/syntax.json"), "utf8"));
const diffCss = (await import(pathToFileURL(join(repoRoot, "apps/web/src/lib/diffRendering.ts")).href)).DIFF_SURFACE_THEME_UNSAFE_CSS;
const SURFACE_CONTEXTS = new Set(["standard-light", "standard-dark", "standard-light-blue-orange", "standard-dark-blue-orange", "grove-dark-contrast-140", "ocean-light", "iris-dark", "standard-dark-contrast-130"]);
const surfaceNames = [...new Set([...diffCss.matchAll(/(--diffs-[\w-]+)\s*:/g)].map((m) => m[1]))];
const names = Object.entries(tokens.categories)
  .filter(([, category]) => !["image", "texture"].includes(category) && !category.startsWith("other") && !category.startsWith("typography.family"))
  .map(([name]) => name)
  .filter((name) => name !== "color-scheme");

const results = await withChrome(async (page) => {
  await page.setViewportSize({ width: 600, height: 400 });
  await page.setContent(`<!doctype html><html><head><style>${css}
${diffCss}</style></head><body><div id="diff" data-diff></div><div id="sb" data-app-sidebar></div><span id="probe"></span></body></html>`);
  const out = [];
  for (const context of contexts) {
    await page.setViewportSize({ width: context.viewportRem ? context.viewportRem * 16 : 600, height: 400 });
    const data = await page.evaluate(
      ({ context, names, roleVars, surfaceNames, surface }) => {
        const html = document.documentElement;
        html.className = [context.dark ? "dark" : "", context.wco ? "wco" : ""].filter(Boolean).join(" ");
        for (const attr of ["data-theme-id", "data-diff-color-scheme", "data-chat-width"]) html.removeAttribute(attr);
        if (context.themeId) html.setAttribute("data-theme-id", context.themeId);
        if (context.diffScheme) html.setAttribute("data-diff-color-scheme", context.diffScheme);
        if (context.chatWidth) html.setAttribute("data-chat-width", context.chatWidth);
        html.removeAttribute("style");
        for (const [role, value] of Object.entries(context.colors ?? {})) html.style.setProperty(roleVars[role], value);
        const contrast = context.contrast ?? 100;
        html.style.setProperty("--appearance-contrast-base", `${Math.min(contrast, 100)}%`);
        html.style.setProperty("--appearance-contrast-boost", `${Math.max(contrast - 100, 0)}%`);
        html.style.setProperty("--appearance-contrast-border-boost", `${Math.max(contrast - 100, 0) / 4}%`);
        const canvas = document.createElement("canvas");
        canvas.width = 1;
        canvas.height = 1;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        const paint = (backdrop, color) => {
          ctx.fillStyle = backdrop;
          ctx.fillRect(0, 0, 1, 1);
          ctx.fillStyle = color;
          ctx.fillRect(0, 0, 1, 1);
          return [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3);
        };
        const probe = document.getElementById("probe");
        const read = (element, scope) => {
          const colors = {};
          const lengths = {};
          element.appendChild(probe);
          // An invalid var() makes the property unset, so it inherits these sentinels.
          element.style.color = "rgb(9, 9, 9)";
          element.style.textIndent = "3.37px";
          for (const name of names) {
            probe.style.cssText = `color: var(${name}); text-indent: var(${name});`;
            const style = getComputedStyle(probe);
            const color = style.color;
            if (color !== "rgb(9, 9, 9)") colors[name] = [...paint("#000", color), ...paint("#fff", color)];
            const indent = style.textIndent;
            if (indent !== "3.37px") lengths[name] = Number.parseFloat(indent);
          }
          element.style.removeProperty("color");
          element.style.removeProperty("text-indent");
          return { colors, lengths };
        };
        let surfaceColors = null;
        if (surface) {
          const diff = document.getElementById("diff");
          const created = context.dark ? surface.dark : surface.light;
          diff.style.cssText = [
            `--diffs-addition-base: var(--diffs-addition-color-override, ${created.created})`,
            `--diffs-deletion-base: var(--diffs-deletion-color-override, ${created.deleted})`,
            `--diffs-modified-base: var(--diffs-modified-color-override, ${created.modified})`,
          ].join(";");
          diff.appendChild(probe);
          // The diff rule forces colour and background, so probe through the inherited fill instead.
          diff.style.fill = "rgb(9, 9, 9)";
          surfaceColors = {};
          for (const name of [...surfaceNames, "--diffs-addition-base", "--diffs-deletion-base", "--diffs-modified-base"]) {
            probe.style.cssText = `fill: var(${name});`;
            const color = getComputedStyle(probe).fill;
            if (color !== "rgb(9, 9, 9)") surfaceColors[name] = [...paint("#000", color), ...paint("#fff", color)];
          }
          document.body.appendChild(probe);
        }
        const root = read(html, "root");
        const sidebar = read(document.getElementById("sb"), "sidebar");
        return { root, sidebar, surface: surfaceColors };
      },
      {
        context, names, roleVars, surfaceNames,
        surface: SURFACE_CONTEXTS.has(context.name)
          ? {
              light: { created: syntax.themes["t3-light"].style.created, deleted: syntax.themes["t3-light"].style.deleted, modified: syntax.themes["t3-light"].style.modified },
              dark: { created: syntax.themes["pierre-dark"].style.created, deleted: syntax.themes["pierre-dark"].style.deleted, modified: syntax.themes["pierre-dark"].style.modified },
            }
          : null,
      },
    );
    out.push({ context, ...data });
  }
  return out;
});

writeFileSync(
  join(crateDir, "fixtures/chrome-tokens.json"),
  `${JSON.stringify({ chrome: "headless Chrome 157", names, contexts: results })}\n`,
);
for (const r of results) {
  console.log(r.context.name.padEnd(34), "root", Object.keys(r.root.colors).length, "colours", Object.keys(r.root.lengths).length, "lengths; sidebar", Object.keys(r.sidebar.colors).length, "colours");
}
