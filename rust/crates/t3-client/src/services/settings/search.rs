//! Port of apps/web/src/components/settings/settingsSearch.ts (logic, navigation groups and
//! moved anchors) and SettingsBreadcrumb.tsx's labels. The item table is generated into
//! `search_items.rs` by `tests/settings_search_oracle.ts`.
use super::scope::{ResolvedSettingsScope, ScopeEnvironment, ScopeKindName};
use crate::{connection::Phase, navigation::SettingsPage, types::EnvironmentId};
use unicode_normalization::UnicodeNormalization;

pub use super::search_items::SETTINGS_SEARCH_ITEMS;

/// `SettingsSearchScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSearchScope {
    Environment,
    EnvironmentDefaults,
    ProjectDefaults,
    Project,
    Checkout,
    Connections,
}

impl SettingsSearchScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::EnvironmentDefaults => "environment-defaults",
            Self::ProjectDefaults => "project-defaults",
            Self::Project => "project",
            Self::Checkout => "checkout",
            Self::Connections => "connections",
        }
    }
}

/// `SettingsSearchItem`. `title` is an English translation key; `to` a settings path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsSearchItem {
    pub id: &'static str,
    pub title: &'static str,
    pub to: &'static str,
    pub target_id: Option<&'static str>,
    pub search_terms: &'static [&'static str],
    pub scope: Option<SettingsSearchScope>,
    pub desktop_only: bool,
    pub mac_only: bool,
    pub windows_only: bool,
    pub release_channel_only: bool,
    pub secrets_feature_only: bool,
    pub cloud_only: bool,
    pub environment_only: bool,
    pub provider_settings_only: bool,
    pub local_backend_management_only: bool,
    pub local_environment_only: bool,
    pub primary_environment_only: bool,
    pub wsl_available_only: bool,
    pub managed_tunnel_only: bool,
    pub peer_links_only: bool,
    pub fast_worktree_checkout_only: bool,
    pub text_generators_only: bool,
    pub secondary: bool,
    pub requires_thread_auto_settlement: bool,
}

impl SettingsSearchItem {
    pub const BASE: Self = Self {
        id: "",
        title: "",
        to: "",
        target_id: None,
        search_terms: &[],
        scope: None,
        desktop_only: false,
        mac_only: false,
        windows_only: false,
        release_channel_only: false,
        secrets_feature_only: false,
        cloud_only: false,
        environment_only: false,
        provider_settings_only: false,
        local_backend_management_only: false,
        local_environment_only: false,
        primary_environment_only: false,
        wsl_available_only: false,
        managed_tunnel_only: false,
        peer_links_only: false,
        fast_worktree_checkout_only: false,
        text_generators_only: false,
        secondary: false,
        requires_thread_auto_settlement: false,
    };
    pub fn page(&self) -> Option<SettingsPage> {
        page_of_path(self.to)
    }
    /// The anchor a jump lands on: `targetId ?? id`.
    pub fn anchor(&self) -> &'static str {
        self.target_id.unwrap_or(self.id)
    }
}

pub fn page_of_path(path: &str) -> Option<SettingsPage> {
    path.trim_end_matches('/')
        .strip_prefix("/settings/")
        .and_then(SettingsPage::from_slug)
}

pub fn settings_path(page: SettingsPage) -> String {
    format!("/settings/{}", page.slug())
}

/// `SETTINGS_SECTION_LABELS`, plus the breadcrumb's detail pages (English keys).
pub fn settings_section_label(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Diagnostics => "Diagnostics",
        SettingsPage::Projects => "Projects",
        SettingsPage::General => "General",
        SettingsPage::Appearance => "Appearance",
        SettingsPage::Chat => "Chat",
        SettingsPage::Diffs => "Diffs & pull requests",
        SettingsPage::Keybindings => "Keybindings",
        SettingsPage::SnapShot => "SnapShots",
        SettingsPage::Providers => "Providers",
        SettingsPage::Threads => "Threads",
        SettingsPage::Browser => "Browser",
        SettingsPage::Devices => "Devices",
        SettingsPage::ScheduledTasks => "Scheduled tasks",
        SettingsPage::SourceControl => "Source control",
        SettingsPage::Worktrees => "Worktrees",
        SettingsPage::Connections => "Environments",
        SettingsPage::Access => "Sharing & access",
        SettingsPage::Secrets => "Secrets",
        SettingsPage::Maintenance => "Maintenance",
        SettingsPage::Labs => "Labs",
        SettingsPage::About => "About",
        SettingsPage::Voice => "Voice",
        SettingsPage::Archived => "Archive",
        SettingsPage::Licenses => "Open source licenses",
    }
}

