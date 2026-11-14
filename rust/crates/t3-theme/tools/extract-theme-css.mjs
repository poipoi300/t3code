import { readFileSync, writeFileSync } from "node:fs";
import { THEME_CSS_HOOKS, THEME_GUIDE } from "../../../../packages/shared/src/themeCss.ts";

const emit = (name, text) => {
  const target = new URL(`../fixtures/${name}`, import.meta.url);
  if (process.argv.includes("--check")) {
    if (readFileSync(target, "utf8") !== text) throw new Error(`${name} is stale`);
  } else writeFileSync(target, text);
};
emit("theme-guide.txt", THEME_GUIDE);
emit("theme-css-hooks.rs",
  "// Derived from packages/shared/src/themeCss.ts.\n[\n" +
  THEME_CSS_HOOKS.map(({ selector, styles }) =>
    `    (${JSON.stringify(selector)}, ${JSON.stringify(styles)}),\n`).join("") + "]\n");
