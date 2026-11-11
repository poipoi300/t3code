import { describe, expect, it } from "vite-plus/test";

import { duplicateLines, type LineDirection, moveLines, type TextLineEdit } from "./lineEditing";

/** `[` and `]` mark the selection; `|` marks a caret. */
function parse(marked: string): { text: string; start: number; end: number } {
  const caret = marked.indexOf("|");
  if (caret !== -1) return { text: marked.replace("|", ""), start: caret, end: caret };
  const start = marked.indexOf("[");
  const end = marked.indexOf("]") - 1;
  return { text: marked.replace("[", "").replace("]", ""), start, end };
}

function apply(text: string, edit: TextLineEdit | null): string | null {
  if (!edit) return null;
  const next = text.slice(0, edit.from) + edit.insert + text.slice(edit.to);
  if (edit.selectionStart === edit.selectionEnd) {
    return `${next.slice(0, edit.selectionStart)}|${next.slice(edit.selectionStart)}`;
  }
  return `${next.slice(0, edit.selectionStart)}[${next.slice(edit.selectionStart, edit.selectionEnd)}]${next.slice(edit.selectionEnd)}`;
}

describe("moveLines", () => {
  it.each<[string, LineDirection, string, string | null]>([
    ["caret line up", "up", "a\nb|b\nc", "b|b\na\nc"],
    ["caret line down", "down", "a\nb|b\nc", "a\nc\nb|b"],
    ["first line up is a no-op", "up", "a|a\nb", null],
    ["last line down is a no-op", "down", "a\nb|b", null],
    ["every touched line", "up", "a\nb[b\nc]c\nd", "b[b\nc]c\na\nd"],
    ["a selection ending at a line start leaves that line", "down", "[a\n]b\nc", "b\n[a\n]c"],
    ["a selection ending at the last line's start stays in the text", "down", "[a\n]b", "b\n[a]"],
    ["past an empty line", "up", "\nb|", "b|\n"],
    ["into an empty last line", "down", "a|\n", "\na|"],
  ])("%s", (_name, direction, marked, expected) => {
    const { text, start, end } = parse(marked);
    expect(apply(text, moveLines(text, start, end, direction))).toBe(expected);
  });
});

describe("duplicateLines", () => {
  it.each<[string, LineDirection, boolean, string, string]>([
    ["down onto the copy", "down", true, "a\nb|b\nc", "a\nbb\nb|b\nc"],
    ["down, staying on the original", "down", false, "a\nb|b\nc", "a\nb|b\nbb\nc"],
    ["up onto the copy", "up", true, "a\nb|b\nc", "a\nb|b\nbb\nc"],
    ["up, staying on the original", "up", false, "a\nb|b\nc", "a\nbb\nb|b\nc"],
    ["every touched line", "down", true, "[a\nb]\nc", "a\nb\n[a\nb]\nc"],
    ["an empty line", "down", true, "|", "\n|"],
  ])("%s", (_name, direction, followCopy, marked, expected) => {
    const { text, start, end } = parse(marked);
    expect(apply(text, duplicateLines(text, start, end, direction, followCopy))).toBe(expected);
  });
});