/// `settingsBreadcrumbLabel`: `None` shows the plain "Settings" crumb.
pub fn settings_breadcrumb_label(pathname: &str) -> Option<&'static str> {
    page_of_path(pathname)
        .filter(|page| {
            // Renamed paths redirect before rendering; only canonical slugs label a crumb.
            pathname.trim_end_matches('/') == settings_path(*page)
        })
        .map(settings_section_label)
}

/// `SETTINGS_DETAIL_PAGE_PARENTS`: detail pages highlight the page that links to them.
pub fn settings_active_nav_page(page: SettingsPage) -> SettingsPage {
    match page {
        SettingsPage::Diagnostics => SettingsPage::Maintenance,
        SettingsPage::Licenses => SettingsPage::About,
        other => other,
    }
}

/// One `SETTINGS_NAV_GROUPS` entry. `heading` is an English key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsNavGroup {
    pub heading: &'static str,
    pub heading_hidden: bool,
    pub pages: Vec<SettingsPage>,
}

/// `SETTINGS_NAV_GROUPS` in sidebar order. Secrets is listed only where its feature is on.
pub fn settings_nav_groups(secrets_enabled: bool) -> Vec<SettingsNavGroup> {
    use SettingsPage::*;
    let mut connections = vec![Connections, Access];
    if secrets_enabled {
        connections.push(Secrets);
    }
    vec![
        SettingsNavGroup {
            heading: "Preferences",
            heading_hidden: true,
            pages: vec![
                General,
                Appearance,
                Chat,
                Diffs,
                Keybindings,
                Voice,
                SnapShot,
            ],
        },
        SettingsNavGroup {
            heading: "Agents",
            heading_hidden: false,
            pages: vec![Providers, Threads, Browser, Devices, ScheduledTasks],
        },
        SettingsNavGroup {
            heading: "Code",
            heading_hidden: false,
            pages: vec![Projects, SourceControl, Worktrees],
        },
        SettingsNavGroup {
            heading: "Connections",
            heading_hidden: false,
            pages: connections,
        },
        SettingsNavGroup {
            heading: "More",
            heading_hidden: true,
            pages: vec![Maintenance, Labs, About, Archived],
        },
    ]
}

/// `SETTINGS_CATEGORY_SCOPES`.
fn category_scope(path: &str) -> Option<SettingsSearchScope> {
    match path {
        "/settings/diagnostics" => Some(SettingsSearchScope::Environment),
        "/settings/projects" => Some(SettingsSearchScope::Project),
        "/settings/source-control" => Some(SettingsSearchScope::EnvironmentDefaults),
        "/settings/worktrees" | "/settings/archived" => Some(SettingsSearchScope::ProjectDefaults),
        "/settings/connections" | "/settings/access" => Some(SettingsSearchScope::Connections),
        _ => None,
    }
}

const RENAMED_SETTINGS_PATHS: [(&str, &str); 3] = [
    ("/settings/integrations", "/settings/browser"),
    ("/settings/storage", "/settings/maintenance"),
    ("/settings/beta", "/settings/labs"),
];
const SPLIT_SETTINGS_PATHS: [&str; 4] = [
    "/settings/general",
    "/settings/appearance",
    "/settings/source-control",
    "/settings/connections",
];
/// Section anchors that moved but are not search results themselves.
const MOVED_SECTION_ANCHORS: [(&str, &str); 3] = [
    ("authorized-clients", "/settings/access"),
    ("text-generation", "/settings/threads"),
    ("about", "/settings/about"),
];

