//! Port of apps/web/src/components/settings/settingsScope.ts, settingsScopeAxis.ts and
//! settingsScopeNavigation.ts: which environments, projects or checkouts a settings page edits.
use crate::{
    connection::Phase,
    types::EnvironmentId,
    workspace::{
        projects::{LogicalProjectKey, SCRATCH_PROJECT_SCOPE_KEY},
        sidebar::SidebarProjectSnapshot,
    },
};
use percent_encoding::percent_decode_str;
use std::{collections::BTreeSet, rc::Rc};
use t3_contracts::{base_schemas::ProjectId, server::ServerConfig};

/// The `?project=&machine=&checkout=` axes of a settings address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SettingsScopeSearch {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub checkout: Option<String>,
}

/// One physical checkout of a settings project group (`SidebarProjectGroupMember`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeMember {
    pub id: ProjectId,
    pub environment_id: EnvironmentId,
    pub workspace_root: String,
    pub physical_project_key: String,
    pub environment_label: Option<String>,
}

/// A logical project as the settings scope picker lists it (`SidebarProjectSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeGroup {
    pub project_key: String,
    pub display_name: String,
    pub members: Vec<ScopeMember>,
}

/// An environment as the settings pages see it (`EnvironmentPresentation`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeEnvironment {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub display_url: Option<String>,
    pub phase: Phase,
    pub config: Option<Rc<ServerConfig>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    ProjectRequired,
    ProjectMissing,
    EnvironmentMissing,
    CheckoutMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeKind {
    All,
    Environment {
        environment_id: EnvironmentId,
    },
    Project {
        group: ScopeGroup,
        environment_id: Option<EnvironmentId>,
    },
    Checkout {
        group: ScopeGroup,
        checkout: ScopeMember,
        environment_id: EnvironmentId,
    },
    /// `message` is an English client sentence (a translation key).
    Unavailable {
        reason: UnavailableReason,
        message: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSettingsScope {
    pub kind: ScopeKind,
    /// "All environments", "Unavailable selection" (keys), or content built from names.
    pub label: ScopeLabel,
    pub members: Vec<ScopeMember>,
    pub environment_ids: Vec<EnvironmentId>,
}

/// A label is either one of the resolver's own English keys or content made of names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeLabel {
    Key(&'static str),
    Content(String),
}

impl ScopeLabel {
    pub fn english(&self) -> &str {
        match self {
            Self::Key(key) => key,
            Self::Content(text) => text,
        }
    }
}

/// `ResolvedSettingsScope["kind"]` without its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKindName {
    All,
    Environment,
    Project,
    Checkout,
    Unavailable,
}

impl ResolvedSettingsScope {
    pub fn kind_name(&self) -> ScopeKindName {
        match self.kind {
            ScopeKind::All => ScopeKindName::All,
            ScopeKind::Environment { .. } => ScopeKindName::Environment,
            ScopeKind::Project { .. } => ScopeKindName::Project,
            ScopeKind::Checkout { .. } => ScopeKindName::Checkout,
            ScopeKind::Unavailable { .. } => ScopeKindName::Unavailable,
        }
    }
    pub fn is_project_scope(&self) -> bool {
        matches!(
            self.kind,
            ScopeKind::Project { .. } | ScopeKind::Checkout { .. }
        )
    }
    pub fn unavailable_message(&self) -> Option<&'static str> {
        match self.kind {
            ScopeKind::Unavailable { message, .. } => Some(message),
            _ => None,
        }
    }
}

/// `retainSettingsScope`'s target keys plus the legacy `environmentId` alias.
const TARGET_INPUT_KEYS: [&str; 4] = ["project", "machine", "checkout", "environmentId"];

/// URLSearchParams then TanStack's `parseSearchWith(JSON.parse)`: a value that parses as JSON
/// takes the parsed value, so only string values survive validation.
fn search_values(raw: &str) -> Vec<(String, Option<String>)> {
    raw.trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |text: &str| {
                percent_decode_str(&text.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            };
            let value = decode(value);
            let value = match serde_json::from_str::<serde_json::Value>(&value) {
                Ok(serde_json::Value::String(text)) => Some(text),
                Ok(_) => None,
                Err(_) => Some(value),
            };
            (decode(key), value)
        })
        .collect()
}

fn first<'a>(values: &'a [(String, Option<String>)], key: &str) -> Option<&'a Option<String>> {
    values.iter().find(|(name, _)| name == key).map(|(_, v)| v)
}

