import { registerHooks } from "node:module";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import assert from "node:assert/strict";

const root = process.cwd();
const url = path => pathToFileURL(resolve(root, path)).href;
const terminal = url("packages/client-runtime/src/state/terminalOutput.ts");
const contracts = "t3-terminal-contracts:source";
const cases = [];
const states = new WeakMap();
let nextState = 1;
let test = "";
let tests = 0;
globalThis.__wp6Call = (name, fn, args) => {
  let input = states.get(args[0]);
  if (input === undefined) {
    states.set(args[0], input = nextState++);
    cases.push({ name: "seed", id: input, expected: args[0], test });
  }
  const result = fn(...args);
  if (["appendOutput", "resetOutput", "clearOutput"].includes(name)) {
    let id = states.get(result);
    if (id === undefined) states.set(result, id = nextState++);
    cases.push({ name, input, args: args.slice(1), id, test });
  } else {
    cases.push({ name, input, args: args.slice(1), expected: result, test });
  }
  return result;
};
globalThis.__wp6Test = (name, fn) => { test = name; tests++; fn(); };
globalThis.__wp6Expect = actual => {
  const make = (negated = false) => {
    const check = fn => { if (negated) assert.throws(fn); else fn(); };
    const subset = (a, b) => { for (const key of Object.keys(b)) { if (b[key] && typeof b[key] === "object") subset(a[key], b[key]); else assert.deepEqual(a[key], b[key], test); } };
    return {
      toBe: expected => check(() => assert.equal(actual, expected, test)),
      toEqual: expected => check(() => assert.deepEqual(actual, expected, test)),
      toMatchObject: expected => check(() => subset(actual, expected)),
      toMatch: expected => check(() => assert.match(actual, expected, test)),
      toBeLessThanOrEqual: expected => check(() => assert.ok(actual <= expected, test)),
      get not() { return make(!negated); },
    };
  };
  return make();
};
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "vite-plus/test") return { url: "data:text/javascript," + encodeURIComponent("export const expect=globalThis.__wp6Expect;export const it=globalThis.__wp6Test;export function describe(name,fn){fn()}"), shortCircuit: true };
    if (specifier === "@t3tools/contracts") return { url: contracts, shortCircuit: true };
    return next(specifier, context);
  },
  load(target, context, next) {
    if (target === contracts) {
      const source = readFileSync(resolve(root, "packages/contracts/src/terminal.ts"), "utf8");
      return { format: "module-typescript", shortCircuit: true, source: source.slice(source.indexOf("export const DEFAULT_TERMINAL_ID"), source.indexOf("const TrimmedNonEmptyStringSchema")) + "\nexport const EnvironmentId={make:x=>x};export const ThreadId={make:x=>x};export const TerminalSessionSnapshot={make:x=>x};" };
    }
    const loaded = next(target, context);
    if (target !== terminal) return loaded;
    let source = readFileSync(resolve(root, "packages/client-runtime/src/state/terminalOutput.ts"), "utf8");
    for (const name of ["appendOutput", "resetOutput", "clearOutput", "terminalOutputText", "terminalOutputReplay", "readTerminalOutputUpdate"]) {
      const exported = source.includes(`export function ${name}(`);
      source = source.replace(`${exported ? "export " : ""}function ${name}(`, `function original_${name}(`);
      source += `\n${exported ? "export " : ""}function ${name}(...args:any[]){return globalThis.__wp6Call('${name}',original_${name},args)}\n`;
    }
    return { ...loaded, source };
  },
});
await import(url("packages/client-runtime/src/state/terminalSession.test.ts"));
const t = await import(terminal);
const histories = [
  "prefix\x1b[31mOK", "prefix\x1b]0;HIDDEN\x07OK", "prefix\x1bPpayload\x1b\\OK",
  "prefix\x1bXpayload\x1aOK", "prefix\x1b[111éOK", "prefix\x1b[111\u009b31mOK",
  "prefix\x1b[111\u009dtitle\x07OK", "prefix\x1b[111🙂mOK", "discard\ufefftail",
];
for (const history of histories) for (const budget of [0, 1, 2, 4, 6, 10, 32]) {
  test = `generated UTF-8/C1 replay ${JSON.stringify(history)} budget ${budget}`;
  let output = t.resetOutput(t.EMPTY_TERMINAL_OUTPUT_STATE, history, budget);
  t.terminalOutputText(output); t.terminalOutputReplay(output);
  output = t.resetOutput(output, "", budget);
  let cursor = t.INITIAL_TERMINAL_OUTPUT_CURSOR;
  for (const chunk of history) {
    output = t.appendOutput(output, chunk, budget);
    cursor = t.readTerminalOutputUpdate(output, cursor).cursor;
    t.readTerminalOutputUpdate(output, t.INITIAL_TERMINAL_OUTPUT_CURSOR);
    t.terminalOutputText(output); t.terminalOutputReplay(output);
  }
  output = t.clearOutput(output, budget);
  t.readTerminalOutputUpdate(output, cursor);
  t.terminalOutputText(output); t.terminalOutputReplay(output);
}
process.stdout.write(JSON.stringify({ tests, cases }));
