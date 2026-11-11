export type LineDirection = "up" | "down";

/** Replace `text.slice(from, to)` with `insert`, then select `selectionStart..selectionEnd`. */
export interface TextLineEdit {
  readonly from: number;
  readonly to: number;
  readonly insert: string;
  readonly selectionStart: number;
  readonly selectionEnd: number;
}

function lineStartAt(text: string, position: number): number {
  return position === 0 ? 0 : text.lastIndexOf("\n", position - 1) + 1;
}

function lineEndAt(text: string, position: number): number {
  const newline = text.indexOf("\n", position);
  return newline === -1 ? text.length : newline;
}

/** Like editors, a selection ending at the start of a line leaves that line out. */
function selectedLines(text: string, start: number, end: number) {
  const lastLinePosition = end > start && text[end - 1] === "\n" ? end - 1 : end;
  return { lineStart: lineStartAt(text, start), lineEnd: lineEndAt(text, lastLinePosition) };
}

export function moveLines(
  text: string,
  start: number,
  end: number,
  direction: LineDirection,
): TextLineEdit | null {
  const { lineStart, lineEnd } = selectedLines(text, start, end);
  const block = text.slice(lineStart, lineEnd);
  if (direction === "up") {
    if (lineStart === 0) return null;
    const previousStart = lineStartAt(text, lineStart - 1);
    const previous = text.slice(previousStart, lineStart - 1);
    const shift = previous.length + 1;
    return {
      from: previousStart,
      to: lineEnd,
      insert: `${block}\n${previous}`,
      selectionStart: start - shift,
      selectionEnd: end - shift,
    };
  }
  if (lineEnd === text.length) return null;
  const nextEnd = lineEndAt(text, lineEnd + 1);
  const next = text.slice(lineEnd + 1, nextEnd);
  const shift = next.length + 1;
  return {
    from: lineStart,
    to: nextEnd,
    insert: `${next}\n${block}`,
    selectionStart: start + shift,
    // A selection that ended at the next line's start would otherwise run past the text.
    selectionEnd: Math.min(end + shift, text.length),
  };
}

/** `followCopy` leaves the selection on the new copy instead of the original lines. */
export function duplicateLines(
  text: string,
  start: number,
  end: number,
  direction: LineDirection,
  followCopy: boolean,
): TextLineEdit {
  const { lineStart, lineEnd } = selectedLines(text, start, end);
  const block = text.slice(lineStart, lineEnd);
  const shift = block.length + 1;
  // The copy above takes the original's offsets; the copy below sits one block further on.
  const selectionShift = (direction === "down") === followCopy ? shift : 0;
  return {
    from: direction === "up" ? lineStart : lineEnd,
    to: direction === "up" ? lineStart : lineEnd,
    insert: direction === "up" ? `${block}\n` : `\n${block}`,
    selectionStart: start + selectionShift,
    selectionEnd: end + selectionShift,
  };
}