/// `validateSettingsRouteSearch`: stale IDs stay visible to the resolver so a removed target
/// reads as unavailable, not as "all".
pub fn validate_settings_route_search(raw: &str) -> SettingsScopeSearch {
    let values = search_values(raw);
    let string = |key: &str| {
        first(&values, key)
            .cloned()
            .flatten()
            .filter(|value| !value.trim().is_empty())
    };
    let environment_alias = first(&values, "environmentId")
        .and_then(Option::as_ref)
        .is_some();
    let machine_present = first(&values, "machine").is_some();
    let project_present = first(&values, "project").is_some();
    SettingsScopeSearch {
        project: string("project"),
        machine: if environment_alias && !machine_present && !project_present {
            string("environmentId")
        } else {
            string("machine")
        },
        checkout: string("checkout"),
    }
}

/// `retainSettingsScope`: a navigation that names no target keeps the previous axes.
pub fn retain_settings_scope(previous: &SettingsScopeSearch, next_raw: &str) -> String {
    let values = search_values(next_raw);
    if TARGET_INPUT_KEYS
        .iter()
        .any(|key| first(&values, key).is_some())
    {
        return next_raw.to_owned();
    }
    let mut query = settings_scope_query(previous);
    let rest = next_raw.trim_start_matches('?');
    if !rest.is_empty() {
        query.push(if query.is_empty() { '?' } else { '&' });
        query.push_str(rest);
    }
    query
}

/// The address query of a scope, in the web's key order. Empty for "all".
pub fn settings_scope_query(search: &SettingsScopeSearch) -> String {
    let parts: Vec<String> = [
        ("project", &search.project),
        ("machine", &search.machine),
        ("checkout", &search.checkout),
    ]
    .into_iter()
    .filter_map(|(key, value)| {
        value
            .as_ref()
            .map(|value| format!("{key}={}", crate::navigation::encode_search_string(value)))
    })
    .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

fn unavailable(reason: UnavailableReason, message: &'static str) -> ResolvedSettingsScope {
    ResolvedSettingsScope {
        kind: ScopeKind::Unavailable { reason, message },
        label: ScopeLabel::Key("Unavailable selection"),
        members: Vec::new(),
        environment_ids: Vec::new(),
    }
}

/// `resolveSettingsScope`: an unavailable selection never broadens a subsequent write.
pub fn resolve_settings_scope(
    search: &SettingsScopeSearch,
    groups: &[ScopeGroup],
    environments: &[ScopeEnvironment],
) -> ResolvedSettingsScope {
    if search.checkout.is_some() && search.project.is_none() {
        return unavailable(
            UnavailableReason::ProjectRequired,
            "Select a project to choose one of its checkouts.",
        );
    }
    let environment = environments
        .iter()
        .find(|candidate| Some(&candidate.environment_id.0) == search.machine.as_ref());
    if search.machine.is_some() && environment.is_none() {
        return unavailable(
            UnavailableReason::EnvironmentMissing,
            "This environment is no longer available.",
        );
    }
    if let Some(project) = &search.project {
        let Some(group) = groups
            .iter()
            .find(|candidate| &candidate.project_key == project)
        else {
            return unavailable(
                UnavailableReason::ProjectMissing,
                "This project is no longer available.",
            );
        };
        let members: Vec<ScopeMember> = group
            .members
            .iter()
            .filter(|member| {
                search
                    .machine
                    .as_ref()
                    .is_none_or(|machine| &member.environment_id.0 == machine)
                    && search
                        .checkout
                        .as_ref()
                        .is_none_or(|checkout| &member.physical_project_key == checkout)
            })
            .cloned()
            .collect();
        if members.is_empty() {
            return unavailable(
                UnavailableReason::CheckoutMissing,
                if search.checkout.is_some() {
                    "This checkout is no longer available in the selected project and environment."
                } else {
                    "This project has no checkout on this environment."
                },
            );
        }
        if search.checkout.is_some() {
            let checkout = members[0].clone();
            let Some(checkout_environment) = environments
                .iter()
                .find(|candidate| candidate.environment_id == checkout.environment_id)
            else {
                return unavailable(
                    UnavailableReason::EnvironmentMissing,
                    "This checkout's environment is no longer available.",
                );
            };
            let shares_environment = group.members.iter().any(|member| {
                member.environment_id == checkout.environment_id
                    && member.physical_project_key != checkout.physical_project_key
            });
            let label = format!(
                "{} / {}{}",
                group.display_name,
                checkout_environment.label,
                if shares_environment {
                    format!(" · {}", checkout.workspace_root)
                } else {
                    String::new()
                }
            );
            return ResolvedSettingsScope {
                environment_ids: vec![checkout.environment_id.clone()],
                kind: ScopeKind::Checkout {
                    group: group.clone(),
                    environment_id: checkout.environment_id.clone(),
                    checkout,
                },
                label: ScopeLabel::Content(label),
                members,
            };
        }
        let mut seen = BTreeSet::new();
        let environment_ids = members
            .iter()
            .filter(|member| seen.insert(member.environment_id.clone()))
            .map(|member| member.environment_id.clone())
            .collect();
        return ResolvedSettingsScope {
            kind: ScopeKind::Project {
                group: group.clone(),
                environment_id: environment.map(|e| e.environment_id.clone()),
            },
            // "All checkouts" is our own English; the caller translates the whole label as
            // content, so it stays English inside a name-built label as the web shows it.
            label: ScopeLabel::Content(format!(
                "{} / {}",
                group.display_name,
                environment.map_or("All checkouts", |e| e.label.as_str())
            )),
            members,
            environment_ids,
        };
    }
    if let Some(environment) = environment {
        return ResolvedSettingsScope {
            kind: ScopeKind::Environment {
                environment_id: environment.environment_id.clone(),
            },
            label: ScopeLabel::Content(environment.label.clone()),
            members: Vec::new(),
            environment_ids: vec![environment.environment_id.clone()],
        };
    }
    ResolvedSettingsScope {
        kind: ScopeKind::All,
        label: ScopeLabel::Key("All environments"),
        members: Vec::new(),
        environment_ids: environments
            .iter()
            .map(|e| e.environment_id.clone())
            .collect(),
    }
}

/// `settingsScopeEnvironmentAddress`.
pub fn settings_scope_environment_address(environment: &ScopeEnvironment) -> String {
    match environment.display_url.as_deref().filter(|url| !url.is_empty()) {
        None => environment.environment_id.0.clone(),
        Some(display) => url::Url::parse(display)
            .ok()
            .and_then(|parsed| {
                let host = parsed.host_str()?.to_owned();
                Some(match parsed.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host,
                })
            })
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| display.to_owned()),
    }
}

