import type { TiptapEditorHTMLElement } from "@tiptap/core";

const EDITABLE_SELECTOR = [
  "input",
  "textarea",
  "select",
  '[contenteditable=""]',
  '[contenteditable="true"]',
  '[contenteditable="plaintext-only"]',
  '[role="textbox"]',
].join(",");

function focusedElement(target: EventTarget | null): Element | null {
  let element = target instanceof Element ? target : null;
  while (element?.shadowRoot?.activeElement) element = element.shadowRoot.activeElement;
  return element;
}

/** Includes focused descendants of open shadow roots, whose key events target the host. */
export function isEditableFocused(target: EventTarget | null = document.activeElement): boolean {
  const element = focusedElement(target);
  return element !== null && element.closest(EDITABLE_SELECTOR) !== null;
}

/** A focused textarea or editable Tiptap editor. A terminal's input textarea is not one. */
export function focusedMultilineText(
  target: EventTarget | null = document.activeElement,
): HTMLTextAreaElement | TiptapEditorHTMLElement | null {
  const element = focusedElement(target);
  if (!(element instanceof HTMLElement) || element.closest("[data-terminal-owner]") !== null) {
    return null;
  }
  if (element instanceof HTMLTextAreaElement) {
    return element.readOnly || element.disabled ? null : element;
  }
  if (!element.isContentEditable) return null;
  const editorElement = element.closest<TiptapEditorHTMLElement>(".ProseMirror");
  return editorElement?.editor?.isEditable === true ? editorElement : null;
}
