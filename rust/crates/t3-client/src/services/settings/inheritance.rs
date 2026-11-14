//! Port of apps/web/src/components/settings/SettingInheritance.tsx (layers, value formatting,
//! chains grouped by resolution) and of `SettingsRow`'s inheritance state, summary, project
//! overrides and administrator lock (settingsLayout.tsx), plus `isManagedSettingPath`
//! (packages/contracts/src/managedPolicy.ts).
use super::{
    scope::{ScopeEnvironment, ScopeGroup, SettingsScopeSearch, settings_scope_environment_labels},
    scoped::{
        ProjectOverrideEntry, ProjectSettingSource, ScopedSettingSource, ScopedSettingsTarget,
        SettingsText, is_project_scoped_setting_key, list_project_overrides,
        scoped_settings_are_mixed, scoped_settings_source,
    },
    service::SettingsScopeView,
};
use serde_json::Value;
use t3_contracts::managed_policy::ManagedPolicySummary;

/// What a value or label shows: an English key (with arguments) or content (a model, a name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Text(SettingsText),
    Content(String),
}

impl Shown {
    fn key(key: &'static str) -> Self {
        Self::Text(SettingsText::key(key))
    }
    pub fn english(&self) -> String {
        match self {
            Self::Text(text) => text.english(),
            Self::Content(content) => content.clone(),
        }
    }
}

/// `formatValue`.
pub fn format_value(key: &str, value: &Value) -> Shown {
    match value {
        Value::Null => Shown::key(match key {
            "pullRequestMergeMethod" => "Last selected",
            "sidebarAutoSettleAfterDays" => "Never",
            "defaultModelSelection" => "Automatic",
            "sourceControlWriterModelSelection" => "Text generation model",
            "reviewGuideModelSelection" => "Source control writer model",
            "defaultThreadEnvMode" | "worktreeSubmodules" => "Inherit",
            _ => "Not set",
        }),
        Value::Bool(on) => Shown::key(if *on { "On" } else { "Off" }),
        Value::Number(number) => {
            if key == "sidebarAutoSettleAfterDays" {
                let count = number.to_string();
                Shown::Text(SettingsText::with(
                    if number.as_f64() == Some(1.0) { "{count} day" } else { "{count} days" },
                    "count",
                    count,
                ))
            } else {
                Shown::Content(number.to_string())
            }
        }
        Value::String(text) => match (key, text.as_str()) {
            ("defaultThreadEnvMode", "worktree") => Shown::key("New worktree"),
            ("defaultThreadEnvMode", "local") => Shown::key("Current checkout"),
            ("worktreeSubmodules", "recursive") => Shown::key("Recursive"),
            ("worktreeSubmodules", "top-level") => Shown::key("Top level only"),
            ("worktreeSubmodules", "none") => Shown::key("Skip"),
            ("pullRequestMergeMethod", "merge") => Shown::key("Merge"),
            ("pullRequestMergeMethod", "squash") => Shown::key("Squash and merge"),
            ("pullRequestMergeMethod", "rebase") => Shown::key("Rebase and merge"),
            ("pullRequestMergeMethod", "fast-forward") => Shown::key("Fast-forward merge"),
            (_, "") => Shown::key("Empty"),
            _ => Shown::Content(text.clone()),
        },
        Value::Array(items) => Shown::Text(SettingsText::with(
            if items.len() == 1 { "{count} item" } else { "{count} items" },
            "count",
            items.len().to_string(),
        )),
        Value::Object(fields) => {
            if key == "backgroundActivity"
                && let Some(profile) = fields.get("profile").and_then(Value::as_str)
            {
                return match profile {
                    "balanced" => Shown::key("Balanced"),
                    "performance" => Shown::key("Performance"),
                    "battery-saver" => Shown::key("Battery saver"),
                    "custom" => Shown::key("Advanced"),
                    other => Shown::Content(other.to_owned()),
                };
            }
            if let Some(model) = fields.get("model").and_then(Value::as_str) {
                return Shown::Content(model.to_owned());
            }
            if let Some(mode) = fields.get("mode").and_then(Value::as_str) {
                if key == "worktreeCleanup" {
                    return Shown::key(if mode == "off" { "Off" } else { "Custom" });
                }
                if key == "sourceControlWritingStyle" {
                    return match mode {
                        "repo_conventions" => Shown::key("Repository conventions"),
                        "conventional_commits" => Shown::key("Conventional Commits"),
                        "custom" => Shown::key("Custom instructions"),
                        other => Shown::Content(other.to_owned()),
                    };
                }
                return Shown::Content(mode.to_owned());
            }
            Shown::key("Custom")
        }
    }
}

