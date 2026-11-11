import { DEFAULT_RESOLVED_KEYBINDINGS } from "@t3tools/shared/keybindings";
import { Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { describe, expect, it } from "vite-plus/test";

import { registerCommands } from "./dispatcher";
import { lineEditingCommands } from "./lineEditingCommands";

function useLineEditing(): DisposableStack {
  const stack = new DisposableStack();
  stack.defer(
    registerCommands(lineEditingCommands, { keybindings: () => DEFAULT_RESOLVED_KEYBINDINGS }),
  );
  return stack;
}

function mount<T extends HTMLElement>(stack: DisposableStack, element: T, parent?: HTMLElement): T {
  const host = parent ?? document.body;
  host.append(element);
  stack.defer(() => element.remove());
  return element;
}

function pressAlt(target: HTMLElement, key: "ArrowUp" | "ArrowDown", shiftKey = false) {
  const event = new KeyboardEvent("keydown", {
    key,
    code: key,
    altKey: true,
    shiftKey,
    bubbles: true,
    cancelable: true,
  });
  target.dispatchEvent(event);
  return event;
}

describe("line editing commands", () => {
  it("moves textarea lines with Alt+Up, and native undo restores them", () => {
    using stack = useLineEditing();
    const textarea = mount(stack, document.createElement("textarea"));
    textarea.value = "one\ntwo\nthree";
    textarea.focus();
    textarea.setSelectionRange(5, 5);

    expect(pressAlt(textarea, "ArrowUp").defaultPrevented).toBe(true);
    expect(textarea.value).toBe("two\none\nthree");
    expect([textarea.selectionStart, textarea.selectionEnd]).toEqual([1, 1]);

    document.execCommand("undo");
    expect(textarea.value).toBe("one\ntwo\nthree");
  });

  it("leaves the key alone at a boundary and in a terminal", () => {
    using stack = useLineEditing();
    const textarea = mount(stack, document.createElement("textarea"));
    textarea.value = "one\ntwo";
    textarea.focus();
    textarea.setSelectionRange(0, 0);
    expect(pressAlt(textarea, "ArrowUp").defaultPrevented).toBe(false);

    const terminal = mount(stack, document.createElement("div"));
    terminal.dataset.terminalOwner = "drawer";
    const terminalInput = mount(stack, document.createElement("textarea"), terminal);
    terminalInput.value = "one\ntwo";
    terminalInput.focus();
    terminalInput.setSelectionRange(5, 5);
    expect(pressAlt(terminalInput, "ArrowUp").defaultPrevented).toBe(false);
    expect(terminalInput.value).toBe("one\ntwo");
  });

  it("moves paragraphs in a focused Tiptap editor", () => {
    using stack = useLineEditing();
    const element = mount(stack, document.createElement("div"));
    const editor = new Editor({
      element,
      extensions: [StarterKit],
      content: "<p>one</p><p>two</p>",
    });
    stack.defer(() => editor.destroy());
    editor.commands.focus("end");

    pressAlt(editor.view.dom, "ArrowUp");

    expect(editor.getHTML()).toBe("<p>two</p><p>one</p>");
    expect(editor.state.selection.$head.parent.textContent).toBe("two");
  });
});
