//! Port of apps/web/src/components/chat/ProviderModelPicker.tsx ProviderModelPicker.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::services::providers::provider_instance_initials;
use t3_client::view_logic::shell::models::{self, ModelOption};
use t3_contracts::base_schemas::ProjectId;
use t3_contracts::model::{
    ANTIGRAVITY_DEFAULT_MODEL, ProviderOptionDescriptor, ProviderOptionSelection,
    ProviderOptionSelectionValue,
};
use t3_contracts::model_selection::ModelSelection;
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_contracts::server::{ServerProvider, ServerProviderAuthStatus, ServerProviderState};
use t3_contracts::settings::ServerSettings;

use crate::i18n::{Value, t, tr};
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, palette, tw};
use crate::ui::icon::{Icon, icon};
use crate::ui::popover::{self, Padding, PopoverHandle, PopoverOptions};
use crate::ui::select::{self, SelectItem, SelectProps};
use crate::ui::switch::{self, ControlState};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelKey {
    pub instance_id: ProviderInstanceId,
    pub model: String,
}

impl From<&ModelSelection> for ModelKey {
    fn from(selection: &ModelSelection) -> Self {
        Self {
            instance_id: selection.instance_id.clone(),
            model: selection.model.clone(),
        }
    }
}

/// Owned presentation of the client's environment/project-scoped provider entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerProvider {
    pub snapshot: ServerProvider,
    pub display_name: String,
    pub enabled: bool,
    pub available: bool,
    pub ready: bool,
    pub continuation_group: Option<String>,
    pub accent_color: Option<String>,
    pub acp_agent_id: Option<String>,
    pub acp_icon_url: Option<String>,
    pub options: Vec<ModelOption>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerData {
    pub providers: Vec<PickerProvider>,
    pub selection: ModelKey,
    pub selected_models: Option<Vec<ModelKey>>,
    pub favorites: BTreeSet<ModelKey>,
    pub locked_provider: Option<String>,
    pub locked_continuation_group: Option<String>,
    pub disabled_models: BTreeMap<ModelKey, String>,
}

impl PickerData {
    /// Catalog order, custom/hidden models and short names come from t3-client.
    pub fn from_catalog(
        providers: &[ServerProvider],
        settings: &ServerSettings,
        project: Option<&ProjectId>,
        selection: &ModelSelection,
    ) -> Self {
        let options = models::options_by_instance(
            &models::Settings {
                server: settings,
                legacy_preferences: None,
            },
            providers,
            Some(selection),
            false,
        );
        Self {
            providers: models::entries_for(providers, settings, project)
                .into_iter()
                .map(|entry| PickerProvider {
                    snapshot: entry.snapshot.clone(),
                    display_name: entry.display_name.clone(),
                    enabled: entry.enabled,
                    available: entry.is_available,
                    ready: entry.ready(),
                    continuation_group: entry.continuation_group_key.map(str::to_owned),
                    accent_color: entry.accent.clone(),
                    acp_agent_id: entry.acp_agent.clone(),
                    acp_icon_url: entry.acp_icon.clone(),
                    options: options
                        .get(&entry.snapshot.instance_id)
                        .cloned()
                        .unwrap_or_default(),
                })
                .collect(),
            selection: selection.into(),
            selected_models: None,
            favorites: BTreeSet::new(),
            locked_provider: None,
            locked_continuation_group: None,
            disabled_models: BTreeMap::new(),
        }
    }

    fn provider(&self, id: &ProviderInstanceId) -> Option<&PickerProvider> {
        self.providers
            .iter()
            .find(|p| &p.snapshot.instance_id == id)
    }

    fn matches_lock(&self, provider: &PickerProvider) -> bool {
        self.locked_provider.as_ref().is_none_or(|driver| {
            &provider.snapshot.driver.0 == driver
                && self.locked_continuation_group.as_ref().is_none_or(|group| {
                    group.is_empty() || provider.continuation_group.as_ref() == Some(group)
                })
        })
    }

    fn model(&self, key: &ModelKey) -> Option<&ModelOption> {
        self.provider(&key.instance_id)?
            .options
            .iter()
            .find(|option| option.slug == key.model)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum CatalogState {
    #[default]
    Ready,
    Loading,
    Offline,
    Error(String),
    Denied(String),
    Unsupported(String),
}

impl CatalogState {
    fn reason(&self) -> Option<String> {
        match self {
            Self::Ready => None,
            Self::Loading => Some(t("Searching project models...")),
            Self::Offline => Some(t("Not connected. Model selection is unavailable.")),
            // Catalogue failures carry server detail: content, never a catalogue key.
            Self::Error(reason) => Some(reason.clone()),
            Self::Denied(reason) | Self::Unsupported(reason) => Some(t(reason)), // client English key
        }
    }
}

/// The client resolves selections, prompt-injected choices and reported values before rendering.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraitsView {
    pub model: Option<ModelKey>,
    pub reasoning: Option<ProviderOptionDescriptor>,
    pub others: Vec<ProviderOptionDescriptor>,
    pub speed: Option<ProviderOptionDescriptor>,
    pub ultracode: Option<ProviderOptionDescriptor>,
    pub summary: Option<String>,
    pub disabled_reason: Option<String>,
    pub unavailable: bool,
}

pub type Pick = Rc<dyn Fn(ModelKey)>;
pub type OptionPick = Rc<dyn Fn(ModelKey, ProviderOptionSelection)>;
pub type ProviderGlyph = Rc<dyn Fn(&PickerProvider, f32) -> AnyElement>;
/// The model-search service supplies ranked identities; None means unsupported.
pub type Search = Rc<dyn Fn(&PickerData, &str) -> Vec<ModelKey>>;

pub struct PickerProps {
    pub data: Prop<Rc<PickerData>>,
    pub catalog: Prop<CatalogState>,
    /// Final display text: the caller translates its own keys; client detail is content.
    pub disabled_reason: Prop<Option<String>>,
    pub traits: Prop<TraitsView>,
    pub docked: bool,
    pub compact: bool,
    pub trigger_label: Prop<Option<String>>,
    pub on_pick: Pick,
    pub on_toggle_model: Option<Pick>,
    pub on_toggle_favorite: Option<Pick>,
    pub on_option_pick: Option<OptionPick>,
    pub on_refresh: Option<Rc<dyn Fn()>>,
    pub on_open_provider_setup: Option<Rc<dyn Fn(ProviderInstanceId)>>,
    pub search: Option<Search>,
    pub provider_glyph: Option<ProviderGlyph>,
    pub footer: Option<Rc<dyn Fn() -> AnyElement>>,
    pub handle: Option<PickerHandle>,
}

impl PickerProps {
    pub fn new(data: Prop<Rc<PickerData>>, on_pick: impl Fn(ModelKey) + 'static) -> Self {
        Self {
            data,
            catalog: CatalogState::Ready.into(),
            disabled_reason: None.into(),
            traits: TraitsView {
                disabled_reason: Some("Model options are unavailable on this client.".into()),
                ..Default::default()
            }
            .into(),
            docked: true,
            compact: false,
            trigger_label: None.into(),
            on_pick: Rc::new(on_pick),
            on_toggle_model: None,
            on_toggle_favorite: None,
            on_option_pick: None,
            on_refresh: None,
            on_open_provider_setup: None,
            search: None,
            provider_glyph: None,
            footer: None,
            handle: None,
        }
    }
}

/// Port of ProviderModelPicker.logic.ts retainedModelWhileProviderLocked.
pub fn retained_model_while_provider_locked(
    entry: Option<&PickerProvider>,
    model: &str,
) -> Option<ModelOption> {
    let entry = entry?;
    if (entry.enabled && entry.available)
        || model.is_empty()
        || matches!(entry.snapshot.driver.0.as_str(), "opencode" | "antigravity")
    {
        return None;
    }
    Some(
        entry
            .snapshot
            .models
            .iter()
            .find(|candidate| candidate.slug.0 == model)
            .map_or_else(
                || ModelOption {
                    slug: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                |candidate| ModelOption {
                    slug: candidate.slug.0.clone(),
                    name: candidate.name.0.clone(),
                    short_name: candidate
                        .short_name
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|s| s.0.clone()),
                    sub_provider: candidate
                        .sub_provider
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(|s| s.0.clone()),
                    ..Default::default()
                },
            ),
    )
}

/// Port of ModelPickerContent.tsx resolveModelPickerSelectedModel.
pub fn resolve_model_picker_selected_model<'a>(
    driver: Option<&str>,
    model: &str,
    options: &'a [ModelOption],
) -> Option<&'a ModelOption> {
    if driver == Some("antigravity") && model == ANTIGRAVITY_DEFAULT_MODEL {
        let available = || {
            options
                .iter()
                .filter(|o| o.slug != ANTIGRAVITY_DEFAULT_MODEL && !o.is_unavailable)
        };
        return available()
            .find(|o| {
                o.aliases
                    .iter()
                    .any(|alias| alias == ANTIGRAVITY_DEFAULT_MODEL)
            })
            .or_else(|| available().find(|o| o.is_default));
    }
    options.iter().find(|option| option.slug == model)
}