/// `settingsScopeEnvironmentLabels`: duplicates of one label get an address to tell them apart.
pub fn settings_scope_environment_labels(
    environment: &ScopeEnvironment,
    environments: &[ScopeEnvironment],
) -> (String, String) {
    let duplicates: Vec<_> = environments
        .iter()
        .filter(|other| {
            other.environment_id != environment.environment_id && other.label == environment.label
        })
        .collect();
    let address = settings_scope_environment_address(environment);
    let secondary = if duplicates
        .iter()
        .any(|other| settings_scope_environment_address(other) == address)
    {
        environment
            .display_url
            .clone()
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| environment.environment_id.0.clone())
    } else {
        address
    };
    (
        environment.label.clone(),
        if duplicates.is_empty() {
            String::new()
        } else {
            secondary
        },
    )
}

/// `settingsScopeEnvironmentLabel`.
pub fn settings_scope_environment_label(
    environment: &ScopeEnvironment,
    environments: &[ScopeEnvironment],
) -> String {
    let (primary, secondary) = settings_scope_environment_labels(environment, environments);
    if secondary.is_empty() {
        primary
    } else {
        format!("{primary} · {secondary}")
    }
}

/// `settingsScopeProjectMachines`.
pub fn settings_scope_project_machines(
    group: &ScopeGroup,
    environments: &[ScopeEnvironment],
    include_addresses: bool,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    group
        .members
        .iter()
        .map(|member| {
            let Some(environment) = environments
                .iter()
                .find(|entry| entry.environment_id == member.environment_id)
            else {
                return member
                    .environment_label
                    .clone()
                    .unwrap_or_else(|| member.environment_id.0.clone());
            };
            let (primary, secondary) = settings_scope_environment_labels(environment, environments);
            let address = if !secondary.is_empty() {
                secondary
            } else if include_addresses {
                settings_scope_environment_address(environment)
            } else {
                String::new()
            };
            if include_addresses || !address.is_empty() {
                format!("{primary} · {address}")
            } else {
                primary
            }
        })
        .filter(|machine| seen.insert(machine.clone()))
        .collect()
}