/// `PROJECT_FILE_BACKED_SETTINGS`: key → built-in value.
fn project_file_backed_built_in(key: &str) -> Option<&'static str> {
    match key {
        "defaultThreadEnvMode" => Some("local"),
        "worktreeSubmodules" => Some("recursive"),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKey {
    Project,
    Environment,
    ProjectFile,
    BuiltIn,
}

/// `InheritanceLayer`. `label` is "Project", the environment's name, "t3.json" or "Default".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritanceLayer {
    pub key: LayerKey,
    pub label: Shown,
    pub value: Shown,
    pub effective: bool,
    pub set: bool,
}

fn default_server_json() -> Value {
    serde_json::to_value(t3_contracts::settings::ServerSettings::default()).unwrap_or_default()
}

/// `settingInheritanceLayers`: matches `resolveProjectSettings`' order.
pub fn setting_inheritance_layers(
    target: &ScopedSettingsTarget,
    environment_settings: &Value,
    key: &str,
) -> Vec<InheritanceLayer> {
    let defaults = default_server_json();
    let environment_value = environment_settings.get(key).unwrap_or(&Value::Null);
    let source = if is_project_scoped_setting_key(key) {
        target
            .sources
            .get(key)
            .copied()
            .unwrap_or(ProjectSettingSource::Environment)
    } else {
        ProjectSettingSource::Environment
    };
    let environment_set = environment_value != defaults.get(key).unwrap_or(&Value::Null);
    let file_backed = project_file_backed_built_in(key);
    let effective_value = target.json.get(key).unwrap_or(&Value::Null);
    let mut layers = Vec::new();
    if target.project_id.is_some() && is_project_scoped_setting_key(key) {
        let from_project = source == ProjectSettingSource::Project;
        layers.push(InheritanceLayer {
            key: LayerKey::Project,
            label: Shown::key("Project"),
            value: if from_project {
                format_value(key, effective_value)
            } else {
                Shown::key("Inherits")
            },
            effective: from_project,
            set: from_project,
        });
    }
    layers.push(InheritanceLayer {
        key: LayerKey::Environment,
        label: Shown::Content(target.label.clone()),
        value: if environment_set {
            format_value(key, environment_value)
        } else {
            Shown::key("Inherits")
        },
        effective: source == ProjectSettingSource::Environment && environment_set,
        set: environment_set,
    });
    if file_backed.is_some() && target.project_id.is_some() {
        let from_file = source == ProjectSettingSource::ProjectFile;
        layers.push(InheritanceLayer {
            key: LayerKey::ProjectFile,
            label: Shown::Content("t3.json".into()),
            value: if from_file {
                format_value(key, effective_value)
            } else {
                Shown::key("Inherits")
            },
            effective: from_file,
            set: from_file,
        });
    }
    let built_in = match file_backed {
        Some(value) => Value::String(value.into()),
        None => defaults.get(key).cloned().unwrap_or(Value::Null),
    };
    layers.push(InheritanceLayer {
        key: LayerKey::BuiltIn,
        label: Shown::key("Default"),
        value: format_value(key, &built_in),
        effective: source == ProjectSettingSource::Environment && !environment_set,
        set: true,
    });
    layers
}

/// `SettingInheritanceState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InheritanceState {
    Default,
    Environment,
    Inherited,
    Overridden,
    Mixed,
}

/// A project that overrides the setting while the scope is wider (`SettingOverridingProject`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverridingProject {
    pub entry: ProjectOverrideEntry,
    pub label: String,
    /// The scope its row opens (`open`).
    pub open: SettingsScopeSearch,
}

/// One environment's chain (`chains` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritanceChain {
    pub id: String,
    pub environment_id: String,
    pub target_label: String,
    /// "Laptop · repo/app": primary label, address and the checkout's last two path segments.
    pub label: String,
    pub machine_label: String,
    pub layers: Vec<InheritanceLayer>,
    /// The overriding projects' values on this environment, by project id.
    pub override_values: Vec<(String, Shown)>,
}

impl InheritanceChain {
    pub fn winning(&self) -> Option<&InheritanceLayer> {
        self.layers.iter().find(|layer| layer.effective)
    }
}

/// What a row's inheritance marker shows (`SettingsRow` + `SettingInheritance`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowInheritance {
    pub state: InheritanceState,
    /// "Built-in default", "Inherited from Laptop", … with " · 2 project overrides" appended.
    pub summary: Vec<SettingsText>,
    /// Chains grouped by identical resolution, the scope's target first.
    pub groups: Vec<Vec<InheritanceChain>>,
    pub multiple_environments: bool,
    pub overriding: Vec<OverridingProject>,
    /// Values set here explain nothing new: the marker waits for hover or focus.
    pub quiet: bool,
}

