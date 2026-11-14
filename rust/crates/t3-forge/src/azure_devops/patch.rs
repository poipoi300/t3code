use super::*;
struct Edit<'a> {
    kind: u8,
    line: &'a str,
}
fn at(v: &[isize], distance: usize, k: isize) -> isize {
    v[(k + distance as isize) as usize]
}
fn edits<'a>(old: &[&'a str], new: &[&'a str]) -> Option<Vec<Edit<'a>>> {
    let started = std::time::Instant::now();
    let mut trace = Vec::<Vec<isize>>::new();
    for distance in 0..=2000 {
        if started.elapsed() > Duration::from_secs(2) {
            return None;
        }
        let d = distance as isize;
        let mut v = vec![0; distance * 2 + 1];
        for k in (-d..=d).step_by(2) {
            let mut x = if distance == 0 {
                0
            } else {
                let previous = &trace[distance - 1];
                if k == -d
                    || k != d
                        && at(previous, distance - 1, k - 1) < at(previous, distance - 1, k + 1)
                {
                    at(previous, distance - 1, k + 1)
                } else {
                    at(previous, distance - 1, k - 1) + 1
                }
            };
            let mut y = x - k;
            while x < old.len() as isize
                && y < new.len() as isize
                && old[x as usize] == new[y as usize]
            {
                x += 1;
                y += 1;
            }
            v[(k + d) as usize] = x;
            if x == old.len() as isize && y == new.len() as isize {
                trace.push(v);
                let mut result = vec![];
                for d in (1..=distance).rev() {
                    let k = x - y;
                    let previous = &trace[d - 1];
                    let sd = d as isize;
                    let previous_k = if k == -sd
                        || k != sd && at(previous, d - 1, k - 1) < at(previous, d - 1, k + 1)
                    {
                        k + 1
                    } else {
                        k - 1
                    };
                    let previous_x = at(previous, d - 1, previous_k);
                    let previous_y = previous_x - previous_k;
                    while x > previous_x && y > previous_y {
                        x -= 1;
                        y -= 1;
                        result.push(Edit {
                            kind: b' ',
                            line: old[x as usize],
                        });
                    }
                    if x == previous_x {
                        y -= 1;
                        result.push(Edit {
                            kind: b'+',
                            line: new[y as usize],
                        });
                    } else {
                        x -= 1;
                        result.push(Edit {
                            kind: b'-',
                            line: old[x as usize],
                        });
                    }
                    x = previous_x;
                    y = previous_y;
                }
                while x > 0 && y > 0 {
                    x -= 1;
                    y -= 1;
                    result.push(Edit {
                        kind: b' ',
                        line: old[x as usize],
                    });
                }
                result.reverse();
                return Some(result);
            }
        }
        trace.push(v);
    }
    None
}
pub(super) struct FilePatch {
    pub section: String,
    pub truncated: bool,
    pub abandoned: bool,
    pub edits: usize,
}
fn range(before: usize, count: usize) -> String {
    match count {
        0 => format!("{before},0"),
        1 => (before + 1).to_string(),
        _ => format!("{},{count}", before + 1),
    }
}
fn header(change: &Change) -> String {
    let old = quote_patch_path(&format!("a/{}", change.old));
    let new = quote_patch_path(&format!("b/{}", change.path));
    let mut lines = vec![format!("diff --git {old} {new}")];
    match change.kind {
        "new" => lines.push("new file mode 100644".into()),
        "deleted" => lines.push("deleted file mode 100644".into()),
        "rename-pure" | "rename-changed" => lines.extend([
            format!(
                "similarity index {}%",
                if change.kind == "rename-pure" { 100 } else { 0 }
            ),
            format!("rename from {}", quote_patch_path(&change.old)),
            format!("rename to {}", quote_patch_path(&change.path)),
        ]),
        _ => {}
    }
    lines.push(format!(
        "--- {}",
        if change.kind == "new" {
            "/dev/null"
        } else {
            &old
        }
    ));
    lines.push(format!(
        "+++ {}",
        if change.kind == "deleted" {
            "/dev/null"
        } else {
            &new
        }
    ));
    lines.join("\n") + "\n"
}
pub(super) fn unreadable(change: &Change) -> FilePatch {
    FilePatch {
        section: header(change),
        truncated: true,
        abandoned: false,
        edits: 0,
    }
}
pub(super) fn file(change: Change, old: String, new: String, binary: bool) -> FilePatch {
    let mut result = FilePatch {
        section: header(&change),
        truncated: false,
        abandoned: false,
        edits: 0,
    };
    if binary || old.contains('\0') || new.contains('\0') {
        result.section.push_str(&format!(
            "Binary files {} and {} differ\n",
            quote_patch_path(&format!("a/{}", change.old)),
            quote_patch_path(&format!("b/{}", change.path))
        ));
        result.truncated = true;
        return result;
    }
    if old.len() > 512 * 1024 || new.len() > 512 * 1024 {
        result.truncated = true;
        return result;
    }
    let old_lines: Vec<_> = old.split_inclusive('\n').collect();
    let new_lines: Vec<_> = new.split_inclusive('\n').collect();
    let edits = if old.is_empty() {
        Some(
            new_lines
                .iter()
                .map(|line| Edit { kind: b'+', line })
                .collect(),
        )
    } else if new.is_empty() {
        Some(
            old_lines
                .iter()
                .map(|line| Edit { kind: b'-', line })
                .collect(),
        )
    } else {
        edits(&old_lines, &new_lines)
    };
    let Some(edits) = edits else {
        result.truncated = true;
        result.abandoned = true;
        result.edits = 2000;
        return result;
    };
    result.edits = edits.iter().filter(|e| e.kind != b' ').count();
    let mut spans = Vec::<(usize, usize)>::new();
    for (i, edit) in edits.iter().enumerate().filter(|(_, e)| e.kind != b' ') {
        let _ = edit;
        let start = i.saturating_sub(3);
        let end = (i + 4).min(edits.len());
        if let Some(last) = spans.last_mut().filter(|last| last.1 >= start) {
            last.1 = end;
        } else {
            spans.push((start, end));
        }
    }
    let mut positions = vec![(0usize, 0usize)];
    for edit in &edits {
        let (mut old, mut new) = *positions.last().expect("initial position exists");
        if edit.kind != b'+' {
            old += 1;
        }
        if edit.kind != b'-' {
            new += 1;
        }
        positions.push((old, new));
    }
    let header_size = result.section.len();
    for (start, end) in spans {
        let (before_old, before_new) = positions[start];
        let (after_old, after_new) = positions[end];
        result.section.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(before_old, after_old - before_old),
            range(before_new, after_new - before_new)
        ));
        for edit in &edits[start..end] {
            result.section.push(edit.kind as char);
            result.section.push_str(edit.line);
            if !edit.line.ends_with('\n') {
                result.section.push_str("\n\\ No newline at end of file\n");
            }
        }
        if result.section.len() > 512 * 1024 {
            result.section.truncate(header_size);
            result.truncated = true;
            break;
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn change(kind: &'static str) -> Change {
        Change {
            path: "new.txt".into(),
            old: "old.txt".into(),
            kind,
            oid: None,
        }
    }
    #[test]
    fn rename_patch_keeps_context_and_newline_markers() {
        let result = file(
            change("rename-changed"),
            "one\ntwo".into(),
            "one\nchanged\n".into(),
            false,
        );
        assert_eq!(
            result.section,
            "diff --git a/old.txt b/new.txt\nsimilarity index 0%\nrename from old.txt\nrename to new.txt\n--- a/old.txt\n+++ b/new.txt\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+changed\n"
        );
        assert_eq!(result.edits, 2);
        assert!(!result.truncated);
    }
    #[test]
    fn empty_range_and_binary_are_explicit() {
        let result = file(change("new"), String::new(), "created\n".into(), false);
        assert!(result.section.contains("@@ -0,0 +1 @@"));
        let binary = file(change("change"), "a".into(), "encoded".into(), true);
        assert!(binary.truncated);
        assert!(
            binary
                .section
                .contains("Binary files a/old.txt and b/new.txt differ")
        );
    }
    #[test]
    fn edit_limit_emits_no_fake_full_replacement() {
        let old = (0..1001).map(|n| format!("old{n}\n")).collect();
        let new = (0..1001).map(|n| format!("new{n}\n")).collect();
        let result = file(change("change"), old, new, false);
        assert!(result.abandoned);
        assert!(result.truncated);
        assert!(!result.section.contains("@@"));
    }
}
