import { type Node as ProseMirrorNode, Schema } from "@tiptap/pm/model";
import { AllSelection, EditorState, TextSelection } from "@tiptap/pm/state";
import { describe, expect, it } from "vite-plus/test";

import { duplicateLinesTransaction, moveLinesTransaction } from "./proseMirrorLineEditing";

const schema = new Schema({
  nodes: {
    doc: { content: "block+" },
    paragraph: { group: "block", content: "text*" },
    bulletList: { group: "block list", content: "listItem+" },
    listItem: { content: "paragraph block*", defining: true },
    codeBlock: { group: "block", content: "text*", code: true, marks: "" },
    text: {},
  },
});

const { paragraph: p, bulletList: ul, listItem: li, codeBlock: code } = schema.nodes;

function text(value: string) {
  return value.length === 0 ? [] : [schema.text(value)];
}

/** `|` marks a caret; `[` and `]` mark a selection's anchor and head. */
function stateWithCaret(doc: ProseMirrorNode): EditorState {
  const markers: Array<{ char: string; pos: number }> = [];
  doc.descendants((node, pos) => {
    if (!node.isText) return;
    [...node.text!].forEach((char, offset) => {
      if ("|[]".includes(char)) markers.push({ char, pos: pos + offset });
    });
  });
  const tr = EditorState.create({ doc }).tr;
  for (const marker of markers.toReversed()) tr.delete(marker.pos, marker.pos + 1);
  // Each earlier marker's removal shifts later positions back by one.
  const at = (char: string) => {
    const index = markers.findIndex((marker) => marker.char === char);
    return markers[index]!.pos - index;
  };
  const caret = markers.some((marker) => marker.char === "|");
  const selection = TextSelection.create(tr.doc, at(caret ? "|" : "["), at(caret ? "|" : "]"));
  return EditorState.create({ doc: tr.doc, selection });
}

/** The document as text, one block per line, with `|` at the caret. */
function render(state: EditorState): string {
  const caret = state.selection.head;
  const lines: string[] = [];
  state.doc.descendants((node, pos, parent) => {
    if (!node.isTextblock) return true;
    const indent = parent?.type === li ? "- " : "";
    const value = node.textContent;
    const offset = caret - pos - 1;
    const marked =
      offset >= 0 && offset <= value.length
        ? `${value.slice(0, offset)}|${value.slice(offset)}`
        : value;
    lines.push(node.type === code ? `\`${marked}\`` : `${indent}${marked}`);
    return false;
  });
  return lines.join("\n");
}

function after(state: EditorState, tr: ReturnType<typeof moveLinesTransaction>): string | null {
  return tr ? render(state.apply(tr)) : null;
}

describe("moveLinesTransaction", () => {
  it("swaps a paragraph with its neighbor and keeps the caret in it", () => {
    const state = stateWithCaret(
      schema.node("doc", null, [p.create(null, text("a")), p.create(null, text("b|"))]),
    );
    expect(after(state, moveLinesTransaction(state, "up"))).toBe("b|\na");
    expect(moveLinesTransaction(state, "down")).toBeNull();
  });

  it("moves a list item with its nested items, not its first paragraph alone", () => {
    const state = stateWithCaret(
      schema.node("doc", null, [
        ul.create(null, [
          li.create(null, [p.create(null, text("one"))]),
          li.create(null, [
            p.create(null, text("tw|o")),
            ul.create(null, [li.create(null, [p.create(null, text("nested"))])]),
          ]),
        ]),
      ]),
    );
    expect(after(state, moveLinesTransaction(state, "up"))).toBe("- tw|o\n- nested\n- one");
  });

  it("leaves out a block the selection only touches at its start", () => {
    const state = stateWithCaret(
      schema.node("doc", null, [
        p.create(null, text("[a")),
        p.create(null, text("]b")),
        p.create(null, text("c")),
      ]),
    );
    expect(after(state, moveLinesTransaction(state, "down"))).toBe("b\na\n|c");
  });

  it("moves lines inside a code block, keeping the selection inside it", () => {
    const state = stateWithCaret(schema.node("doc", null, [code.create(null, text("a\nb|\nc"))]));
    expect(after(state, moveLinesTransaction(state, "down"))).toBe("`a\nc\nb|`");
    const lastLine = stateWithCaret(schema.node("doc", null, [code.create(null, text("[a\n]b"))]));
    expect(after(lastLine, moveLinesTransaction(lastLine, "down"))).toBe("`b\na|`");
  });

  it("refuses a move the parent's content rules do not allow", () => {
    const state = stateWithCaret(
      schema.node("doc", null, [
        ul.create(null, [
          li.create(null, [
            p.create(null, text("x")),
            ul.create(null, [li.create(null, [p.create(null, text("on[e"))])]),
            ul.create(null, [li.create(null, [p.create(null, text("t]wo"))])]),
          ]),
        ]),
      ]),
    );
    expect(moveLinesTransaction(state, "up")).toBeNull();
  });
});

describe("duplicateLinesTransaction", () => {
  const state = stateWithCaret(
    schema.node("doc", null, [p.create(null, text("a|")), p.create(null, text("b"))]),
  );

  it.each([
    ["down", true, "a\na|\nb"],
    ["down", false, "a|\na\nb"],
    ["up", true, "a|\na\nb"],
    ["up", false, "a\na|\nb"],
  ] as const)("duplicates %s with followCopy=%s", (direction, followCopy, expected) => {
    expect(after(state, duplicateLinesTransaction(state, direction, followCopy))).toBe(expected);
  });

  it("keeps a select-all selection over the whole duplicated document", () => {
    const all = state.apply(state.tr.setSelection(new AllSelection(state.doc)));
    const next = all.apply(duplicateLinesTransaction(all, "down", true)!);
    expect(next.doc.textBetween(0, next.doc.content.size, "\n")).toBe("a\nb\na\nb");
    expect(next.selection).toBeInstanceOf(AllSelection);
  });

  it("duplicates a code block line, not the whole block", () => {
    const codeState = stateWithCaret(schema.node("doc", null, [code.create(null, text("x|\ny"))]));
    expect(after(codeState, duplicateLinesTransaction(codeState, "down", true))).toBe("`x\nx|\ny`");
  });
});
