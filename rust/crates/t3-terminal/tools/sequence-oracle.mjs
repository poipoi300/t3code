import { readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { terminalSequenceStateAfter, terminalSequenceEnd, retainedTerminalHistory } from "../../../../packages/contracts/src/terminal.ts";

const units = (s) => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i));
const fragments = ["text", "\n", "é", "界", "🙂", "\ufeff", "\u0090", "\u009b", "\u009c", "\u009d", "\u001b[31m", "\u001b[", "\u001b]0;", "\u001bP", "\u001b_", "\u0007", "\u001b\\", "\u0018", "\u001a", "\ud83d", "\ude80"];
let seed = 0x12345678;
const random = () => (seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0);
const states = [];
for (let i = 0; i < 3000; i++) {
  const text = fragments[random() % fragments.length] + fragments[random() % fragments.length];
  const state = i % 7;
  const cut = random() % (text.length + 1);
  const first = terminalSequenceStateAfter(state, text, 0, cut);
  states.push({state, units: units(text), cut, first, after: terminalSequenceStateAfter(first, text, cut), end: terminalSequenceEnd(state, text)});
}
const retained = [];
for (let i = 0; i < 1000; i++) {
  const text = fragments[random() % 19] + fragments[random() % 19] + fragments[random() % 19];
  const bytes = random() % 16;
  retained.push({text, bytes, kept: retainedTerminalHistory(text, bytes)});
}
const bounded = [];
for (const lines of [0, 1, 3, 5000]) for (const bytes of [0, 3, 8, 64, 65536]) {
  let full = "";
  const steps = [];
  for (let i = 0; i < 300; i++) {
    const clear = i % 73 === 0;
    if (clear) full = "";
    const chunk = fragments[random() % 19] + fragments[random() % 19];
    full += chunk;
    const terminated = full.endsWith("\n");
    const parts = full.split("\n");
    if (terminated) parts.pop();
    const capped = parts.slice(Math.max(0, parts.length - lines)).join("\n") + (terminated ? "\n" : "");
    const points = Array.from(capped);
    let start = points.length, size = 0;
    while (start > 0 && size + Buffer.byteLength(points[start - 1]) <= bytes) size += Buffer.byteLength(points[--start]);
    let kept = points.slice(start).join("");
    if (lines > 0 && bytes > 0) kept = kept.slice(terminalSequenceEnd(terminalSequenceStateAfter(0, full, 0, full.length - kept.length), kept));
    steps.push({clear, chunk, kept});
  }
  bounded.push({lines, bytes, steps});
}
const manager = readFileSync(new URL("../../../../apps/server/src/terminal/Manager.ts", import.meta.url), "utf8");
const code = manager.slice(manager.indexOf("function isCsiFinalByte"), manager.indexOf("function legacySafeThreadId"));
const sanitize = new Function(stripTypeScriptTypes(code) + "\nreturn sanitizeTerminalHistoryChunk;")();
const sanitized = [];
for (const kind of ["[0", "[", "]11;?", "P$qm", "Pplain", "_", "^"]) {
  for (const content of ["é", "嚠", "界", "🙂", "\u0090\u009b", "\u009d", "\u0098", "\u009a", "\u001b[6n", "\u0018", "\u001a", "\u0007", "名", "\u007f", "\0"]) {
    const input = "A\u001b" + kind + content + "6nHELLO\u001b\\OK";
    for (let cut = 0; cut <= input.length; cut++) {
      if (cut > 0 && /[\ud800-\udbff]/.test(input[cut-1])) continue;
      const chunks = [input.slice(0, cut), input.slice(cut)];
      let pending = "", kept = "";
      for (const chunk of chunks) {
        const result = sanitize(pending, chunk);
        kept += result.visibleText;
        pending = result.pendingControlSequence;
      }
      sanitized.push({chunks, kept});
    }
  }
}
writeFileSync(new URL("../fixtures/sequences.json", import.meta.url), JSON.stringify({states, retained, bounded, sanitized}) + "\n");
