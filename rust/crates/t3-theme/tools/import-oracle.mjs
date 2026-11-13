// Runs the real vscodeThemeImport.ts and openVsxThemes.ts and records their results in
// fixtures/import-oracle.json. Run: node --import ./tools/ts-loader.mjs tools/import-oracle.mjs
import { writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, repoRoot, rng, pnpm } from "./lib.mjs";

const src = (rel) => import(pathToFileURL(join(repoRoot, rel)).href);
const vscode = await src("apps/web/src/vscodeThemeImport.ts");
const openVsx = await src("apps/web/src/openVsxThemes.ts");
const JSZip = (await import(pnpm("jszip@3.10.1", "jszip/lib/index.js"))).default;

const attempt = (fn) => {
  try {
    return { ok: fn() };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
};
const rand = rng(1234);
const pick = (list) => list[Math.floor(rand() * list.length)];

const VSCODE_DARK = {
  name: "pierre-dark-soft", type: "dark",
  colors: {
    "editor.background": "#171717", "editor.foreground": "#d4d4d4", foreground: "#d4d4d4", "sideBar.background": "#101010",
    "sideBar.foreground": "#8a8a8a", "sideBar.border": "#1d1d1d", focusBorder: "#69b1ff", "button.background": "#69b1ff",
    "button.foreground": "#171717", "input.border": "#2c2c2c", "input.placeholderForeground": "#525252",
    "terminal.background": "#101010", "terminal.foreground": "#8a8a8a", "list.hoverBackground": "#1f3e5e59",
    "list.activeSelectionBackground": "#1f3e5e99",
  },
  tokenColors: [],
};
const t = (name, type, colors) => ({ name, type, colors });
const files = [
  VSCODE_DARK,
  t("Dark placeholder fallback", "dark", { "editor.background": "#1e1e2e", "editor.foreground": "#cdd6f4", "input.placeholderForeground": "#cdd6f473" }),
  t("Dark+", "dark", { "editor.background": "#1e1e1e" }),
  t("Light+", "light", { "editor.background": "#ffffff" }),
  t("Catppuccin Mocha", "dark", { "editor.background": "#1e1e2e", "input.border": "#00000000", "input.background": "#313244" }),
  t("Gruvbox Light Soft", "light", { "editor.background": "#f2e5bc", "button.background": "#45858880", "input.background": "#45858880", "input.border": "#928374" }),
  t("Vitesse Dark", "dark", { "editor.background": "#121212", focusBorder: "#00000000", "button.background": "#4d9375" }),
  t("Raised focus", "dark", { "editor.background": "#000000", focusBorder: "#111111", "editorWidget.background": "#111111", "button.background": "#4d9375" }),
  t("Transparent button", "dark", { "editor.background": "#121212", focusBorder: "#4d9375", "button.background": "#00000000" }),
  { type: "dark", colors: { "editor.background": "#202020", "editor.findMatchBackground": "#33669980", "editor.findMatchForeground": "#ffffffcc", "editor.findMatchHighlightBackground": "#ffcc0040", "editor.findMatchHighlightForeground": "#ffffff" } },
  ...["light", "dark"].flatMap((type) => {
    const base = { "editor.background": type === "dark" ? "#101010" : "#fafafa" };
    return [
      { type, colors: base },
      { type, colors: { ...base, "editor.findMatchBackground": "not-a-colour", "editor.findMatchForeground": 42, "editor.findMatchHighlightBackground": null, "editor.findMatchHighlightForeground": "invalid" } },
      { type, colors: { ...base, "editor.findMatchBackground": "#ffffcc", "editor.findMatchForeground": "#ffffff", "editor.findMatchHighlightBackground": "#101020", "editor.findMatchHighlightForeground": "#111111" } },
    ];
  }),
  t("Untyped", undefined, { "editor.background": "#fdfdfd", "editor.foreground": "#202020" }),
  t("High contrast", "hc-light", { "editor.background": "#ffffff" }),
  t("High contrast dark", "hc-black", { "editor.background": "#ffffff" }),
  ...[["dark", "#282828"], ["light", "#ffffff"]].map(([type, canvas]) =>
    t("Gruvbox", type, { "editor.background": canvas, "editorError.foreground": "#fb4934", "editorWarning.foreground": "#b57614" })),
  t("Hidden squiggles", "dark", { "editor.background": "#101010", "editorError.foreground": "#ff000000" }),
  t("Split", "dark", { "editor.background": "#101010", "editor.foreground": "#f5f5f5", "sideBar.background": "#fafafa", "terminal.background": "#fbfbfb" }),
  t("Vibrant", "dark", {
    "editor.background": "color(display-p3 0.039216 0.039216 0.039216)", "editor.foreground": "color(display-p3 0.980392 0.980392 0.980392)",
    focusBorder: "color(display-p3 0.308664 0.645271 1.000000)", "editor.selectionBackground": "color(display-p3 0.308664 0.645271 1.000000 / 0.300000)",
  }),
  { name: "No background", colors: { "editor.foreground": "#fff" } },
  { colors: {} },
  "not an object", [], null,
];
// Randomised workbench themes.
const KEYS = [
  "editor.background", "editorPane.background", "editor.foreground", "foreground", "sideBar.background", "activityBar.background",
  "sideBar.foreground", "sideBar.border", "focusBorder", "button.background", "button.foreground", "textLink.foreground",
  "activityBarBadge.background", "progressBar.background", "badge.background", "input.border", "input.background",
  "input.placeholderForeground", "terminal.background", "panel.background", "terminal.foreground", "list.hoverBackground",
  "list.activeSelectionBackground", "list.inactiveSelectionBackground", "editorWidget.background", "dropdown.background",
  "menu.background", "quickInput.background", "panel.border", "editorGroup.border", "contrastBorder", "descriptionForeground",
  "disabledForeground", "editorError.foreground", "errorForeground", "editorWarning.foreground", "textCodeBlock.background",
  "terminalCursor.foreground", "editorCursor.foreground", "terminal.selectionBackground", "editor.selectionBackground",
  "scrollbarSlider.background", "editor.findMatchBackground", "editor.findMatchForeground",
  "editor.findMatchHighlightBackground", "editor.findMatchHighlightForeground",
];
const hexByte = () => Math.floor(rand() * 256).toString(16).padStart(2, "0");
const randomColor = () => {
  const roll = rand();
  if (roll < 0.5) return `#${hexByte()}${hexByte()}${hexByte()}`;
  if (roll < 0.7) return `#${hexByte()}${hexByte()}${hexByte()}${hexByte()}`;
  if (roll < 0.76) return `#${Math.floor(rand() * 4096).toString(16).padStart(3, "0")}`;
  if (roll < 0.82) return `${hexByte()}${hexByte()}${hexByte()}`;
  if (roll < 0.9) return `color(display-p3 ${rand().toFixed(4)} ${rand().toFixed(4)} ${rand().toFixed(4)}${rand() < 0.5 ? ` / ${rand().toFixed(2)}` : ""})`;
  if (roll < 0.94) return `color(srgb ${rand().toFixed(3)} ${rand().toFixed(3)} ${rand().toFixed(3)})`;
  return pick(["nope", "", 12, null, "#12", "rgb(1 2 3)", "color(rec2020 1 1 1)"]);
};
for (let i = 0; i < 160; i++) {
  const colors = {};
  const count = 1 + Math.floor(rand() * 18);
  colors["editor.background"] = rand() < 0.93 ? randomColor() : undefined;
  for (let k = 0; k < count; k++) colors[pick(KEYS)] = randomColor();
  const theme = { name: pick(["dracula", "Solarized Light", "github-dark", "my.theme_x", "  spaced  ", "Dark+", "Light+", "x".repeat(60)]), colors };
  if (rand() < 0.7) theme.type = pick(["dark", "light", "hc-black", "hc-light", "DARK", "weird"]);
  if (rand() < 0.2) theme.displayName = pick(["Fancy Name", "a-b", ""]);
  files.push(theme);
}
const vscodeFiles = files.map((input) => ({
  input: input === undefined ? null : JSON.parse(JSON.stringify(input)),
  isVsCode: vscode.isVsCodeThemeFile(input),
  parsed: attempt(() => vscode.parseVsCodeThemeFile(input)),
}));

const texts = [
  JSON.stringify(VSCODE_DARK),
  '{ // line comment\n "name": "Test", "colors": { "editor.background": "#002b36" } }',
  '{ /* block comment */ "name": "Test", "colors": { "editor.background": "#002b36" } }',
  '{ "name": "Test", "colors": { "editor.background": "#002b36", }, "tokenColors": [ { "scope": ["comment",], "settings": {}, }, ], }',
  "", '{ "name": "Test", "colors": { "editor.background": "#002b36" }', '{ "name": "Test" "colors": { "editor.background": "#002b36" } }',
  '{ "name": "Test", "colors": { "editor.background": "#002b36" } } /* unterminated', '{ "name": "Test", "colors": { "editor.background": "#002b36" } } extra',
  JSON.stringify({ version: 1, name: "Native theme", appearance: "dark", colors: { canvas: "#002b36" } }),
  '{ /* native theme */ "version": 1, "name": "Native theme", "appearance": "dark", "colors": { "canvas": "#002b36" } }',
  '{ "version": 1, "name": "Native theme", "appearance": "dark", "colors": { "canvas": "#002b36" },}',
  '{ /* not a theme */ "colors": { "editor.foreground": "#fff" } }',
  '{ "version": 1, "name": "Bad native", "appearance": "dark", "colors": { "unknown": "#fff" } }',
  "[1, 2", "null", '"string"', "{}", '{"tokenColors": []}',
];
const importTexts = texts.map((source) => ({ source, parsed: attempt(() => vscode.parseThemeImport(source)) }));

// pairing and collisions
const make = (name, type, extra = {}) =>
  vscode.parseVsCodeThemeFile({ name, type, colors: { "editor.background": type === "dark" ? "#101014" : "#fdfdfd", "editor.foreground": type === "dark" ? "#e6e6e6" : "#1f1f1f", focusBorder: "#69b1ff", ...extra } });
const pairingSets = [
  [make("Light+", "light"), make("Dark+", "dark")],
  [make("github-dark", "dark"), make("github-light", "light"), make("github-dark-colorblind", "dark"), make("github-light-colorblind", "light"), make("github-dark-dimmed", "dark")],
  [make("solar-dark", "dark"), make("solar-dark-soft", "dark"), make("solar-light", "light")],
  [make("Night Light", "light"), make("Night Dark", "dark"), make("Plain", "dark"), make("DARKEST", "dark")],
  [make("Aurora Dark", "dark"), make("Aurora Light", "light"), make("Aurora Dark", "dark", { "editor.background": "#111118" })],
  [make("lightning", "light"), make("darkness", "dark")],
];
const pairing = pairingSets.map((themes) => ({
  themes,
  plain: vscode.pairVsCodeThemes(themes),
  withId: vscode.pairVsCodeThemes(themes, { pairedId: (light, dark) => `paired-${light.id}-${dark.id}` }),
}));
const dracula = (bg) => vscode.parseVsCodeThemeFile({ name: "Dracula", type: "dark", colors: { "editor.background": bg, "editor.foreground": "#f8f8f2" } });
const collisionSets = [
  [{ theme: dracula("#282a36"), sourceName: "dracula.json" }, { theme: dracula("#22232e"), sourceName: "dracula-soft.json" }],
  [{ theme: dracula("#282a36") }, { theme: dracula("#22232e") }],
  [{ theme: dracula("#282a36"), sourceName: "dracula.json" }, { theme: dracula("#22232e"), sourceName: "dracula.json" }, { theme: dracula("#111111") }],
  [{ theme: dracula("#282a36"), sourceName: "Dracula" }, { theme: dracula("#22232e"), sourceName: "my_theme.v2.json" }],
];
const collisions = collisionSets.map((entries) => ({
  entries: entries.map(({ theme, sourceName }) => ({ theme, sourceName: sourceName ?? null })),
  out: vscode.resolveThemeLabelCollisions(entries),
}));
const humanize = ["github-dark", "Already Spaced", "plain", "a_b.c", "--x--", "", "  ", "Ünï-code", "dracula.soft"].map((raw) => ({ raw, out: vscode.humanizeThemeName(raw) }));

// --- Open VSX imports through a stubbed fetch
const ASSET_ROOT = "https://open-vsx.org/api/demo/theme/1.0.0/file";
const extension = {
  collectionId: "open-vsx:demo.theme", id: "demo.theme", name: "Demo Theme", publisher: "demo", description: "", downloadCount: 1,
  iconUrl: null, sourceUrl: null, license: "MIT", manifestUrl: `${ASSET_ROOT}/package.json`,
  sha256Url: `${ASSET_ROOT}/demo.theme-1.0.0.sha256`, version: "1.0.0", vsixUrl: `${ASSET_ROOT}/demo.theme-1.0.0.vsix`,
};
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function buildPackage(manifest, { deflate = false, junk = 20, extraFiles = {} } = {}) {
  const zip = new JSZip();
  zip.file("extension/.gitkeep", "");
  for (let i = 0; i < junk; i++) zip.file(`extension/node_modules/package-${i}.js`, "");
  zip.file("extension/themes/base.jsonc", `{
    // inherited workbench colors
    "colors": {
      "editor.foreground": "#eeeeee", "focusBorder": "#8b5cf6", "editor.findMatchBackground": "#663399",
      "editor.findMatchForeground": "#ffffff", "editor.findMatchHighlightBackground": "#445566",
      "editor.findMatchHighlightForeground": "#ffffff", "ignored.key": "#123456",
    },
  }`);
  zip.file("extension/themes/demo-dark.json", `{ "include": "./base.jsonc", "colors": { "editor.background": "#111111", "editor.findMatchHighlightBackground": "#ffffff20" } }`);
  zip.file("extension/themes/demo-light.json", `{ "colors": { "editor.background": "#fafafa", "editor.foreground": "#222222", "focusBorder": "#8b5cf6", "editor.findMatchBackground": "#335577", "editor.findMatchForeground": "#ffffff", "editor.findMatchHighlightBackground": "#bbddff", "editor.findMatchHighlightForeground": "#112233" } }`);
  zip.file("extension/themes/demo.json", `{ "colors": { "editor.background": "#181818", "editor.foreground": "#eeeeee" } }`);
  zip.file("extension/themes/cycle-a.json", `{ "include": "./cycle-b.json", "colors": { "editor.background": "#101010" } }`);
  zip.file("extension/themes/cycle-b.json", `{ "include": "./cycle-a.json", "colors": {} }`);
  zip.file("extension/themes/escape.json", `{ "include": "../../../outside.json", "colors": { "editor.background": "#101010" } }`);
  zip.file("extension/themes/notatheme.json", `{ "colors": {} }`);
  for (const [name, text] of Object.entries(extraFiles)) zip.file(name, text);
  zip.file("extension/package.json", JSON.stringify(manifest));
  const bytes = await zip.generateAsync({
    type: "uint8array",
    comment: `PK\u0005\u0006${"x".repeat(26)}`,
    ...(deflate ? { compression: "DEFLATE" } : {}),
  });
  return bytes;
}

const baseManifest = () => ({
  publisher: "demo", name: "theme", version: "1.0.0", license: "MIT",
  contributes: {
    themes: [
      { label: "Demo Dark", uiTheme: "vs-dark", path: "./themes/demo-dark.json" },
      { label: "Demo Light", uiTheme: "vs", path: "./themes/demo-light.json" },
      { label: "Demo", uiTheme: "vs-dark", path: "./themes/demo.json" },
    ],
  },
});
const scenarios = [];
async function scenario(name, mutate, options = {}) {
  const manifest = baseManifest();
  mutate?.(manifest);
  const packageBytes = options.bytes ?? (await buildPackage(manifest, options));
  const checksum = options.checksum ?? sha(packageBytes);
  const advertised = options.advertised ?? { contributes: { themes: [{ label: "Wrong", path: "./themes/missing.json" }] } };
  globalThis.fetch = async (input) => {
    const url = String(input);
    if (url === extension.manifestUrl) return new Response(JSON.stringify(advertised), { status: 200 });
    if (url === extension.sha256Url) return new Response(`${checksum}  demo.vsix`);
    return new Response(packageBytes, { status: 200, headers: { "Content-Length": String(packageBytes.byteLength) } });
  };
  const result = await openVsx.importOpenVsxThemeExtension(extension).then(
    (themes) => ({ ok: themes }),
    (error) => ({ error: error instanceof Error ? error.message : String(error) }),
  );
  scenarios.push({
    name, extension,
    package: Buffer.from(packageBytes).toString("base64"), checksum, advertised, result,
  });
}
await scenario("ok");
await scenario("ok-deflate", null, { deflate: true });
await scenario("renamed", (m) => m.contributes.themes.forEach((th, i) => (th.label = ["Renamed Dark", "Renamed Light", "Renamed Solo"][i])));
await scenario("duplicate-path", (m) => m.contributes.themes.push({ label: "Duplicate Demo", uiTheme: "vs-dark", path: "./themes/demo.json" }));
await scenario("missing-path", (m) => (m.contributes.themes[2].path = "./themes/missing.json"));
await scenario("license-mismatch", (m) => (m.license = "Proprietary"));
await scenario("license-missing", (m) => delete m.license);
await scenario("wrong-publisher", (m) => (m.publisher = "other"));
await scenario("wrong-version", (m) => (m.version = "2.0.0"));
await scenario("include-cycle", (m) => m.contributes.themes.push({ label: "Cycle", uiTheme: "vs-dark", path: "./themes/cycle-a.json" }));
await scenario("include-escape", (m) => m.contributes.themes.push({ label: "Escape", uiTheme: "vs-dark", path: "./themes/escape.json" }));
await scenario("not-a-theme", (m) => m.contributes.themes.push({ label: "Empty", uiTheme: "vs-dark", path: "./themes/notatheme.json" }));
await scenario("unsafe-path", (m) => (m.contributes.themes[0].path = "../../../etc/passwd"));
await scenario("absolute-path", (m) => (m.contributes.themes[0].path = "/etc/passwd"));
await scenario("no-path", (m) => delete m.contributes.themes[0].path);
await scenario("no-themes", (m) => (m.contributes.themes = []));
await scenario("hc-and-unknown-ui-theme", (m) => {
  m.contributes.themes[0].uiTheme = "hc-black";
  m.contributes.themes[1].uiTheme = "weird";
  delete m.contributes.themes[2].label;
});
await scenario("checksum-mismatch", null, { checksum: "0".repeat(64) });
await scenario("checksum-invalid", null, { checksum: "zzzz" });
await scenario("not-a-zip", null, { bytes: new Uint8Array([1, 2, 3]), advertised: { contributes: { themes: [{ path: "./theme.json" }] } } });

writeFileSync(
  join(crateDir, "fixtures/import-oracle.json"),
  `${JSON.stringify({ vscodeFiles, importTexts, pairing, collisions, humanize, scenarios })}\n`,
);
console.log({ vscodeFiles: vscodeFiles.length, importTexts: importTexts.length, pairing: pairing.length, scenarios: scenarios.length });
console.log(scenarios.map((s) => `${s.name}: ${s.result.error ?? `${s.result.ok.length} themes`}`).join("\n"));