fn find_item(target: &str) -> Option<&'static SettingsSearchItem> {
    SETTINGS_SEARCH_ITEMS
        .iter()
        .find(|item| item.id == target)
        .or_else(|| {
            SETTINGS_SEARCH_ITEMS
                .iter()
                .find(|item| item.target_id == Some(target))
        })
}

/// `movedSettingsPath`: where an old settings link now lives. `hash` has no leading `#`.
pub fn moved_settings_path(pathname: &str, hash: &str) -> Option<&'static str> {
    let renamed = RENAMED_SETTINGS_PATHS
        .iter()
        .find(|(from, _)| *from == pathname)
        .map(|(_, to)| *to);
    if !SPLIT_SETTINGS_PATHS.contains(&pathname) && renamed.is_none() {
        return None;
    }
    let anchored = (!hash.is_empty()).then(|| find_item(hash)).flatten();
    let to = anchored
        .map(|item| item.to)
        .or_else(|| {
            MOVED_SECTION_ANCHORS
                .iter()
                .find(|(anchor, _)| *anchor == hash)
                .map(|(_, to)| *to)
        })
        .or(renamed)?;
    (to != pathname).then_some(to)
}

/// `getSettingsSearchTargetScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsSearchTargetScope {
    pub title: &'static str,
    pub scope: Option<SettingsSearchScope>,
    pub requires_thread_auto_settlement: bool,
}

pub fn get_settings_search_target_scope(target_id: &str) -> Option<SettingsSearchTargetScope> {
    find_item(target_id).map(|item| SettingsSearchTargetScope {
        title: item.title,
        scope: item.scope.or_else(|| category_scope(item.to)),
        requires_thread_auto_settlement: item.requires_thread_auto_settlement,
    })
}

/// `getThreadAutoSettlementSearchAvailability`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoSettlementAvailability {
    pub eligible_environment_ids: Vec<EnvironmentId>,
    pub is_target_available: bool,
}

pub fn get_thread_auto_settlement_search_availability(
    environments: &[ScopeEnvironment],
    scope: Option<&ResolvedSettingsScope>,
) -> AutoSettlementAvailability {
    let eligible: Vec<EnvironmentId> = environments
        .iter()
        .filter(|e| e.phase == Phase::Connected && e.config.is_some())
        .map(|e| e.environment_id.clone())
        .collect();
    let selected: Vec<_> = eligible
        .iter()
        .filter(|id| scope.is_some_and(|scope| scope.environment_ids.contains(id)))
        .collect();
    AutoSettlementAvailability {
        is_target_available: scope.is_some_and(|scope| {
            scope.kind_name() != ScopeKindName::Unavailable && !selected.is_empty()
        }),
        eligible_environment_ids: eligible,
    }
}

/// `isSettingsSearchScopeAvailable`.
pub fn is_settings_search_scope_available(
    required: Option<SettingsSearchScope>,
    kind: ScopeKindName,
) -> bool {
    use ScopeKindName as K;
    match required {
        None | Some(SettingsSearchScope::Connections) => true,
        Some(SettingsSearchScope::Environment) => kind == K::Environment,
        Some(SettingsSearchScope::Checkout) => kind == K::Checkout,
        Some(SettingsSearchScope::Project) => matches!(kind, K::Project | K::Checkout),
        Some(SettingsSearchScope::EnvironmentDefaults) => matches!(kind, K::Environment | K::All),
        Some(SettingsSearchScope::ProjectDefaults) => {
            matches!(kind, K::Environment | K::All | K::Project | K::Checkout)
        }
    }
}

/// `searchableSetting`: a row's anchor id and English title. Panics on an unknown id, as the
/// TypeScript's non-null assertion would; ids are checked by the item table's tests.
pub fn searchable_setting(id: &str) -> (&'static str, &'static str) {
    let item = SETTINGS_SEARCH_ITEMS
        .iter()
        .find(|item| item.id == id)
        .unwrap_or_else(|| panic!("unknown settings search item {id}"));
    (item.id, item.title)
}

