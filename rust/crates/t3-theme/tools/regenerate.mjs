// Regenerates every fixture. Needs the monorepo checkout with dependencies installed and Chrome.
//   node tools/regenerate.mjs          rewrite all fixtures
//   node tools/regenerate.mjs --check  fail if a derived fixture (not an oracle) is stale
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { crateDir } from "./lib.mjs";

const check = process.argv.includes("--check");
const run = (script, ts = false) => {
  const args = [...(ts ? ["--import", "./tools/ts-loader.mjs"] : []), join("tools", script), ...(check ? ["--check"] : [])];
  const result = spawnSync(process.execPath, args, { cwd: crateDir, stdio: "inherit", env: { ...process.env, NODE_NO_WARNINGS: "1" } });
  if (result.status !== 0) process.exit(result.status ?? 1);
};
// Derived fixtures.
run("extract-palettes.mjs", true);
run("extract-tokens.mjs", true);
run("extract-syntax.mjs", true);
run("extract-misc.mjs", true);
run("extract-theme-css.mjs", true);
run("sources.mjs");
if (!check) {
  // Oracles record what real implementations compute; they are not checked, only rewritten.
  run("color-oracle.mjs");
  run("palette-oracle.mjs", true);
  run("import-oracle.mjs", true);
  run("chrome-cascade.mjs", true);
}