fn display_name(option: &ModelOption, short: bool) -> String {
    let name = if short {
        option.short_name.as_deref().unwrap_or(&option.name)
    } else {
        &option.name
    };
    let qualifier = option.sub_provider.as_deref().unwrap_or_default().trim();
    let rest = name
        .get(..qualifier.len())
        .filter(|prefix| prefix.to_lowercase() == qualifier.to_lowercase())
        .and_then(|_| name.get(qualifier.len()..));
    if !qualifier.is_empty()
        && let Some(rest) = rest
        && rest
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || ".:/-".contains(c))
    {
        let stripped = rest.trim_start_matches(|c: char| c.is_whitespace() || ".:/-".contains(c));
        if !stripped.is_empty() {
            return stripped.into();
        }
    }
    name.into()
}

fn trigger_title(data: &PickerData, key: &ModelKey, short: bool) -> String {
    let entry = data.provider(&key.instance_id);
    let driver = entry.map(|p| p.snapshot.driver.0.as_str());
    let selected = entry
        .and_then(|p| resolve_model_picker_selected_model(driver, &key.model, &p.options))
        .cloned()
        .or_else(|| retained_model_while_provider_locked(entry, &key.model))
        .or_else(|| {
            entry
                .filter(|_| !matches!(driver, Some("opencode" | "antigravity")))
                .and_then(|p| p.options.first())
                .cloned()
        });
    selected.map_or_else(
        || {
            if key.model.is_empty() || key.model == ANTIGRAVITY_DEFAULT_MODEL {
                t("Choose model")
            } else {
                key.model.clone()
            }
        },
        |model| {
            let label = display_name(&model, short);
            if model.is_unavailable && !short {
                format!("{label} ({})", t("Unavailable"))
            } else {
                label
            }
        },
    )
}

fn include_option(data: &PickerData, provider: &PickerProvider, option: &ModelOption) -> bool {
    if provider.snapshot.driver.0 == "antigravity" && option.slug == ANTIGRAVITY_DEFAULT_MODEL {
        return false;
    }
    provider.ready
        || (provider.enabled
            && matches!(
                provider.snapshot.driver.0.as_str(),
                "opencode" | "antigravity" | "cursor"
            )
            && provider.snapshot.instance_id == data.selection.instance_id
            && option.slug == data.selection.model
            && option.is_unavailable)
}

/// Port of ModelPickerContent.tsx shouldOfferModelPickerSetup / ProviderStatusBanner.hasProviderSetup.
fn should_offer_model_picker_setup(provider: &PickerProvider) -> bool {
    let setup = provider.snapshot.setup.as_ref().and_then(Option::as_ref);
    let has_setup = provider.snapshot.driver.0 == "antigravity"
        || setup.is_some_and(|s| s.can_authenticate || s.can_install);
    provider.enabled
        && provider.snapshot.status != ServerProviderState::Disabled
        && has_setup
        && (!provider.ready
            || !provider.snapshot.installed
            || provider.snapshot.auth.status == ServerProviderAuthStatus::Unauthenticated
            || provider.options.iter().all(|option| option.is_unavailable))
}

fn initial_tab(data: &PickerData, can_setup: bool) -> Option<ProviderInstanceId> {
    let active_unavailable = data
        .provider(&data.selection.instance_id)
        .is_some_and(|p| !p.ready && p.options.iter().any(|o| include_option(data, p, o)));
    if data.locked_provider.is_some()
        || active_unavailable
        || (can_setup
            && data
                .provider(&data.selection.instance_id)
                .is_some_and(should_offer_model_picker_setup))
    {
        return Some(data.selection.instance_id.clone());
    }
    let favorite = data.favorites.iter().any(|key| {
        data.provider(&key.instance_id).is_some_and(|p| {
            p.options
                .iter()
                .any(|o| o.slug == key.model && include_option(data, p, o))
        })
    });
    (!favorite).then(|| data.selection.instance_id.clone())
}