/// `SettingsSearchAvailability`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsSearchAvailability {
    pub local_environment_disabled: bool,
    pub has_cloud_public_config: bool,
    pub has_environment: bool,
    pub has_provider_settings_environment: bool,
    pub can_manage_local_backend: bool,
    pub is_wsl_settings_row_visible: bool,
    pub has_thread_auto_settlement: bool,
    pub managed_tunnel_active: bool,
    pub has_peer_link_host: bool,
    pub has_fast_worktree_checkout_environment: bool,
    pub has_text_generators_environment: bool,
    pub has_primary_environment: bool,
}

impl SettingsSearchAvailability {
    pub fn everything() -> Self {
        Self {
            local_environment_disabled: false,
            has_cloud_public_config: true,
            has_environment: true,
            has_provider_settings_environment: true,
            can_manage_local_backend: true,
            is_wsl_settings_row_visible: true,
            has_thread_auto_settlement: true,
            managed_tunnel_active: true,
            has_peer_link_host: true,
            has_fast_worktree_checkout_environment: true,
            has_text_generators_environment: true,
            has_primary_environment: true,
        }
    }

    /// `useAvailableSettingsSearchItems`'s environment-derived flags. Facts the client does not
    /// hold yet (cloud config, WSL, peer links, managed tunnel, local backend) stay false, which
    /// hides those rows from search as the web does when they are absent.
    pub fn from_environments(environments: &[ScopeEnvironment], primary: bool) -> Self {
        let configured = |e: &&ScopeEnvironment| e.config.is_some();
        Self {
            has_environment: environments.iter().any(|e| configured(&e)),
            has_provider_settings_environment: environments
                .iter()
                .any(|e| e.phase == Phase::Connected && e.config.is_some()),
            has_fast_worktree_checkout_environment: environments.iter().any(|e| {
                e.phase == Phase::Connected
                    && e.config.as_ref().is_some_and(|c| {
                        c.environment.capabilities.fast_worktree_checkout == Some(true)
                    })
            }),
            has_text_generators_environment: environments.iter().any(|e| {
                e.config.as_ref().is_some_and(|config| {
                    serde_json::to_value(config)
                        .ok()
                        .and_then(|v| {
                            v.pointer("/managedPolicy/limits/providers/allowedDrivers")
                                .cloned()
                        })
                        .is_none()
                })
            }),
            has_thread_auto_settlement: !get_thread_auto_settlement_search_availability(
                environments,
                None,
            )
            .eligible_environment_ids
            .is_empty(),
            has_primary_environment: primary,
            ..Self::default()
        }
    }
}

/// `filterAvailableSettingsSearchItems`.
pub fn filter_available_settings_search_items(
    availability: &SettingsSearchAvailability,
) -> Vec<&'static SettingsSearchItem> {
    let a = availability;
    SETTINGS_SEARCH_ITEMS
        .iter()
        .filter(|item| {
            (!item.cloud_only || a.has_cloud_public_config)
                && (!item.environment_only || a.has_environment)
                && (!item.provider_settings_only || a.has_provider_settings_environment)
                && (!item.local_backend_management_only || a.can_manage_local_backend)
                && (!item.local_environment_only || !a.local_environment_disabled)
                && (!item.primary_environment_only || a.has_primary_environment)
                && (!item.wsl_available_only || a.is_wsl_settings_row_visible)
                && (!item.requires_thread_auto_settlement || a.has_thread_auto_settlement)
                && (!item.managed_tunnel_only || a.managed_tunnel_active)
                && (!item.peer_links_only || a.has_peer_link_host)
                && (!item.fast_worktree_checkout_only || a.has_fast_worktree_checkout_environment)
                && (!item.text_generators_only || a.has_text_generators_environment)
        })
        .collect()
}

/// The host facts `settingsForThisPlatform` reads (`isElectron`, the hosted channel, the secrets
/// flag and `navigator.platform`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsSearchPlatform {
    pub desktop: bool,
    pub release_channel: bool,
    pub secrets_feature: bool,
    pub mac: bool,
    pub windows: bool,
}

