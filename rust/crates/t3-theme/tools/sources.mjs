// Records a hash of every upstream file the fixtures were generated from, so the Rust test
// `sources_have_not_drifted` fails as soon as one changes without the fixtures being regenerated.
//   node tools/sources.mjs          writes fixtures/sources.json
//   node tools/sources.mjs --check  exits 1 when it is stale
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { crateDir, repoRoot } from "./lib.mjs";

export const SOURCES = [
  "apps/web/src/index.css",
  "apps/web/src/themePalette.ts",
  "apps/web/src/themePalette.test.ts",
  "apps/web/src/themeCss.ts",
  "apps/web/src/themeCss.browser.test.ts",
  "apps/web/src/themeCssHooks.test.ts",
  "apps/web/src/appearanceFonts.ts",
  "apps/web/src/appearanceContrast.ts",
  "apps/web/src/vscodeThemeImport.ts",
  "apps/web/src/vscodeThemeImport.test.ts",
  "apps/web/src/openVsxThemes.ts",
  "apps/web/src/openVsxThemes.test.ts",
  "apps/web/src/panelAnimations.ts",
  "apps/web/src/components/Sidebar.motion.ts",
  "apps/web/src/projectIconColors.ts",
  "apps/web/src/pierre-tree-theme.ts",
  "apps/web/src/lib/readableSyntaxTheme.ts",
  "apps/web/src/lib/diffRendering.ts",
  "apps/web/src/lib/syntaxHighlighting.ts",
  "apps/web/src/lib/contextChipAccents.ts",
  "packages/shared/src/themePalettes.ts",
  "packages/shared/src/themeCss.ts",
  "packages/shared/src/themeCss.test.ts",
  "packages/shared/src/themePreview.ts",
  "packages/contracts/src/settings.ts",
  "apps/mobile/src/lib/mobileTheme.ts",
  "apps/mobile/src/lib/mobileThemeRuntime.ts",
  "apps/mobile/src/lib/mobileThemeVariables.ts",
  "apps/mobile/src/features/terminal/terminalTheme.ts",
  "node_modules/.pnpm/tailwindcss@4.3.3/node_modules/tailwindcss/theme.css",
];

const hash = (path) => createHash("sha256").update(readFileSync(join(repoRoot, path))).digest("hex");
const text = `${JSON.stringify(Object.fromEntries(SOURCES.map((p) => [p, hash(p)])), null, 1)}\n`;
const target = join(crateDir, "fixtures/sources.json");
if (process.argv.includes("--check")) process.exit(existsSync(target) && readFileSync(target, "utf8") === text ? 0 : 1);
writeFileSync(target, text);
console.log("recorded", SOURCES.length, "sources");
