// Runs the real apps/web/src/themePalette.ts over many inputs and records the results in
// fixtures/palette-oracle.json. Run: node --import ./tools/ts-loader.mjs tools/palette-oracle.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, repoRoot, rng } from "./lib.mjs";

const src = (rel) => import(pathToFileURL(join(repoRoot, rel)).href);
const palette = await src("apps/web/src/themePalette.ts");
const shared = await src("packages/shared/src/themePalettes.ts");

const rand = rng(11);
const pick = (list) => list[Math.floor(rand() * list.length)];
const hex = () => `#${Math.floor(rand() * 0xffffff).toString(16).padStart(6, "0")}`;
const roles = shared.THEME_COLOR_ROLES;
const attempt = (fn) => {
  try {
    return { ok: fn() };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
};

// --- colour strings
const corpus = JSON.parse(readFileSync(join(crateDir, "fixtures/color-corpus.json"), "utf8")).entries
  .map((e) => e.css)
  .filter((css) => !css.startsWith("color-mix("));
const colorStrings = [
  ...corpus,
  "#abcd", "rebeccapurple", "transparent", "oklch(0.5 1e303 0)", "oklch(62% 0.2 280deg / 50%)", "rgb(10 20 30 / none)",
  "rgb(10 20 30 / 50%)", "hsl(350 80% 50%)", "hwb(45 10% 20%)", "lab(60% 40 30)", "lch(60% 50 120)", "oklab(0.6 0.1 -0.1)",
  "color(display-p3 0.8 0.2 0.3)", "not-a-color", "", "  #fff  ", "#12", "var(--x)", "oklch(2 0.1 10)", "oklch(-1 -0.1 -10)",
  "oklch(0.5 0.1 none)", "oklch(none none none)", "rgb(1,2,3)", "rgba(1,2,3,0.5)", "RED", "#FFF", "oklch(0.5 0.2 720)",
];
const colors = colorStrings.map((css) => ({
  css,
  canonical: palette.toCanonicalThemeColor(css),
  hex: palette.themeColorToHex(css),
}));

// --- guided generator
const seeds = [
  ["light", "#f4f9f2", "#1d8a4e"], ["dark", "#101a2c", "#4f8fe8"], ["dark", "#211a23", "#df5398"],
  ["light", "#fdf6ec", "#c2571b"], ["light", "#111827", "#8ab4f8"], ["dark", "#f5ecf5", "#a84370"],
  ["dark", "#000000", "#ffffff"], ["light", "#ffffff", "#000000"], ["dark", "#808080", "#808080"],
  ["light", "garbage", "also garbage"], ["dark", "oklch(0.2 0.02 300)", "oklch(0.7 0.2 20)"],
  ["light", "#777777", "#777778"], ["dark", "#0a0a0a", "#0b0b0b"],
];
for (let i = 0; i < 90; i++) seeds.push([pick(["light", "dark"]), hex(), hex()]);
const vivid = seeds.map(([appearance, canvas, accent]) => ({
  appearance, canvas, accent, colors: palette.createVividThemeColors(appearance, canvas, accent),
}));

// --- single-role edits
const editValues = ["#ffffff", "#000000", "#33669980", "oklch(0.62 0.2 280)", "hsl(350 80% 50%)", "#777777", "rgb(250 245 250 / 40%)", "garbage"];
const edits = [];
for (const appearance of ["light", "dark"]) {
  const bases = [
    { label: "default", colors: palette.getDefaultThemeColors(appearance) },
    { label: "vivid", colors: palette.createVividThemeColors(appearance, appearance === "dark" ? "#14202e" : "#f2f6ee", "#3a7bd5") },
  ];
  for (const base of bases) {
    for (const role of roles) {
      for (const value of editValues) {
        const updated = palette.updateThemeColorFamily(appearance, base.colors, role, value);
        const changed = {};
        for (const key of roles) if (updated[key] !== base.colors[key]) changed[key] = updated[key];
        edits.push({ appearance, base: base.label, role, value, changed });
      }
    }
  }
}

// --- readable colour solve
const readable = [];
for (let i = 0; i < 250; i++) {
  const color = pick([hex(), `oklch(${rand().toFixed(3)} ${(rand() * 0.3).toFixed(3)} ${Math.floor(rand() * 360)})`]);
  const surface = pick([hex(), "#000000", "#ffffff", "#777777"]);
  const min = pick([3, 4.5, 5, 7]);
  readable.push({ color, surface, min, out: palette.readableThemeColorOn(color, surface, min) });
}

// --- context chips
const palettes = [
  ["standard-light", palette.getStandardThemeColors("light")],
  ["standard-dark", palette.getStandardThemeColors("dark")],
  ["painted-light", palette.getStandardPaintedThemeColors("light")],
  ["painted-dark", palette.getStandardPaintedThemeColors("dark")],
  ...shared.BUILT_IN_THEMES.flatMap((theme) =>
    palette.getThemeModes(theme).map((mode) => [`${theme.id}-${mode}`, palette.getThemeColorsForMode(theme, mode)]),
  ),
  ...vivid.slice(0, 40).map((v, i) => [`vivid-${i}`, v.colors]),
];
const light = palette.getStandardThemeColors("light");
for (const [name, surface, text] of [["white-gray", "#ffffff", "#767676"], ["mid-gray", "#777777", "#767676"]]) {
  palettes.push([name, { ...light, canvas: surface, surface, surfaceRaised: surface, messageSurface: surface, text, messageForeground: text }]);
}
palettes.push(["blue-bubble", { ...light, messageSurface: "#1b4ed8", messageForeground: "#ffffff" }]);
const chips = palettes.map(([name, colorsForPalette]) => ({
  name,
  colors: colorsForPalette,
  variables: Object.entries(palette.contextChipVariables(colorsForPalette)),
}));

// --- theme files
const fileInputs = [
  { version: 1, appearance: "dark", colors: { canvas: "#101820" }, name: "夜" },
  { version: 1, appearance: "dark", colors: { canvas: "#101820" }, name: "朝" },
  { version: 1, appearance: "dark", colors: { canvas: "#101820" }, name: " 夜 " },
  { version: 1, name: "Legacy", appearance: "dark", colors: { canvas: "#101010" } },
  {
    version: 1, name: "Search", appearance: "dark",
    colors: { searchMatchBackground: "#224466", searchMatchForeground: "#ffffff", searchMatchActiveBackground: "#aaddff", searchMatchActiveForeground: "#112233" },
    variants: { light: { searchMatchBackground: "#ddeeff", searchMatchForeground: "#112233" } },
  },
  {
    version: 1, name: "Translucent", appearance: "light",
    colors: {
      canvas: "oklch(62% 0.2 280deg / 50%)", accent: "#abcd", focus: "rgb(10 20 30 / 50%)", error: "hsl(350 80% 50%)",
      warning: "hwb(45 10% 20%)", update: "lab(60% 40 30)", messageAction: "lch(60% 50 120)", sidebar: "oklab(0.6 0.1 -0.1)",
      terminalCursor: "color(display-p3 0.8 0.2 0.3)", terminalSelection: "rebeccapurple", terminalScrollbar: "transparent",
      terminalScrollbarHover: "rgb(10 20 30 / none)",
    },
  },
  { version: 1, name: "Extreme chroma", appearance: "light", colors: { accent: "oklch(0.5 1e303 0)" } },
  {
    version: 1, id: "aurora", name: "Aurora", appearance: "light",
    colors: { canvas: "#f8fbff", text: "#10243d" }, variants: { dark: { canvas: "#101827", text: "#eef5ff" } },
  },
  { version: 1, id: "aurora", name: "Aurora", appearance: "light", colors: { canvas: "#f8fbff" }, sidebarArtwork: true },
  { version: 1, id: "managed", name: "Managed", appearance: "dark", colors: { canvas: "#000" }, managed: true, collection: { id: "open-vsx:a.b", label: " A B " } },
  { version: 2, name: "x", appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "", appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "x".repeat(49), appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "x", appearance: "blue", colors: { canvas: "#000" } },
  { version: 1, name: "x", appearance: "dark" },
  { version: 1, name: "x", appearance: "dark", colors: {} },
  { version: 1, name: "x", appearance: "dark", colors: { nope: "#000" } },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "nope" } },
  { version: 1, name: "x", id: "Bad Id", appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "x", id: "grove", appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "Dark", appearance: "dark", colors: { canvas: "#000" } },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "#000" }, collection: { id: "bad id", label: "x" } },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "#000" }, variants: { dark: { canvas: "#111" } } },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "#000" }, variants: { blue: { canvas: "#111" } } },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "#000" }, variants: [] },
  { version: 1, name: "x", appearance: "dark", colors: { canvas: "#000" }, variants: { light: {} } },
  [], null, "text", 5,
];
const files = fileInputs.map((input) => {
  const parsed = attempt(() => palette.parseThemeFile(input));
  const entry = { input, parsed };
  if (parsed.ok) entry.serialized = attempt(() => palette.serializeThemeFile(parsed.ok));
  return entry;
});

