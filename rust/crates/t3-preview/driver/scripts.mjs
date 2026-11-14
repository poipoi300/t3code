export const EDITABLE_AT_POINT_SCRIPT = `(x, y) => {
  let element = document.elementFromPoint(x, y);
  while (element && element.shadowRoot) {
    const inner = element.shadowRoot.elementFromPoint(x, y);
    if (!inner || inner === element) break;
    element = inner;
  }
  if (element && element.tagName === "LABEL" && element.control) element = element.control;
  if (!element) return false;
  if (element.tagName === "IFRAME" || element.tagName === "FRAME") return true;
  if (element.isContentEditable) return true;
  if (element.tagName === "TEXTAREA") return !element.disabled && !element.readOnly;
  if (element.tagName !== "INPUT") return false;
  const nonText = ["button", "checkbox", "color", "file", "hidden", "image", "radio", "range", "reset", "submit"];
  return !nonText.includes(element.type) && !element.disabled && !element.readOnly;
}`;
export const asRecord = (value         )                                 =>
  typeof value === "object" && value !== null ? (value                           ) : null;

export const num = (value         , fallback = 0) =>
  typeof value === "number" && Number.isFinite(value) ? value : fallback;

export const modifiersOf = (message                         ) => {
  const value = num(message.modifiers);
  return Number.isInteger(value) && value >= 0 && value < 16 ? value : 0;
};

// Linux/headless Chromium needs commands for Cmd editing shortcuts and Ctrl copy/cut.
export const editingCommand = (key        , modifiers        ) => {
  const modifier = modifiers & 0b0111;
  if (modifier !== 2 && modifier !== 4) return null;
  const lower = key.toLowerCase();
  const command =
    lower === "c"
      ? "copy"
      : lower === "x"
        ? "cut"
        : modifier !== 4
          ? null
          : lower === "a"
            ? "selectAll"
            : lower === "z"
              ? modifiers & 8
                ? "redo"
                : "undo"
              : null;
  return command ? { commands: [command] } : null;
};

const CLIPBOARD_TEXT_LIMIT = 1024 * 1024;
export const CLIPBOARD_WORLD = "t3-preview-clipboard";
// Chromium shares its clipboard across contexts; capture real copy/cut keys in a page-inaccessible world instead.
export const CLIPBOARD_SCRIPT = `(() => {
  let keying = false;
  // One entry per copy or cut key, oldest first, so a later key cannot overwrite an unread copy.
  const copies = [];
  // The same Ctrl or Cmd + C or X that editingCommand sends as copy or cut.
  addEventListener("keydown", (event) => {
    if (!event.isTrusted || event.altKey || event.ctrlKey === event.metaKey) return;
    const key = event.key.toLowerCase();
    if (key !== "c" && key !== "x") return;
    keying = true;
    copies.push({ at: Date.now(), text: "" });
    setTimeout(() => { keying = false; });
  }, true);
  const selection = () => {
    let field = document.activeElement;
    while (field?.shadowRoot?.activeElement) field = field.shadowRoot.activeElement;
    if ((field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) && field.selectionStart !== null)
      return field.value.slice(field.selectionStart, field.selectionEnd ?? field.selectionStart);
    return String(document.getSelection() ?? "");
  };
  const record = (text) => {
    const copy = copies.at(-1);
    if (copy) copy.text = String(text ?? "").slice(0, ${CLIPBOARD_TEXT_LIMIT});
  };
  // Capturing at window runs before page handlers can stop the event, and
  // before cut removes the selection.
  const snapshot = (event) => {
    if (keying && event.isTrusted) record(selection());
  };
  // Bubbling back to window picks up text a page handler set instead.
  const replace = (event) => {
    if (keying && event.isTrusted && event.defaultPrevented) record(event.clipboardData?.getData("text/plain"));
  };
  for (const type of ["copy", "cut"]) {
    addEventListener(type, snapshot, true);
    addEventListener(type, replace);
  }
  // A key's copy was keyed between its dispatch and acknowledgement; older copies are stale.
  globalThis.takeCopy = (since, until) => {
    while (copies.length > 0 && copies[0].at < since) copies.shift();
    return copies[0] && copies[0].at <= until ? copies.shift().text : "";
  };
})();`;

