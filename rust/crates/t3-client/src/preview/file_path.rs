use crate::view_logic::session::links::{
    format_file_path_position, is_windows_absolute, split_file_path_position,
};

fn strip_drive(path: &str) -> &str {
    if path.starts_with('/') && is_windows_absolute(&path[1..]) {
        &path[1..]
    } else {
        path
    }
}

pub fn format_workspace_relative_path(path: &str, workspace_root: Option<&str>) -> String {
    let mut position = split_file_path_position(path, "");
    let normalized = position.path.replace('\\', "/");
    let path = strip_drive(&normalized);
    let mut display = path.to_owned();
    if let Some(root) = workspace_root.filter(|root| !root.is_empty()) {
        let normalized_root = root.trim_end_matches(['/', '\\']).replace('\\', "/");
        let root = strip_drive(&normalized_root);
        let label = root.rsplit('/').next().unwrap_or(root);
        let windows = is_windows_absolute(root);
        let compare = if windows {
            path.to_lowercase()
        } else {
            path.into()
        };
        let compare_root = if windows {
            root.to_lowercase()
        } else {
            root.into()
        };
        let compare_label = if windows {
            label.to_lowercase()
        } else {
            label.into()
        };
        if compare == compare_root {
            display = label.into();
        } else if compare.starts_with(&format!("{compare_root}/")) {
            display = format!("{label}/{}", &path[root.len() + 1..]);
        } else if !path.starts_with('/') && !is_windows_absolute(path) {
            let relative = path
                .strip_prefix("./")
                .map_or(path, |path| path.trim_start_matches('/'))
                .trim_start_matches('/');
            display = if compare.starts_with(&format!("{compare_label}/")) {
                path.into()
            } else {
                format!("{label}/{relative}")
            };
        }
    }
    position.path = display;
    format_file_path_position(&position)
}