const names = ["Aurora Night", "  Spaced   Out  ", "!!!", "", "夜", "朝", "Ünïcode", "a".repeat(80), "x-y_z", "--a--", "Tab\tName", "日本語 theme"];
const ids = names.map((name) => ({ name, id: palette.themeIdFromName(name) }));

// --- preference resolution (built-ins only; the saved library is covered by unit tests)
const themeNames = ["t3-chat", "grove", "ocean", "ember", "iris", "system", "light", "dark", "t3-chat-dark", "t3-grove", "t3-iris", "unknown"];
const halvesList = [null, { dark: "grove" }, { light: "ocean" }, { light: "grove", dark: "iris" }, { dark: "unknown" }];
const resolution = [];
for (const theme of themeNames) {
  for (const systemDark of [true, false]) {
    for (const followSystem of [undefined, true, false]) {
      for (const mode of [undefined, "light", "dark", "system"]) {
        for (const halves of halvesList) {
          resolution.push({
            theme, systemDark, followSystem: followSystem ?? null, mode: mode ?? null, halves,
            appearance: palette.resolveThemeAppearance(theme, systemDark, followSystem, mode, halves),
            desktop: palette.resolveDesktopTheme(theme, followSystem, mode, halves),
          });
        }
      }
    }
  }
}
const preferences = themeNames.map((theme) => ({
  theme,
  known: palette.isKnownThemePreference(theme),
  mode: palette.getThemePreferenceMode(theme),
  canonical: palette.canonicalThemePreference(theme),
  artwork: palette.themeAllowsSidebarArtwork(theme),
}));
const halvesRaw = [null, "", "{}", "not json", "[]", '{"light":"grove"}', '{"dark":"grove","light":"ocean"}', '{"light":5,"dark":"nope"}', '{"dark":"t3-iris"}'];
const halves = halvesRaw.map((raw) => ({ raw, parsed: palette.parseThemeHalves(raw) }));

