//! The files of a patch, as apps/web/src/lib/diffRendering.ts reads them through
//! @pierre/diffs' `parsePatchFiles` (partial patches: hunk lines only).
//!
//! The view draws the patch itself (NSH `DiffView` parses and lays out the rows it shows); this
//! index holds what the panel decides with: paths, change types, counts and each file's bytes,
//! so hiding files joins byte ranges instead of re-reading the patch.

use std::{ops::Range, rc::Rc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeType {
    Change,
    RenamePure,
    RenameChanged,
    New,
    Deleted,
}

impl FileChangeType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::RenamePure => "rename-pure",
            Self::RenameChanged => "rename-changed",
            Self::New => "new",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFile {
    pub name: String,
    pub prev_name: Option<String>,
    pub change: FileChangeType,
    pub additions: u64,
    pub deletions: u64,
    pub hunks: u32,
    /// This file's bytes in [`PatchFiles::source`].
    pub range: Range<usize>,
    /// Each hunk's first line numbers and how many of its lines each side shows.
    pub hunk_lines: Vec<HunkLines>,
}

/// A hunk as the view lays it out: the old side shows its context and deleted lines, the new
/// side its context and added lines, numbered from the header's starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkLines {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
}

impl PatchFile {
    /// `resolveFileDiffPath`: the parser-decoded name, including Git's quoted names.
    pub fn path(&self) -> &str {
        &self.name
    }
    /// `resolveFileDiffPreviousPath`.
    pub fn previous_path(&self) -> &str {
        self.prev_name.as_deref().unwrap_or(&self.name)
    }
    /// `buildFileDiffIdentityKey`: the change type tells a deletion/addition pair apart.
    pub fn identity_key(&self) -> String {
        format!(
            "{}\u{0}{}\u{0}{}",
            self.previous_path(),
            self.path(),
            self.change.as_str()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFiles {
    pub source: Rc<str>,
    pub files: Vec<PatchFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffLineStat {
    pub additions: u64,
    pub deletions: u64,
}

impl PatchFiles {
    /// `getDiffLineStat` over the given files.
    pub fn line_stat<'a>(files: impl IntoIterator<Item = &'a PatchFile>) -> DiffLineStat {
        files
            .into_iter()
            .fold(DiffLineStat::default(), |total, file| DiffLineStat {
                additions: total.additions + file.additions,
                deletions: total.deletions + file.deletions,
            })
    }

    /// The patch with only the files `shown` keeps, in order. Every file shown returns the
    /// source itself, so the view does not re-read an unchanged patch.
    pub fn joined(&self, shown: impl Fn(&PatchFile) -> bool) -> Rc<str> {
        let kept: Vec<_> = self.files.iter().filter(|file| shown(file)).collect();
        if kept.len() == self.files.len() {
            return self.source.clone();
        }
        let mut text = String::with_capacity(kept.iter().map(|f| f.range.len() + 1).sum());
        for file in kept {
            let part = &self.source[file.range.clone()];
            text.push_str(part);
            if !part.ends_with('\n') {
                text.push('\n');
            }
        }
        Rc::from(text)
    }

    /// DiffPanel.tsx's file keys: a git selection keys each path by its occurrence, since
    /// metadata carries no change type; other selections use the identity key.
    pub fn file_keys(&self, git_selection: bool) -> Vec<String> {
        let mut occurrences = std::collections::BTreeMap::<String, usize>::new();
        self.files
            .iter()
            .map(|file| {
                if !git_selection {
                    return file.identity_key();
                }
                let path_key = serde_json::to_string(&[
                    file.prev_name.as_deref().unwrap_or(&file.name),
                    &file.name,
                ])
                .unwrap_or_default();
                let occurrence = occurrences.entry(path_key.clone()).or_default();
                let key = format!("{path_key}:{occurrence}");
                *occurrence += 1;
                key
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderablePatch {
    Files(Rc<PatchFiles>),
    /// "Unsupported diff format. Showing raw patch."
    Raw(Rc<str>),
}

/// `getRenderablePatch`: `None` for an absent or blank patch.
pub fn renderable_patch(patch: &Rc<str>) -> Option<RenderablePatch> {
    if patch.trim().is_empty() {
        return None;
    }
    let files = parse_patch_files(patch);
    Some(if files.is_empty() {
        RenderablePatch::Raw(patch.clone())
    } else {
        RenderablePatch::Files(Rc::new(PatchFiles {
            source: patch.clone(),
            files,
        }))
    })
}

pub fn parse_patch_files(data: &str) -> Vec<PatchFile> {
    let git = data.starts_with("diff --git") || data.contains("\ndiff --git");
    let parts = if git {
        split_at_line_prefix(data, 0..data.len(), "diff --git")
    } else {
        split_unified_files(data)
    };
    parts
        .into_iter()
        .filter(|part| {
            if git {
                data[part.clone()].starts_with("diff --git")
            } else {
                unified_file_header_at(data, part.start)
            }
        })
        .filter_map(|part| process_file(data, part, git))
        .collect()
}

fn process_file(data: &str, range: Range<usize>, git: bool) -> Option<PatchFile> {
    let mut file: Option<PatchFile> = None;
    for hunk in split_at_line_prefix(data, range.clone(), "@@ ") {
        let mut lines: Vec<&str> = lines_with_newlines(&data[hunk]);
        let Some(first) = lines.first() else {
            continue;
        };
        // As in @pierre/diffs, the first block is always the file header.
        let Some(current) = file.as_mut() else {
            file = Some(read_file_header(&lines, git, range.clone()));
            continue;
        };
        let Some(header) = parse_hunk_header(first) else {
            continue;
        };
        while lines
            .last()
            .is_some_and(|line| matches!(*line, "\n" | "\r" | "\r\n" | ""))
        {
            lines.pop();
        }
        let (mut additions, mut deletions, mut context) = (0u64, 0u64, 0u64);
        for line in &lines[1..] {
            if additions >= header.addition_count
                && deletions >= header.deletion_count
                && !line.starts_with('\\')
                && !(hunk_body_line(line) && !format_patch_version_separator(line))
            {
                break;
            }
            match line.as_bytes().first() {
                Some(b'+') => additions += 1,
                Some(b'-') => deletions += 1,
                Some(b' ') => context += 1,
                _ => {}
            }
        }
        current.hunk_lines.push(HunkLines {
            old_start: header.deletion_start,
            old_lines: context + deletions,
            new_start: header.addition_start,
            new_lines: context + additions,
        });
        current.additions += additions;
        current.deletions += deletions;
        current.hunks += 1;
    }
    let mut file = file?;
    if !git
        && let Some(prev) = &file.prev_name
        && prev != &file.name
    {
        file.change = if file.hunks > 0 {
            FileChangeType::RenameChanged
        } else {
            FileChangeType::RenamePure
        };
    }
    if !matches!(
        file.change,
        FileChangeType::RenamePure | FileChangeType::RenameChanged
    ) {
        file.prev_name = None;
    }
    Some(file)
}

fn read_file_header(lines: &[&str], git: bool, range: Range<usize>) -> PatchFile {
    let mut file = PatchFile {
        name: String::new(),
        prev_name: None,
        change: FileChangeType::Change,
        additions: 0,
        deletions: 0,
        hunks: 0,
        range,
        hunk_lines: Vec::new(),
    };
    for raw in lines {
        let line = raw.trim_end_matches(['\n', '\r']);
        if raw.starts_with("diff --git") {
            if let Some((prev, name)) = git_header_names(raw.trim()) {
                if prev != name {
                    file.prev_name = Some(prev);
                }
                file.name = name;
            }
            continue;
        }
        if (line.starts_with("---") || line.starts_with("+++"))
            && let Some(name) = filename_header(line)
        {
            let name = decode_diff_file_name(name, git);
            if line.starts_with("---") && name != "/dev/null" {
                file.prev_name = Some(name.clone());
                file.name = name;
            } else if line.starts_with("+++") && name != "/dev/null" {
                file.name = name;
            }
        } else if git {
            if line.starts_with("new file mode") {
                file.change = FileChangeType::New;
            }
            if line.starts_with("deleted file mode") {
                file.change = FileChangeType::Deleted;
            }
            if line.starts_with("similarity index") {
                file.change = if line.starts_with("similarity index 100%") {
                    FileChangeType::RenamePure
                } else {
                    FileChangeType::RenameChanged
                };
            }
            if let Some(rest) = line
                .strip_prefix("rename from ")
                .or_else(|| line.strip_prefix("copy from "))
            {
                file.prev_name = Some(decode_diff_file_name(rest, false));
            }
            if let Some(rest) = line
                .strip_prefix("rename to ")
                .or_else(|| line.strip_prefix("copy to "))
            {
                file.name = decode_diff_file_name(rest, false);
            }
        }
    }
    file
}

/// `^(---|\+\+\+)\s+([^\t\r\n]+)`.
fn filename_header(line: &str) -> Option<&str> {
    let rest = &line[3..];
    let trimmed = rest.trim_start_matches([' ', '\t', '\u{b}', '\u{c}']);
    if trimmed.len() == rest.len() {
        return None;
    }
    let name = trimmed.split(['\t', '\r', '\n']).next().unwrap_or_default();
    (!name.is_empty()).then_some(name)
}

/// `^diff --git ("a/(?:[^"\\]|\\.)*"|a/.+?) ("b/(?:[^"\\]|\\.)*"|b/.+?)$`.
fn git_header_names(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    let (a, b) = if rest.starts_with("\"a/") {
        let end = quoted_end(rest)?;
        (&rest[..end], rest[end..].strip_prefix(' ')?)
    } else {
        if !rest.starts_with("a/") {
            return None;
        }
        // The shortest a-part after which the rest is a valid b-part.
        let split = (3..rest.len())
            .filter(|i| rest.is_char_boundary(*i))
            .find(|i| {
                let b = &rest[*i..];
                b.strip_prefix(' ').is_some_and(|b| {
                    (b.starts_with("b/") && b.len() > 2)
                        || (b.starts_with("\"b/") && quoted_end(b) == Some(b.len()))
                })
            })?;
        (&rest[..split], &rest[split + 1..])
    };
    let b_valid = if b.starts_with('"') {
        b.starts_with("\"b/") && quoted_end(b) == Some(b.len())
    } else {
        b.starts_with("b/") && b.len() > 2
    };
    b_valid.then(|| {
        (
            decode_diff_file_name(a, true),
            decode_diff_file_name(b, true),
        )
    })
}

/// The byte just past a quoted name's closing quote.
fn quoted_end(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some(index + 1),
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    None
}

fn decode_diff_file_name(value: &str, strip_git_prefix: bool) -> String {
    let raw = value.trim();
    let name = if raw.starts_with('"') {
        parse_quoted_name(raw).unwrap_or_else(|| raw.to_owned())
    } else {
        raw.to_owned()
    };
    if strip_git_prefix && (name.starts_with("a/") || name.starts_with("b/")) {
        name[2..].to_owned()
    } else {
        name
    }
}

/// @pierre/diffs parseQuotedDiffFileName: C escapes and UTF-8 octal bytes.
fn parse_quoted_name(input: &str) -> Option<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut name = String::new();
    let mut index = 1;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            return Some(name);
        }
        if c != '\\' {
            name.push(c);
            index += 1;
            continue;
        }
        let named = match chars.get(index + 1) {
            Some('"') => Some('"'),
            Some('\\') => Some('\\'),
            Some('a') => Some('\u{7}'),
            Some('b') => Some('\u{8}'),
            Some('t') => Some('\t'),
            Some('n') => Some('\n'),
            Some('v') => Some('\u{b}'),
            Some('f') => Some('\u{c}'),
            Some('r') => Some('\r'),
            _ => None,
        };
        if let Some(named) = named {
            name.push(named);
            index += 2;
            continue;
        }
        let mut bytes = Vec::new();
        loop {
            let digit = |at: usize| chars.get(at).and_then(|c| c.to_digit(8));
            let (Some(a), Some(b), Some(c)) =
                (digit(index + 1), digit(index + 2), digit(index + 3))
            else {
                return None;
            };
            if a > 3 {
                return None;
            }
            bytes.push((a * 64 + b * 8 + c) as u8);
            index += 4;
            if !(chars.get(index) == Some(&'\\')
                && chars
                    .get(index + 1)
                    .is_some_and(|c| ('0'..='7').contains(c)))
            {
                break;
            }
        }
        name.push_str(&String::from_utf8_lossy(&bytes));
    }
    None
}

struct HunkHeader {
    deletion_start: u64,
    deletion_count: u64,
    addition_start: u64,
    addition_count: u64,
}

/// `parseHunkHeader`: `@@ -a[,b] +c[,d] @@`.
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let bytes = line.strip_prefix("@@ -")?.as_bytes();
    let mut index = 0;
    let number = |index: &mut usize| {
        let start = *index;
        let mut value = 0u64;
        while let Some(digit) = bytes.get(*index).filter(|b| b.is_ascii_digit()) {
            value = value
                .saturating_mul(10)
                .saturating_add(u64::from(digit - b'0'));
            *index += 1;
        }
        (*index > start).then_some(value)
    };
    let deletion_start = number(&mut index)?;
    let mut deletion_count = 1;
    if bytes.get(index) == Some(&b',') {
        index += 1;
        deletion_count = number(&mut index)?;
    }
    if bytes.get(index..index + 2) != Some(b" +") {
        return None;
    }
    index += 2;
    let addition_start = number(&mut index)?;
    let mut addition_count = 1;
    if bytes.get(index) == Some(&b',') {
        index += 1;
        addition_count = number(&mut index)?;
    }
    (bytes.get(index..index + 3) == Some(b" @@")).then_some(HunkHeader {
        deletion_start,
        deletion_count,
        addition_start,
        addition_count,
    })
}

fn hunk_body_line(line: &str) -> bool {
    matches!(line.as_bytes().first(), Some(b'+' | b'-' | b' '))
}

fn format_patch_version_separator(line: &str) -> bool {
    line.strip_prefix("--")
        .is_some_and(|rest| rest.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')))
}

fn lines_with_newlines(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        return vec![""];
    }
    contents.split_inclusive('\n').collect()
}

/// `splitAtLinePrefix` over `range` of `data`, as byte ranges.
fn split_at_line_prefix(data: &str, range: Range<usize>, prefix: &str) -> Vec<Range<usize>> {
    let contents = &data[range.clone()];
    let base = range.start;
    let find = |from: usize| {
        contents[from..]
            .find(&format!("\n{prefix}"))
            .map(|at| from + at + 1)
    };
    let first = if contents.starts_with(prefix) {
        Some(0)
    } else {
        find(0)
    };
    let Some(first) = first else {
        return vec![range];
    };
    let mut parts = Vec::new();
    if first > 0 {
        parts.push(base..base + first);
    }
    let mut start = first;
    while let Some(next) = find(start + 1) {
        parts.push(base + start..base + next);
        start = next;
    }
    parts.push(base + start..range.end);
    parts
}

fn next_line_start(contents: &str, start: usize) -> usize {
    contents[start..]
        .find('\n')
        .map_or(contents.len(), |at| start + at + 1)
}

fn unified_header_line_at(contents: &str, start: usize, prefix: &str) -> bool {
    let Some(rest) = contents.get(start..).and_then(|r| r.strip_prefix(prefix)) else {
        return false;
    };
    let mut chars = rest.chars();
    if !matches!(chars.next(), Some(' ' | '\t')) {
        return false;
    }
    chars
        .take_while(|c| !matches!(c, '\n' | '\r'))
        .any(|c| !matches!(c, ' ' | '\t'))
}

fn unified_file_header_at(contents: &str, start: usize) -> bool {
    unified_header_line_at(contents, start, "---")
        && unified_header_line_at(contents, next_line_start(contents, start), "+++")
}

/// `splitUnifiedDiffFiles`: file headers outside hunk bodies start a file.
fn split_unified_files(contents: &str) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    if contents.is_empty() {
        parts.push(0..0);
        return parts;
    }
    let (mut part_start, mut line_start) = (0, 0);
    let (mut deletions, mut additions) = (0u64, 0u64);
    let mut opened = false;
    while line_start < contents.len() {
        let next = next_line_start(contents, line_start);
        if deletions == 0 && additions == 0 {
            if unified_file_header_at(contents, line_start) {
                if line_start > part_start {
                    parts.push(part_start..line_start);
                }
                part_start = line_start;
                opened = true;
                line_start = next_line_start(contents, next);
                continue;
            }
            if opened
                && contents[line_start..].starts_with("@@ -")
                && let Some(header) = parse_hunk_header(&contents[line_start..next])
            {
                deletions = header.deletion_count;
                additions = header.addition_count;
            }
            line_start = next;
            continue;
        }
        match contents.as_bytes()[line_start] {
            b' ' => {
                deletions = deletions.saturating_sub(1);
                additions = additions.saturating_sub(1);
            }
            b'-' => deletions = deletions.saturating_sub(1),
            b'+' => additions = additions.saturating_sub(1),
            _ => {}
        }
        line_start = next;
    }
    parts.push(part_start..contents.len());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(patch: &str) -> Vec<PatchFile> {
        match renderable_patch(&Rc::from(patch)) {
            Some(RenderablePatch::Files(files)) => files.files.clone(),
            other => panic!("not files: {other:?}"),
        }
    }

    #[test]
    fn a_type_change_gives_each_block_its_own_identity() {
        let patch = [
            "diff --git a/AGENTS.md b/AGENTS.md",
            "deleted file mode 100644",
            "--- a/AGENTS.md",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-duplicated instructions",
            "diff --git a/AGENTS.md b/AGENTS.md",
            "new file mode 120000",
            "--- /dev/null",
            "+++ b/AGENTS.md",
            "@@ -0,0 +1 @@",
            "+CLAUDE.md",
        ]
        .join("\n");
        let parsed = files(&patch);
        assert_eq!(parsed[0].change, FileChangeType::Deleted);
        assert_eq!(parsed[1].change, FileChangeType::New);
        assert_ne!(parsed[0].identity_key(), parsed[1].identity_key());
        assert_eq!((parsed[0].additions, parsed[0].deletions), (0, 1));
        assert_eq!((parsed[1].additions, parsed[1].deletions), (1, 0));
    }

    #[test]
    fn identities_stay_stable_when_another_file_changes() {
        let patch = |second: &str| {
            [
                "diff --git a/unchanged.ts b/unchanged.ts",
                "--- a/unchanged.ts",
                "+++ b/unchanged.ts",
                "@@ -1 +1 @@",
                "-before",
                "+after",
                "diff --git a/changed.ts b/changed.ts",
                "--- a/changed.ts",
                "+++ b/changed.ts",
                "@@ -1 +1 @@",
                "-old",
                &format!("+{second}"),
            ]
            .join("\n")
        };
        let before = files(&patch("new"));
        let after = files(&patch("newer"));
        assert_eq!(before[0].identity_key(), after[0].identity_key());
        assert_eq!(before[1].identity_key(), after[1].identity_key());
        assert_eq!(
            PatchFiles::line_stat(&before),
            DiffLineStat {
                additions: 2,
                deletions: 2
            }
        );
    }

    #[test]
    fn reads_quoted_octal_names_and_quoted_renames() {
        let quoted = |written: &str| {
            [
                format!("diff --git \"a/{written}\" \"b/{written}\""),
                "index 1111111..2222222 100644".into(),
                format!("--- \"a/{written}\""),
                format!("+++ \"b/{written}\""),
                "@@ -1 +1 @@".into(),
                "-before".into(),
                "+after".into(),
                String::new(),
            ]
            .join("\n")
        };
        assert_eq!(
            files(&quoted("caf\\303\\251/r\\303\\251sum\\303\\251.ts"))[0].path(),
            "café/résumé.ts"
        );
        let rename = [
            "diff --git \"a/old\\tname.ts\" \"b/new\\tname.ts\"",
            "similarity index 90%",
            "rename from \"old\\tname.ts\"",
            "rename to \"new\\tname.ts\"",
            "",
        ]
        .join("\n");
        let file = &files(&rename)[0];
        assert_eq!(file.path(), "new\tname.ts");
        assert_eq!(file.previous_path(), "old\tname.ts");
        assert_eq!(file.change, FileChangeType::RenameChanged);
    }

    #[test]
    fn unified_patches_and_unsupported_text() {
        // `diff -u` names: no git prefixes to strip (with `a/` and `b/` @pierre/diffs keeps them,
        // and the differing names read as a rename).
        let unified = "--- x.txt\n+++ x.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n--- y.txt\n+++ y.txt\n@@ -1 +1 @@\n-1\n+2\n";
        let parsed = files(unified);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path(), "x.txt");
        assert_eq!(parsed[0].change, FileChangeType::Change);
        assert_eq!(parsed[1].range.start, unified.find("--- y.txt").unwrap());
        assert_eq!(
            renderable_patch(&Rc::from("not a patch")),
            Some(RenderablePatch::Raw(Rc::from("not a patch")))
        );
        assert_eq!(renderable_patch(&Rc::from("  \n")), None);
    }

    #[test]
    fn a_hunk_stops_at_its_declared_counts_and_ignores_no_newline_markers() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\ntrailing commit text\n";
        let file = &files(patch)[0];
        assert_eq!((file.additions, file.deletions, file.hunks), (1, 1, 1));
    }

    #[test]
    fn joining_shown_files_keeps_the_source_when_nothing_is_hidden() {
        let patch: Rc<str> = Rc::from(
            "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/src/a.test.ts b/src/a.test.ts\n@@ -1 +1 @@\n-a\n+b",
        );
        let Some(RenderablePatch::Files(parsed)) = renderable_patch(&patch) else {
            panic!("files");
        };
        assert!(Rc::ptr_eq(&parsed.joined(|_| true), &patch));
        let hidden = parsed.joined(|file| !file.path().contains(".test."));
        assert_eq!(
            &*hidden,
            "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1 @@\n-a\n+b\n"
        );
        assert_eq!(
            parsed.file_keys(true),
            vec![
                r#"["src/a.ts","src/a.ts"]:0"#.to_owned(),
                r#"["src/a.test.ts","src/a.test.ts"]:0"#.to_owned()
            ]
        );
    }
}
