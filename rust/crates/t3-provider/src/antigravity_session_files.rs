use std::{
    fs,
    path::{Path, PathBuf},
};
fn regular(path: &Path, directory: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink()
            && if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
    })
}
pub fn remove(profile: &Path, session: Option<&str>, cwd: &Path) {
    let Some(session) = session.filter(|session| {
        uuid::Uuid::parse_str(session).is_ok_and(|id| {
            id.get_version_num() == 4 && id.hyphenated().to_string().eq_ignore_ascii_case(session)
        })
    }) else {
        return;
    };
    let Ok(profile) = fs::canonicalize(profile) else {
        return;
    };
    let acp = profile.join("antigravity-acp");
    let conversations = acp.join("conversations");
    if !regular(&acp, true) || !regular(&conversations, true) {
        return;
    }
    let metadata = conversations.join(format!("{session}.meta"));
    if !regular(&metadata, false)
        || !fs::metadata(&metadata).is_ok_and(|metadata| metadata.len() <= 65536)
    {
        return;
    }
    let matches = fs::read(&metadata)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["cwd"].as_str().map(PathBuf::from))
        .is_some_and(|stored| stored == cwd);
    if !matches {
        return;
    }
    for suffix in [".db", ".db-wal", ".db-shm", ".db-journal", ".meta"] {
        let path = conversations.join(format!("{session}{suffix}"));
        if regular(&path, false) {
            let _ = fs::remove_file(path);
        }
    }
    let brain = acp.join("brain");
    let session = brain.join(session);
    if regular(&brain, true)
        && regular(&session, true)
        && fs::canonicalize(&session)
            .is_ok_and(|resolved| resolved.parent() == Some(brain.as_path()))
    {
        let _ = fs::remove_dir_all(session);
    }
}

pub struct RetiredTemps {
    parent: PathBuf,
    paths: Vec<PathBuf>,
}
impl RetiredTemps {
    pub fn remove(self) {
        for path in self.paths {
            if regular(&path, true)
                && fs::canonicalize(&path)
                    .is_ok_and(|resolved| resolved.parent() == Some(self.parent.as_path()))
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}
/// Retire private runtime files once, before this instance can launch a child.
pub fn retire(temp_directory: &Path) -> RetiredTemps {
    let empty = || RetiredTemps {
        parent: PathBuf::new(),
        paths: vec![],
    };
    let Some(parent) = temp_directory
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
    else {
        return empty();
    };
    let Some(name) = temp_directory.file_name().and_then(|name| name.to_str()) else {
        return empty();
    };
    let temp_directory = parent.join(name);
    let prefix = format!("{name}.old-");
    if regular(&temp_directory, true) {
        let moved = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir_in(&parent)
            .ok()
            .is_some_and(|retired| {
                if fs::rename(&temp_directory, retired.path().join("runtime")).is_ok() {
                    let _ = retired.keep();
                    true
                } else {
                    false
                }
            });
        if !moved
            && fs::canonicalize(&temp_directory)
                .is_ok_and(|resolved| resolved.parent() == Some(parent.as_path()))
        {
            let _ = fs::remove_dir_all(&temp_directory);
        }
    }
    let paths = fs::read_dir(&parent)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .map(|entry| entry.path())
        .collect();
    RetiredTemps { parent, paths }
}