// --- published environment themes
const published = [
  { id: "seeded", name: "Seeded", appearance: "dark", canvas: "#101820", accent: "#3a7bd5" },
  { id: "seeded-light", name: "Seeded Light", appearance: "light", canvas: "#f4f9f2", accent: "#1d8a4e" },
  { id: "palette", name: "Palette", appearance: "light", colors: { canvas: "#ffffff", accent: "#123456", bogus: "#000" } },
  { id: "mixed", name: "Mixed", appearance: "dark", canvas: "#101820", accent: "#3a7bd5", variants: { light: { canvas: "#fafafa" } } },
  { id: "light-half", name: "Light Half", appearance: "dark", colors: {}, variants: { light: { canvas: "#fafafa" } } },
  { id: "empty", name: "Empty", appearance: "dark", colors: {} },
  { id: "grove", name: "Reserved", appearance: "dark", canvas: "#101820", accent: "#3a7bd5" },
  { id: "dark", name: "Reserved Keyword", appearance: "dark", canvas: "#101820", accent: "#3a7bd5" },
  { id: "overlay", name: "Overlay", appearance: "dark", canvas: "#101820", accent: "#3a7bd5", colors: { text: "#eee" } },
];
const publishedOut = palette.publishedThemeDefinitions(published);

writeFileSync(
  join(crateDir, "fixtures/palette-oracle.json"),
  `${JSON.stringify({
    colors, vivid, edits, readable, chips, files, ids, resolution, preferences, halves, published, publishedOut,
    standard: {
      light: palette.getStandardThemeColors("light"),
      dark: palette.getStandardThemeColors("dark"),
      paintedLight: palette.getStandardPaintedThemeColors("light"),
      paintedDark: palette.getStandardPaintedThemeColors("dark"),
      defaultLight: palette.getDefaultThemeColors("light"),
      defaultDark: palette.getDefaultThemeColors("dark"),
    },
  })}\n`,
);
console.log({ colors: colors.length, vivid: vivid.length, edits: edits.length, readable: readable.length, chips: chips.length, files: files.length, resolution: resolution.length });
