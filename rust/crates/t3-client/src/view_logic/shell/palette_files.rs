//! ProjectFilePicker.logic.ts and ProjectContentSearchDialog.tsx presentation, without UI code.

use crate::view_logic::{JsTrim, is_js_whitespace};
use t3_contracts::project::{ProjectContentMatch, ProjectEntry, ProjectEntryKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTarget {
    pub thread: crate::types::ThreadKey,
    pub cwd: String,
    pub project_name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSearchView {
    pub entries: std::rc::Rc<Vec<ProjectEntry>>,
    pub matched_query: String,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentSearchView {
    pub matches: std::rc::Rc<Vec<ProjectContentMatch>>,
    pub can_read_files: bool,
    pub checking_access: bool,
    pub has_query: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub invalid_regex: bool,
    pub truncated: bool,
}

/// The composed query service owns access, normalization, debounce, capability and stale replies.
pub trait PaletteSearchClient {
    fn watch_target(&self) -> futures::stream::LocalBoxStream<'static, Option<SearchTarget>>;
    fn files(
        &self,
        target: SearchTarget,
        query: String,
    ) -> futures::stream::LocalBoxStream<'static, FileSearchView>;
    fn contents(
        &self,
        target: SearchTarget,
        query: String,
        options: SearchOptions,
    ) -> futures::stream::LocalBoxStream<'static, ContentSearchView>;
    /// Recheck the destination session's filesystem scope at activation.
    fn can_open(&self, target: &SearchTarget) -> bool;
}

pub const FILE_PICKER_LIMIT: usize = 200;
pub const VISIBLE_MATCH_WINDOW: usize = 100;

pub fn has_query(query: &str) -> bool {
    query.chars().any(|character| !is_js_whitespace(character))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatch {
    pub name: String,
    pub name_match_indices: Vec<usize>,
    pub path: String,
    pub path_match_indices: Vec<usize>,
}

fn match_indices(value: &str, query: &[u16]) -> Vec<usize> {
    if query.is_empty() {
        return vec![];
    }
    let mut indices = vec![];
    for (index, unit) in value.to_lowercase().encode_utf16().enumerate() {
        if unit == query[indices.len()] {
            indices.push(index);
            if indices.len() == query.len() {
                return indices;
            }
        }
    }
    vec![]
}

/// Server order and inclusion are authoritative, even when the local highlighter finds no match.
pub fn file_picker_matches(
    entries: &[ProjectEntry],
    raw_query: &str,
    limit: usize,
) -> Vec<FileMatch> {
    let query: Vec<u16> = raw_query
        .js_trim()
        .trim_start_matches(['@', '.', '/'])
        .to_lowercase()
        .chars()
        .filter(|ch| !is_js_whitespace(*ch))
        .collect::<String>()
        .encode_utf16()
        .collect();
    entries
        .iter()
        .filter(|entry| entry.kind == ProjectEntryKind::File)
        .take(limit)
        .map(|entry| {
            let path = entry.path.0.clone();
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            FileMatch {
                name_match_indices: match_indices(&name, &query),
                path_match_indices: match_indices(&path, &query),
                name,
                path,
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchGroup {
    pub path: String,
    pub result_indices: Vec<usize>,
}

/// Map insertion order matches JavaScript; an interleaved later match joins its first file group.
pub fn group_matches(matches: &[ProjectContentMatch], visible_count: usize) -> Vec<MatchGroup> {
    let mut groups: Vec<MatchGroup> = vec![];
    let mut by_path = std::collections::BTreeMap::new();
    for (index, item) in matches.iter().take(visible_count).enumerate() {
        let at = *by_path.entry(&item.path.0).or_insert_with(|| {
            groups.push(MatchGroup {
                path: item.path.0.clone(),
                result_indices: vec![],
            });
            groups.len() - 1
        });
        groups[at].result_indices.push(index);
    }
    groups
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPart {
    pub text: String,
    pub highlighted: bool,
}

/// Range offsets are UTF-16 (the server/JavaScript contract), never Rust byte offsets.
pub fn text_parts(value: &str, ranges: &[(usize, usize)]) -> Vec<TextPart> {
    let mut parts: Vec<TextPart> = vec![];
    let mut offset = 0;
    for character in value.chars() {
        let end = offset + character.len_utf16();
        let highlighted = ranges
            .iter()
            .any(|(start, stop)| stop > start && *start < end && *stop > offset);
        if let Some(part) = parts
            .last_mut()
            .filter(|part| part.highlighted == highlighted)
        {
            part.text.push(character);
        } else {
            parts.push(TextPart {
                text: character.into(),
                highlighted,
            });
        }
        offset = end;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::base_schemas::TrimmedNonEmptyString;

    fn file(path: &str) -> ProjectEntry {
        ProjectEntry {
            path: TrimmedNonEmptyString(path.into()),
            kind: ProjectEntryKind::File,
            ignored: None,
        }
    }

    #[test]
    fn ordered_subsequence_highlights_preserve_server_order_and_unmatched_entries() {
        let entries = [
            file("src/TestFlags.tsx"),
            file("src/SubtestFlow.tsx"),
            file("src/useSubtestFlags.ts"),
            file("src/useSubtestFlags/useTabActivity.ts"),
        ];
        let matches = file_picker_matches(&entries, "testf", FILE_PICKER_LIMIT);
        assert_eq!(
            matches
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            [
                "TestFlags.tsx",
                "SubtestFlow.tsx",
                "useSubtestFlags.ts",
                "useTabActivity.ts"
            ]
        );
        let matches = file_picker_matches(&entries, "tsfl", FILE_PICKER_LIMIT);
        assert_eq!(matches[0].name_match_indices, [0, 2, 4, 5]);
        assert_eq!(matches[0].path_match_indices, [4, 6, 8, 9]);
        assert!(matches[3].name_match_indices.is_empty());
    }

    #[test]
    fn normalizes_path_prefixes_and_filters_directories_before_limiting() {
        let mut directory = file("src");
        directory.kind = ProjectEntryKind::Directory;
        let entries = [directory, file("src/index.ts"), file("src/second.ts")];
        assert_eq!(
            file_picker_matches(&entries, "@/src", 1)[0].path_match_indices,
            [0, 1, 2]
        );
        assert!(file_picker_matches(&entries, "src", 0).is_empty());
        assert_eq!(
            file_picker_matches(&entries, "zz", 1)[0].path,
            "src/index.ts"
        );
    }

    #[test]
    fn groups_interleaved_matches_in_first_seen_file_order_with_global_indices() {
        let matches = ["b.ts", "a.ts", "b.ts", "c.ts"].map(|path| ProjectContentMatch {
            path: TrimmedNonEmptyString(path.into()),
            line_number: 1,
            line_content: "line".into(),
            match_ranges: vec![],
        });
        assert_eq!(
            group_matches(&matches, 3),
            [
                MatchGroup {
                    path: "b.ts".into(),
                    result_indices: vec![0, 2]
                },
                MatchGroup {
                    path: "a.ts".into(),
                    result_indices: vec![1]
                }
            ]
        );
    }

    #[test]
    fn utf16_highlights_preserve_literal_content_and_merge_overlapping_ranges() {
        let parts = text_parts("a😀計画.ts", &[(1, 3), (3, 5), (99, 100), (7, 6)]);
        assert_eq!(
            parts,
            [
                TextPart {
                    text: "a".into(),
                    highlighted: false
                },
                TextPart {
                    text: "😀計画".into(),
                    highlighted: true
                },
                TextPart {
                    text: ".ts".into(),
                    highlighted: false
                }
            ]
        );
        assert_eq!(
            parts
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "a😀計画.ts"
        );
    }
}