fn visible_models(
    data: &PickerData,
    tab: Option<&ProviderInstanceId>,
    query: &str,
    expanded: &BTreeSet<ProviderInstanceId>,
    search: Option<&Search>,
) -> (Vec<ModelKey>, bool) {
    let searching = !query.trim().is_empty();
    let ranked = if searching {
        search.map(|search| search(data, query)).unwrap_or_default()
    } else {
        data.providers
            .iter()
            .flat_map(|p| {
                p.options.iter().map(move |o| ModelKey {
                    instance_id: p.snapshot.instance_id.clone(),
                    model: o.slug.clone(),
                })
            })
            .collect()
    };
    let mut items: Vec<_> = ranked
        .into_iter()
        .filter(|key| {
            let Some(p) = data.provider(&key.instance_id) else {
                return false;
            };
            let Some(o) = data.model(key) else {
                return false;
            };
            data.matches_lock(p)
                && include_option(data, p, o)
                && (searching
                    || tab.map_or_else(
                        || data.favorites.contains(key),
                        |tab| tab == &key.instance_id,
                    ))
        })
        .collect();
    if searching {
        return (items, false);
    }
    let favorites = data
        .favorites
        .iter()
        .map(|k| models::provider_model_key(&k.instance_id, &k.model))
        .collect();
    let order: Vec<_> = if tab.is_none() {
        data.providers
            .iter()
            .map(|p| p.snapshot.instance_id.clone())
            .collect()
    } else {
        vec![]
    };
    models::sort_provider_models(
        &mut items,
        |key| (&key.instance_id, key.model.as_str()),
        &order,
        &favorites,
        tab.is_some(),
    );
    let legacy = tab.is_some()
        && items.iter().any(|key| {
            data.model(key).is_some_and(|o| o.is_legacy) && !data.favorites.contains(key)
        });
    if legacy {
        let (mut current, older): (Vec<_>, Vec<_>) = items.into_iter().partition(|key| {
            !data.model(key).is_some_and(|o| o.is_legacy) || data.favorites.contains(key)
        });
        if tab.is_some_and(|id| expanded.contains(id)) {
            current.extend(older);
        }
        items = current;
    }
    (items, legacy)
}

fn tallest_tab_height(data: &PickerData) -> f32 {
    let mut tallest = visible_models(data, None, "", &BTreeSet::new(), None)
        .0
        .len();
    for provider in &data.providers {
        let (rows, legacy) = visible_models(
            data,
            Some(&provider.snapshot.instance_id),
            "",
            &BTreeSet::new(),
            None,
        );
        tallest = tallest.max(rows.len() + usize::from(legacy));
    }
    (63.0 + 32.0 * tallest as f32 + 2.0 * tallest.saturating_sub(1) as f32).min(346.0)
}

fn initial_expanded(data: &PickerData) -> BTreeSet<ProviderInstanceId> {
    if data
        .model(&data.selection)
        .is_some_and(|option| option.is_legacy)
        && !data.favorites.contains(&data.selection)
    {
        BTreeSet::from([data.selection.instance_id.clone()])
    } else {
        BTreeSet::new()
    }
}

#[derive(Clone, Copy)]
pub struct PickerHandle(PopoverHandle);

pub fn model_picker_create_handle() -> PickerHandle {
    PickerHandle(popover::popover_create_handle())
}

impl PickerHandle {
    pub fn set_open(self, open: bool) {
        self.0.set_open(open);
    }

    pub fn is_open(self) -> bool {
        self.0.is_open()
    }
}

#[derive(Clone, Copy)]
struct PickerState {
    popup: PopoverHandle,
    query: Signal<String>,
    tab: Signal<Option<ProviderInstanceId>>,
    expanded: Signal<BTreeSet<ProviderInstanceId>>,
}

fn can_pick(props: &PickerProps, key: &ModelKey) -> bool {
    if props.disabled_reason.get().is_some() || props.catalog.get() != CatalogState::Ready {
        return false;
    }
    let data = props.data.get();
    data.provider(&key.instance_id).is_some_and(|p| {
        data.matches_lock(p)
            && p.options
                .iter()
                .any(|o| o.slug == key.model && include_option(&data, p, o))
    }) && !data.disabled_models.contains_key(key)
}

fn pick_model(props: &PickerProps, state: PickerState, key: ModelKey, additive: bool) {
    if !can_pick(props, &key) {
        return;
    }
    if additive && let Some(toggle) = &props.on_toggle_model {
        toggle(key);
    } else {
        (props.on_pick)(key);
    }
    if !props.docked {
        state.popup.set_open(false);
    }
}

pub fn provider_model_picker(props: PickerProps) -> impl IntoElement {
    let docked = props.docked;
    let props = Rc::new(props);
    let state = PickerState {
        popup: props.handle.unwrap_or_else(model_picker_create_handle).0,
        query: signal(String::new()),
        tab: signal(initial_tab(
            &props.data.get_untracked(),
            props.on_open_provider_setup.is_some(),
        )),
        expanded: signal(initial_expanded(&props.data.get_untracked())),
    };
    let opening = props.clone();
    let was_open = stored(false);
    // Refresh is an external service action; palette and keybindings can open this handle too.
    effect(move || {
        let open = state.popup.is_open();
        let previous = was_open.replace(open);
        if open && !previous {
            untracked(|| {
                state.query.set(String::new());
                state.tab.set(initial_tab(
                    &opening.data.get_untracked(),
                    opening.on_open_provider_setup.is_some(),
                ));
                if let Some(refresh) = &opening.on_refresh {
                    refresh();
                }
            });
        }
    });
    let label_props = props.clone();
    let content_props = props.clone();
    let trigger = nsh_base::button()
        .accessibility_label(move || Some(trigger_label(&label_props, false).into()))
        .disabled({ let props = props.clone(); move || props.disabled_reason.get().is_some() })
        // ComposerControl's selectors move to children; its pseudo hit area becomes hit_slop.
        // NSH has no text-box trimming or DOM app-region semantics; those hooks are omitted.
        .app_class(tw(&cn([Some("relative inline-flex shrink-0 cursor-pointer items-center justify-center whitespace-nowrap rounded-(--control-radius) border border-transparent text-base outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-64 h-7 gap-1.5 px-2 font-medium text-foreground/80 hover:text-foreground sm:text-sm min-w-0 shrink justify-between"), props.compact.then_some("max-w-42 shrink-0")])))
        .hit_slop(move || EdgeInsets::all(px(if pointer() == Pointer::Coarse { 8.0 } else { 0.0 })))
        .child(view().app_class(tw("flex min-w-0 flex-1 items-center gap-1.5")).child(dynamic(move || {
            let data = props.data.get();
            let selected = data.selected_models.as_ref().map_or_else(|| vec![data.selection.clone()], Clone::clone);
            let mut row = view().app_class(tw("flex min-w-0 flex-1 items-center gap-1.5"));
            if props.trigger_label.get().is_none() {
                for key in selected.iter().take(3) { if let Some(provider) = data.provider(&key.instance_id) { row = row.child(provider_instance_icon(provider, props.provider_glyph.as_ref(), 16.0)); } }
            }
            row.child(text(trigger_label(&props, true)).app_class(tw("min-w-0 flex-1 overflow-hidden truncate max-w-56"))).child(describe_composer_effort(props.traits.get())).into_element()
        })));
    popover::popover(
        state.popup,
        PopoverOptions {
            side: if docked {
                Placement::Top
            } else {
                Placement::Bottom
            },
            align: Align::Start,
            padding: Padding::None,
            touch_backdrop: true,
            ..Default::default()
        },
        trigger,
        move || model_picker_content(content_props.clone(), state).into_element(),
    )
}

