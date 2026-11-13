// Web captures for capture.ps1: the slice at a desktop width and at a phone width, in a headless
// Chromium-family browser. Uses the monorepo's installed playwright-core; installs nothing.
// Usage: node capture-web.mjs <url> <out-dir> <label> <browser-executable>

import { createRequire } from "node:module";
import { readdirSync } from "node:fs";
import { join, resolve } from "node:path";

const [url, out, label, executablePath] = process.argv.slice(2);
const pnpm = resolve(import.meta.dirname, "../../node_modules/.pnpm");
const entry = readdirSync(pnpm).find((name) => name.startsWith("playwright-core@"));
if (!entry) throw new Error(`playwright-core is not installed under ${pnpm}`);
const require = createRequire(join(pnpm, entry, "node_modules/playwright-core/package.json"));
const { chromium } = require("playwright-core");

const browser = await chromium.launch({ executablePath, headless: true });
const capture = async (name, viewport, mobile) => {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: mobile ? 3 : 1,
    isMobile: mobile,
    hasTouch: mobile,
  });
  const page = await context.newPage();
  await page.goto(url);
  const press = (text) => page.getByRole("button", { name: text, exact: true }).click();
  await page.getByText("Connect to T3 Code").waitFor({ timeout: 60_000 });
  await press("Fill in the demo server");
  await press("Connect");
  // The accessible name is the full title; the visible text may be truncated with an ellipsis.
  await page.getByRole("button", { name: "Build the shared Rust UI slice", exact: true }).waitFor({ timeout: 30_000 });
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(out, `${label}-web-${name}-home.png`) });
  await press("Build the shared Rust UI slice");
  await page.getByText("The project has a single math module.").first().waitFor({ timeout: 30_000 });
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(out, `${label}-web-${name}-thread.png`) });
  await context.close();
};

try {
  await capture("desktop", { width: 1280, height: 800 }, false);
  await capture("phone", { width: 390, height: 844 }, true);
} finally {
  await browser.close();
}
console.log(`web captures written to ${out}`);