pub const ALL_VALUE: &str = "all";

/// `environmentAxisValue`.
pub fn environment_axis_value(
    search: &SettingsScopeSearch,
    resolved_environment_id: Option<&str>,
) -> String {
    search
        .machine
        .clone()
        .or_else(|| resolved_environment_id.map(str::to_owned))
        .unwrap_or_else(|| ALL_VALUE.into())
}

/// `projectAxisValue`.
pub fn project_axis_value(search: &SettingsScopeSearch) -> String {
    search.project.clone().unwrap_or_else(|| ALL_VALUE.into())
}

/// `projectScopeMenuGroups`.
pub fn project_scope_menu_groups<'a>(
    groups: &'a [ScopeGroup],
    search: &SettingsScopeSearch,
) -> Vec<&'a ScopeGroup> {
    groups
        .iter()
        .filter(|group| {
            search.machine.as_ref().is_none_or(|machine| {
                group
                    .members
                    .iter()
                    .any(|member| &member.environment_id.0 == machine)
            })
        })
        .collect()
}

/// `selectEnvironmentAxis`: changing an axis drops an older checkout narrowing.
pub fn select_environment_axis(search: &SettingsScopeSearch, value: &str) -> SettingsScopeSearch {
    SettingsScopeSearch {
        project: search.project.clone(),
        machine: (value != ALL_VALUE).then(|| value.to_owned()),
        checkout: None,
    }
}

/// `selectProjectAxis`.
pub fn select_project_axis(search: &SettingsScopeSearch, value: &str) -> SettingsScopeSearch {
    SettingsScopeSearch {
        project: (value != ALL_VALUE).then(|| value.to_owned()),
        machine: search.machine.clone(),
        checkout: None,
    }
}

/// `settingsScopeCheckoutChoices`: empty unless the selection spans several checkouts of one
/// environment, which only this axis can split.
pub fn settings_scope_checkout_choices<'a>(
    group: &'a ScopeGroup,
    search: &SettingsScopeSearch,
) -> Vec<&'a ScopeMember> {
    let selected = group
        .members
        .iter()
        .find(|member| Some(&member.physical_project_key) == search.checkout.as_ref());
    let environment = search
        .machine
        .clone()
        .or_else(|| selected.map(|member| member.environment_id.0.clone()));
    let members: Vec<_> = group
        .members
        .iter()
        .filter(|member| {
            environment
                .as_ref()
                .is_none_or(|id| &member.environment_id.0 == id)
        })
        .collect();
    if members.len() > 1
        && members
            .iter()
            .all(|member| member.environment_id == members[0].environment_id)
    {
        members
    } else {
        Vec::new()
    }
}

/// `selectCheckoutAxis`.
pub fn select_checkout_axis(
    search: &SettingsScopeSearch,
    checkout: Option<&ScopeMember>,
) -> SettingsScopeSearch {
    match checkout {
        Some(checkout) => SettingsScopeSearch {
            project: search.project.clone(),
            machine: Some(checkout.environment_id.0.clone()),
            checkout: Some(checkout.physical_project_key.clone()),
        },
        None => SettingsScopeSearch {
            checkout: None,
            ..search.clone()
        },
    }
}

/// `fileBasename` of packages/shared/src/path.ts.
pub fn file_basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return path;
    }
    trimmed
        .rfind(['/', '\\'])
        .map_or(trimmed, |index| &trimmed[index + 1..])
}

/// `settingsScopeCheckoutLabel`: repeated folder names fall back to the full path.
pub fn settings_scope_checkout_label(checkout: &ScopeMember, checkouts: &[&ScopeMember]) -> String {
    let name = file_basename(&checkout.workspace_root);
    if checkouts
        .iter()
        .filter(|other| file_basename(&other.workspace_root) == name)
        .count()
        > 1
    {
        checkout.workspace_root.clone()
    } else {
        name.to_owned()
    }
}

