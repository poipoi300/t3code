//! Port of apps/web/src/components/diffs/diffFileTree.logic.ts: the diff panel's file tree.
//!
//! The web hands paths to @pierre/trees' imperative model and patches it with batch updates;
//! NSH's `Tree` takes items, so this builds them: directories in first-seen order, empty
//! directory chains flattened into one row (`flattenEmptyDirectories`). Directory ids end in
//! `/` and file ids do not, so a file and a directory at one path never collide and the web's
//! zero-width aliases (`diffFileTreeModel`) are not needed.

use super::files::{FileChangeType, PatchFile};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Added,
    Deleted,
    Renamed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileTreeEntry {
    pub path: String,
    pub status: GitStatus,
}

fn git_status(change: FileChangeType) -> GitStatus {
    match change {
        FileChangeType::New => GitStatus::Added,
        FileChangeType::Deleted => GitStatus::Deleted,
        FileChangeType::RenamePure | FileChangeType::RenameChanged => GitStatus::Renamed,
        FileChangeType::Change => GitStatus::Modified,
    }
}

/// `diffFileTreeEntries`: one entry per path; a path with two different changes (a file
/// replaced by a symlink) is modified.
pub fn diff_file_tree_entries<'a>(
    files: impl IntoIterator<Item = &'a PatchFile>,
) -> Vec<DiffFileTreeEntry> {
    let mut entries: Vec<DiffFileTreeEntry> = Vec::new();
    let mut index = BTreeMap::<String, usize>::new();
    for file in files {
        let status = git_status(file.change);
        match index.get(file.path()) {
            Some(&at) => {
                if entries[at].status != status {
                    entries[at].status = GitStatus::Modified;
                }
            }
            None => {
                index.insert(file.path().to_owned(), entries.len());
                entries.push(DiffFileTreeEntry {
                    path: file.path().to_owned(),
                    status,
                });
            }
        }
    }
    entries
}

/// `collectDirectoryPaths`: every ancestor directory, each ending in `/`, in first-seen order.
pub fn collect_directory_paths(paths: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut directories = Vec::new();
    for path in paths {
        let mut directory = String::new();
        let segments: Vec<&str> = path.split('/').collect();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            directory.push_str(segment);
            directory.push('/');
            if seen.insert(directory.clone()) {
                directories.push(directory.clone());
            }
        }
    }
    directories
}

/// `diffFileTreePositions`: each file's index, and each directory's first file's index.
pub fn diff_file_tree_positions(paths: &[String]) -> BTreeMap<String, usize> {
    let mut positions = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        positions.insert(path.clone(), index);
        let segments: Vec<&str> = path.split('/').collect();
        let mut directory = String::new();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            directory.push_str(segment);
            directory.push('/');
            positions.entry(directory.clone()).or_insert(index);
        }
    }
    positions
}

/// `orderDiffFilesLikeTree`: a flat list in tree order, each directory contiguous.
pub fn order_like_tree<T: Clone>(items: &[T], path: impl Fn(&T) -> &str) -> Vec<T> {
    let paths: Vec<String> = items.iter().map(|item| path(item).to_owned()).collect();
    let positions = diff_file_tree_positions(&paths);
    let mut keyed: Vec<(Vec<usize>, &T)> = paths
        .iter()
        .zip(items)
        .map(|(path, item)| {
            let segments: Vec<&str> = path.split('/').collect();
            let mut directory = String::new();
            let mut key = Vec::new();
            for segment in &segments[..segments.len().saturating_sub(1)] {
                directory.push_str(segment);
                directory.push('/');
                key.push(positions[&directory]);
            }
            key.push(positions[path]);
            (key, item)
        })
        .collect();
    keyed.sort_by(|(left, _), (right, _)| left.cmp(right));
    keyed.into_iter().map(|(_, item)| item.clone()).collect()
}

/// One row of the tree: a directory (its id ends in `/`) or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTreeNode {
    pub id: String,
    /// The shown name: a file's basename, or a flattened chain such as `src/lib`.
    pub label: String,
    /// A file's status; `None` for a directory.
    pub status: Option<GitStatus>,
    pub children: Vec<DiffTreeNode>,
}

impl DiffTreeNode {
    pub fn is_directory(&self) -> bool {
        self.status.is_none()
    }
}

#[derive(Default)]
struct Building {
    directories: Vec<(String, Building)>,
    files: Vec<(usize, String, GitStatus)>,
    position: usize,
}

/// The tree's items: siblings ranked by first position (`compareDiffFileTreeEntries`), then
/// depth, then path; a directory whose only child is a directory is merged into it.
pub fn diff_file_tree(entries: &[DiffFileTreeEntry]) -> Vec<DiffTreeNode> {
    let mut root = Building::default();
    for (index, entry) in entries.iter().enumerate() {
        let segments: Vec<&str> = entry.path.split('/').collect();
        let (name, directories) = segments.split_last().unwrap_or((&"", &[]));
        let mut node = &mut root;
        let mut prefix = String::new();
        for segment in directories {
            prefix.push_str(segment);
            prefix.push('/');
            let at = match node.directories.iter().position(|(id, _)| id == &prefix) {
                Some(at) => at,
                None => {
                    node.directories.push((
                        prefix.clone(),
                        Building {
                            position: index,
                            ..Default::default()
                        },
                    ));
                    node.directories.len() - 1
                }
            };
            node = &mut node.directories[at].1;
        }
        let _ = name;
        node.files.push((index, entry.path.clone(), entry.status));
    }
    finish(root)
}