pub fn settings_for_this_platform<'a>(
    items: &[&'a SettingsSearchItem],
    platform: SettingsSearchPlatform,
) -> Vec<&'a SettingsSearchItem> {
    items
        .iter()
        .copied()
        .filter(|item| {
            (platform.desktop || !item.desktop_only)
                && (!item.release_channel_only || platform.desktop || platform.release_channel)
                && (!item.secrets_feature_only || platform.secrets_feature)
                && (!item.mac_only || platform.mac)
                && (!item.windows_only || platform.windows)
        })
        .collect()
}

/// `normalizeSearchText`: NFKD without combining marks, lower case, single spaces.
pub fn normalize_search_text(value: &str) -> String {
    let stripped: String = value
        .nfkd()
        .filter(|c| {
            !matches!(
                unicode_general_category::get_general_category(*c),
                unicode_general_category::GeneralCategory::NonspacingMark
                    | unicode_general_category::GeneralCategory::SpacingMark
                    | unicode_general_category::GeneralCategory::EnclosingMark
            )
        })
        .collect();
    stripped
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `searchSettings`: `translate` maps an English key to the displayed text so search also works
/// in the active language.
pub fn search_settings<'a>(
    query: &str,
    items: &[&'a SettingsSearchItem],
    platform: SettingsSearchPlatform,
    translate: impl Fn(&str) -> String,
) -> Vec<&'a SettingsSearchItem> {
    let normalized = normalize_search_text(query);
    if normalized.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = normalized.split(' ').collect();
    let mut ranked: Vec<(usize, u8, &'a SettingsSearchItem)> =
        settings_for_this_platform(items, platform)
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let title = normalize_search_text(item.title);
                let section = page_of_path(item.to).map_or("", settings_section_label);
                let mut fields = vec![
                    title.clone(),
                    normalize_search_text(section),
                    normalize_search_text(&translate(item.title)),
                    normalize_search_text(&translate(section)),
                ];
                fields.extend(item.search_terms.iter().map(|t| normalize_search_text(t)));
                if !tokens
                    .iter()
                    .all(|token| fields.iter().any(|field| field.contains(token)))
                {
                    return None;
                }
                let exact_phrase = fields.iter().any(|field| field.contains(&normalized));
                let rank = if title == normalized {
                    5
                } else if title.starts_with(&normalized) {
                    4
                } else if title.contains(&normalized) {
                    3
                } else if tokens.iter().all(|token| title.contains(token)) {
                    2
                } else if exact_phrase {
                    1
                } else {
                    0
                };
                Some((index, rank, item))
            })
            .collect();
    ranked.sort_by(|left, right| {
        left.2
            .secondary
            .cmp(&right.2.secondary)
            .then(right.1.cmp(&left.1))
            .then(left.0.cmp(&right.0))
    });
    ranked.into_iter().map(|(_, _, item)| item).collect()
}

/// `isAdvancedTypographyTarget`: rows that exist only with separate fonts on.
pub fn is_advanced_typography_target(target_id: &str) -> bool {
    matches!(target_id, "prompt-font" | "terminal-font")
}

/// `SETTINGS_DEVICE_ONLY_PATHS`: pages without a scope picker, whose client settings save for
/// this device.
pub fn is_device_only_settings_page(page: SettingsPage) -> bool {
    matches!(
        page,
        SettingsPage::General
            | SettingsPage::Appearance
            | SettingsPage::Chat
            | SettingsPage::Diffs
            | SettingsPage::SnapShot
            | SettingsPage::Connections
            | SettingsPage::Access
            | SettingsPage::About
    )
}

/// `SettingsRouteLayout`'s `singleEnvironment` pages.
pub fn is_single_environment_settings_page(page: SettingsPage) -> bool {
    matches!(page, SettingsPage::Providers | SettingsPage::Secrets)
}

/// `SettingsScopeNotice`'s `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTarget {
    Environment,
    All,
    Project,
    Checkout,
}

/// One button of a scope notice (`ScopeChoice`). `label` and `secondary` are content, except
/// "Open all environments", which is a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeChoice {
    pub label: String,
    pub label_is_key: bool,
    pub secondary: String,
    pub search: super::scope::SettingsScopeSearch,
}