/// `selectSingleEnvironmentScope`: pages that edit one environment pick the primary, then a
/// connected one, then the first.
pub fn select_single_environment_scope(
    search: &SettingsScopeSearch,
    scope: &ResolvedSettingsScope,
    environments: &[ScopeEnvironment],
    primary: Option<&EnvironmentId>,
) -> SettingsScopeSearch {
    if search.machine.is_some() || scope.kind_name() == ScopeKindName::Unavailable {
        return search.clone();
    }
    let candidates: Vec<_> = environments
        .iter()
        .filter(|environment| scope.environment_ids.contains(&environment.environment_id))
        .collect();
    let selected = candidates
        .iter()
        .find(|environment| Some(&environment.environment_id) == primary)
        .or_else(|| {
            candidates
                .iter()
                .find(|environment| environment.phase == Phase::Connected)
        })
        .or_else(|| candidates.first());
    match selected {
        Some(environment) => SettingsScopeSearch {
            machine: Some(environment.environment_id.0.clone()),
            ..search.clone()
        },
        None => search.clone(),
    }
}

/// The key a logical project group carries in settings addresses (TypeScript's string keys).
pub fn logical_project_scope_key(key: &LogicalProjectKey) -> String {
    match key {
        LogicalProjectKey::Repository(key) => key.clone(),
        LogicalProjectKey::Physical(key) => key.legacy_preference_key(),
        LogicalProjectKey::Scratch => SCRATCH_PROJECT_SCOPE_KEY.into(),
    }
}