fn trigger_label(props: &PickerProps, short: bool) -> String {
    if let Some(label) = props.trigger_label.get() {
        return label;
    }
    let data = props.data.get();
    if let Some(selected) = &data.selected_models {
        if selected.is_empty() {
            return t("Choose models");
        }
        let labels = selected
            .iter()
            .take(if short { 2 } else { usize::MAX })
            .map(|key| trigger_title(&data, key, short))
            .collect::<Vec<_>>()
            .join(", ");
        return if short && selected.len() > 2 {
            tr(
                "{models}, {count} more",
                &[
                    ("models", Value::Text(labels)),
                    ("count", Value::Int((selected.len() - 2) as i64)),
                ],
            )
        } else {
            labels
        };
    }
    trigger_title(&data, &data.selection, short)
}

/// Port of ProviderInstanceIcon.tsx; branded artwork is supplied by UI-ICONS through this slot.
fn provider_instance_icon(
    provider: &PickerProvider,
    glyph: Option<&ProviderGlyph>,
    size: f32,
) -> AnyElement {
    if let Some(glyph) = glyph {
        return glyph(provider, size);
    }
    // The frozen icon module contains Lucide only; use the client's initials fallback.
    text(provider_instance_initials(&provider.display_name))
        .accessible(false)
        .w(px(size))
        .h(px(size))
        .app_class(tw("shrink-0 text-xs"))
        .into_element()
}

/// Port of ModelPickerContent.tsx ModelPickerContent.
fn model_picker_content(props: Rc<PickerProps>, state: PickerState) -> impl IntoElement {
    let body_props = props.clone();
    view().app_class(tw("flex flex-col"))
        .child(view().app_class(tw(if props.docked { "flex h-86.5 min-h-0 max-sm:h-auto max-sm:flex-col" } else { "flex max-h-86.5 min-h-0" }))
            .when(!props.docked, |row| { let props = props.clone(); row.h(move || px(tallest_tab_height(&props.data.get()))) })
            .child(view().app_class(tw(if props.docked { "relative flex w-screen max-w-90 flex-row overflow-hidden max-sm:h-56 max-sm:w-full max-sm:max-w-none" } else { "relative flex w-screen max-w-90 flex-row overflow-hidden" }))
                .child(dynamic(move || { let data = body_props.data.get(); if !state.query.get().trim().is_empty() || data.providers.iter().all(|p| !p.enabled) { return None; } Some(model_picker_sidebar(body_props.clone(), state)) }))
                .child(model_picker_list(props.clone(), state)))
            .when(props.docked, |row| row.child(view().app_class(tw("flex min-h-0 w-48 flex-col sm:shrink-0 border-l border-border/70 bg-muted/40 max-sm:h-64 max-sm:w-full max-sm:border-t max-sm:border-l-0")).child(composer_effort_column(props.clone())))))
        .when(props.docked, |root| root.child(composer_picker_footer(props.clone())))
}

/// Port of ModelPickerSidebar.tsx ModelPickerSidebar.
fn model_picker_sidebar(props: Rc<PickerProps>, state: PickerState) -> impl IntoElement {
    let data = props.data.get();
    let mut providers: Vec<_> = data.providers.iter().filter(|p| p.enabled).collect();
    providers.sort_by_key(|p| !data.matches_lock(p));
    let mut rail = view().app_class(tw("relative flex min-h-full flex-col gap-1 p-1"))
        .child(nsh_base::button().accessibility_label(t("Favorites")).app_class(tw("relative flex w-full cursor-pointer aspect-square items-center justify-center rounded-md hover:bg-foreground/10 focus-visible:bg-foreground/10 focus-visible:outline-none"))
            .on_press(move || state.tab.set(None)).child(text(t("Favorites")).app_class(tw("text-xs"))))
        .child(view().app_class(tw("border-b border-border/70")));
    for provider in providers {
        let id = provider.snapshot.instance_id.clone();
        let selected_id = id.clone();
        let disabled = !data.matches_lock(provider)
            || (!provider.ready
                && !provider
                    .options
                    .iter()
                    .any(|o| include_option(&data, provider, o))
                && !(props.on_open_provider_setup.is_some()
                    && should_offer_model_picker_setup(provider)));
        let reason = if !data.matches_lock(provider) {
            tr(
                "{provider} is unavailable in this thread. Start a new thread to switch providers.",
                &[("provider", Value::Text(provider.display_name.clone()))],
            )
        } else {
            provider_status(provider)
        };
        rail = rail.child(view().app_class(tw("relative w-full"))
            .child(nsh_base::button().accessibility_label(reason.clone()).disabled(disabled).on_press(move || if !disabled { state.tab.set(Some(id.clone())); })
                .app_class(tw(&cn([Some("relative flex w-full cursor-pointer aspect-square items-center justify-center rounded-md hover:bg-foreground/10 focus-visible:bg-foreground/10 focus-visible:outline-none"), disabled.then_some("opacity-50 cursor-not-allowed hover:bg-transparent")])))
                .child(provider_instance_icon(provider, props.provider_glyph.as_ref(), 20.0)))
            .child(dynamic(move || (state.tab.get().as_ref() == Some(&selected_id)).then(|| view().app_class(tw("pointer-events-none absolute right-1 z-10 h-5 w-0.75 rounded-full bg-primary")).top(px(8.0))))));
    }
    view()
        .label(t("Providers"))
        .app_class(tw("w-11 shrink-0 overflow-hidden bg-muted/30"))
        .child(scroll().child(rail))
}

