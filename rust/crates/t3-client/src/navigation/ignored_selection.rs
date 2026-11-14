//! AddProjectToMachineDialog's ignored-file query and cross-query selection session.
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::RpcEndpoint,
    services::resources::{Resource, ResourcePolicy},
    types::Generation,
    workspace::{
        ProjectKey,
        ignored_files::{self, COPIED_FILES_MAX_COUNT, COPYABLE_FILE_MAX_BYTES, FileSelection},
    },
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    project::{ProjectIgnoredFile, ProjectListIgnoredFilesResult},
    rpc::WsProjectsListIgnoredFilesRpc,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub file: ProjectIgnoredFile,
    pub selected: bool,
    pub disabled: bool,
    pub size: String,
    pub too_large: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub query: String,
    pub can_search: bool,
    pub show_search: bool,
    pub loading: bool,
    pub searching: bool,
    pub truncated: bool,
    pub all_shown_selected: bool,
    pub selected_count: usize,
    pub total_count: usize,
    pub selected_bytes: u64,
    pub selected_size: String,
    pub error: Option<String>,
    pub files: Vec<Choice>,
}
struct State {
    query: String,
    selected: Option<FileSelection>,
    results: BTreeMap<String, Rc<ProjectListIgnoredFilesResult>>,
    errors: BTreeMap<String, String>,
    generation: Option<Generation>,
}
impl State {
    fn selection(&self) -> FileSelection {
        let mut result = FileSelection::default();
        if let Some(selected) = &self.selected {
            for file in selected.files() {
                result.select(file, true);
            }
        } else if let Some(all) = self.results.get("") {
            return FileSelection::defaults(&all.files);
        }
        result
    }
}
#[derive(Clone)]
pub struct IgnoredFilesSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    platform: P,
    project: ProjectKey,
    state: Rc<RefCell<State>>,
    changed: Resource<()>,
}
impl<P: Platform + Clone + 'static> IgnoredFilesSession<P> {
    pub fn new(runtime: ClientRuntime<P>, platform: P, project: ProjectKey) -> Self {
        Self {
            runtime,
            platform,
            project,
            state: Rc::new(RefCell::new(State {
                query: String::new(),
                selected: None,
                results: BTreeMap::new(),
                errors: BTreeMap::new(),
                generation: None,
            })),
            changed: Resource::new(ResourcePolicy::default(), Generation::default()),
        }
    }
    pub fn set_query(&self, query: String) -> Result<(), ClientError> {
        let mut length = 0;
        let query = query
            .chars()
            .take_while(|c| {
                length += c.len_utf16();
                length <= 256
            })
            .collect();
        self.state.borrow_mut().query = query;
        self.changed.invalidate();
        Ok(())
    }
    fn target(
        &self,
    ) -> Result<
        (
            String,
            t3_contracts::environment::ExecutionEnvironmentCapabilities,
            Generation,
        ),
        ClientError,
    > {
        let snapshot = self.runtime.store().snapshot();
        let state = snapshot
            .environments
            .get(&self.project.environment)
            .ok_or(ClientError::Disconnected)?;
        let config = state.config.as_ref().ok_or(ClientError::Disconnected)?;
        let project = state
            .shell
            .snapshot
            .as_ref()
            .and_then(|s| s.projects.iter().find(|p| p.id == self.project.project))
            .ok_or(ClientError::Cancelled)?;
        Ok((
            project.workspace_root.0.clone(),
            config.environment.capabilities.clone(),
            state.session,
        ))
    }
    fn query_key(&self) -> String {
        self.target()
            .ok()
            .and_then(|(cwd, capabilities, _)| {
                ignored_files::request(&cwd, &self.state.borrow().query, &capabilities)
            })
            .and_then(|request| request.query.map(|query| query.0))
            .unwrap_or_default()
    }
    pub fn snapshot(&self) -> Rc<View> {
        let capabilities = self.target().ok().map(|(_, capabilities, _)| capabilities);
        let can_list = capabilities
            .as_ref()
            .is_some_and(|c| c.ignored_file_listing == Some(true));
        let can_search = can_list
            && capabilities
                .as_ref()
                .is_some_and(|c| c.ignored_file_search == Some(true));
        let key = self.query_key();
        let state = self.state.borrow();
        let result = state.results.get(&key);
        let all = state.results.get("");
        let selected = state.selection();
        let limited = selected.files().len() >= COPIED_FILES_MAX_COUNT;
        let files = result.map_or(&[][..], |result| result.files.as_slice());
        let error = state
            .errors
            .get(&key)
            .cloned()
            .or_else(|| self.target().err().map(|e| e.to_string()));
        Rc::new(View {
            query: state.query.clone(),
            can_search,
            show_search: can_search
                && (all.is_some_and(|r| r.truncated) || !state.query.is_empty()),
            loading: can_list && result.is_none() && error.is_none(),
            searching: !key.is_empty(),
            truncated: result.is_some_and(|r| r.truncated),
            all_shown_selected: files
                .iter()
                .filter(|file| file.byte_length <= COPYABLE_FILE_MAX_BYTES)
                .all(|file| selected.contains(&file.path.0)),
            selected_count: selected.files().len(),
            total_count: all.map_or(0, |r| r.files.len()),
            selected_bytes: selected.bytes(),
            selected_size: format_bytes(selected.bytes()),
            error,
            files: files
                .iter()
                .map(|file| Choice {
                    file: file.clone(),
                    selected: selected.contains(&file.path.0),
                    disabled: file.byte_length > COPYABLE_FILE_MAX_BYTES
                        || limited && !selected.contains(&file.path.0),
                    size: format_bytes(file.byte_length),
                    too_large: file.byte_length > COPYABLE_FILE_MAX_BYTES,
                })
                .collect(),
        })
    }
    pub fn selected(&self) -> Vec<ProjectIgnoredFile> {
        self.state.borrow().selection().files().to_vec()
    }
    pub fn toggle_file(&self, path: &str, checked: bool) -> Result<(), ClientError> {
        let view = self.snapshot();
        let choice = view
            .files
            .iter()
            .find(|file| file.file.path.0 == path)
            .filter(|file| !file.disabled || !checked)
            .ok_or(ClientError::Cancelled)?;
        let mut state = self.state.borrow_mut();
        let mut selected = state.selection();
        selected.select(&choice.file, checked);
        state.selected = Some(selected);
        drop(state);
        self.changed.invalidate();
        Ok(())
    }
    pub fn toggle_shown(&self) -> Result<(), ClientError> {
        let key = self.query_key();
        let mut state = self.state.borrow_mut();
        let files = state
            .results
            .get(&key)
            .ok_or(ClientError::Cancelled)?
            .files
            .clone();
        let mut selected = state.selection();
        selected.toggle_shown(&files);
        state.selected = Some(selected);
        drop(state);
        self.changed.invalidate();
        Ok(())
    }
    async fn refresh(&self) -> Result<(), ClientError> {
        let (cwd, capabilities, generation) = self.target()?;
        {
            let mut state = self.state.borrow_mut();
            if state.generation != Some(generation) {
                state.results.clear();
                state.errors.clear();
                state.generation = Some(generation);
            }
        }
        if capabilities.ignored_file_listing != Some(true) {
            return Ok(());
        }
        let key = self.query_key();
        for query in [String::new(), key.clone()] {
            if self.state.borrow().results.contains_key(&query)
                || self.state.borrow().errors.contains_key(&query)
            {
                continue;
            }
            if !query.is_empty() {
                self.platform.sleep(120).await;
                if self.query_key() != key {
                    return Err(ClientError::Cancelled);
                }
            }
            let input = ignored_files::request(&cwd, &query, &capabilities)
                .ok_or(ClientError::Cancelled)?;
            let endpoint = self
                .runtime
                .client(&self.project.environment)
                .and_then(|c| c.session())
                .ok_or(ClientError::Disconnected)?;
            let result = if endpoint.available::<WsProjectsListIgnoredFilesRpc>(&input) {
                endpoint.call::<WsProjectsListIgnoredFilesRpc>(input).await
            } else {
                Err(ClientError::PermissionDenied {
                    method: "projects.listIgnoredFiles".into(),
                })
            };
            if self.target()?.2 != generation {
                return Err(ClientError::Cancelled);
            }
            match result {
                Ok(result) => {
                    self.state
                        .borrow_mut()
                        .results
                        .insert(query, Rc::new(result));
                }
                Err(error) => {
                    self.state
                        .borrow_mut()
                        .errors
                        .insert(query, error.to_string());
                }
            }
        }
        Ok(())
    }
    pub fn watch(&self) -> LocalBoxStream<'static, Rc<View>> {
        let changes = futures::stream::select(
            self.runtime.store().subscribe().map(|_| ()),
            self.changed
                .observe(self.platform.monotonic_ms())
                .map(|_| ()),
        )
        .boxed_local();
        let this = self.clone();
        futures::stream::unfold(
            (this, changes, 0u8),
            |(this, mut changes, phase)| async move {
                if phase == 0 {
                    let view = this.snapshot();
                    return Some((view, (this, changes, 1)));
                }
                if phase == 2 {
                    changes.next().await?;
                }
                loop {
                    let outcome = {
                        let refresh = this.refresh();
                        futures::pin_mut!(refresh);
                        match futures::future::select(refresh, changes.next()).await {
                            futures::future::Either::Left((result, _)) => Some(Some(result)),
                            futures::future::Either::Right((Some(_), _)) => Some(None),
                            futures::future::Either::Right((None, _)) => None,
                        }
                    };
                    match outcome {
                        Some(Some(result)) => {
                            if let Err(error) = result
                                && !matches!(error, ClientError::Cancelled)
                            {
                                let key = this.query_key();
                                this.state
                                    .borrow_mut()
                                    .errors
                                    .insert(key, error.to_string());
                            }
                            let view = this.snapshot();
                            return Some((view, (this, changes, 2)));
                        }
                        Some(None) => {}
                        None => return None,
                    }
                }
            },
        )
        .boxed_local()
    }
}
pub fn format_bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut index = 0;
    loop {
        let divisor = 1024u128.pow(index as u32 + 1);
        let value = u128::from(value);
        let precision = if value >= 100 * divisor {
            0usize
        } else if value >= 10 * divisor {
            1
        } else {
            2
        };
        let scale = 10u128.pow(precision as u32);
        let rounded = (value * scale * 2 + divisor) / (divisor * 2);
        if rounded >= 1024 * scale && index < units.len() - 1 {
            index += 1;
            continue;
        }
        let number = if precision == 0 {
            rounded.to_string()
        } else {
            format!("{}.{:0precision$}", rounded / scale, rounded % scale)
        };
        return format!("{number} {}", units[index]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_byte_format_rounds_half_up_and_changes_units_after_rounding() {
        for (value, expected) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.00 KB"),
            (1152, "1.13 KB"),
            (1048166, "1.00 MB"),
            (1048576, "1.00 MB"),
        ] {
            assert_eq!(super::format_bytes(value), expected);
        }
    }
}