/// A group of choices: a project section (`title` is its name) or one plain group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeGroup {
    pub title: Option<String>,
    pub choices: Vec<NoticeChoice>,
}

/// What `SettingsScopeBoundary` shows instead of the page when a search jump's target needs
/// another scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeNotice {
    pub message: super::scoped::SettingsText,
    pub target: NoticeTarget,
    /// The anchor the choices keep (`targetId`).
    pub target_id: String,
    pub groups: Vec<NoticeGroup>,
}

/// `SettingsScopeNotice`'s `renderChoices`.
fn notice_groups(
    target: NoticeTarget,
    eligible: Option<&[EnvironmentId]>,
    search: &super::scope::SettingsScopeSearch,
    groups: &[super::scope::ScopeGroup],
    environments: &[ScopeEnvironment],
) -> Vec<NoticeGroup> {
    use super::scope::{
        SettingsScopeSearch, settings_scope_environment_address, settings_scope_environment_labels,
        settings_scope_project_machines,
    };
    match target {
        NoticeTarget::Checkout => groups
            .iter()
            .filter(|group| search.project.as_ref().is_none_or(|p| &group.project_key == p))
            .map(|group| NoticeGroup {
                title: (!group.display_name.is_empty()).then(|| group.display_name.clone()),
                choices: group
                    .members
                    .iter()
                    .map(|member| {
                        let (primary, secondary) = match environments
                            .iter()
                            .find(|e| e.environment_id == member.environment_id)
                        {
                            Some(environment) => settings_scope_environment_labels(environment, environments),
                            None => (
                                member.environment_label.clone().unwrap_or_else(|| "Environment".into()),
                                member.environment_id.0.clone(),
                            ),
                        };
                        NoticeChoice {
                            label: primary,
                            label_is_key: false,
                            secondary: [secondary, member.workspace_root.clone()]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · "),
                            search: SettingsScopeSearch {
                                project: Some(group.project_key.clone()),
                                machine: Some(member.environment_id.0.clone()),
                                checkout: Some(member.physical_project_key.clone()),
                            },
                        }
                    })
                    .collect(),
            })
            .collect(),
        NoticeTarget::Project => vec![NoticeGroup {
            title: None,
            choices: groups
                .iter()
                .map(|group| {
                    let same_name = groups
                        .iter()
                        .any(|other| other.project_key != group.project_key && other.display_name == group.display_name);
                    let mut parts = Vec::new();
                    if same_name {
                        parts.extend(settings_scope_project_machines(group, environments, false));
                    }
                    if let Some(root) = group.members.first().map(|m| m.workspace_root.clone()) {
                        parts.push(root);
                    }
                    NoticeChoice {
                        label: group.display_name.clone(),
                        label_is_key: false,
                        secondary: parts.join(" · "),
                        search: SettingsScopeSearch {
                            project: Some(group.project_key.clone()),
                            ..Default::default()
                        },
                    }
                })
                .collect(),
        }],
        NoticeTarget::Environment => vec![NoticeGroup {
            title: None,
            choices: environments
                .iter()
                .filter(|e| eligible.is_none_or(|ids| ids.contains(&e.environment_id)))
                .map(|environment| {
                    let (primary, secondary) = settings_scope_environment_labels(environment, environments);
                    NoticeChoice {
                        label: primary,
                        label_is_key: false,
                        secondary: if secondary.is_empty() {
                            settings_scope_environment_address(environment)
                        } else {
                            secondary
                        },
                        search: SettingsScopeSearch {
                            machine: Some(environment.environment_id.0.clone()),
                            ..Default::default()
                        },
                    }
                })
                .collect(),
        }],
        NoticeTarget::All => vec![NoticeGroup {
            title: None,
            choices: vec![NoticeChoice {
                label: "Open all environments".into(),
                label_is_key: true,
                secondary: String::new(),
                search: SettingsScopeSearch::default(),
            }],
        }],
    }
}

