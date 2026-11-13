//! Undo grouping independent of the editor transaction implementation.
pub const GROUP_DELAY_MS: u64 = 1_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Insert,
    Delete,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replace {
    pub deleted: usize,
    pub inserted: usize,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct Transaction<'a> {
    pub changed: bool,
    pub clipboard_or_drop: bool,
    pub composition: bool,
    pub history: bool,
    pub steps: &'a [Option<Replace>],
}
pub fn change_kind(transaction: Transaction<'_>) -> Option<ChangeKind> {
    if !transaction.changed {
        return None;
    }
    if transaction.clipboard_or_drop {
        return Some(ChangeKind::Other);
    }
    if transaction.composition {
        return Some(ChangeKind::Insert);
    }
    let mut kind = None;
    for step in transaction.steps {
        let Some(step) = step else {
            return Some(ChangeKind::Other);
        };
        let next = match (step.inserted > 0, step.deleted > 0) {
            (true, false) => ChangeKind::Insert,
            (false, true) => ChangeKind::Delete,
            _ => ChangeKind::Other,
        };
        if kind.is_some_and(|k| k != next) {
            return Some(ChangeKind::Other);
        }
        kind = Some(next);
    }
    kind
}
pub fn group_by_change_kind(
    transaction: Transaction<'_>,
    previous: Option<ChangeKind>,
) -> (Option<ChangeKind>, bool) {
    if transaction.history {
        return (Some(ChangeKind::Other), false);
    }
    let Some(kind) = change_kind(transaction) else {
        return (previous, false);
    };
    (
        Some(kind),
        previous.is_some_and(|p| p != kind || kind == ChangeKind::Other),
    )
}

#[derive(Debug, Clone)]
pub struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    previous: Option<ChangeKind>,
    last_edit: Option<u64>,
}
impl<T: Clone> Default for History<T> {
    fn default() -> Self {
        Self {
            undo: vec![],
            redo: vec![],
            previous: None,
            last_edit: None,
        }
    }
}
impl<T: Clone> History<T> {
    pub fn record(&mut self, before: &T, transaction: Transaction<'_>, now_ms: u64) {
        let (kind, boundary) = group_by_change_kind(transaction, self.previous);
        if transaction.changed && !transaction.history {
            if self.undo.is_empty()
                || boundary
                || self
                    .last_edit
                    .is_none_or(|last| now_ms.saturating_sub(last) > GROUP_DELAY_MS)
            {
                self.undo.push(before.clone());
            }
            self.redo.clear();
            self.last_edit = Some(now_ms);
        }
        self.previous = kind;
    }
    pub fn undo(&mut self, current: &T) -> Option<T> {
        let prior = self.undo.pop()?;
        self.redo.push(current.clone());
        self.previous = Some(ChangeKind::Other);
        Some(prior)
    }
    pub fn redo(&mut self, current: &T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        self.previous = Some(ChangeKind::Other);
        Some(next)
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn apply(
        history: &mut History<String>,
        text: &mut String,
        insert: &str,
        delete: usize,
        clipboard: bool,
        composition: bool,
        now: u64,
    ) {
        let steps = [Some(Replace {
            deleted: delete,
            inserted: insert.len(),
        })];
        history.record(
            text,
            Transaction {
                changed: true,
                clipboard_or_drop: clipboard,
                composition,
                steps: &steps,
                ..Transaction::default()
            },
            now,
        );
        text.truncate(text.len() - delete);
        text.push_str(insert);
    }
    #[test]
    fn typing_deleting_typing() {
        let mut h = History::default();
        let mut t = String::new();
        apply(&mut h, &mut t, "hello world", 0, false, false, 80);
        apply(&mut h, &mut t, "", 5, false, false, 160);
        apply(&mut h, &mut t, "there", 0, false, false, 240);
        for expected in ["hello ", "hello world", ""] {
            t = h.undo(&t).unwrap();
            assert_eq!(t, expected);
        }
    }
    #[test]
    fn paste_cut_and_rewrite_boundaries() {
        for (initial, steps, expected) in [
            (
                "",
                vec![
                    ("see ", 0, false),
                    ("pasted text", 0, true),
                    (" ok", 0, false),
                ],
                vec!["see pasted text", "see ", ""],
            ),
            (
                "",
                vec![
                    ("hello world", 0, false),
                    ("", 1, false),
                    ("", 3, true),
                    ("", 1, false),
                ],
                vec!["hello w", "hello worl", "hello world", ""],
            ),
            (
                "draft ",
                vec![
                    ("one", 0, false),
                    ("draft one @chip ", 9, false),
                    ("then", 0, false),
                ],
                vec!["draft one @chip ", "draft one", "draft "],
            ),
        ] {
            let mut h = History::default();
            let mut t = initial.to_string();
            for (i, (insert, delete, clipboard)) in steps.into_iter().enumerate() {
                apply(
                    &mut h,
                    &mut t,
                    insert,
                    delete,
                    clipboard,
                    false,
                    i as u64 * 80,
                );
            }
            for value in expected {
                t = h.undo(&t).unwrap();
                assert_eq!(t, value);
            }
        }
    }
    #[test]
    fn composition_is_typing_and_idle_separates() {
        let mut h = History::default();
        let mut t = String::new();
        apply(&mut h, &mut t, "go ", 0, false, false, 80);
        apply(&mut h, &mut t, "に", 0, false, true, 160);
        apply(&mut h, &mut t, "にほん", 3, false, true, 240);
        assert_eq!(h.undo(&t).unwrap(), "");
        let mut h = History::default();
        let mut t = String::new();
        apply(&mut h, &mut t, "a", 0, false, false, 0);
        apply(&mut h, &mut t, "b", 0, false, false, 1001);
        assert_eq!(h.undo(&t).unwrap(), "a");
    }
}