/// `useSettingsProjectGroups`: sidebar groups sorted by display name.
pub fn scope_groups(snapshots: &[SidebarProjectSnapshot]) -> Vec<ScopeGroup> {
    let mut groups: Vec<ScopeGroup> = snapshots
        .iter()
        .map(|snapshot| ScopeGroup {
            project_key: logical_project_scope_key(&snapshot.group.key),
            display_name: snapshot.group.label.clone(),
            members: snapshot
                .group
                .members
                .iter()
                .map(|member| ScopeMember {
                    id: member.project.id.clone(),
                    environment_id: member.environment.clone(),
                    workspace_root: member.project.workspace_root.0.clone(),
                    physical_project_key: member.physical_key().legacy_preference_key(),
                    environment_label: snapshot
                        .environment_labels
                        .get(&member.physical_key())
                        .cloned()
                        .flatten(),
                })
                .collect(),
        })
        .collect();
    // localeCompare's base ordering: case-insensitive first, then the exact text.
    groups.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(id: &str, label: &str) -> ScopeEnvironment {
        ScopeEnvironment {
            environment_id: EnvironmentId(id.into()),
            label: label.into(),
            display_url: None,
            phase: Phase::Connected,
            config: None,
        }
    }
    fn member(id: &str, environment: &str) -> ScopeMember {
        ScopeMember {
            id: ProjectId(id.into()),
            environment_id: EnvironmentId(environment.into()),
            workspace_root: format!("/repos/{id}"),
            physical_project_key: format!("{environment}:/repos/{id}"),
            environment_label: None,
        }
    }
    fn group(key: &str, members: &[&ScopeMember]) -> ScopeGroup {
        ScopeGroup {
            project_key: key.into(),
            display_name: key.into(),
            members: members.iter().map(|m| (*m).clone()).collect(),
        }
    }
    fn fixture() -> (Vec<ScopeEnvironment>, [ScopeMember; 4], Vec<ScopeGroup>) {
        let environments = vec![
            environment("laptop", "Laptop"),
            environment("server", "Server"),
        ];
        let first = member("first", "laptop");
        let second = member("second", "laptop");
        let third = member("third", "server");
        let other = member("other", "server");
        let groups = vec![
            group("t3code", &[&first, &second, &third]),
            group("other", &[&other]),
        ];
        (environments, [first, second, third, other], groups)
    }
    fn search(project: Option<&str>, machine: Option<&str>, checkout: Option<&str>) -> SettingsScopeSearch {
        SettingsScopeSearch {
            project: project.map(Into::into),
            machine: machine.map(Into::into),
            checkout: checkout.map(Into::into),
        }
    }

    #[test]
    fn retains_an_orphan_checkout_so_it_cannot_turn_into_all_environments() {
        let (environments, [first, ..], groups) = fixture();
        let parsed = validate_settings_route_search(&format!(
            "?checkout={}",
            crate::navigation::encode_search_string(&first.physical_project_key)
        ));
        assert_eq!(parsed, search(None, None, Some(&first.physical_project_key)));
        let scope = resolve_settings_scope(&parsed, &groups, &environments);
        assert!(matches!(
            scope.kind,
            ScopeKind::Unavailable {
                reason: UnavailableReason::ProjectRequired,
                ..
            }
        ));
        assert!(scope.members.is_empty() && scope.environment_ids.is_empty());
    }

    #[test]
    fn keeps_all_physical_members_in_a_project_aggregate() {
        let (environments, [first, second, third, _], groups) = fixture();
        let scope = resolve_settings_scope(&search(Some("t3code"), None, None), &groups, &environments);
        assert!(matches!(
            scope.kind,
            ScopeKind::Project {
                environment_id: None,
                ..
            }
        ));
        assert_eq!(scope.members, vec![first, second, third]);
        assert_eq!(
            scope.environment_ids,
            vec![EnvironmentId("laptop".into()), EnvironmentId("server".into())]
        );
    }

    #[test]
    fn narrows_a_checkout_target_to_exactly_one_member() {
        let (environments, [_, second, ..], groups) = fixture();
        let scope = resolve_settings_scope(
            &search(Some("t3code"), None, Some(&second.physical_project_key)),
            &groups,
            &environments,
        );
        assert_eq!(scope.kind_name(), ScopeKindName::Checkout);
        assert_eq!(
            scope.label,
            ScopeLabel::Content("t3code / Laptop · /repos/second".into())
        );
        assert_eq!(scope.members, vec![second]);
        assert_eq!(scope.environment_ids, vec![EnvironmentId("laptop".into())]);
    }

    #[test]
    fn never_widens_an_invalid_or_stale_target() {
        let (environments, [first, ..], groups) = fixture();
        let first_key = first.physical_project_key.as_str();
        for case in [
            search(Some("missing"), None, None),
            search(None, Some("removed"), None),
            search(Some("t3code"), Some("removed"), None),
            search(Some("t3code"), None, Some("deleted")),
            search(Some("other"), Some("laptop"), None),
            search(Some("other"), None, Some(first_key)),
            search(Some("t3code"), Some("server"), Some(first_key)),
        ] {
            let scope = resolve_settings_scope(&case, &groups, &environments);
            assert_eq!(scope.kind_name(), ScopeKindName::Unavailable, "{case:?}");
            assert!(scope.members.is_empty() && scope.environment_ids.is_empty());
        }
    }

    #[test]
    fn leaves_a_removed_checkout_unavailable_while_siblings_remain() {
        let (environments, [first, second, third, _], groups) = fixture();
        let target = search(Some("t3code"), Some("laptop"), Some(&first.physical_project_key));
        let scope = resolve_settings_scope(&target, &groups, &environments);
        assert_eq!(scope.members, vec![first.clone()]);
        let scope = resolve_settings_scope(&target, &[group("t3code", &[&second, &third])], &environments);
        assert_eq!(scope.kind_name(), ScopeKindName::Unavailable);
    }

    #[test]
    fn does_not_select_another_environment_after_the_last_local_checkout_goes() {
        let (environments, [first, second, third, _], groups) = fixture();
        let target = search(Some("t3code"), Some("laptop"), None);
        assert_eq!(
            resolve_settings_scope(&target, &groups, &environments).members,
            vec![first, second]
        );
        assert_eq!(
            resolve_settings_scope(&target, &[group("t3code", &[&third])], &environments).kind_name(),
            ScopeKindName::Unavailable
        );
    }

    #[test]
    fn rejects_a_cached_checkout_whose_environment_was_removed() {
        let (environments, [_, _, third, _], groups) = fixture();
        let scope = resolve_settings_scope(
            &search(Some("t3code"), None, Some(&third.physical_project_key)),
            &groups,
            &environments[..1],
        );
        assert!(matches!(
            scope.kind,
            ScopeKind::Unavailable {
                reason: UnavailableReason::EnvironmentMissing,
                ..
            }
        ));
    }

    #[test]
    fn axes_drop_an_older_checkout_narrowing() {
        let checkout = search(Some("app"), Some("first"), Some("app@first"));
        assert_eq!(
            select_environment_axis(&checkout, "second"),
            search(Some("app"), Some("second"), None)
        );
        assert_eq!(
            select_project_axis(&checkout, "app"),
            search(Some("app"), Some("first"), None)
        );
        assert_eq!(select_project_axis(&checkout, ALL_VALUE), search(None, Some("first"), None));
        assert_eq!(select_environment_axis(&checkout, ALL_VALUE), search(Some("app"), None, None));
    }

    #[test]
    fn offers_checkouts_only_where_an_environment_cannot_tell_them_apart() {
        let m = |environment: &str, key: &str| ScopeMember {
            physical_project_key: key.into(),
            ..member(key, environment)
        };
        let (a, b, c) = (m("laptop", "a"), m("laptop", "b"), m("server", "c"));
        let all = group("g", &[&a, &b, &c]);
        let pair = group("g", &[&a, &b]);
        assert_eq!(
            settings_scope_checkout_choices(&all, &search(None, Some("laptop"), None)),
            vec![&all.members[0], &all.members[1]]
        );
        assert_eq!(settings_scope_checkout_choices(&pair, &search(None, None, None)).len(), 2);
        assert_eq!(settings_scope_checkout_choices(&all, &search(None, None, Some("b"))).len(), 2);
        assert!(settings_scope_checkout_choices(&all, &search(None, None, None)).is_empty());
        assert!(settings_scope_checkout_choices(&all, &search(None, Some("server"), None)).is_empty());
    }

    #[test]
    fn labels_a_checkout_by_its_full_path_only_when_its_folder_name_repeats() {
        let with_root = |key: &str, root: &str| ScopeMember {
            workspace_root: root.into(),
            ..member(key, "laptop")
        };
        let first = with_root("a", "/work/one/app");
        let second = with_root("b", "C:\\work\\two\\app");
        let other = with_root("c", "/work/tools");
        let all = [&first, &second, &other];
        assert_eq!(settings_scope_checkout_label(&second, &all), "C:\\work\\two\\app");
        assert_eq!(settings_scope_checkout_label(&other, &all), "tools");
    }

    #[test]
    fn route_search_keeps_strings_maps_the_legacy_alias_and_drops_json_values() {
        assert_eq!(
            validate_settings_route_search("?environmentId=remote"),
            search(None, Some("remote"), None)
        );
        assert_eq!(
            validate_settings_route_search("?environmentId=remote&project=p"),
            search(Some("p"), None, None)
        );
        assert_eq!(validate_settings_route_search("?project=123"), SettingsScopeSearch::default());
        assert_eq!(
            validate_settings_route_search("?project=%22123%22&machine=%20"),
            search(Some("123"), None, None)
        );
        assert_eq!(
            validate_settings_route_search("?checkout=remote-server%3A%2Fhome%2Fuser%2FT3+Code"),
            search(None, None, Some("remote-server:/home/user/T3 Code"))
        );
    }

    #[test]
    fn retains_the_previous_scope_unless_a_target_key_is_named() {
        let previous = search(Some("repository:t3code"), Some("remote-server"), None);
        assert_eq!(
            retain_settings_scope(&previous, ""),
            "?project=repository%3At3code&machine=remote-server"
        );
        assert_eq!(
            retain_settings_scope(&previous, "?tab=x"),
            "?project=repository%3At3code&machine=remote-server&tab=x"
        );
        assert_eq!(retain_settings_scope(&previous, "?machine=other"), "?machine=other");
        assert_eq!(retain_settings_scope(&previous, "?environmentId=e"), "?environmentId=e");
        assert_eq!(
            validate_settings_route_search(&settings_scope_query(&previous)),
            previous
        );
    }

    #[test]
    fn single_environment_pages_prefer_the_primary_then_a_connected_environment() {
        let (mut environments, _, groups) = fixture();
        environments[0].phase = Phase::Backoff;
        let all = search(None, None, None);
        let scope = resolve_settings_scope(&all, &groups, &environments);
        assert_eq!(
            select_single_environment_scope(&all, &scope, &environments, None).machine.as_deref(),
            Some("server")
        );
        assert_eq!(
            select_single_environment_scope(
                &all,
                &scope,
                &environments,
                Some(&EnvironmentId("laptop".into()))
            )
            .machine
            .as_deref(),
            Some("laptop")
        );
    }

    #[test]
    fn duplicate_environment_labels_gain_their_address() {
        let mut a = environment("a", "Mac");
        a.display_url = Some("http://10.0.0.1:3773".into());
        let mut b = environment("b", "Mac");
        b.display_url = Some("http://10.0.0.2:3773".into());
        let c = environment("c", "Linux");
        let all = [a.clone(), b.clone(), c.clone()];
        assert_eq!(settings_scope_environment_label(&a, &all), "Mac · 10.0.0.1:3773");
        assert_eq!(settings_scope_environment_label(&c, &all), "Linux");
    }
}
