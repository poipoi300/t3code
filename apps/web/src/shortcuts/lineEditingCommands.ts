import { closeHistory } from "@tiptap/pm/history";
import type { EditorState, Transaction } from "@tiptap/pm/state";

import { markAsLineEdit } from "../composer-undo-grouping";
import { getClientSettings } from "../hooks/useSettings";
import { focusedMultilineText } from "../lib/editableFocus";
import {
  duplicateLines,
  type LineDirection,
  moveLines,
  type TextLineEdit,
} from "../lib/lineEditing";
import { duplicateLinesTransaction, moveLinesTransaction } from "../lib/proseMirrorLineEditing";
import { type CommandHandler, type CommandHandlers, registerCommands } from "./dispatcher";

function applyTextareaEdit(textarea: HTMLTextAreaElement, edit: TextLineEdit): void {
  const direction = textarea.selectionDirection;
  textarea.setSelectionRange(edit.from, edit.to);
  // insertText keeps native undo and fires `input`, which controlled fields need.
  document.execCommand("insertText", false, edit.insert);
  textarea.setSelectionRange(edit.selectionStart, edit.selectionEnd, direction);
}

/** Returns false at a boundary, so the key keeps its native caret movement. */
function lineCommand(
  editText: (text: string, start: number, end: number) => TextLineEdit | null,
  editDocument: (state: EditorState) => Transaction | null,
): CommandHandler {
  return (event) => {
    const target = focusedMultilineText(event.target);
    if (target instanceof HTMLTextAreaElement) {
      const edit = editText(target.value, target.selectionStart, target.selectionEnd);
      if (!edit) return false;
      applyTextareaEdit(target, edit);
    } else {
      const view = target?.editor?.view;
      const tr = view ? editDocument(view.state) : null;
      if (!view || !tr) return false;
      view.dispatch(markAsLineEdit(closeHistory(tr)).scrollIntoView());
    }
    event.preventDefault();
  };
}

function moveCommand(direction: LineDirection): CommandHandler {
  return lineCommand(
    (text, start, end) => moveLines(text, start, end, direction),
    (state) => moveLinesTransaction(state, direction),
  );
}

function duplicateCommand(direction: LineDirection): CommandHandler {
  const followCopy = () => getClientSettings().duplicateLineFollowsCopy;
  return lineCommand(
    (text, start, end) => duplicateLines(text, start, end, direction, followCopy()),
    (state) => duplicateLinesTransaction(state, direction, followCopy()),
  );
}

export const lineEditingCommands = {
  "text.moveLineUp": moveCommand("up"),
  "text.moveLineDown": moveCommand("down"),
  "text.duplicateLineUp": duplicateCommand("up"),
  "text.duplicateLineDown": duplicateCommand("down"),
} satisfies CommandHandlers;

export function registerLineEditingCommands(): () => void {
  return registerCommands(lineEditingCommands);
}
