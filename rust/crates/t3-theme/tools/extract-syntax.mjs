// Emits fixtures/syntax.json: the Highlights ("zed") palettes the app highlights code with
// (pierre-light, pierre-dark and the darkened t3-light derived from pierre-light), resolved
// by the real @pierre/diffs and apps/web/src/lib/readableSyntaxTheme.ts.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { crateDir, repoRoot } from "./lib.mjs";

const diffs = await import(pathToFileURL(join(repoRoot, "apps/web/node_modules/@pierre/diffs/dist/index.js")).href);
await import(pathToFileURL(join(repoRoot, "apps/web/src/lib/readableSyntaxTheme.ts")).href);
const readable = await import(pathToFileURL(join(repoRoot, "apps/web/src/lib/readableSyntaxTheme.ts")).href);
const diffRendering = await import(pathToFileURL(join(repoRoot, "apps/web/src/lib/diffRendering.ts")).href);

const themes = {};
for (const name of ["pierre-light", "pierre-dark", "t3-light"]) {
  const resolved = await diffs.getResolvedOrResolveTheme(name, "highlights");
  themes[name] = resolved.zed;
}
const readableCases = [];
for (const [scope, style] of Object.entries(themes["pierre-light"].style.syntax)) {
  const color = typeof style === "string" ? style : style.color;
  if (color) readableCases.push({ scope, input: color, output: readable.readableSyntaxColor(color) });
}
for (const color of ["#ffffff", "#000000", "#1ca1c7", "#636363", "#1a85d4", "#abcdef80", "#12345678", "nope", "#fff"]) {
  readableCases.push({ scope: null, input: color, output: readable.readableSyntaxColor(color) });
}
const text = `${JSON.stringify({ diffThemeNames: diffRendering.DIFF_THEME_NAMES, themes, readableCases })}\n`;
const target = join(crateDir, "fixtures/syntax.json");
if (process.argv.includes("--check")) {
  process.exit(existsSync(target) && readFileSync(target, "utf8") === text ? 0 : 1);
}
writeFileSync(target, text);
console.log(Object.keys(themes), readableCases.length, Object.keys(themes["pierre-light"].style).join(","));
