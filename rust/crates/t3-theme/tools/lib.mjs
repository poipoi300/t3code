// Shared helpers for the extraction and oracle scripts. Run with Node >= 22.
import { pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const crateDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// The monorepo checkout: crate lives at <root>/rust/crates/t3-theme once merged.
export const repoRoot = process.env.T3_REPO_ROOT ?? resolve(crateDir, "../../..");
export const pnpm = (pkg, rel) =>
  pathToFileURL(join(repoRoot, "node_modules/.pnpm", pkg, "node_modules", rel)).href;
export const culori = () => import(pnpm("culori@4.0.2", "culori/bundled/culori.mjs"));
export const chromePath = process.env.CHROME_PATH ?? "C:/Program Files/Google/Chrome/Application/chrome.exe";

export async function withChrome(run) {
  const { chromium } = await import(pnpm("playwright-core@1.64.1", "playwright-core/index.mjs"));
  const browser = await chromium.launch({ executablePath: chromePath, headless: true });
  try {
    return await run(await browser.newPage());
  } finally {
    await browser.close();
  }
}

export function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