/// `projectOverrides` of settingsLayout.tsx.
fn overriding_projects(view: &SettingsScopeView, scoped_keys: &[&str]) -> Vec<OverridingProject> {
    if scoped_keys.is_empty() {
        return Vec::new();
    }
    list_project_overrides(&view.selected.connected, scoped_keys)
        .into_iter()
        .filter_map(|entry| {
            let group: &ScopeGroup = view.groups.iter().find(|group| {
                group.members.iter().any(|member| {
                    member.environment_id == entry.environment_id && member.id == entry.project_id
                })
            })?;
            Some(OverridingProject {
                label: group.display_name.clone(),
                open: SettingsScopeSearch {
                    project: Some(group.project_key.clone()),
                    machine: view.search.machine.clone(),
                    checkout: None,
                },
                entry,
            })
        })
        .collect()
}

fn environment_json(environment: &ScopeEnvironment) -> Option<Value> {
    environment
        .config
        .as_ref()
        .map(|config| serde_json::to_value(&config.settings).unwrap_or_default())
}

/// The marker for a server-scoped row with `keys`, or `None` where the web renders none.
pub fn row_inheritance(view: &SettingsScopeView, keys: &[&str]) -> Option<RowInheritance> {
    let key = *keys.first()?;
    if view.targets.is_empty() {
        return None;
    }
    let scoped_keys: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| is_project_scoped_setting_key(key))
        .collect();
    let project_scope = view.scope.is_project_scope();
    let mixed = scoped_settings_are_mixed(&view.targets, keys);
    let source = project_scope.then(|| scoped_settings_source(&view.targets, &scoped_keys));
    let defaults = default_server_json();
    let connected = &view.selected.connected;
    let customized = keys.iter().any(|key| {
        view.targets.iter().any(|target| {
            connected
                .iter()
                .find(|e| e.environment_id == target.environment_id)
                .and_then(environment_json)
                .is_some_and(|settings| {
                    settings.get(*key).unwrap_or(&Value::Null)
                        != defaults.get(*key).unwrap_or(&Value::Null)
                })
        })
    });
    let inherited_from = match (source, view.scope.environment_ids.as_slice()) {
        (Some(ScopedSettingSource::Environment), [only]) => view
            .selected
            .environments
            .iter()
            .find(|e| &e.environment_id == only)
            .map_or_else(|| "environment".to_owned(), |e| e.label.clone()),
        _ => "environment".to_owned(),
    };
    let (state, summary) = if mixed {
        (InheritanceState::Mixed, SettingsText::key("Mixed across selected environments"))
    } else {
        match source {
            Some(ScopedSettingSource::Project) => {
                (InheritanceState::Overridden, SettingsText::key("Overridden for this project"))
            }
            Some(ScopedSettingSource::ProjectFile) => (
                InheritanceState::Inherited,
                SettingsText::key("Inherited from the repository's t3.json"),
            ),
            Some(ScopedSettingSource::Environment) if !scoped_keys.is_empty() => (
                InheritanceState::Inherited,
                SettingsText::with("Inherited from {label}", "label", inherited_from),
            ),
            _ if customized => (InheritanceState::Environment, SettingsText::key("Set on the environment")),
            _ => (InheritanceState::Default, SettingsText::key("Built-in default")),
        }
    };
    let overriding = if project_scope {
        Vec::new()
    } else {
        overriding_projects(view, &scoped_keys)
    };
    let mut summary = vec![summary];
    if !overriding.is_empty() {
        summary.push(SettingsText::with(
            if overriding.len() == 1 { "{count} project override" } else { "{count} project overrides" },
            "count",
            overriding.len().to_string(),
        ));
    }
    let all = &view.all_environments;
    let chains: Vec<InheritanceChain> = view
        .targets
        .iter()
        .filter_map(|target| {
            let environment = connected.iter().find(|e| e.environment_id == target.environment_id)?;
            let settings = environment_json(environment)?;
            let (primary, secondary) = settings_scope_environment_labels(environment, all);
            let path = view
                .scope
                .members
                .iter()
                .find(|m| m.environment_id == target.environment_id && Some(&m.id) == target.project_id.as_ref())
                .map(|m| {
                    let parts: Vec<&str> = m.workspace_root.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
                    parts[parts.len().saturating_sub(2)..].join("/")
                })
                .unwrap_or_default();
            let join = |parts: &[&str]| parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            let tail = join(&[&secondary, &path]);
            Some(InheritanceChain {
                id: format!(
                    "{}:{}",
                    target.environment_id.0,
                    target.project_id.as_ref().map_or("", |p| p.0.as_str())
                ),
                environment_id: target.environment_id.0.clone(),
                target_label: target.label.clone(),
                label: join(&[&primary, &tail]),
                machine_label: join(&[&primary, &secondary]),
                layers: setting_inheritance_layers(target, &settings, key),
                override_values: overriding
                    .iter()
                    .filter(|p| p.entry.environment_id == target.environment_id)
                    .map(|p| {
                        let value = settings["projectSettingsOverrides"][&p.entry.project_id.0]
                            .get(key)
                            .cloned()
                            .unwrap_or(Value::Null);
                        (p.entry.project_id.0.clone(), format_value(key, &value))
                    })
                    .collect(),
            })
        })
        .collect();
    let multiple = {
        let mut ids: Vec<_> = view.targets.iter().map(|t| &t.environment_id).collect();
        ids.sort();
        ids.dedup();
        ids.len() > 1
    };
    let target = view.target();
    let primary_index = chains
        .iter()
        .position(|c| {
            target.is_some_and(|t| c.environment_id == t.environment_id.0 && c.id.ends_with(t.project_id.as_ref().map_or("", |p| p.0.as_str())))
        })
        .unwrap_or(0);
    let mut ordered = chains.clone();
    if multiple && !ordered.is_empty() {
        let first = ordered.remove(primary_index.min(ordered.len() - 1));
        ordered.insert(0, first);
    }
    let resolution = |chain: &InheritanceChain, target_settings: Vec<Value>| {
        (
            chain.winning().map(|l| l.key),
            chain.layers.iter().map(|l| (l.key, l.value.clone())).collect::<Vec<_>>(),
            target_settings,
        )
    };
    let settings_of = |chain: &InheritanceChain| {
        view.targets
            .iter()
            .find(|t| {
                t.environment_id.0 == chain.environment_id
                    && chain.id.ends_with(t.project_id.as_ref().map_or("", |p| p.0.as_str()))
            })
            .map(|t| keys.iter().map(|k| t.json.get(*k).cloned().unwrap_or(Value::Null)).collect())
            .unwrap_or_default()
    };
    let mut groups: Vec<Vec<InheritanceChain>> = Vec::new();
    for chain in ordered {
        let found = multiple.then(|| {
            groups.iter().position(|group| {
                group.first().is_some_and(|first| {
                    resolution(first, settings_of(first)) == resolution(&chain, settings_of(&chain))
                })
            })
        });
        match found.flatten() {
            Some(index) => groups[index].push(chain),
            None => groups.push(vec![chain]),
        }
    }
    Some(RowInheritance {
        quiet: matches!(state, InheritanceState::Default | InheritanceState::Environment)
            && overriding.is_empty(),
        state,
        summary,
        groups,
        multiple_environments: multiple,
        overriding,
    })
}

