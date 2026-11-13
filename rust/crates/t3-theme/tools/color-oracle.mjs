// Writes fixtures/color-corpus.json: colour strings with the values culori computes
// and the 8-bit pixels Chrome paints. Rust tests compare against it.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { crateDir, culori, rng, withChrome } from "./lib.mjs";

const { parse, converter, formatHex8 } = await culori();
const toOklch = converter("oklch");
const toOklab = converter("oklab");
const toRgb = converter("rgb");
const toLab = converter("lab");
const rand = rng(29747);
const pick = (a) => a[Math.floor(rand() * a.length)];
const fix = (v, n) => Number(v.toFixed(n));

const css = [];
for (let i = 0; i < 500; i++) css.push(`oklch(${fix(rand(), 6)} ${fix(rand() * 0.4, 6)} ${fix(rand() * 360, 3)})`);
for (let i = 0; i < 120; i++) css.push(`oklch(${fix(rand(), 6)} ${fix(rand() * 0.5, 6)} ${fix(rand() * 360, 3)} / ${fix(rand(), 3)})`);
for (let i = 0; i < 100; i++) css.push(`oklab(${fix(rand(), 5)} ${fix(rand() * 0.8 - 0.4, 5)} ${fix(rand() * 0.8 - 0.4, 5)})`);
for (let i = 0; i < 80; i++) css.push(`lab(${fix(rand() * 100, 3)} ${fix(rand() * 250 - 125, 3)} ${fix(rand() * 250 - 125, 3)})`);
for (let i = 0; i < 80; i++) css.push(`lch(${fix(rand() * 100, 3)} ${fix(rand() * 150, 3)} ${fix(rand() * 360, 2)})`);
for (const space of ["display-p3", "a98-rgb", "prophoto-rgb", "rec2020", "srgb", "srgb-linear", "xyz-d65", "xyz-d50"]) {
  for (let i = 0; i < 30; i++) css.push(`color(${space} ${fix(rand(), 5)} ${fix(rand(), 5)} ${fix(rand(), 5)})`);
}
for (let i = 0; i < 60; i++) css.push(`hsl(${Math.floor(rand() * 360)} ${Math.floor(rand() * 100)}% ${Math.floor(rand() * 100)}%)`);
for (let i = 0; i < 60; i++) css.push(`hwb(${Math.floor(rand() * 360)} ${Math.floor(rand() * 100)}% ${Math.floor(rand() * 100)}%)`);
for (let i = 0; i < 60; i++) css.push(`rgb(${Math.floor(rand() * 256)} ${Math.floor(rand() * 256)} ${Math.floor(rand() * 256)} / ${fix(rand(), 2)})`);
for (let i = 0; i < 40; i++) css.push(`rgba(${Math.floor(rand() * 256)}, ${Math.floor(rand() * 256)}, ${Math.floor(rand() * 256)}, ${fix(rand(), 2)})`);
for (let i = 0; i < 60; i++) css.push(`#${Math.floor(rand() * 0xffffff).toString(16).padStart(6, "0")}`);
for (let i = 0; i < 20; i++) css.push(`#${Math.floor(rand() * 0xffffffff).toString(16).padStart(8, "0")}`);
const namedSource = await import("node:fs").then((fs) => fs.readFileSync(join(crateDir, "src/color/named.rs"), "utf8"));
for (const m of namedSource.matchAll(/\("([a-z]+)", 0x/g)) css.push(m[1]);
css.push("transparent", "#fff", "#abc", "#abcd", "oklch(0.7 0.4 150)", "oklch(1 0.3 20)", "oklch(0 0.3 20)", "oklch(0.5 0 0)", "hsl(120deg 50% 50%)", "hsl(0.5turn 50% 50%)", "rgb(100% 50% 0%)", "rgb(10%, 20%, 30%)");

const mixSpaces = ["srgb", "oklab", "oklch", "srgb-linear"];
const base = css.filter((c) => !c.includes("/") || rand() < 0.2).slice(0, 400);
const mixes = [];
for (let i = 0; i < 400; i++) {
  const a = pick(base);
  const b = rand() < 0.2 ? "transparent" : pick(base);
  const s = pick(mixSpaces);
  const pa = rand() < 0.15 ? "" : ` ${Math.floor(rand() * 101)}%`;
  const pb = rand() < 0.15 ? "" : ` ${Math.floor(rand() * 101)}%`;
  mixes.push(`color-mix(in ${s}, ${a}${pa}, ${b}${pb})`);
}

const all = [...css, ...mixes];
const painted = await withChrome(async (page) =>
  page.evaluate((list) => {
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    const over = (backdrop, value) => {
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = backdrop;
      ctx.fillRect(0, 0, 1, 1);
      ctx.fillStyle = value;
      ctx.fillRect(0, 0, 1, 1);
      return [...ctx.getImageData(0, 0, 1, 1).data];
    };
    return list.map((value) => {
      ctx.fillStyle = "#123456";
      ctx.fillStyle = value;
      const accepted = ctx.fillStyle !== "#123456" || value === "#123456";
      return { accepted, onBlack: over("#000", value), onWhite: over("#fff", value) };
    });
  }, all),
);

const entries = all.map((value, index) => {
  const entry = { css: value, chrome: painted[index] };
  if (!value.startsWith("color-mix(")) {
    const parsed = parse(value);
    entry.culori = parsed
      ? {
          oklch: [toOklch(parsed).l ?? 0, toOklch(parsed).c ?? 0, toOklch(parsed).h ?? 0],
          oklab: [toOklab(parsed).l, toOklab(parsed).a, toOklab(parsed).b],
          lab: [toLab(parsed).l, toLab(parsed).a, toLab(parsed).b],
          srgb: [toRgb(parsed).r, toRgb(parsed).g, toRgb(parsed).b],
          alpha: parsed.alpha ?? 1,
        }
      : null;
  }
  return entry;
});
writeFileSync(join(crateDir, "fixtures/color-corpus.json"), JSON.stringify({ seed: 29747, chrome: "headless Chrome 157", entries }) + "\n");
console.log(entries.length, "entries;", entries.filter((e) => !e.chrome.accepted).length, "rejected by Chrome");
