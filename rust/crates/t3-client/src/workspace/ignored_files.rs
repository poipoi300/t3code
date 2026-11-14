use t3_contracts::{
    environment::ExecutionEnvironmentCapabilities,
    project::{ProjectIgnoredFile, ProjectListIgnoredFilesInput},
};

pub const COPIED_FILES_MAX_COUNT: usize = 200;
pub const COPYABLE_FILE_MAX_BYTES: u64 = 1024 * 1024;

pub fn request(
    cwd: &str,
    query: &str,
    capabilities: &ExecutionEnvironmentCapabilities,
) -> Option<ProjectListIgnoredFilesInput> {
    if capabilities.ignored_file_listing != Some(true) {
        return None;
    }
    let query = crate::view_logic::JsTrim::js_trim(query);
    let mut length = 0;
    let query = (capabilities.ignored_file_search == Some(true) && !query.is_empty()).then(|| {
        t3_contracts::base_schemas::TrimmedString(
            query
                .chars()
                .take_while(|character| {
                    length += character.len_utf16();
                    length <= 256
                })
                .collect(),
        )
    });
    Some(ProjectListIgnoredFilesInput {
        cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(cwd.into()),
        query,
    })
}

pub fn is_default_copied_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, ".env" | ".envrc" | ".dev.vars")
        || name
            .strip_prefix(".env.")
            .is_some_and(|suffix| !suffix.is_empty())
}

#[derive(Default)]
pub struct FileSelection(Vec<ProjectIgnoredFile>);
impl FileSelection {
    pub fn files(&self) -> &[ProjectIgnoredFile] {
        &self.0
    }
    pub fn defaults(files: &[ProjectIgnoredFile]) -> Self {
        let mut selected = Self::default();
        for file in files
            .iter()
            .filter(|file| is_default_copied_file(&file.path.0))
        {
            selected.select(file, true);
        }
        selected
    }
    pub fn bytes(&self) -> u64 {
        self.0.iter().map(|file| file.byte_length).sum()
    }
    pub fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|file| file.path.0 == path)
    }
    pub fn select(&mut self, file: &ProjectIgnoredFile, checked: bool) {
        if !checked {
            self.0.retain(|selected| selected.path != file.path);
            return;
        }
        if self.0.len() >= COPIED_FILES_MAX_COUNT || file.byte_length > COPYABLE_FILE_MAX_BYTES {
            return;
        }
        if let Some(selected) = self
            .0
            .iter_mut()
            .find(|selected| selected.path == file.path)
        {
            *selected = file.clone();
        } else {
            self.0.push(file.clone());
        }
    }
    pub fn toggle_shown(&mut self, files: &[ProjectIgnoredFile]) {
        let copyable = files
            .iter()
            .filter(|file| file.byte_length <= COPYABLE_FILE_MAX_BYTES)
            .collect::<Vec<_>>();
        let all = copyable.iter().all(|file| self.contains(&file.path.0));
        for file in copyable {
            self.select(file, !all);
        }
    }
}