fn basename(id: &str) -> &str {
    id.trim_end_matches('/').rsplit('/').next().unwrap_or(id)
}

fn finish(node: Building) -> Vec<DiffTreeNode> {
    let mut rows: Vec<(usize, u8, String, DiffTreeNode)> = Vec::new();
    for (id, directory) in node.directories {
        let position = directory.position;
        let mut label = basename(&id).to_owned();
        let mut id = id;
        let mut directory = directory;
        while directory.files.is_empty() && directory.directories.len() == 1 {
            let (child_id, child) = directory.directories.pop().expect("one child");
            label = format!("{label}/{}", basename(&child_id));
            id = child_id;
            directory = child;
        }
        let children = finish(directory);
        rows.push((
            position,
            0,
            id.clone(),
            DiffTreeNode {
                id,
                label,
                status: None,
                children,
            },
        ));
    }
    for (position, path, status) in node.files {
        rows.push((
            position,
            1,
            path.clone(),
            DiffTreeNode {
                label: basename(&path).to_owned(),
                id: path,
                status: Some(status),
                children: Vec::new(),
            },
        ));
    }
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| a.2.cmp(&b.2)));
    rows.into_iter().map(|(_, _, _, node)| node).collect()
}

/// Every directory id under `nodes`, for "Expand all folders".
pub fn directory_ids(nodes: &[DiffTreeNode]) -> Vec<String> {
    let mut ids = Vec::new();
    for node in nodes {
        if node.is_directory() {
            ids.push(node.id.clone());
            ids.extend(directory_ids(&node.children));
        }
    }
    ids
}

/// The ancestor directory ids a file shows under, for a reveal.
pub fn ancestor_ids(nodes: &[DiffTreeNode], file: &str) -> Vec<String> {
    for node in nodes {
        if node.is_directory() && file.starts_with(&node.id) {
            let mut ids = vec![node.id.clone()];
            ids.extend(ancestor_ids(&node.children, file));
            return ids;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::diff_panel::files::PatchFile;

    fn file(change: FileChangeType, name: &str) -> PatchFile {
        PatchFile {
            name: name.into(),
            prev_name: None,
            change,
            additions: 0,
            deletions: 0,
            hunks: 0,
            range: 0..0,
            hunk_lines: Vec::new(),
        }
    }

    #[test]
    fn a_file_to_symlink_change_folds_into_one_modified_entry() {
        let entries = diff_file_tree_entries(&[
            file(FileChangeType::Change, "CLAUDE.md"),
            file(FileChangeType::Deleted, "AGENTS.md"),
            file(FileChangeType::New, "AGENTS.md"),
            file(FileChangeType::New, "docs/new.md"),
        ]);
        assert_eq!(
            entries,
            vec![
                DiffFileTreeEntry {
                    path: "CLAUDE.md".into(),
                    status: GitStatus::Modified
                },
                DiffFileTreeEntry {
                    path: "AGENTS.md".into(),
                    status: GitStatus::Modified
                },
                DiffFileTreeEntry {
                    path: "docs/new.md".into(),
                    status: GitStatus::Added
                },
            ]
        );
    }

    #[test]
    fn a_test_listed_last_joins_its_directory_in_first_seen_order() {
        let paths = [
            "server/device/Host.ts",
            "web/settings.ts",
            "server/ws.ts",
            "server/device/Host.test.ts",
            "web/settings.test.ts",
        ];
        assert_eq!(
            order_like_tree(&paths, |p| p),
            vec![
                "server/device/Host.ts",
                "server/device/Host.test.ts",
                "server/ws.ts",
                "web/settings.ts",
                "web/settings.test.ts",
            ]
        );
    }

    #[test]
    fn the_tree_flattens_empty_chains_and_keeps_first_seen_order() {
        let entries: Vec<_> = ["src/lib/deep/a.ts", "README.md", "src/b.ts", "docs/x/y.md"]
            .iter()
            .map(|path| DiffFileTreeEntry {
                path: (*path).into(),
                status: GitStatus::Modified,
            })
            .collect();
        let tree = diff_file_tree(&entries);
        let labels: Vec<_> = tree.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, vec!["src", "README.md", "docs/x"]);
        assert_eq!(tree[0].children[0].label, "lib/deep");
        assert_eq!(tree[0].children[0].id, "src/lib/deep/");
        assert_eq!(tree[0].children[1].id, "src/b.ts");
        assert_eq!(
            directory_ids(&tree),
            vec!["src/", "src/lib/deep/", "docs/x/"]
        );
        assert_eq!(
            ancestor_ids(&tree, "src/lib/deep/a.ts"),
            vec!["src/", "src/lib/deep/"]
        );
        assert_eq!(
            collect_directory_paths(&["a/b/c.ts".into()]),
            vec!["a/", "a/b/"]
        );
    }
}