/// `SettingsScopeBoundary`'s search-target branches: the notice that replaces the page, if any.
pub fn search_target_notice(
    view: &super::service::SettingsScopeView,
    target_id: &str,
) -> Option<ScopeNotice> {
    use super::scoped::SettingsText;
    if target_id.is_empty() || view.scope.kind_name() == ScopeKindName::Unavailable {
        return None;
    }
    let target = get_settings_search_target_scope(target_id)?;
    let notice = |message, target, eligible: Option<&[EnvironmentId]>| ScopeNotice {
        message,
        target,
        target_id: target_id.to_owned(),
        groups: notice_groups(target, eligible, &view.search, &view.groups, &view.all_environments),
    };
    if target.requires_thread_auto_settlement {
        let availability =
            get_thread_auto_settlement_search_availability(&view.all_environments, Some(&view.scope));
        if !availability.is_target_available {
            let message = SettingsText::with(
                if availability.eligible_environment_ids.is_empty() {
                    "{title} requires a supporting environment. Connect or update an environment to continue."
                } else {
                    "{title} requires a supporting environment. Choose one to continue."
                },
                "title",
                target.title,
            );
            let eligible = availability.eligible_environment_ids.clone();
            return Some(notice(message, NoticeTarget::Environment, Some(&eligible)));
        }
    }
    if !is_settings_search_scope_available(target.scope, view.scope.kind_name()) {
        let kind = match target.scope {
            Some(SettingsSearchScope::Environment) => NoticeTarget::Environment,
            Some(SettingsSearchScope::Project) => NoticeTarget::Project,
            Some(SettingsSearchScope::Checkout) => NoticeTarget::Checkout,
            _ => NoticeTarget::All,
        };
        let message = SettingsText::with(
            "{title} is not available for the selected target. Choose its owning scope to continue.",
            "title",
            target.title,
        );
        return Some(notice(message, kind, None));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_names_a_settings_page_and_ids_are_unique() {
        let mut ids = std::collections::BTreeSet::new();
        for item in SETTINGS_SEARCH_ITEMS {
            assert!(item.page().is_some(), "{}", item.to);
            assert!(ids.insert(item.id), "duplicate {}", item.id);
        }
    }

    #[test]
    fn normalizes_marks_case_and_spacing() {
        assert_eq!(
            normalize_search_text("  Café   Ｆont\tsize "),
            "cafe font size"
        );
    }

    #[test]
    fn breadcrumb_labels_canonical_pages_and_detail_pages_highlight_their_parent() {
        assert_eq!(
            settings_breadcrumb_label("/settings/general/"),
            Some("General")
        );
        assert_eq!(
            settings_breadcrumb_label("/settings/open-source-licenses"),
            Some("Open source licenses")
        );
        assert_eq!(settings_breadcrumb_label("/settings"), None);
        assert_eq!(settings_breadcrumb_label("/settings/integrations"), None);
        assert_eq!(
            settings_active_nav_page(SettingsPage::Diagnostics),
            SettingsPage::Maintenance
        );
    }

    #[test]
    fn search_scope_availability_follows_the_owning_scope() {
        use ScopeKindName as K;
        let defaults = Some(SettingsSearchScope::EnvironmentDefaults);
        assert!(is_settings_search_scope_available(defaults, K::All));
        assert!(!is_settings_search_scope_available(defaults, K::Project));
        assert!(is_settings_search_scope_available(
            Some(SettingsSearchScope::Project),
            K::Checkout
        ));
        assert!(!is_settings_search_scope_available(
            Some(SettingsSearchScope::Checkout),
            K::Project
        ));
        assert!(is_settings_search_scope_available(None, K::Unavailable));
    }

    #[test]
    fn platform_filters_hide_desktop_and_secret_rows_elsewhere() {
        let all: Vec<_> = SETTINGS_SEARCH_ITEMS.iter().collect();
        let web = settings_for_this_platform(&all, SettingsSearchPlatform::default());
        assert!(
            web.iter()
                .all(|item| !item.desktop_only && !item.secrets_feature_only)
        );
        let desktop = settings_for_this_platform(
            &all,
            SettingsSearchPlatform {
                desktop: true,
                ..Default::default()
            },
        );
        assert!(desktop.len() > web.len());
    }
}