/// `isManagedSettingPath`.
pub fn is_managed_setting_path(summary: &ManagedPolicySummary, path: &str) -> bool {
    summary.locked_settings.iter().any(|locked| {
        locked == path
            || locked.starts_with(&format!("{path}."))
            || path.starts_with(&format!("{locked}."))
    })
}

/// `managedBy`: the organization whose policy locks any of `paths` on a connected target.
pub fn managed_by(view: &SettingsScopeView, paths: &[&str]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    view.selected.connected.iter().find_map(|environment| {
        let policy = environment.config.as_ref()?.managed_policy.as_ref()?;
        paths
            .iter()
            .any(|path| is_managed_setting_path(policy, path))
            .then(|| policy.organization.0.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_format_as_the_web_shows_them() {
        assert_eq!(format_value("sidebarAutoSettleAfterDays", &Value::Null).english(), "Never");
        assert_eq!(format_value("sidebarAutoSettleAfterDays", &json!(1)).english(), "1 day");
        assert_eq!(format_value("sidebarAutoSettleAfterDays", &json!(7)).english(), "7 days");
        assert_eq!(format_value("confirm", &json!(true)).english(), "On");
        assert_eq!(format_value("defaultThreadEnvMode", &json!("worktree")).english(), "New worktree");
        assert_eq!(
            format_value("textGenerationModelSelection", &json!({"instanceId": "codex", "model": "gpt"})),
            Shown::Content("gpt".into())
        );
        assert_eq!(format_value("worktreeCleanup", &json!({"mode": "off"})).english(), "Off");
        assert_eq!(format_value("x", &json!([1, 2])).english(), "2 items");
        assert_eq!(format_value("x", &json!("")).english(), "Empty");
        assert_eq!(format_value("x", &json!({})).english(), "Custom");
    }

    #[test]
    fn a_locked_path_matches_its_parents_and_children() {
        let policy: ManagedPolicySummary = serde_json::from_value(json!({
            "organization": "Example Corp",
            "lockedSettings": ["storageCleanup.logsAfterDays", "enableTelemetry"]
        }))
        .expect("policy");
        assert!(is_managed_setting_path(&policy, "storageCleanup"));
        assert!(is_managed_setting_path(&policy, "storageCleanup.logsAfterDays"));
        assert!(is_managed_setting_path(&policy, "enableTelemetry.x"));
        assert!(!is_managed_setting_path(&policy, "storage"));
    }
}
