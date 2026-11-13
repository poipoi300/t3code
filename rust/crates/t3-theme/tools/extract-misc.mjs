// Emits fixtures/misc.json: project icon colours, the theme picker's preview rendering spec
// (with what mixThemePreviewBase returns) and the file tree's theme CSS.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, repoRoot, rng } from "./lib.mjs";
import { parseValue } from "./css-value.mjs";

const src = (rel) => import(pathToFileURL(join(repoRoot, rel)).href);
const icons = await src("apps/web/src/projectIconColors.ts");
const preview = await src("packages/shared/src/themePreview.ts");
const tree = await src("apps/web/src/pierre-tree-theme.ts");

const rand = rng(77);
const hex = () => `#${Math.floor(rand() * 0xffffff).toString(16).padStart(6, "0")}`;
const previewCases = [];
for (const mode of ["light", "dark"]) {
  for (const colors of Object.values(preview.STANDARD_THEME_PREVIEW_COLORS)) {
    previewCases.push({ colors, mode, out: preview.mixThemePreviewBase(colors, mode) });
  }
  for (let i = 0; i < 40; i++) {
    const colors = {
      canvas: i % 2 ? hex() : `oklch(${rand().toFixed(3)} ${(rand() * 0.3).toFixed(3)} ${Math.floor(rand() * 360)})`,
      accent: hex(),
      messageAction: hex(),
    };
    previewCases.push({ colors, mode, out: preview.mixThemePreviewBase(colors, mode) });
  }
  previewCases.push({ colors: { canvas: "not a colour", accent: "#fff", messageAction: "#000" }, mode, out: preview.mixThemePreviewBase({ canvas: "not a colour", accent: "#fff", messageAction: "#000" }, mode) });
}

const treeVars = [];
const hostBody = /:host\s*\{([\s\S]*?)\}/.exec(tree.PIERRE_TREE_UNSAFE_CSS)?.[1] ?? "";
for (const m of hostBody.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) treeVars.push({ name: m[1], value: parseValue(m[2]) });

const fixture = {
  projectIconColors: icons.PROJECT_ICON_COLORS,
  themePreview: {
    standardColors: preview.STANDARD_THEME_PREVIEW_COLORS,
    renderSpecs: preview.THEME_PREVIEW_RENDER_SPECS,
    cases: previewCases,
  },
  fileTree: {
    unsafeCss: tree.PIERRE_TREE_UNSAFE_CSS,
    hostVariables: treeVars,
    styles: { light: tree.pierreTreeStyle("light"), dark: tree.pierreTreeStyle("dark") },
  },
};
const text = `${JSON.stringify(fixture)}\n`;
const target = join(crateDir, "fixtures/misc.json");
if (process.argv.includes("--check")) process.exit(existsSync(target) && readFileSync(target, "utf8") === text ? 0 : 1);
writeFileSync(target, text);
console.log(fixture.projectIconColors.length, previewCases.length, treeVars.map((v) => v.name).join(" "));