fn provider_status(provider: &PickerProvider) -> String {
    if provider.ready {
        return provider.display_name.clone();
    }
    let state = if !provider.enabled || provider.snapshot.status == ServerProviderState::Disabled {
        t("Disabled in settings.")
    } else if provider.snapshot.status == ServerProviderState::Error {
        t("Unavailable")
    } else if provider.snapshot.status == ServerProviderState::Warning {
        t("Limited")
    } else {
        t("Not ready")
    };
    let message = provider
        .snapshot
        .message
        .as_ref()
        .and_then(Option::as_ref)
        .map_or("", |s| s.0.as_str());
    // The provider's name and status message are content.
    [provider.display_name.clone(), state, message.to_owned()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(": ")
}

/// Port of ModelPickerContent.tsx ModelPickerList; NSH owns virtual row recycling.
fn model_picker_list(props: Rc<PickerProps>, state: PickerState) -> impl IntoElement {
    let query_props = props.clone();
    let rows_props = props.clone();
    let search = nsh_base::input()
        .label(t("Search models..."))
        .placeholder(t("Search models..."))
        .value(move || state.query.get())
        .disabled(props.search.is_none())
        .on_change(move |query: &str| {
            if query_props.search.is_some() {
                state.query.set(query.into());
            }
        })
        .app_class(tw(
            "h-8.25 min-w-0 border-b border-border/70 bg-transparent text-sm",
        ));
    view().app_class(tw("flex min-h-0 flex-1 flex-col overflow-hidden bg-muted/40 border-l border-border/70"))
        .child(view().app_class(tw("shrink-0 px-3 pt-2.5")).child(search))
        .child(dynamic(move || if rows_props.search.is_none() { Some(text(t("Model search is unavailable on this client.")).app_class(tw("px-3 py-1 text-xs text-muted-foreground"))) } else { None }))
        .child(dynamic({ let props = props.clone(); move || {
            let data = props.data.get();
            let (models, legacy) = visible_models(&data, state.tab.get().as_ref(), &state.query.get(), &state.expanded.get(), props.search.as_ref());
            let rows = Rc::new(models); let count = rows.len(); let rows_props = props.clone();
            let list = virtual_list(count, move |at| {
                let Some(key) = rows.get(at.index()) else { return view().into_element(); };
                model_list_row(rows_props.clone(), state, key.clone()).into_element()
            }).estimated_item_size(px(34.0)).flex_1();
            let mut column = view().app_class(tw("flex min-h-0 flex-1 flex-col overflow-hidden pr-px")).child(list);
            if legacy && let Some(tab) = state.tab.get() {
                let expanded = state.expanded.get().contains(&tab);
                column = column.child(nsh_base::button().accessibility_label(t(if expanded { "Show less" } else { "Show more" }))
                    .on_press(move || state.expanded.update(|set| { if !set.remove(&tab) { set.insert(tab.clone()); } }))
                    .app_class(tw("h-8 w-full cursor-pointer px-2 text-sm text-muted-foreground hover:text-foreground")).child(text(t(if expanded { "Show less" } else { "Show more" }))));
            }
            column.child(model_picker_provider_notices(props.clone(), state, count == 0)).into_element()
        }}))
}

/// Port of ModelListRow.tsx ModelListRow.
fn model_list_row(props: Rc<PickerProps>, state: PickerState, key: ModelKey) -> impl IntoElement {
    let data = props.data.get();
    let Some(model) = data.model(&key) else {
        return view().into_element();
    };
    let Some(provider) = data.provider(&key.instance_id) else {
        return view().into_element();
    };
    let label = format!(
        "{}, {}",
        display_name(model, false),
        provider.display_name
    );
    let disabled = !can_pick(&props, &key);
    let selected = data
        .selected_models
        .as_ref()
        .map_or_else(|| data.selection == key, |models| models.contains(&key));
    let favorite = data.favorites.contains(&key);
    let pick_props = props.clone();
    let pick_key = key.clone();
    let mut content = view()
        .app_class(tw("flex min-w-0 flex-1 items-center gap-2 text-left"))
        .child(provider_instance_icon(
            provider,
            props.provider_glyph.as_ref(),
            16.0,
        ))
        .child(
            text(display_name(model, false))
                .app_class(tw("min-w-0 truncate text-sm font-normal leading-snug")),
        );
    if selected {
        content = content.child(icon(Icon::Check, 14.0, move || palette().foreground));
    }
    if let Some(sub) = &model.sub_provider {
        content = content.child(text(sub.clone()).app_class(tw(
            "min-w-0 shrink truncate text-xs text-muted-foreground/70",
        )));
    }
    if model.badge_new {
        content = content.child(text(t("New")).label(t("New model")).app_class(tw("shrink-0 rounded border border-update/35 bg-update/15 px-0.5 py-px text-3xs font-bold uppercase leading-none tracking-wide text-update-foreground")));
    }
    if model.is_unavailable {
        content =
            content.child(text(t("Unavailable")).app_class(tw("text-xs text-muted-foreground")));
    }
    let mut row = view().app_class(tw("flex h-8 w-full items-center gap-1.5 pl-2 pr-px"))
        .child(nsh_base::button().accessibility_label(label).disabled(disabled).on_press(move || pick_model(&pick_props, state, pick_key.clone(), false))
            .app_class(tw("group relative flex min-w-0 flex-1 cursor-pointer items-center rounded-sm px-2 py-1 text-sm outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-64")).child(content));
    if let Some(toggle) = props.on_toggle_favorite.clone() {
        let favorite_key = key.clone();
        let favorite_props = props.clone();
        row = row.child(
            nsh_base::button()
                .accessibility_label(t(if favorite {
                    "Remove from favorites"
                } else {
                    "Add to favorites"
                }))
                .disabled(disabled)
                .on_press(move || {
                    if can_pick(&favorite_props, &favorite_key) {
                        toggle(favorite_key.clone());
                    }
                })
                // UI-ICONS has not exposed Star; the translated action is an explicit interim control.
                .child(
                    text(t(if favorite {
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
                    }))
                    .app_class(tw("text-xs")),
                ),
        );
    }
    if let Some(toggle) = props.on_toggle_model.clone() {
        row = row.child(
            nsh_base::button()
                .accessibility_label(t("Toggle model selection"))
                .disabled(disabled)
                .on_press(move || {
                    if can_pick(&props, &key) {
                        toggle(key.clone());
                    }
                })
                .child(icon(Icon::Plus, 14.0, move || palette().foreground)),
        );
    }
    row.into_element()
}

/// Port of ModelPickerContent.tsx ModelPickerProviderNotices.
fn model_picker_provider_notices(
    props: Rc<PickerProps>,
    state: PickerState,
    empty: bool,
) -> impl IntoElement {
    let reason = props
        .disabled_reason
        .get()
        .or_else(|| props.catalog.get().reason());
    let data = props.data.get();
    let mut notices = view();
    if let Some(reason) = reason {
        notices = notices.child(
            text(reason).app_class(tw("px-3 py-2 text-xs leading-snug text-muted-foreground")),
        );
    } else if empty {
        notices = notices.child(
            text(t(
                if state.tab.get().is_none() && state.query.get().trim().is_empty() {
                    "No favorites yet"
                } else {
                    "No models found"
                },
            ))
            .app_class(tw("px-3 py-2 text-sm text-muted-foreground")),
        );
    }
    if state.query.get().trim().is_empty()
        && let Some(setup) = &props.on_open_provider_setup
    {
        for provider in data.providers.iter().filter(|p| {
            p.enabled
                && should_offer_model_picker_setup(p)
                && data.matches_lock(p)
                && state
                    .tab
                    .get()
                    .as_ref()
                    .is_none_or(|id| id == &p.snapshot.instance_id)
        }) {
            let setup = setup.clone();
            let id = provider.snapshot.instance_id.clone();
            notices =
                notices.child(
                    view()
                        .app_class(tw("border-t border-border/70 p-2"))
                        .child(text(provider_status(provider)).app_class(tw(
                            "px-1 py-1.5 text-xs leading-snug text-muted-foreground",
                        )))
                        .child(
                            nsh_base::button()
                                .accessibility_label(t("Open provider setup"))
                                .on_press(move || {
                                    state.popup.set_open(false);
                                    setup(id.clone());
                                })
                                .child(text(t("Open provider setup"))),
                        ),
                );
        }
    }
    notices
}

/// Port of ComposerModelEffort.tsx describeComposerEffort; the client supplies the summary.
pub fn describe_composer_effort(traits: TraitsView) -> impl IntoElement {
    view().child(traits.summary.map(|summary| {
        text(summary).app_class(tw(
            "flex shrink-0 items-center gap-1.5 text-muted-foreground -ms-0.5 max-w-24 truncate",
        ))
    }))
}

fn descriptor_label(descriptor: &ProviderOptionDescriptor) -> &str {
    match descriptor {
        ProviderOptionDescriptor::Select(d) => &d.label,
        ProviderOptionDescriptor::Boolean(d) => &d.label,
    }
}

fn options_enabled(props: &PickerProps, traits: &TraitsView) -> bool {
    traits.model.as_ref() == Some(&props.data.get().selection)
        && !traits.unavailable
        && traits.disabled_reason.is_none()
        && props.disabled_reason.get().is_none()
        && props.catalog.get() == CatalogState::Ready
        && props.on_option_pick.is_some()
        && can_pick(props, &props.data.get().selection)
}

fn pick_option(props: &PickerProps, model: &ModelKey, selection: ProviderOptionSelection) {
    let traits = props.traits.get();
    if !options_enabled(props, &traits) || traits.model.as_ref() != Some(model) {
        return;
    }
    let offered = traits
        .reasoning
        .iter()
        .chain(&traits.others)
        .chain(traits.speed.iter())
        .chain(traits.ultracode.iter())
        .any(|descriptor| match (descriptor, &selection.value) {
            (ProviderOptionDescriptor::Select(d), ProviderOptionSelectionValue::String(value)) => {
                d.id == selection.id && d.options.iter().any(|choice| &choice.id == value)
            }
            (ProviderOptionDescriptor::Boolean(d), ProviderOptionSelectionValue::Boolean(_)) => {
                d.id == selection.id
            }
            _ => false,
        });
    if !offered {
        return;
    }
    if let Some(pick) = &props.on_option_pick {
        pick(model.clone(), selection);
    }
}

const COLUMN_ROW: &str = "flex h-8 w-full shrink-0 cursor-pointer items-center gap-2 rounded-sm px-2 text-left text-sm text-foreground outline-none hover:bg-foreground/[0.09] focus-visible:bg-foreground/[0.09] disabled:cursor-not-allowed disabled:opacity-64";

/// Port of ComposerModelEffort.tsx ComposerEffortColumn.
pub fn composer_effort_column(props: Rc<PickerProps>) -> impl IntoElement {
    dynamic(move || {
        let traits = props.traits.get();
        let column = view().app_class(tw("flex min-h-0 flex-1 flex-col")).child(
            view().app_class(tw("shrink-0 px-3 pt-2.5")).child(
                view()
                    .app_class(tw(
                        "flex h-8.25 items-center gap-2 border-b border-border/70 pb-1.5 text-sm",
                    ))
                    .child(
                        // A provider's descriptor label is content; the fallbacks are ours.
                        text(traits.reasoning.as_ref().map_or_else(
                            || {
                                if traits.others.is_empty() {
                                    t("Effort")
                                } else {
                                    t("Options")
                                }
                            },
                            |descriptor| descriptor_label(descriptor).to_owned(),
                        ))
                        .app_class(tw("text-muted-foreground")),
                    )
                    .child(
                        text(trigger_title(
                            &props.data.get(),
                            &props.data.get().selection,
                            true,
                        ))
                        .app_class(tw("ms-auto min-w-0 truncate text-muted-foreground text-xs")),
                    ),
            ),
        );
        let mut levels = view().app_class(tw(
            "flex min-h-0 flex-1 flex-col gap-0.5 px-2 pt-2.5 pb-1.5",
        ));
        if let Some(reason) = &traits.disabled_reason {
            levels = levels.child(
                // PickerProps::new's own English default, or a client sentence.
                text(t(reason)).app_class(tw("px-2 pb-1.5 text-muted-foreground/80 text-xs")),
            );
        }
        if let Some(effort) = &traits.reasoning {
            levels = levels.child(effort_rows(props.clone(), &traits, effort));
        }
        for other in &traits.others {
            levels = levels.child(option_row(props.clone(), &traits, other));
        }
        if traits.reasoning.is_none() && traits.others.is_empty() {
            levels = levels.child(text(t("No effort levels")).app_class(tw(
                "flex h-8 items-center px-2 text-muted-foreground text-sm",
            )));
        }
        column.child(scroll().flex_1().child(levels))
    })
}

/// Port of ComposerModelEffort.tsx EffortRows and ColumnRow.
fn effort_rows(
    props: Rc<PickerProps>,
    traits: &TraitsView,
    descriptor: &ProviderOptionDescriptor,
) -> AnyElement {
    let disabled = !options_enabled(&props, traits);
    let Some(model) = &traits.model else {
        return view().into_element();
    };
    let choices: Vec<_> = match descriptor {
        ProviderOptionDescriptor::Select(d) => d
            .options
            .iter()
            .map(|o| {
                (
                    o.label.clone(),
                    ProviderOptionSelectionValue::String(o.id.clone()),
                    d.current_value.as_ref().and_then(Option::as_ref) == Some(&o.id),
                    o.is_default == Some(Some(true)),
                )
            })
            .collect(),
        ProviderOptionDescriptor::Boolean(d) => [true, false]
            .into_iter()
            .map(|value| {
                (
                    if value { "On" } else { "Off" }.into(),
                    ProviderOptionSelectionValue::Boolean(value),
                    (d.current_value.flatten() == Some(true)) == value,
                    false,
                )
            })
            .collect(),
    };
    let mut group = view()
        .role(AccessibilityRole::Radiogroup)
        .label(descriptor_label(descriptor).to_owned())
        .app_class(tw("flex flex-col gap-0.5"));
    for (label, value, checked, default) in choices {
        let props = props.clone();
        let model = model.clone();
        let id = match descriptor {
            ProviderOptionDescriptor::Select(d) => d.id.clone(),
            ProviderOptionDescriptor::Boolean(d) => d.id.clone(),
        };
        group =
            group.child(
                nsh_base::radio()
                    .accessibility_label(label.clone())
                    .checked(checked)
                    .disabled(disabled)
                    .app_class(tw(COLUMN_ROW))
                    .on_change(move |_: &bool| {
                        pick_option(
                            &props,
                            &model,
                            ProviderOptionSelection {
                                id: id.clone(),
                                value: value.clone(),
                            },
                        )
                    })
                    .child(text(label.clone()).app_class(tw("min-w-0 flex-1 truncate")))
                    .child(checked.then(|| icon(Icon::Check, 14.0, move || palette().foreground)))
                    .child((default && !checked).then(|| {
                        text(t("Default")).app_class(tw("text-muted-foreground/70 text-xs"))
                    })),
            );
    }
    group.into_element()
}

/// Port of ComposerModelEffort.tsx OptionMenuRow / OptionSwitchRow; select owns dismissal/focus.
fn option_row(
    props: Rc<PickerProps>,
    traits: &TraitsView,
    descriptor: &ProviderOptionDescriptor,
) -> AnyElement {
    let Some(model) = traits.model.clone() else {
        return view().into_element();
    };
    match descriptor {
        ProviderOptionDescriptor::Select(d) => {
            let mut select = SelectProps::new(d.label.clone());
            select.variant = select::Variant::Ghost;
            select.size = select::Size::Compact;
            select.items = d
                .options
                .iter()
                .map(|option| SelectItem::new(option.id.clone(), option.label.clone()))
                .collect::<Vec<_>>()
                .into();
            select.value = select::Value::Controlled(
                d.current_value.clone().flatten().map(SharedString::from),
            )
            .into();
            let enabled = props.clone();
            select.disabled = Prop::bind(move || !options_enabled(&enabled, &enabled.traits.get()));
            let id = d.id.clone();
            view()
                .child(text(d.label.clone()).app_class(tw(
                    "px-2 pt-1.5 pb-1 font-medium text-muted-foreground text-xs",
                )))
                .child(select::select(select, move |next| {
                    if let Some(value) = next {
                        pick_option(
                            &props,
                            &model,
                            ProviderOptionSelection {
                                id: id.clone(),
                                value: ProviderOptionSelectionValue::String(value.to_string()),
                            },
                        );
                    }
                }))
                .into_element()
        }
        ProviderOptionDescriptor::Boolean(d) => {
            let id = d.id.clone();
            let current = d.current_value.flatten() == Some(true);
            let enabled = props.clone();
            view()
                .app_class(tw("flex h-auto w-full items-center gap-2 px-2 py-1"))
                .child(
                    text(d.label.clone())
                        .app_class(tw("min-w-0 flex-1 truncate text-xs text-muted-foreground")),
                )
                .child(switch::switch(
                    d.label.clone(),
                    ControlState {
                        checked: current.into(),
                        disabled: Prop::bind(move || {
                            !options_enabled(&enabled, &enabled.traits.get())
                        }),
                        ..Default::default()
                    },
                    switch::Size::Sm,
                    move |value| {
                        pick_option(
                            &props,
                            &model,
                            ProviderOptionSelection {
                                id: id.clone(),
                                value: ProviderOptionSelectionValue::Boolean(value),
                            },
                        )
                    },
                ))
                .into_element()
        }
    }
}

/// Port of ComposerModelEffort.tsx ComposerPickerFooter. Permissions mount via the footer slot.
pub fn composer_picker_footer(props: Rc<PickerProps>) -> impl IntoElement {
    dynamic(move || {
        let traits = props.traits.get();
        let mut footer = view().app_class(tw(
            "flex flex-wrap items-center gap-2 border-t border-border/70 bg-muted/40 px-3 py-2",
        ));
        if !traits.unavailable {
            if let Some(speed) = &traits.speed {
                footer = footer.child(speed_control(props.clone(), &traits, speed));
            }
            if let Some(ultracode) = &traits.ultracode {
                footer = footer.child(option_row(props.clone(), &traits, ultracode));
            }
        }
        if let Some(slot) = &props.footer {
            footer = footer.child(untracked(|| slot()));
        }
        footer
    })
}

/// Port of ComposerModelEffort.tsx SpeedControl and SpeedSegment.
fn speed_control(
    props: Rc<PickerProps>,
    traits: &TraitsView,
    descriptor: &ProviderOptionDescriptor,
) -> AnyElement {
    let Some(model) = &traits.model else {
        return view().into_element();
    };
    let (id, choices): (_, Vec<_>) = match descriptor {
        ProviderOptionDescriptor::Select(d) => (
            &d.id,
            d.options
                .iter()
                .map(|choice| {
                    (
                        choice.label.clone(),
                        ProviderOptionSelectionValue::String(choice.id.clone()),
                        d.current_value.as_ref().and_then(Option::as_ref) == Some(&choice.id),
                    )
                })
                .collect(),
        ),
        ProviderOptionDescriptor::Boolean(d) => (
            &d.id,
            [false, true]
                .into_iter()
                .map(|value| {
                    (
                        if value { "Fast" } else { "Standard" }.into(),
                        ProviderOptionSelectionValue::Boolean(value),
                        (d.current_value.flatten() == Some(true)) == value,
                    )
                })
                .collect(),
        ),
    };
    let mut group = view()
        .role(AccessibilityRole::Radiogroup)
        .label(descriptor_label(descriptor).to_owned())
        .app_class(tw("flex rounded-lg bg-input/40 p-0.5"));
    for (label, value, checked) in choices {
        let props = props.clone();
        let model = model.clone();
        let id = id.clone();
        group = group.child(nsh_base::radio().checked(checked).disabled(!options_enabled(&props, traits)).accessibility_label(label.clone())
            .app_class(tw("flex h-6 cursor-pointer items-center justify-center gap-1 px-2 text-muted-foreground text-xs outline-none hover:text-foreground checked:bg-input/72 checked:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"))
            .on_change(move |_: &bool| pick_option(&props, &model, ProviderOptionSelection { id: id.clone(), value: value.clone() }))
            .child(text(label.clone())));
    }
    group.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(driver: &str) -> PickerProvider {
        let snapshot = serde_json::from_value(serde_json::json!({
            "instanceId": driver, "driver": driver, "enabled": true, "installed": true,
            "version": null, "status": "ready", "auth": {"status":"authenticated"},
            "checkedAt":"2026-01-01T00:00:00.000Z", "models":[
                {"slug":"opus", "name":"Claude Opus", "shortName":"Opus", "isCustom":false, "capabilities":{}}
            ], "slashCommands":[], "skills":[]
        }))
        .expect("provider");
        PickerProvider {
            snapshot,
            display_name: driver.into(),
            enabled: true,
            available: true,
            ready: true,
            continuation_group: None,
            accent_color: None,
            acp_agent_id: None,
            acp_icon_url: None,
            options: vec![ModelOption {
                slug: "opus".into(),
                name: "Claude Opus".into(),
                ..Default::default()
            }],
        }
    }

    fn data(provider: PickerProvider) -> PickerData {
        PickerData {
            selection: ModelKey {
                instance_id: provider.snapshot.instance_id.clone(),
                model: "opus".into(),
            },
            providers: vec![provider],
            favorites: BTreeSet::new(),
            selected_models: None,
            locked_provider: None,
            locked_continuation_group: None,
            disabled_models: BTreeMap::new(),
        }
    }

    #[test]
    fn hidden_saved_models_keep_their_metadata_while_their_provider_cannot_run() {
        let mut entry = provider("claudeAgent");
        assert!(retained_model_while_provider_locked(Some(&entry), "opus").is_none());
        entry.enabled = false;
        let retained =
            retained_model_while_provider_locked(Some(&entry), "opus").expect("retained");
        assert_eq!(retained.name, "Claude Opus");
        assert_eq!(retained.short_name.as_deref(), Some("Opus"));
        entry.enabled = true;
        entry.available = false;
        assert!(retained_model_while_provider_locked(Some(&entry), "opus").is_some());
        let unknown =
            retained_model_while_provider_locked(Some(&entry), "saved").expect("unknown retained");
        assert_eq!(
            (unknown.slug.as_str(), unknown.name.as_str()),
            ("saved", "saved")
        );
        assert!(retained_model_while_provider_locked(None, "opus").is_none());
        assert!(retained_model_while_provider_locked(Some(&entry), "").is_none());
        for driver in ["opencode", "antigravity"] {
            entry.snapshot.driver.0 = driver.into();
            assert!(retained_model_while_provider_locked(Some(&entry), "saved").is_none());
        }
    }

    #[test]
    fn favorites_and_legacy_sections_keep_catalog_order_and_restore_hidden_rows() {
        let mut data = data(provider("codex"));
        data.providers[0].options.extend([
            ModelOption {
                slug: "old".into(),
                is_legacy: true,
                ..Default::default()
            },
            ModelOption {
                slug: "starred".into(),
                is_legacy: true,
                ..Default::default()
            },
        ]);
        let favorite = ModelKey {
            instance_id: data.selection.instance_id.clone(),
            model: "starred".into(),
        };
        data.favorites.insert(favorite.clone());
        assert_eq!(initial_tab(&data, false), None);
        let tab = &data.selection.instance_id;
        let (collapsed, legacy) = visible_models(&data, Some(tab), "", &BTreeSet::new(), None);
        assert!(legacy);
        assert_eq!(
            collapsed
                .iter()
                .map(|k| k.model.as_str())
                .collect::<Vec<_>>(),
            ["starred", "opus"]
        );
        let expanded = BTreeSet::from([tab.clone()]);
        let (expanded, _) = visible_models(&data, Some(tab), "", &expanded, None);
        assert_eq!(
            expanded
                .iter()
                .map(|k| k.model.as_str())
                .collect::<Vec<_>>(),
            ["starred", "opus", "old"]
        );
        assert_eq!(
            visible_models(&data, None, "", &BTreeSet::new(), None).0,
            [favorite]
        );
        data.providers[0].ready = false;
        data.providers[0].enabled = false;
        assert_eq!(initial_tab(&data, false).as_ref(), Some(tab));
    }

    #[test]
    fn searching_preserves_rank_but_drops_stale_unknown_and_locked_results() {
        let mut data = data(provider("codex"));
        let mut other = provider("claudeAgent");
        other.continuation_group = Some("other".into());
        data.providers.push(other);
        data.locked_provider = Some("codex".into());
        let allowed = data.selection.clone();
        let denied = ModelKey {
            instance_id: ProviderInstanceId("claudeAgent".into()),
            model: "opus".into(),
        };
        let unknown = ModelKey {
            instance_id: ProviderInstanceId("gone".into()),
            model: "opus".into(),
        };
        let result = vec![denied, allowed.clone(), unknown];
        let search: Search = Rc::new(move |_, _| result.clone());
        assert_eq!(
            visible_models(&data, None, "opus", &BTreeSet::new(), Some(&search)).0,
            [allowed]
        );
        data.locked_continuation_group = Some("different".into());
        assert!(
            visible_models(&data, None, "opus", &BTreeSet::new(), Some(&search))
                .0
                .is_empty()
        );
    }

    #[test]
    fn open_code_retains_only_its_active_unavailable_choice_when_not_ready() {
        let mut data = data(provider("opencode"));
        data.providers[0].ready = false;
        data.providers[0].options[0].is_unavailable = true;
        data.providers[0].options.push(ModelOption {
            slug: "unavailable-other".into(),
            is_unavailable: true,
            ..Default::default()
        });
        let tab = Some(&data.selection.instance_id);
        assert_eq!(
            visible_models(&data, tab, "", &BTreeSet::new(), None).0,
            [data.selection.clone()]
        );
        data.providers[0].enabled = false;
        assert!(
            visible_models(&data, tab, "", &BTreeSet::new(), None)
                .0
                .is_empty()
        );
    }
    #[test]
    fn antigravity_default_uses_available_alias_then_default_without_selecting_the_placeholder() {
        let options = vec![
            ModelOption {
                slug: ANTIGRAVITY_DEFAULT_MODEL.into(),
                is_default: true,
                ..Default::default()
            },
            ModelOption {
                slug: "hidden".into(),
                aliases: vec![ANTIGRAVITY_DEFAULT_MODEL.into()],
                is_unavailable: true,
                ..Default::default()
            },
            ModelOption {
                slug: "default".into(),
                is_default: true,
                ..Default::default()
            },
            ModelOption {
                slug: "alias".into(),
                aliases: vec![ANTIGRAVITY_DEFAULT_MODEL.into()],
                ..Default::default()
            },
        ];
        assert_eq!(
            resolve_model_picker_selected_model(
                Some("antigravity"),
                ANTIGRAVITY_DEFAULT_MODEL,
                &options
            )
            .map(|o| o.slug.as_str()),
            Some("alias")
        );
        assert_eq!(
            resolve_model_picker_selected_model(
                Some("antigravity"),
                ANTIGRAVITY_DEFAULT_MODEL,
                &options[..3]
            )
            .map(|o| o.slug.as_str()),
            Some("default")
        );
        assert!(
            resolve_model_picker_selected_model(
                Some("antigravity"),
                ANTIGRAVITY_DEFAULT_MODEL,
                &options[..2]
            )
            .is_none()
        );
    }
}
