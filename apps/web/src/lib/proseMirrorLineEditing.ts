import { Fragment, NodeRange, type Node as ProseMirrorNode } from "@tiptap/pm/model";
import {
  AllSelection,
  type EditorState,
  NodeSelection,
  Selection,
  TextSelection,
  type Transaction,
} from "@tiptap/pm/state";

import { duplicateLines, type LineDirection, moveLines, type TextLineEdit } from "./lineEditing";

function isListItemRange(range: NodeRange): boolean {
  if (range.depth === 0) return false;
  const list = range.$from.node(range.depth - 1);
  return list.type.spec.group?.split(" ").includes("list") === true;
}

/** The blocks the selection touches. Inside a list item, the item itself is the line. */
function selectedBlocks(state: EditorState): NodeRange | null {
  const { $from, empty } = state.selection;
  let { $to } = state.selection;
  // Like a textarea, a selection ending at the start of a block leaves that block out.
  if (!empty && $to.parentOffset === 0 && $to.depth > 0) {
    const previous = Selection.findFrom(state.doc.resolve($to.before()), -1, true);
    if (previous && previous.$head.pos >= $from.pos) $to = previous.$head;
  }
  let range = $from.blockRange($to);
  while (range && range.startIndex === 0 && isListItemRange(range)) {
    range = new NodeRange(range.$from, range.$to, range.depth - 1);
  }
  return range;
}

function blockFragment(range: NodeRange): Fragment {
  const blocks: ProseMirrorNode[] = [];
  for (let index = range.startIndex; index < range.endIndex; index += 1) {
    blocks.push(range.parent.child(index));
  }
  return Fragment.fromArray(blocks);
}

function shiftSelection(state: EditorState, tr: Transaction, shift: number): Transaction {
  const { selection } = state;
  if (selection instanceof AllSelection) return tr.setSelection(new AllSelection(tr.doc));
  return tr.setSelection(
    selection instanceof NodeSelection
      ? NodeSelection.create(tr.doc, selection.from + shift)
      : TextSelection.create(tr.doc, selection.anchor + shift, selection.head + shift),
  );
}

/** Code blocks hold their lines as text, so they edit like a textarea. */
function codeBlockLineEdit(
  state: EditorState,
  edit: (text: string, start: number, end: number) => TextLineEdit | null,
): Transaction | null {
  const { $from, $to, anchor, head } = state.selection;
  const base = $from.start();
  const result = edit($from.parent.textContent, $from.pos - base, $to.pos - base);
  if (!result) return null;
  const tr = state.tr.insertText(result.insert, base + result.from, base + result.to);
  const [anchorOffset, headOffset] =
    anchor <= head
      ? [result.selectionStart, result.selectionEnd]
      : [result.selectionEnd, result.selectionStart];
  return tr.setSelection(TextSelection.create(tr.doc, base + anchorOffset, base + headOffset));
}

function isInCodeBlock(state: EditorState): boolean {
  const { $from, $to } = state.selection;
  return $from.sameParent($to) && $from.parent.type.spec.code === true;
}

export function moveLinesTransaction(
  state: EditorState,
  direction: LineDirection,
): Transaction | null {
  if (isInCodeBlock(state)) {
    return codeBlockLineEdit(state, (text, start, end) => moveLines(text, start, end, direction));
  }
  const range = selectedBlocks(state);
  if (!range) return null;
  const blocks = blockFragment(range);
  if (direction === "up") {
    if (range.startIndex === 0) return null;
    const previous = range.parent.child(range.startIndex - 1);
    const reordered = blocks.append(Fragment.from(previous));
    if (!range.parent.canReplace(range.startIndex - 1, range.endIndex, reordered)) return null;
    const tr = state.tr.replaceWith(range.start - previous.nodeSize, range.end, reordered);
    return shiftSelection(state, tr, -previous.nodeSize);
  }
  if (range.endIndex === range.parent.childCount) return null;
  const next = range.parent.child(range.endIndex);
  const reordered = Fragment.from(next).append(blocks);
  if (!range.parent.canReplace(range.startIndex, range.endIndex + 1, reordered)) return null;
  const tr = state.tr.replaceWith(range.start, range.end + next.nodeSize, reordered);
  return shiftSelection(state, tr, next.nodeSize);
}

/** `followCopy` leaves the selection on the new copy instead of the original blocks. */
export function duplicateLinesTransaction(
  state: EditorState,
  direction: LineDirection,
  followCopy: boolean,
): Transaction | null {
  if (isInCodeBlock(state)) {
    return codeBlockLineEdit(state, (text, start, end) =>
      duplicateLines(text, start, end, direction, followCopy),
    );
  }
  const range = selectedBlocks(state);
  if (!range) return null;
  const blocks = blockFragment(range);
  const index = direction === "up" ? range.startIndex : range.endIndex;
  if (!range.parent.canReplace(index, index, blocks)) return null;
  const tr = state.tr.insert(direction === "up" ? range.start : range.end, blocks);
  return shiftSelection(state, tr, (direction === "down") === followCopy ? blocks.size : 0);
}
