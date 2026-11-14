//! Port of apps/web/src/routes/_chat.settings.tsx (SettingsRouteLayout, SettingsContentLayout,
//! SettingsScopeBoundary, RestoreDeviceDefaultsButton) and, from components/settings,
//! SettingsSidebarNav.tsx, SettingsScopeContext.tsx, SettingsBreadcrumb.tsx,
//! SettingsScopeSentence.tsx, SettingsGroup.tsx and settingsLayout.tsx (SettingsPageContainer,
//! SettingsSection, SettingsRow, SettingResetButton); also AppRouteModal.tsx,
//! WorkspacePageContainer.tsx and WorkspaceBreadcrumb.tsx, which only the settings route uses
//! here. The scope model, search, plans and writes are `t3_client::services::settings`.

use std::rc::Rc;

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use nsh_core::{KeyEvent, prelude::*};
use serde_json::Value;
use t3_client::services::settings::{
    panels::changed_setting_labels,
    scope::{
        ALL_VALUE, ScopeKindName, ScopeLabel, SettingsScopeSearch, environment_axis_value,
        file_basename, project_axis_value, project_scope_menu_groups, retain_settings_scope,
        select_checkout_axis, select_environment_axis, select_project_axis,
        settings_scope_checkout_choices, settings_scope_checkout_label,
        settings_scope_environment_address, settings_scope_environment_label,
        settings_scope_environment_labels, settings_scope_project_machines, settings_scope_query,
        validate_settings_route_search,
    },
    scoped::{
        ProjectOverrideEntry, ScopedSettingSource, SettingsPatch, SettingsText,
        is_project_scoped_setting_key, scoped_settings_are_mixed, scoped_settings_source,
    },
    search::{
        SettingsSearchAvailability, SettingsSearchItem, SettingsSearchPlatform,
        filter_available_settings_search_items, is_device_only_settings_page, moved_settings_path,
        page_of_path, settings_active_nav_page, settings_breadcrumb_label, settings_nav_groups,
        settings_path,
    },
    service::{SettingsScopeView, SettingsUpdateOutcome, SettingsWriteAccess},
};
use t3_contracts::settings::{ClientSettingsSchema, ServerSettings};

use super::search::{NavSearch, settings_search_field, settings_search_results};
use crate::i18n::{Text, t, tr};
use crate::runtime::Shared;
use crate::state::nav::{Navigator, Route, SettingsPage, navigator};
use crate::state::url::{UrlRequest, UrlState};
use crate::theme::{Scope, cn, palette, tw, tw_with};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon, icon_with_class};
use crate::ui::menu::{
    ItemOptions, MenuOptions, menu, menu_popup_with, menu_radio_group, menu_radio_item,
    menu_radio_item_indicator, menu_separator, menu_trigger,
};
use crate::ui::toast::{ToastOptions, Variant as ToastVariant, toast_manager};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// The settings pages' client surface. Implemented over `t3_client`'s `SettingsService` in
/// `runtime/services/settings.rs`; every rule lives there.
pub trait SettingsService {
    fn changes(&self) -> LocalBoxStream<'static, ()>;
    fn scope_view(&self, page: SettingsPage, search: &SettingsScopeSearch) -> SettingsScopeView;
    fn client_settings(&self) -> Rc<ClientSettingsSchema>;
    fn write_access(&self, view: &SettingsScopeView) -> SettingsWriteAccess;
    fn update(
        &self,
        view: &SettingsScopeView,
        patch: SettingsPatch,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome>;
    fn clear(
        &self,
        view: &SettingsScopeView,
        keys: Vec<&'static str>,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome>;
    fn clear_project_overrides(
        &self,
        view: &SettingsScopeView,
        entries: Vec<ProjectOverrideEntry>,
        keys: Vec<&'static str>,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome>;
    fn text_generation_targets(
        &self,
        view: &SettingsScopeView,
    ) -> t3_client::services::settings::models::TextGenerationTargets;
    fn model_disabled_reason(
        &self,
        view: &SettingsScopeView,
        instance: &t3_contracts::provider_instance::ProviderInstanceId,
        model: &str,
        text_generation: bool,
    ) -> Option<SettingsText>;
    fn last_enabled_grouping_mode(&self) -> LocalBoxFuture<'static, &'static str>;
    fn remember_enabled_grouping_mode(&self, mode: &str) -> LocalBoxFuture<'static, ()>;
    fn search_availability(&self) -> SettingsSearchAvailability;
}

/// The settings pages' services, built by the shell from `rt().backend().client()` (the only
/// source: there is no context override).
#[derive(Clone)]
pub struct SettingsRuntime(pub Rc<dyn SettingsService>);

/// Translates a client sentence with its named content arguments.
pub fn settings_text(text: &SettingsText) -> String {
    let args: Vec<(&str, t3_i18n::Value)> = text
        .args
        .iter()
        .map(|(name, value)| (*name, t3_i18n::Value::Text(value.clone())))
        .collect();
    tr(text.key, &args) // client English key
}

fn scope_label(label: &ScopeLabel) -> String {
    match label {
        ScopeLabel::Key(key) => Text::client_key(*key).resolve().to_string(),
        ScopeLabel::Content(text) => text.clone(),
    }
}

thread_local! {
    /// Without the URL service the settings address (scope and anchor) lives here: the frozen
    /// router rebuilds the shell per route, and the address must outlive that, as the URL does.
    static LOCAL_ADDRESS: std::cell::RefCell<(SettingsScopeSearch, String)> = Default::default();
}

/// Where the settings route reads and writes its address: the URL service when the app has
/// one, else the navigator with this route's own search and hash.
#[derive(Clone, Copy)]
struct SettingsLocation {
    urls: Option<UrlState>,
    nav: Navigator,
    local_search: Signal<SettingsScopeSearch>,
    local_hash: Signal<String>,
}

impl SettingsLocation {
    fn search(self) -> SettingsScopeSearch {
        match self.urls {
            Some(urls) => urls
                .location
                .with(|location| validate_settings_route_search(&location.search)),
            None => self.local_search.get(),
        }
    }

    fn raw_search(self) -> String {
        match self.urls {
            Some(urls) => urls.location.with(|location| location.search.clone()),
            None => settings_scope_query(&self.local_search.get_untracked()),
        }
    }

    /// The anchor, without `#`.
    fn hash(self) -> String {
        match self.urls {
            Some(urls) => urls
                .location
                .with(|location| location.hash.trim_start_matches('#').to_owned()),
            None => self.local_hash.get(),
        }
    }

    fn pathname(self) -> Option<String> {
        self.urls
            .map(|urls| urls.location.with(|location| location.pathname.clone()))
    }

    /// `navigate({ to, search, hash, replace: true })`; `None` keeps the scope
    /// (`retainSettingsScope`).
    fn navigate(self, page: SettingsPage, search: Option<&SettingsScopeSearch>, hash: &str) {
        match self.urls {
            Some(urls) => {
                let query = match search {
                    Some(search) => settings_scope_query(search),
                    None => retain_settings_scope(
                        &validate_settings_route_search(&untracked(|| self.raw_search())),
                        "",
                    ),
                };
                let hash = if hash.is_empty() {
                    String::new()
                } else {
                    format!("#{hash}")
                };
                urls.request(UrlRequest::Replace(format!(
                    "{}{query}{hash}",
                    settings_path(page)
                )));
            }
            None => batch(|| {
                let search = search
                    .cloned()
                    .unwrap_or_else(|| self.local_search.get_untracked());
                LOCAL_ADDRESS.with(|address| {
                    *address.borrow_mut() = (search.clone(), hash.to_owned());
                });
                self.local_search.set(search);
                self.local_hash.set(hash.to_owned());
                if self.nav.top() != Route::Settings(page) {
                    self.nav.open(Route::Settings(page));
                }
            }),
        }
    }

    /// `AppRouteModal`'s close: back to the page that opened it.
    fn close(self) {
        match self.urls {
            Some(urls) => urls.close_modal(),
            None => self.nav.back(),
        }
    }
}

/// Value equality, so an unchanged scope does not repaint the page on every client change.
#[derive(Clone, PartialEq)]
struct ViewRef(Rc<SettingsScopeView>);

/// `useSettingsScope()` plus the merged settings (`useScopedSettings`) and writes
/// (`useUpdateScopedSettings`). Provided by [`screen`]; read with [`use_settings`].
#[derive(Clone, Copy)]
pub struct SettingsContext {
    pub page: SettingsPage,
    revision: Signal<u64>,
    runtime: Stored<Option<SettingsRuntime>>,
    location: SettingsLocation,
    view: Memo<Option<ViewRef>>,
    client: Memo<Shared<ClientSettingsSchema>>,
    /// The page content's width, for the rows' `@min-[32rem]/settings-row` container query.
    pub content_width: Signal<f32>,
    /// Bumped by Restore device defaults to remount the content (`restoreSignal`).
    pub restore_signal: Signal<u64>,
    /// The settings screen's owner: writes started by a control outlive it (rule 7).
    owner: Owner,
}

pub fn use_settings() -> SettingsContext {
    use_context::<SettingsContext>().expect("the settings shell provides its context")
}

impl SettingsContext {
    fn new(
        page: SettingsPage,
        runtime: Option<SettingsRuntime>,
        location: SettingsLocation,
    ) -> Self {
        let revision = signal(0u64);
        if let Some(runtime) = runtime.clone() {
            let mut changes = runtime.0.changes();
            spawn(async move {
                while changes.next().await.is_some() {
                    revision.update(|value| *value += 1);
                }
            });
        }
        let runtime = stored(runtime);
        let view = computed(move || {
            revision.get();
            let search = location.search();
            runtime.with(|runtime| {
                runtime
                    .as_ref()
                    .map(|runtime| ViewRef(Rc::new(runtime.0.scope_view(page, &search))))
            })
        });
        let client = computed(move || {
            revision.get();
            Shared(runtime.with(|runtime| {
                runtime.as_ref().map_or_else(
                    || Rc::new(ClientSettingsSchema::default()),
                    |r| r.0.client_settings(),
                )
            }))
        });
        Self {
            page,
            revision,
            runtime,
            location,
            view,
            client,
            owner: Owner::current(),
            content_width: signal(0.0),
            restore_signal: signal(0),
        }
    }

    pub fn available(self) -> bool {
        self.runtime.with(|runtime| runtime.is_some())
    }

    pub fn view(self) -> Option<Rc<SettingsScopeView>> {
        self.view.get().map(|view| view.0)
    }

    fn view_untracked(self) -> Option<Rc<SettingsScopeView>> {
        self.view.get_untracked().map(|view| view.0)
    }

    /// The device half of `useScopedSettings()`.
    pub fn client_settings(self) -> Rc<ClientSettingsSchema> {
        self.client.get().0
    }

    /// The server half of `useScopedSettings()`: the scope's target, else the defaults.
    pub fn server_settings(self) -> Rc<ServerSettings> {
        self.view().map_or_else(
            || Rc::new(ServerSettings::default()),
            |view| view.server_settings(),
        )
    }

    /// One merged setting by its wire name (`mergeEnvironmentSettings`: server values win).
    pub fn setting(self, key: &str) -> Value {
        let server = serde_json::to_value(self.server_settings().as_ref()).unwrap_or_default();
        match server.get(key) {
            Some(value) => value.clone(),
            None => serde_json::to_value(self.client_settings().as_ref())
                .unwrap_or_default()
                .get(key)
                .cloned()
                .unwrap_or(Value::Null),
        }
    }

    /// `useScopedSettingsMixed(keys)`.
    pub fn mixed(self, keys: &[&str]) -> bool {
        self.view()
            .is_some_and(|view| scoped_settings_are_mixed(&view.targets, keys))
    }

    pub fn write_access(self) -> SettingsWriteAccess {
        match (self.view(), self.runtime.get()) {
            (Some(view), Some(runtime)) => runtime.0.write_access(&view),
            _ => SettingsWriteAccess::Disconnected,
        }
    }

    /// Opens another settings page with a scope (the web's `navigate({ to, search })`).
    pub fn open_page(self, page: SettingsPage, search: SettingsScopeSearch) {
        self.location.navigate(page, Some(&search), "");
    }

    /// `selectScope(next)`.
    pub fn select_scope(self, next: SettingsScopeSearch) {
        self.location.navigate(self.page, Some(&next), "");
    }

    /// `useUpdateScopedSettings()(patch)`.
    pub fn update(self, patch: Value) {
        self.update_with(patch, || {});
    }

    /// `updateSettings(patch, { onFailed })`.
    pub fn update_with(self, patch: Value, on_failed: impl Fn() + 'static) {
        let (Some(runtime), Some(view)) = (self.runtime.get(), self.view_untracked()) else {
            return;
        };
        let patch = patch.as_object().cloned().unwrap_or_default();
        let outcome = runtime.0.update(&view, patch);
        self.spawn(async move {
            let outcome = outcome.await;
            if outcome.failed {
                on_failed();
            }
            show_outcome(&outcome);
        });
    }

    /// Starts a write under the screen's owner: a select or menu disposes the control that
    /// picked the value as it closes, and the write must still run.
    pub fn spawn(self, task: impl std::future::Future<Output = ()> + 'static) {
        self.owner.run(move || {
            spawn(task);
        });
    }

    /// `useClearScopedSettings()(keys)`.
    pub fn clear(self, keys: Vec<&'static str>) {
        let (Some(runtime), Some(view)) = (self.runtime.get(), self.view_untracked()) else {
            return;
        };
        let outcome = runtime.0.clear(&view, keys);
        self.spawn(async move { show_outcome(&outcome.await) });
    }

    /// `useClearProjectOverrides()(entries, keys)`.
    pub fn clear_project_overrides(
        self,
        entries: Vec<ProjectOverrideEntry>,
        keys: Vec<&'static str>,
    ) {
        let (Some(runtime), Some(view)) = (self.runtime.get(), self.view_untracked()) else {
            return;
        };
        let outcome = runtime.0.clear_project_overrides(&view, entries, keys);
        self.spawn(async move { show_outcome(&outcome.await) });
    }

    /// Bumped by every client change, including reads `view()` compares equal (model lists).
    pub fn revision(self) -> u64 {
        self.revision.get()
    }

    /// `useTextGenerationTargets` for this scope's representative environment.
    pub fn text_generation_targets(
        self,
    ) -> t3_client::services::settings::models::TextGenerationTargets {
        self.revision();
        match (self.runtime.get(), self.view()) {
            (Some(runtime), Some(view)) => runtime.0.text_generation_targets(&view),
            _ => Default::default(),
        }
    }

    /// `useScopedModelDisabledReason`.
    pub fn model_disabled_reason(
        self,
        instance: &t3_contracts::provider_instance::ProviderInstanceId,
        model: &str,
        text_generation: bool,
    ) -> Option<SettingsText> {
        match (self.runtime.get(), self.view_untracked()) {
            (Some(runtime), Some(view)) => {
                runtime
                    .0
                    .model_disabled_reason(&view, instance, model, text_generation)
            }
            _ => None,
        }
    }

    pub fn last_enabled_grouping_mode(self) -> Option<LocalBoxFuture<'static, &'static str>> {
        self.runtime
            .get()
            .map(|runtime| runtime.0.last_enabled_grouping_mode())
    }

    pub fn remember_enabled_grouping_mode(self, mode: &str) {
        if let Some(runtime) = self.runtime.get() {
            let remember = runtime.0.remember_enabled_grouping_mode(mode);
            self.spawn(remember);
        }
    }

    /// The anchor a search jump asked for (`location.hash`).
    pub fn target_id(self) -> String {
        self.location.hash()
    }

    /// `clearTargetHash`.
    pub fn clear_target(self) {
        if !untracked(|| self.location.hash()).is_empty() {
            self.location.navigate(self.page, None, "");
        }
    }
}

fn show_outcome(outcome: &SettingsUpdateOutcome) {
    let Some(toast) = &outcome.toast else {
        return;
    };
    let description = toast
        .description
        .iter()
        .map(settings_text)
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    // Translated here with their arguments: content to the toast.
    toast_manager().add(ToastOptions {
        variant: if toast.error {
            ToastVariant::Error
        } else {
            ToastVariant::Warning
        },
        title: Text::content(settings_text(&toast.title)),
        description: Some(Text::content(description)),
        ..Default::default()
    });
}

/// `SETTINGS_SECTION_ICONS`.
pub fn section_icon(page: SettingsPage) -> Icon {
    match page {
        SettingsPage::General => Icon::Settings2,
        SettingsPage::Diagnostics | SettingsPage::Maintenance => Icon::Wrench,
        SettingsPage::Voice => Icon::Mic,
        SettingsPage::Appearance => Icon::Palette,
        SettingsPage::Chat => Icon::MessageSquareText,
        SettingsPage::Diffs => Icon::FileDiff,
        SettingsPage::Projects => Icon::PanelsTopLeft,
        SettingsPage::Keybindings => Icon::Keyboard,
        // The web draws its own `snap-shot` glyph; ScanLine is the nearest bundled frame.
        SettingsPage::SnapShot => Icon::ScanLine,
        SettingsPage::Providers => Icon::Bot,
        SettingsPage::Threads => Icon::MessagesSquare,
        SettingsPage::Browser => Icon::Globe,
        SettingsPage::Devices => Icon::Smartphone,
        SettingsPage::ScheduledTasks => Icon::CalendarClock,
        SettingsPage::SourceControl => Icon::GitBranch,
        SettingsPage::Worktrees => Icon::FolderTree,
        SettingsPage::Connections => Icon::Server,
        SettingsPage::Access => Icon::ShieldCheck,
        SettingsPage::Secrets => Icon::KeyRound,
        SettingsPage::Labs => Icon::FlaskConical,
        SettingsPage::About | SettingsPage::Licenses => Icon::Info,
        SettingsPage::Archived => Icon::Archive,
    }
}

/// The settings route: mount with `Route::Settings(page) => settings::shell::screen(*page)`.
pub fn screen(page: SettingsPage) -> AnyElement {
    untracked(|| settings_route_layout(page, host_runtime()).into_element())
}

/// `screen` over a given client runtime (the router passes the host's through `screen`).
pub fn screen_for<P: t3_client::platform::Platform + Clone + 'static>(
    page: SettingsPage,
    client: &t3_client::composition::ClientRuntime<P>,
) -> AnyElement {
    let runtime = crate::runtime::services::settings::settings(client);
    untracked(|| settings_route_layout(page, Some(runtime)).into_element())
}

/// `None` only for the demo backend and an unavailable host, which have no settings store.
fn host_runtime() -> Option<SettingsRuntime> {
    crate::runtime::rt()
        .backend()
        .client()
        .map(|client| crate::runtime::services::settings::settings(&client))
}

/// `SettingsRouteLayout` with `SettingsScopeProvider`.
fn settings_route_layout(page: SettingsPage, runtime: Option<SettingsRuntime>) -> impl IntoElement {
    let location = SettingsLocation {
        urls: use_context::<UrlState>(),
        nav: navigator(),
        local_search: signal(LOCAL_ADDRESS.with(|address| address.borrow().0.clone())),
        local_hash: signal(LOCAL_ADDRESS.with(|address| address.borrow().1.clone())),
    };
    // beforeLoad: an old settings link's anchor finds the page its row moved to.
    effect(move || {
        let (Some(pathname), hash) = (location.pathname(), location.hash()) else {
            return;
        };
        if let Some(to) = moved_settings_path(&pathname, &hash).and_then(page_of_path) {
            location.navigate(to, None, &hash);
        }
    });
    let context = SettingsContext::new(page, runtime, location);
    provide_context(context);
    app_route_modal(t("Settings"), location, settings_content_layout(context))
}

/// Port of apps/web/src/components/AppRouteModal.tsx AppRouteModal.
pub fn app_route_modal(
    label: String,
    location: impl Into<ModalClose>,
    children: impl IntoElement,
) -> impl IntoElement {
    let close = location.into();
    // NSH's modal layer replaces Base UI's portal, viewport and focus handling; Esc and the
    // backdrop close it. Its native layer owns the window-controls inset (title_bar()).
    nsh_base::dialog()
        .role(AccessibilityRole::Dialog)
        .label(label)
        .animation(if reduce_motion() {
            ModalAnimation::None
        } else {
            ModalAnimation::Fade
        })
        .close_on_escape(true)
        .close_on_backdrop_press(true)
        .on_cancel(move || close.close())
        .app_class(tw("w-full h-full"))
        .child(
            view()
                .test_id("dialog-backdrop")
                .accessible(false)
                .on_press(move || close.close())
                .app_class(tw("fixed inset-0 z-50 bg-background/60 dark:bg-background/64 backdrop-blur-[4px]")),
        )
        .child(
            view()
                .pointer_events(PointerEvents::BoxNone)
                .app_class(tw("fixed inset-0 z-50 flex items-center justify-center sm:p-6 lg:p-10"))
                .child(
                    view()
                        .app_class(tw("relative flex size-full min-h-0 min-w-0 flex-col overflow-hidden bg-background text-foreground outline-none sm:max-h-[60rem] sm:max-w-[80rem] sm:rounded-2xl sm:border sm:shadow-2xl/40"))
                        .inline_style(move || {
                            // max-sm:pt-(--app-modal-top-inset) and max-sm:pb-safe.
                            let compact = crate::state::layout::layout_class().is_compact();
                            let area = safe_area();
                            let top = area.top.0.max(nsh_core::element::desktop::title_bar().top.0);
                            nsh_core::element::Style::from(
                                nsh_core::element::StyleRefinement::default()
                                    .pt(px(if compact { top } else { 0.0 }))
                                    .pb(px(if compact { area.bottom.0 } else { 0.0 })),
                            )
                        })
                        .child(children),
                ),
        )
}

/// How an [`app_route_modal`] leaves.
#[derive(Clone, Copy)]
pub struct ModalClose(SettingsLocation);

impl ModalClose {
    fn close(self) {
        self.0.close();
    }
}

impl From<SettingsLocation> for ModalClose {
    fn from(location: SettingsLocation) -> Self {
        Self(location)
    }
}

/// `AppRouteModalClose`.
fn app_route_modal_close(close: SettingsLocation) -> impl IntoElement {
    let handle = tooltip_create_handle();
    view()
        .app_class(tw("relative flex shrink-0"))
        .tooltip(tooltip(
            handle,
            TooltipOptions {
                side: Placement::Bottom,
                ..Default::default()
            },
            text(t("Close (Esc)")),
        ))
        .child(button(
            ButtonContent::Icon {
                icon: Icon::X,
                label: t("Close"),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconSm,
            || false,
            move || close.close(),
        ))
}

/// `SettingsContentLayout`.
fn settings_content_layout(context: SettingsContext) -> impl IntoElement {
    let page = context.page;
    // `contentScopeKey`: the environment for Diagnostics and Providers, else the address scope.
    let scope_key = computed(move || {
        if matches!(page, SettingsPage::Diagnostics | SettingsPage::Providers) {
            format!(
                "{:?}",
                context
                    .view()
                    .and_then(|v| v.selected.representative.clone())
                    .map(|e| e.environment_id)
            )
        } else {
            format!("{:?}", context.location.search())
        }
    });
    view()
        .app_class(tw("relative flex min-h-0 flex-1 flex-row"))
        .child(settings_sidebar_nav(context))
        .child(
            view()
                .role(AccessibilityRole::Group)
                .label(t("Settings content"))
                .app_class(tw("flex min-h-0 min-w-0 flex-1 flex-col"))
                .child(
                    view()
                        .app_class(tw(
                            "flex h-12 shrink-0 flex-row items-center gap-3 border-b ps-5 pe-2",
                        ))
                        .child(settings_breadcrumb(page))
                        .child(
                            view()
                                .app_class(tw("ms-auto flex shrink-0 flex-row items-center gap-1"))
                                .child(
                                    (page == SettingsPage::General)
                                        .then(|| restore_device_defaults_button(context)),
                                )
                                .child(app_route_modal_close(context.location)),
                        ),
                )
                .child(
                    view()
                        .app_class(tw("min-h-0 flex flex-1 flex-col"))
                        // `key={contentScopeKey:restoreSignal}`: a new scope or a restore remounts.
                        .child(dynamic(move || {
                            context.restore_signal.get();
                            scope_key.get();
                            untracked(|| settings_scope_boundary(context).into_element())
                        })),
                ),
        )
}

/// The page body each settings package renders inside the shell.
fn page_content(page: SettingsPage) -> AnyElement {
    use super::*;
    match page {
        SettingsPage::General => general::screen(),
        SettingsPage::Appearance => appearance::screen(),
        SettingsPage::Chat => chat::screen(),
        SettingsPage::Diffs => diffs::screen(),
        SettingsPage::Threads => threads::screen(),
        SettingsPage::Worktrees => worktrees::screen(),
        SettingsPage::Projects => projects::screen(),
        SettingsPage::Providers => providers::screen(),
        SettingsPage::SourceControl => source_control::screen(),
        SettingsPage::Secrets => secrets::screen(),
        SettingsPage::ScheduledTasks => scheduled_tasks::screen(),
        SettingsPage::Browser => browser::screen(),
        SettingsPage::Devices => devices::screen(),
        SettingsPage::Connections => connections::screen(),
        SettingsPage::Access => access::screen(),
        SettingsPage::Voice => voice::screen(),
        SettingsPage::Keybindings => keybindings::screen(),
        SettingsPage::Archived => archived::screen(),
        SettingsPage::Diagnostics => diagnostics::screen(),
        SettingsPage::Maintenance => maintenance::screen(),
        SettingsPage::Labs => labs::screen(),
        SettingsPage::About => about::screen(),
        SettingsPage::Licenses => licenses::screen(),
        SettingsPage::SnapShot => snap_shot::screen(),
    }
}

/// `SettingsScopeBoundary`.
fn settings_scope_boundary(context: SettingsContext) -> impl IntoElement {
    let page = context.page;
    let blocked = computed(move || {
        let view = context.view();
        if is_device_only_settings_page(page) || page == SettingsPage::Projects {
            None
        } else {
            match &view {
                Some(view) => match view.scope.unavailable_message() {
                    Some(message) => Some(Text::client_key(message).resolve().to_string()),
                    None if view.scope.kind_name() == ScopeKindName::Environment
                        && view.selected.connected.is_empty() =>
                    {
                        Some(tr(
                            "Reconnect {label} to change its settings.",
                            &[(
                                "label",
                                t3_i18n::Value::Text(scope_label(&view.scope.label)),
                            )],
                        ))
                    }
                    None => None,
                },
                None => None,
            }
        }
    });
    let notice = computed(move || {
        let target = context.target_id();
        context.view().and_then(|view| {
            t3_client::services::settings::search::search_target_notice(&view, &target)
        })
    });
    dynamic(move || {
        if let Some(notice) = notice.get() {
            return untracked(|| settings_scope_notice(context, notice).into_element());
        }
        match blocked.get() {
            Some(message) => settings_page_container_plain(
                text(message)
                    .app_class(tw("text-sm text-muted-foreground"))
                    .into_element(),
            )
            .into_element(),
            None => untracked(|| page_content(page)),
        }
    })
}

/// Port of SettingsScopeNotice.tsx: the scopes that own a search target, as a page.
fn settings_scope_notice(
    context: SettingsContext,
    notice: t3_client::services::settings::search::ScopeNotice,
) -> impl IntoElement {
    let page = context.page;
    let mut groups = view().app_class(tw("flex flex-col gap-4"));
    for group in notice.groups.iter().cloned() {
        let mut rows = view();
        for (index, choice) in group.choices.into_iter().enumerate() {
            let target_id = notice.target_id.clone();
            let label = if choice.label_is_key {
                Text::client_key(choice.label.clone()).resolve().to_string()
            } else {
                choice.label.clone()
            };
            let search = choice.search.clone();
            rows = rows.child(
                nsh_base::button()
                    .accessibility_label(Some(SharedString::from(label.clone())))
                    .on_press(move || context.location.navigate(page, Some(&search), &target_id))
                    .app_class(tw(&cn([
                        Some("flex w-full min-w-0 cursor-pointer flex-row items-center gap-3 px-3 py-3 text-left outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring sm:px-4"),
                        (index > 0).then_some("border-t border-border/50"),
                    ])))
                    .child(
                        view()
                            .app_class(tw("min-w-0 flex-1"))
                            .child(text(label).number_of_lines(1u32).app_class(tw("block truncate text-sm font-medium")))
                            .child((!choice.secondary.is_empty()).then(|| {
                                text(choice.secondary.clone())
                                    .number_of_lines(1u32)
                                    .app_class(tw("block truncate text-xs text-muted-foreground"))
                            })),
                    )
                    .child(icon(Icon::ChevronRight, 16.0, move || palette().muted_foreground)),
            );
        }
        groups = groups.child(match group.title {
            Some(title) => settings_section("", Text::content(title), rows).into_element(),
            None => settings_group(rows).into_element(),
        });
    }
    settings_page_container(
        view()
            .app_class(tw("flex flex-col gap-3"))
            .child(
                text(settings_text(&notice.message))
                    .app_class(tw("px-3 text-sm text-muted-foreground sm:px-4")),
            )
            .child(groups),
    )
}

/// `RestoreDeviceDefaultsButton` (`useSettingsRestore`, without the theme store's labels).
fn restore_device_defaults_button(context: SettingsContext) -> impl IntoElement {
    let labels = computed(move || {
        let server = context.server_settings();
        let client = context.client_settings();
        changed_setting_labels(&t3_client::services::settings::panels::unified_settings(
            &server, &client,
        ))
    });
    let narrow = move || crate::state::layout::layout_class().is_compact();
    dynamic(move || {
        let disabled = labels.with(Vec::is_empty);
        let label = t("Restore device defaults");
        button(
            if narrow() {
                ButtonContent::Icon {
                    icon: Icon::RotateCcw,
                    label,
                }
            } else {
                ButtonContent::IconLabel {
                    icon: Icon::RotateCcw,
                    label,
                }
            },
            ButtonVariant::Ghost,
            ButtonSize::Xs,
            move || disabled,
            move || restore_defaults(context, labels.get_untracked()),
        )
        .into_element()
    })
}

fn restore_defaults(context: SettingsContext, labels: Vec<&'static str>) {
    if labels.is_empty() {
        return;
    }
    let list = labels
        .iter()
        .map(|label| Text::client_key(*label).resolve().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let message = format!(
        "{}\n{}",
        t("Restore default settings?"),
        tr(
            "This will reset: {labels}.",
            &[("labels", t3_i18n::Value::Text(list))]
        )
    );
    let Some(confirm) = crate::ui::confirm::request_confirm_dialog(
        message,
        Some(crate::ui::confirm::ConfirmDialogOptions {
            variant: crate::ui::confirm::ConfirmDialogVariant::Destructive,
            initial_focus: None,
        }),
    ) else {
        return;
    };
    context.spawn(async move {
        if !confirm.await {
            return;
        }
        let patch = Value::Object(t3_client::services::settings::panels::restore_defaults_patch());
        context.update(patch);
        context.restore_signal.update(|value| *value += 1);
    });
}

/// `SettingsBreadcrumb` in `WorkspaceBreadcrumb`.
fn settings_breadcrumb(page: SettingsPage) -> impl IntoElement {
    let section = settings_breadcrumb_label(&settings_path(page));
    view()
        .role(AccessibilityRole::Navigation)
        .label(t("Settings breadcrumb"))
        .app_class(tw("min-w-0"))
        .child(
            view()
                .app_class(tw(
                    "m-0 flex min-w-0 flex-row items-center gap-2 p-0 text-sm sm:gap-3",
                ))
                .child(section.map(|_| {
                    view()
                        .app_class(tw("flex flex-row shrink-0 items-center gap-2 sm:gap-3"))
                        .child(text(t("Settings")).app_class(tw(
                            "flex min-w-0 items-center font-medium shrink-0 text-muted-foreground",
                        )))
                        .child(
                            text(Text::content("/").resolve())
                                .accessible(false)
                                .app_class(tw("flex shrink-0 items-center text-icon-muted")),
                        )
                }))
                .child(
                    text(Text::client_key(section.unwrap_or("Settings")).resolve())
                        .number_of_lines(1u32)
                        .app_class(tw(
                            "flex min-w-0 items-center font-medium text-foreground truncate",
                        )),
                ),
        )
}

/// `SettingsSidebarNav`.
fn settings_sidebar_nav(context: SettingsContext) -> impl IntoElement {
    let page = context.page;
    let active_page = settings_active_nav_page(page);
    // Below md the nav is an icon rail; searching expands it over the page.
    let narrow = move || window().width.0 < 768.0;
    let desktop = nsh_core::element::desktop::current_window()
        .state()
        .is_some();
    let platform = SettingsSearchPlatform {
        desktop,
        release_channel: false,
        secrets_feature: false,
        mac: crate::host::is_mac(),
        windows: crate::host::is_windows(),
    };
    let runtime = context.runtime;
    let search = NavSearch::new(
        move || {
            let availability = runtime.with(|runtime| {
                runtime
                    .as_ref()
                    .map_or_else(SettingsSearchAvailability::default, |r| {
                        r.0.search_availability()
                    })
            });
            filter_available_settings_search_items(&availability)
        },
        platform,
    );
    let location = context.location;
    let pick: Rc<dyn Fn(&'static SettingsSearchItem)> = Rc::new(move |item| {
        search.clear();
        search.compact_open.set(false);
        let Some(to) = item.page() else { return };
        location.navigate(to, None, item.anchor());
    });
    let compact_open = search.compact_open;
    view()
        .role(AccessibilityRole::Navigation)
        .label(t("Settings sections"))
        .on_key_down({
            let pick = pick.clone();
            move |event: &KeyEvent| {
                // The Find (Mod+F) and `/` shortcuts focus the search field.
                let find = event.key.eq_ignore_ascii_case("f")
                    && (event.modifiers.cmd || event.modifiers.ctrl)
                    && !event.modifiers.alt
                    && !event.modifiers.shift;
                if find {
                    search.focus();
                }
                let _ = &pick;
            }
        })
        .app_class(move || {
            Some(tw_with(
                Scope::Sidebar,
                &cn([
                    Some("flex w-60 shrink-0 flex-col border-e bg-sidebar text-sidebar-foreground"),
                    narrow().then_some("w-14"),
                    (compact_open.get() && narrow())
                        .then_some("absolute inset-y-0 start-0 z-10 w-full"),
                ]),
                &[],
            ))
        })
        .child(
            scroll().app_class(tw("h-auto min-h-0 flex-1")).child(
                view()
                    .app_class(tw_with(
                        Scope::Sidebar,
                        "flex w-full min-w-0 flex-col p-(--sidebar-content-inset)",
                        &[],
                    ))
                    .child(
                        view()
                            .app_class(tw("flex flex-col gap-2"))
                            .child(settings_search_field(search, narrow, pick.clone()))
                            .child(settings_search_results(search, pick.clone()))
                            .child(when(
                                move || !search.is_searching(),
                                move || nav_groups(context, active_page, narrow, search),
                            )),
                    ),
            ),
        )
}

fn nav_groups(
    context: SettingsContext,
    active_page: SettingsPage,
    narrow: impl Fn() -> bool + Copy + 'static,
    search: NavSearch,
) -> impl IntoElement {
    let location = context.location;
    let compact_open = search.compact_open;
    let mut groups = view().app_class(tw("flex flex-col gap-4"));
    groups = groups.child(when(
        move || narrow() && !compact_open.get(),
        move || {
            nav_row(
                t("Search settings"),
                Icon::Search,
                None,
                || false,
                narrow,
                compact_open,
                move || search.focus(),
            )
        },
    ));
    for (index, group) in settings_nav_groups(false).into_iter().enumerate() {
        let heading = group.heading;
        let hidden = group.heading_hidden;
        let mut rows = view().app_class(tw("flex w-full min-w-0 flex-col gap-1"));
        for page in group.pages {
            let label = Text::client_key(
                t3_client::services::settings::search::settings_section_label(page),
            );
            rows = rows.child(nav_row(
                label.resolve().to_string(),
                section_icon(page),
                Some(label),
                move || page == active_page,
                narrow,
                compact_open,
                move || location.navigate(page, None, ""),
            ));
        }
        groups = groups.child(
            view()
                .app_class(move || {
                    Some(tw(&cn([
                        Some("flex flex-col gap-1"),
                        (narrow() && index > 0).then_some("border-t border-sidebar-border pt-4"),
                    ])))
                })
                // `sr-only` headings stay out of the tree; the group carries the name.
                .child(when(
                    move || !hidden && !(narrow() && !compact_open.get()),
                    move || {
                        text(Text::client_key(heading).resolve())
                            .role(AccessibilityRole::Header)
                            .app_class(tw_with(
                                Scope::Sidebar,
                                "px-2 text-xs font-medium text-sidebar-muted-foreground/80",
                                &[],
                            ))
                    },
                ))
                .child(
                    view()
                        .role(AccessibilityRole::Group)
                        .label(Text::client_key(heading).resolve())
                        .child(rows),
                ),
        );
    }
    groups
}

/// `SidebarMenuButton` as the settings nav uses it.
fn nav_row(
    label: String,
    glyph: Icon,
    visible_label: Option<Text>,
    active: impl Fn() -> bool + Copy + 'static,
    narrow: impl Fn() -> bool + Copy + 'static,
    compact_open: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let accessible = SharedString::from(label);
    nsh_base::button()
        .accessibility_label(Some(accessible))
        .selected(active)
        .on_press(on_press)
        .app_class(move || {
            let attrs: &[(&str, &str)] = if active() { &[("data-active", "true")] } else { &[] };
            Some(tw_with(
                Scope::Sidebar,
                "flex w-full cursor-pointer flex-row items-center gap-(--sidebar-control-gap) overflow-hidden text-left outline-hidden ring-ring hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-inset active:bg-sidebar-row-active active:text-sidebar-foreground data-[active=true]:bg-sidebar-row-selected data-[active=true]:font-medium data-[active=true]:text-sidebar-foreground h-8 rounded-(--control-radius) px-(--sidebar-row-content-inset) py-1.5 text-sm font-medium text-sidebar-muted-foreground/80",
                attrs,
            ))
        })
        .child(icon(glyph, 16.0, move || {
            if active() {
                palette().sidebar_foreground
            } else {
                palette().sidebar_icon
            }
        }))
        .child(visible_label.map(|label| {
            when(
                move || !(narrow() && !compact_open.get()),
                move || text(label.resolve()).number_of_lines(1u32).app_class(tw("truncate")),
            )
        }))
}

/// `WorkspacePageContainer` (readable width) inside the settings scroll area.
fn settings_page_container_plain(children: AnyElement) -> impl IntoElement {
    scroll().app_class(tw("flex-1")).child(
        view()
            .app_class(tw(
                "mx-auto flex w-full max-w-4xl flex-col gap-6 px-5 pt-6 pb-12 sm:px-6 gap-8",
            ))
            .child(children),
    )
}

/// `SettingsPageContainer`: the scroll area, the scope sentence and the page's sections.
pub fn settings_page_container(children: impl IntoElement) -> impl IntoElement {
    let context = use_settings();
    scroll().app_class(tw("flex-1")).child(
        view()
            .on_layout(move |bounds: &Bounds<Points>| {
                // The rows' container is the page column minus the page padding.
                let _ = context.content_width.try_set(bounds.size.width.0);
            })
            .app_class(tw(
                "mx-auto flex w-full max-w-4xl flex-col gap-6 px-5 pt-6 pb-12 sm:px-6 gap-8",
            ))
            .child(settings_scope_sentence(context))
            .child(children),
    )
}

/// `useSettingsSearchTarget`: when a search jump names this anchor, scroll it to the middle,
/// focus it and mark it until it loses focus (the web's pulse ends on blur; this is static).
fn search_target(context: SettingsContext, id: &'static str) -> (ElementRef, Signal<bool>) {
    let target = element_ref();
    let highlighted = signal(false);
    if !id.is_empty() {
        target.on_focus_out(move || highlighted.set(false));
    }
    let _ = context;
    (target, highlighted)
}

fn reveal_if_target(
    context: SettingsContext,
    id: &'static str,
    target: ElementRef,
    highlighted: Signal<bool>,
) {
    if id.is_empty() || untracked(|| context.target_id()) != id {
        return;
    }
    target.reveal(
        nsh_core::RevealOptions::default()
            .vertical(nsh_core::ScrollAlignment::Center)
            .animated(!reduce_motion()),
    );
    target.focus();
    if !reduce_motion() {
        highlighted.set(true);
    }
    context.clear_target();
}

/// `SettingsSection` (grouped variant) with its `SettingsGroup`.
pub fn settings_section(
    id: &'static str,
    title: Text,
    children: impl IntoElement,
) -> impl IntoElement {
    let context = use_settings();
    let (target, highlighted) = search_target(context, id);
    view()
        .test_id(id)
        .element_ref(&target)
        .focusable(!id.is_empty())
        .on_layout(move |_: &Bounds<Points>| reveal_if_target(context, id, target, highlighted))
        .app_class(move || {
            Some(tw(&cn([
                Some("flex flex-col gap-2.5 rounded-xl outline-none"),
                highlighted.get().then_some("ring-2 ring-primary/40"),
            ])))
        })
        .child(
            view()
                .app_class(tw("flex min-h-7 flex-row items-start justify-between gap-4 px-3 sm:px-4"))
                .child(
                    view().app_class(tw("flex min-w-0 flex-row items-center gap-1")).child(
                        text(move || title.resolve())
                            .role(AccessibilityRole::Header)
                            .app_class(tw("flex min-h-7 items-center gap-2 text-base font-semibold text-foreground")),
                    ),
                ),
        )
        .child(settings_group(children))
}

/// `SettingsGroup` (grouped, divided): rows are separated by a top border.
pub fn settings_group(children: impl IntoElement) -> impl IntoElement {
    view()
        .app_class(tw("relative overflow-visible text-foreground rounded-xl border border-border/60 bg-card/40 shadow-xs/5"))
        .child(children)
}

/// What a page passes to [`settings_row`].
pub struct SettingsRowProps {
    /// The search anchor (`searchableSetting(id).id`).
    pub id: &'static str,
    pub title: Text,
    pub description: Option<Box<dyn Fn() -> Text>>,
    /// A line under the description (`status`); `None` from the closure shows nothing.
    pub status: Option<Box<dyn Fn() -> Option<Text>>>,
    /// The reset button and when to show it (`resetAction`).
    pub reset: Option<RowReset>,
    pub control: Option<Box<dyn Fn() -> AnyElement>>,
    pub server_scoped: bool,
    pub setting_keys: &'static [&'static str],
    /// Dot paths inside a setting an administrator can lock (`managedPaths`).
    pub managed_paths: &'static [&'static str],
    /// Why policy turns this row's control off (`managed`), from the contract's denial helpers.
    pub managed: Option<Text>,
    /// Content below the title and control (`children`), inert when the row is unavailable.
    pub children: Option<Box<dyn Fn() -> AnyElement>>,
    /// Not the first row of its group: draws the divider (`[&>*+*]:border-t`).
    pub divided: bool,
}

impl SettingsRowProps {
    /// A row for `searchableSetting(id)`: its anchor and translated title.
    pub fn searchable(id: &'static str) -> Self {
        let (anchor, title) = t3_client::services::settings::search::searchable_setting(id);
        Self {
            id: anchor,
            title: Text::client_key(title),
            description: None,
            status: None,
            reset: None,
            control: None,
            server_scoped: false,
            setting_keys: &[],
            managed_paths: &[],
            managed: None,
            children: None,
            divided: true,
        }
    }
    /// A row with a plain anchor and title (the web's `id=` and `title=` without a search item).
    pub fn titled(id: &'static str, title: Text) -> Self {
        let mut props = Self::searchable("language");
        props.id = id;
        props.title = title;
        props
    }
    pub fn description(mut self, description: impl Fn() -> Text + 'static) -> Self {
        self.description = Some(Box::new(description));
        self
    }
    pub fn status(mut self, status: impl Fn() -> Option<Text> + 'static) -> Self {
        self.status = Some(Box::new(status));
        self
    }
    pub fn control<E: IntoElement>(mut self, control: impl Fn() -> E + 'static) -> Self {
        self.control = Some(Box::new(move || control().into_element()));
        self
    }
    pub fn reset(mut self, reset: RowReset) -> Self {
        self.reset = Some(reset);
        self
    }
    pub fn server(mut self, keys: &'static [&'static str]) -> Self {
        self.server_scoped = true;
        self.setting_keys = keys;
        self
    }
    pub fn managed_paths(mut self, paths: &'static [&'static str]) -> Self {
        self.managed_paths = paths;
        self
    }
    pub fn managed(mut self, reason: Text) -> Self {
        self.managed = Some(reason);
        self
    }
    pub fn children<E: IntoElement>(mut self, children: impl Fn() -> E + 'static) -> Self {
        self.children = Some(Box::new(move || children().into_element()));
        self
    }
    pub fn first(mut self) -> Self {
        self.divided = false;
        self
    }
}

/// `SettingResetButton` for a row: `label` is the lowercase English name in its aria label.
pub struct RowReset {
    pub label: &'static str,
    pub visible: Box<dyn Fn() -> bool>,
    pub on_reset: Rc<dyn Fn()>,
}

impl RowReset {
    pub fn new(
        label: &'static str,
        visible: impl Fn() -> bool + 'static,
        on_reset: impl Fn() + 'static,
    ) -> Self {
        Self {
            label,
            visible: Box::new(visible),
            on_reset: Rc::new(on_reset),
        }
    }
}

/// `SettingsRow`. The administrator lock (`managedPolicy`) and the inheritance marker are not
/// ported yet (see the SET-SHELL hand-off).
pub fn settings_row(props: SettingsRowProps) -> impl IntoElement {
    let context = use_settings();
    let SettingsRowProps {
        id,
        title,
        description,
        status,
        reset,
        control,
        server_scoped,
        setting_keys,
        managed_paths,
        managed,
        children,
        divided,
    } = props;
    let lockable: Vec<&'static str> = setting_keys.iter().chain(managed_paths).copied().collect();
    let managed_by = computed(move || {
        context.view().and_then(|view| {
            t3_client::services::settings::inheritance::managed_by(&view, &lockable)
        })
    });
    let inheritance = computed(move || {
        if !server_scoped || setting_keys.is_empty() {
            return None;
        }
        context.view().and_then(|view| {
            t3_client::services::settings::inheritance::row_inheritance(&view, setting_keys)
        })
    });
    let managed = stored(managed);
    let wide = move || {
        context.content_width.get() >= 512.0
            || context.content_width.get() == 0.0 && window().width.0 >= 640.0
    };
    let scoped_keys: Vec<&'static str> = setting_keys
        .iter()
        .copied()
        .filter(|key| is_project_scoped_setting_key(key))
        .collect();
    let scoped_keys = Rc::new(scoped_keys);
    let state = computed({
        let scoped_keys = scoped_keys.clone();
        move || {
            let view = context.view();
            let project_scope = view.as_ref().is_some_and(|v| v.scope.is_project_scope());
            let access = if server_scoped {
                context.write_access()
            } else {
                SettingsWriteAccess::Writable
            };
            let environment_wide = project_scope && server_scoped && scoped_keys.is_empty();
            let source = view
                .as_ref()
                .filter(|_| project_scope)
                .map(|v| scoped_settings_source(&v.targets, &scoped_keys));
            (access, environment_wide, project_scope, source)
        }
    });
    // An administrator's lock on any target wins over every other reason a control is inert.
    let inert_message = move || {
        if let Some(reason) = managed.get() {
            return Some(reason.resolve().to_string());
        }
        if let Some(organization) = managed_by.get() {
            return Some(tr(
                "Managed by {organization}.",
                &[("organization", t3_i18n::Value::Text(organization))],
            ));
        }
        let (access, environment_wide, _, _) = state.get();
        access.text().map(|text| settings_text(&text)).or_else(|| {
            environment_wide
                .then(|| t("Environment-wide setting. Select an environment to change it."))
        })
    };
    let locked = move || managed.with(|m| m.is_some()) || managed_by.with(Option::is_some);
    let reset = reset.map(Rc::new);
    let title_text = title.clone();
    let (target, highlighted) = search_target(context, id);
    let has_children = children.is_some();
    let children = children.map(Rc::new);
    view()
        .test_id(id)
        .element_ref(&target)
        .focusable(!id.is_empty())
        .on_layout(move |_: &Bounds<Points>| reveal_if_target(context, id, target, highlighted))
        .app_class(move || {
            Some(tw(&cn([
                Some("rounded-xl px-3 sm:px-4 outline-none"),
                Some(if has_children { "pt-3 pb-1" } else { "py-3" }),
                divided.then_some("rounded-none border-t border-border/50"),
                highlighted.get().then_some("ring-2 ring-inset ring-primary/40"),
            ])))
        })
        .child(
            view()
                .app_class(move || {
                    Some(tw(if wide() {
                        "grid grid-cols-[minmax(0,1fr)_auto] items-center gap-8"
                    } else {
                        "flex flex-col gap-3"
                    }))
                })
                .child(
                    view()
                        .app_class(tw("min-w-0 flex-1 flex flex-col gap-1"))
                        .child(
                            view()
                                .app_class(tw("flex min-h-5 flex-row items-center gap-1.5"))
                                .child(
                                    text(move || title_text.resolve())
                                        .role(AccessibilityRole::Header)
                                        .app_class(tw("text-sm font-medium text-foreground")),
                                )
                                .child(dynamic(move || {
                                    let inheritance = inheritance.get()?;
                                    let quiet = inheritance.quiet;
                                    let can_clear = context.write_access() == SettingsWriteAccess::Writable;
                                    Some(
                                        view()
                                            .app_class(tw(&cn([
                                                Some("inline-flex h-5 w-5 shrink-0 items-center justify-center"),
                                                // Values set right here explain nothing new: the marker waits for hover or focus.
                                                quiet.then_some("opacity-0 hover:opacity-100 focus-within:opacity-100"),
                                            ])))
                                            .child(setting_inheritance(context, inheritance, setting_keys, can_clear))
                                            .into_element(),
                                    )
                                }))
                                .child(
                                    view()
                                        .app_class(tw("inline-flex h-5 w-5 shrink-0 items-center justify-center"))
                                        .child(dynamic({
                                            let reset = reset.clone();
                                            let scoped_keys = scoped_keys.clone();
                                            let title = title.clone();
                                            move || {
                                                let (access, _, project_scope, source) = state.get();
                                                if access != SettingsWriteAccess::Writable || locked() {
                                                    return None;
                                                }
                                                if project_scope && !scoped_keys.is_empty() {
                                                    let show = matches!(
                                                        source,
                                                        Some(ScopedSettingSource::Project | ScopedSettingSource::Mixed)
                                                    );
                                                    let keys = scoped_keys.as_ref().clone();
                                                    let label = title.resolve().to_string();
                                                    return show.then(|| {
                                                        setting_reset_button(
                                                            label,
                                                            t("Reset to inherited value"),
                                                            move || context.clear(keys.clone()),
                                                        )
                                                        .into_element()
                                                    });
                                                }
                                                let reset = reset.clone()?;
                                                (reset.visible)().then(|| {
                                                    let on_reset = reset.on_reset.clone();
                                                    setting_reset_button(
                                                        t(reset.label),
                                                        t("Reset to default"),
                                                        move || on_reset(),
                                                    )
                                                    .into_element()
                                                })
                                            }
                                        })),
                                ),
                        )
                        .child(description.map(|description| {
                            text(move || description().resolve())
                                .app_class(tw("max-w-2xl text-xs leading-normal text-muted-foreground"))
                        }))
                        .child(status.map(|status| {
                            let status = Rc::new(status);
                            dynamic(move || {
                                status().map(|line| {
                                    text(line.resolve())
                                        .app_class(tw("pt-0.5 text-xs text-muted-foreground"))
                                })
                            })
                        })),
                )
                .child(control.map(|control| {
                    let control = Rc::new(control);
                    view()
                        .app_class(move || {
                            Some(tw(if wide() {
                                "flex w-auto min-w-0 shrink-0 flex-row items-center justify-end gap-2"
                            } else {
                                "flex w-full min-w-0 shrink-0 flex-row items-center gap-2"
                            }))
                        })
                        .child(dynamic(move || {
                            let control = control.clone();
                            match inert_message() {
                                Some(message) => inert_control(message, untracked(|| control())).into_element(),
                                None => untracked(|| control()),
                            }
                        }))
                })),
        )
        .child(children.map(|children| {
            dynamic(move || {
                let inert = server_scoped && state.with(|(access, ..)| *access != SettingsWriteAccess::Writable);
                let children = children.clone();
                let content = untracked(|| children());
                if inert {
                    // The web's `inert`: one accessibility element, nothing inside reachable.
                    view()
                        .accessible(true)
                        .pointer_events(PointerEvents::None)
                        .app_class(tw("opacity-50"))
                        .child(content)
                        .into_element()
                } else {
                    content
                }
            })
        }))
}

/// `inertControl`: the control dimmed and unreachable, with the reason as a tooltip.
fn inert_control(message: String, control: AnyElement) -> impl IntoElement {
    let handle = tooltip_create_handle();
    // The web's `inert` subtree: one accessibility element named by the reason, so the control
    // inside is neither announced nor pressable.
    view()
        .focusable(true)
        .accessible(true)
        .label(message.clone())
        .tooltip(tooltip(handle, TooltipOptions::default(), text(message)))
        .app_class(tw("flex w-full flex-row items-center rounded-md outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"))
        .child(
            view()
                .pointer_events(PointerEvents::None)
                .app_class(tw("flex w-full flex-row items-center gap-2 opacity-50"))
                .child(control),
        )
}

/// `SettingResetButton`.
pub fn setting_reset_button(
    label: String,
    tooltip_text: String,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let handle = tooltip_create_handle();
    let accessible = tr(
        "Reset {label} to default",
        &[("label", t3_i18n::Value::Text(label))],
    );
    view()
        .app_class(tw("relative flex shrink-0"))
        .tooltip(tooltip(handle, TooltipOptions::default(), text(tooltip_text)))
        // UI-BUTTON has no icon-micro size; its 20px ghost-muted look is written here.
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(accessible)))
                .on_press(on_press)
                .app_class(tw("flex size-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"))
                .child(icon(Icon::Undo2, 12.0, move || palette().muted_foreground)),
        )
}

/// One option of a scope menu (`ScopeOption`).
#[derive(Clone, PartialEq)]
struct ScopeOption {
    value: String,
    label: String,
    secondary: String,
    offline: bool,
}

/// `SettingsScopeSentence`: "Applying settings for <project> across <environment> in <checkout>".
fn settings_scope_sentence(context: SettingsContext) -> impl IntoElement {
    dynamic(move || {
        let scope_view = context.view()?;
        if scope_view.device_only {
            return None;
        }
        let search = scope_view.search.clone();
        let environments = scope_view.all_environments.clone();
        let groups = scope_view.groups.clone();
        // ProjectScopeMenu.
        let selected_project = groups
            .iter()
            .find(|g| Some(&g.project_key) == search.project.as_ref());
        let menu_groups = project_scope_menu_groups(&groups, &search);
        let machines: Vec<(String, String)> = menu_groups
            .iter()
            .map(|g| {
                (
                    g.project_key.clone(),
                    settings_scope_project_machines(g, &environments, true).join(", "),
                )
            })
            .collect();
        let project_options: Vec<ScopeOption> = menu_groups
            .iter()
            .map(|group| {
                let mine = machines
                    .iter()
                    .find(|(k, _)| *k == group.project_key)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default();
                let root = group
                    .members
                    .first()
                    .map(|m| m.workspace_root.clone())
                    .unwrap_or_default();
                let same_name: Vec<_> = machines
                    .iter()
                    .filter(|(key, _)| {
                        *key != group.project_key
                            && menu_groups.iter().any(|g| {
                                &g.project_key == key && g.display_name == group.display_name
                            })
                    })
                    .collect();
                let secondary = if same_name.is_empty() {
                    if environments.len() > 1 {
                        root
                    } else {
                        String::new()
                    }
                } else if same_name.iter().any(|(_, m)| *m == mine) {
                    format!("{mine} · {root}")
                } else {
                    mine
                };
                ScopeOption {
                    value: group.project_key.clone(),
                    label: group.display_name.clone(),
                    secondary,
                    offline: false,
                }
            })
            .collect();
        let project_label = selected_project
            .map(|g| g.display_name.clone())
            .unwrap_or_else(|| {
                if search.project.is_some() {
                    t("Unavailable project")
                } else {
                    t("All projects")
                }
            });
        // EnvironmentScopeMenu.
        let checkout_environment = match &scope_view.scope.kind {
            t3_client::services::settings::scope::ScopeKind::Checkout {
                environment_id, ..
            } => Some(environment_id.0.clone()),
            _ => None,
        };
        let environment_value = environment_axis_value(&search, checkout_environment.as_deref());
        let selected_environment = environments
            .iter()
            .find(|e| e.environment_id.0 == environment_value);
        let member_ids: Option<Vec<_>> =
            selected_project.map(|g| g.members.iter().map(|m| m.environment_id.clone()).collect());
        let environment_options: Vec<ScopeOption> = environments
            .iter()
            .filter(|e| {
                member_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&e.environment_id))
            })
            .map(|environment| {
                let (primary, secondary) =
                    settings_scope_environment_labels(environment, &environments);
                ScopeOption {
                    value: environment.environment_id.0.clone(),
                    label: primary,
                    secondary: if environments.len() <= 1 {
                        String::new()
                    } else if secondary.is_empty() {
                        settings_scope_environment_address(environment)
                    } else {
                        secondary
                    },
                    offline: environment.phase != t3_client::connection::Phase::Connected,
                }
            })
            .collect();
        let environment_label = match selected_environment {
            Some(environment) => settings_scope_environment_label(environment, &environments),
            None if environment_value != ALL_VALUE => t("Unavailable environment"),
            None if scope_view.single_environment => t("No environments"),
            None => t("All environments"),
        };
        // CheckoutScopeMenu.
        let checkouts: Vec<_> = selected_project
            .map(|g| {
                settings_scope_checkout_choices(g, &search)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let checkout_refs: Vec<_> = checkouts.iter().collect();
        let checkout_label = checkouts
            .iter()
            .find(|m| Some(&m.physical_project_key) == search.checkout.as_ref())
            .map(|m| settings_scope_checkout_label(m, &checkout_refs))
            .unwrap_or_else(|| {
                if search.checkout.is_some() {
                    t("Unavailable checkout")
                } else {
                    t("All checkouts")
                }
            });
        let checkout_options: Vec<ScopeOption> = checkouts
            .iter()
            .map(|m| ScopeOption {
                value: m.physical_project_key.clone(),
                label: file_basename(&m.workspace_root).to_owned(),
                secondary: m.workspace_root.clone(),
                offline: false,
            })
            .collect();
        let preposition = if search.machine.is_some()
            || scope_view.scope.kind_name() == ScopeKindName::Checkout
        {
            t("on")
        } else {
            t("across")
        };
        let project_search = search.clone();
        let environment_search = search.clone();
        let checkout_search = search.clone();
        let sentence = view()
            .app_class(tw("flex min-w-0 flex-row flex-wrap items-center gap-x-1.5 gap-y-1 px-3 text-sm text-muted-foreground sm:px-4"))
            .child(
                view()
                    .app_class(tw("flex min-w-0 flex-row items-center gap-1.5"))
                    .child(text(t("Applying settings for")).app_class(tw("shrink-0")))
                    .child(scope_menu(
                        t("Project scope"),
                        project_label,
                        project_axis_value(&search),
                        project_options,
                        Some(ScopeOption { value: ALL_VALUE.into(), label: t("All projects"), secondary: String::new(), offline: false }),
                        move |next| context.select_scope(select_project_axis(&project_search, &next)),
                    )),
            )
            .child(
                view()
                    .app_class(tw("flex min-w-0 flex-row items-center gap-1.5"))
                    .child(text(preposition).app_class(tw("shrink-0")))
                    .child(scope_menu(
                        t("Environment scope"),
                        environment_label,
                        environment_value,
                        environment_options,
                        (!scope_view.single_environment).then(|| ScopeOption {
                            value: ALL_VALUE.into(),
                            label: t("All environments"),
                            secondary: String::new(),
                            offline: false,
                        }),
                        move |next| context.select_scope(select_environment_axis(&environment_search, &next)),
                    )),
            )
            .child((!checkouts.is_empty()).then(|| {
                let choices = checkouts.clone();
                view()
                    .app_class(tw("flex min-w-0 flex-row items-center gap-1.5"))
                    .child(text(t("in")).app_class(tw("shrink-0")))
                    .child(scope_menu(
                        t("Checkout scope"),
                        checkout_label,
                        search.checkout.clone().unwrap_or_else(|| ALL_VALUE.into()),
                        checkout_options,
                        Some(ScopeOption { value: ALL_VALUE.into(), label: t("All checkouts"), secondary: String::new(), offline: false }),
                        move |next| {
                            let member = choices.iter().find(|m| m.physical_project_key == next);
                            context.select_scope(select_checkout_axis(&checkout_search, member));
                        },
                    ))
            }));
        Some(sentence.into_element())
    })
}

/// `ScopeMenu`. The web switches to a searchable combobox above six options; NSH's menu takes
/// the same list (its sheet scrolls), so one control serves both sizes here.
fn scope_menu(
    aria: String,
    label: String,
    value: String,
    options: Vec<ScopeOption>,
    all: Option<ScopeOption>,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    let trigger_label = format!("{aria}: {label}");
    let on_change = Rc::new(on_change);
    let has_all = all.is_some();
    let choices: Vec<ScopeOption> = all.into_iter().chain(options).collect();
    let current = value.clone();
    menu(
        menu_trigger(
            trigger_label,
            || false,
            view()
                .app_class(tw("inline-flex h-7 min-w-0 max-w-72 shrink-0 flex-row items-center justify-center gap-1.5 rounded-md border border-input bg-background px-2 text-sm font-medium text-foreground shadow-xs/5 hover:bg-accent/50 dark:bg-input/32"))
                .child(text(label).number_of_lines(1u32).app_class(tw("min-w-0 truncate")))
                .child(icon_with_class(Icon::ChevronDown, "-me-0.5 size-3.5 shrink-0 text-muted-foreground opacity-70")),
        ),
        MenuOptions {
            align: Align::Start,
            ..Default::default()
        },
        menu_popup_with(move || {
            let on_change = on_change.clone();
            let current = current.clone();
            let mut items = view().app_class(tw("w-72 max-w-full"));
            for (index, option) in choices.iter().cloned().enumerate() {
                let on_change = on_change.clone();
                let value = option.value.clone();
                items = items.child(menu_radio_item(
                    option.label.clone(),
                    option.value.clone(),
                    ItemOptions::default(),
                    view()
                        .app_class(tw("flex min-w-0 flex-row items-center gap-2"))
                        .child(
                            view()
                                .app_class(tw("min-w-0 flex-1"))
                                .child(text(option.label.clone()).number_of_lines(1u32).app_class(tw("block truncate")))
                                .child((!option.secondary.is_empty()).then(|| {
                                    text(option.secondary.clone())
                                        .number_of_lines(1u32)
                                        .app_class(tw("block truncate text-xs text-muted-foreground"))
                                })),
                        )
                        .child(option.offline.then(|| text(t("Offline")).app_class(tw("shrink-0 text-xs text-muted-foreground"))))
                        .child(menu_radio_item_indicator()),
                    move || on_change(value.clone()),
                ));
                if index == 0 && has_all {
                    items = items.child(menu_separator());
                }
            }
            let current = current.clone();
            menu_radio_group(String::new(), move || current.clone(), items, |_| {})
        }),
    )
}

/// The settings pickers' `Select` (`SelectTrigger size="sm"`, `SelectPopup align="end"`): item
/// titles are keys, or content such as native language names.
pub fn settings_select(
    label: &'static str,
    width: &'static str,
    items: Vec<(&'static str, Text)>,
    value: impl Fn() -> String + 'static,
    disabled: impl Fn() -> bool + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    use crate::ui::select::{Popup, SelectItem, SelectProps, Size, Value as SelectValue, select};
    let mut props = SelectProps::new(label);
    props.items = items
        .into_iter()
        .map(|(value, title)| SelectItem::new(value, title.resolve()))
        .collect::<Vec<_>>()
        .into();
    props.value = Prop::bind(move || SelectValue::Controlled(Some(SharedString::from(value()))));
    props.disabled = Prop::bind(disabled);
    props.size = Size::Sm;
    props.popup = Popup {
        align: Align::End,
        ..Popup::default()
    };
    view()
        .app_class(tw(width))
        .child(select(props, move |next| {
            if let Some(next) = next {
                on_change(next.to_string());
            }
        }))
}

/// Port of AdvancedSettings.tsx: power-user rows folded at the end of a section. A search jump
/// to one of `target_ids` opens the fold first.
pub fn advanced_settings(
    target_ids: &'static [&'static str],
    children: impl IntoElement,
) -> impl IntoElement {
    use crate::ui::collapsible::{
        CollapsibleHandle, collapsible, collapsible_panel, collapsible_trigger,
    };
    let context = use_settings();
    let open = signal(false);
    effect(move || {
        let target = context.target_id();
        if target_ids.contains(&target.as_str()) {
            open.set(true);
        }
    });
    let handle =
        CollapsibleHandle::controlled(move || open.get(), || false, move |next| open.set(next));
    collapsible(
        view()
            .app_class(tw("border-t border-border/50"))
            .child(collapsible_trigger(
                handle,
                Text::from("Advanced"),
                view()
                    .app_class(tw("flex flex-row items-center gap-1.5"))
                    .child(view().inline_style(move || {
                        nsh_core::element::Style::from(
                            nsh_core::element::StyleRefinement::default()
                                .rotate(deg(if open.get() { 90.0 } else { 0.0 })),
                        )
                    }).child(icon_with_class(Icon::ChevronRight, "size-3.5 shrink-0")))
                    .child(text(t("Advanced"))),
                "group flex min-h-10 w-full flex-row items-center gap-1.5 rounded-b-xl px-3 text-left text-xs font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring sm:px-4",
            ))
            .child(collapsible_panel(
                handle,
                !reduce_motion(),
                view().app_class(tw("border-t border-border/50")).child(children),
            )),
    )
}

fn shown(value: &t3_client::services::settings::inheritance::Shown) -> String {
    use t3_client::services::settings::inheritance::Shown;
    match value {
        Shown::Text(text) => settings_text(text),
        Shown::Content(content) => content.clone(),
    }
}

/// Port of SettingInheritance.tsx: where a row's value comes from, per environment, with the
/// projects that override it and their reset.
fn setting_inheritance(
    context: SettingsContext,
    inheritance: t3_client::services::settings::inheritance::RowInheritance,
    keys: &'static [&'static str],
    can_clear: bool,
) -> impl IntoElement {
    use crate::ui::popover::{Padding, PopoverOptions, Size, popover, popover_create_handle};
    use t3_client::services::settings::inheritance::{InheritanceState, LayerKey};
    let summary = inheritance
        .summary
        .iter()
        .map(settings_text)
        .collect::<Vec<_>>()
        .join(" · ");
    let handle = popover_create_handle();
    let hint = tooltip_create_handle();
    let state = inheritance.state;
    let label = tr(
        "{summary}. Show where this value comes from",
        &[("summary", t3_i18n::Value::Text(summary.clone()))],
    );
    let trigger = view()
        .app_class(tw("relative flex shrink-0"))
        .tooltip(tooltip(hint, TooltipOptions::default(), text(summary)))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from(label)))
                .on_layout(move |bounds: &Bounds<Points>| handle.measure_anchor(*bounds))
                .on_press(move || handle.set_open(!handle.is_open()))
                // UI-BUTTON has no icon-micro size; its 20px ghost-muted look is written here.
                .app_class(tw("flex size-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"))
                .child(icon(Icon::Layers, 12.0, move || match state {
                    InheritanceState::Overridden => palette().primary,
                    InheritanceState::Mixed => palette().warning,
                    _ => palette().muted_foreground,
                })),
        );
    let inheritance = Rc::new(inheritance);
    popover(
        handle,
        PopoverOptions {
            align: Align::Start,
            width: Size::Md,
            padding: Padding::None,
            ..Default::default()
        },
        trigger,
        move || {
            let inheritance = inheritance.clone();
            let multiple = inheritance.multiple_environments;
            let group_count = inheritance.groups.len();
            let mut sections = view().app_class(tw("flex flex-col"));
            for (index, members) in inheritance.groups.iter().enumerate() {
                let Some(first) = members.first() else {
                    continue;
                };
                let chain = {
                    let mut list = view()
                        .role(AccessibilityRole::List)
                        .app_class(tw("flex flex-col text-sm"));
                    for layer in &first.layers {
                        let name = if layer.key == LayerKey::Environment {
                            t("Environment")
                        } else {
                            shown(&layer.label)
                        };
                        list = list.child(
                            view()
                                .app_class(tw(&cn([
                                    Some("grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 rounded-md px-2 py-1"),
                                    layer.effective.then_some("bg-foreground/[0.06]"),
                                ])))
                                .child(text(name).number_of_lines(1u32).app_class(tw(if layer.effective {
                                    "min-w-0 truncate font-medium text-foreground"
                                } else {
                                    "min-w-0 truncate text-muted-foreground"
                                })))
                                .child(
                                    view()
                                        .app_class(tw(if layer.effective {
                                            "flex flex-row items-center gap-1.5 text-foreground"
                                        } else if layer.set {
                                            "flex flex-row items-center gap-1.5 text-muted-foreground"
                                        } else {
                                            "flex flex-row items-center gap-1.5 text-muted-foreground/60"
                                        }))
                                        .child(
                                            text(shown(&layer.value))
                                                .number_of_lines(1u32)
                                                .app_class(tw("max-w-32 truncate")),
                                        )
                                        .child(if layer.effective {
                                            icon(Icon::Check, 14.0, move || palette().primary).into_element()
                                        } else {
                                            view().app_class(tw("size-3.5 shrink-0")).into_element()
                                        }),
                                ),
                        );
                    }
                    list
                };
                let mut section = view().app_class(tw(&cn([
                    Some("px-3 py-2.5"),
                    (index > 0).then_some("border-t border-border/60"),
                ])));
                if !multiple {
                    section = section.child(
                        text(first.target_label.clone())
                            .role(AccessibilityRole::Header)
                            .number_of_lines(1u32)
                            .app_class(tw("pb-1.5 text-xs font-medium text-muted-foreground")),
                    );
                }
                if multiple && group_count > 1 {
                    let winning = first.winning();
                    let value = winning.map(|l| shown(&l.value)).unwrap_or_default();
                    let layer = winning
                        .map(|l| {
                            if l.key == LayerKey::Environment {
                                t("Environment")
                            } else {
                                shown(&l.label)
                            }
                        })
                        .unwrap_or_default();
                    let fold = crate::ui::collapsible::CollapsibleHandle::new(false);
                    let mut members_list = view().app_class(tw(
                        "flex min-w-0 flex-col gap-0.5 text-xs text-muted-foreground",
                    ));
                    for member in members {
                        members_list = members_list.child(
                            text(member.label.clone())
                                .number_of_lines(1u32)
                                .app_class(tw("truncate")),
                        );
                    }
                    section = section
                        .child(crate::ui::collapsible::collapsible_trigger(
                            fold,
                            Text::content(value.clone()),
                            view()
                                .app_class(tw("flex w-full min-w-0 flex-col"))
                                .child(
                                    view()
                                        .app_class(tw("flex min-w-0 flex-row items-center gap-2"))
                                        .child(text(value).number_of_lines(1u32).app_class(tw(
                                            "min-w-0 flex-1 truncate text-sm text-foreground",
                                        )))
                                        .child(
                                            text(layer)
                                                .app_class(tw("text-xs text-muted-foreground")),
                                        )
                                        .child(icon_with_class(
                                            Icon::ChevronRight,
                                            "size-3 shrink-0 text-muted-foreground",
                                        )),
                                )
                                .child(members_list),
                            "w-full justify-start rounded-md px-2 py-1.5 text-left hover:bg-accent",
                        ))
                        .child(crate::ui::collapsible::collapsible_panel(
                            fold,
                            !reduce_motion(),
                            view().app_class(tw("pt-1.5")).child(chain),
                        ));
                } else {
                    section = section.child(chain).child(multiple.then(|| {
                        text(t("Same on all machines"))
                            .app_class(tw("px-2 pt-1.5 text-xs text-muted-foreground"))
                    }));
                }
                let overriding: Vec<_> = inheritance
                    .overriding
                    .iter()
                    .filter(|p| {
                        members
                            .iter()
                            .any(|m| m.environment_id == p.entry.environment_id.0)
                    })
                    .cloned()
                    .collect();
                if !overriding.is_empty() {
                    let entries: Vec<_> = overriding.iter().map(|p| p.entry.clone()).collect();
                    let scoped: Vec<&'static str> = keys
                        .iter()
                        .copied()
                        .filter(|k| is_project_scoped_setting_key(k))
                        .collect();
                    let mut projects = view()
                        .role(AccessibilityRole::List)
                        .app_class(tw("mt-0.5 flex flex-col text-sm"));
                    for project in overriding.iter().cloned() {
                        let value = members
                            .iter()
                            .flat_map(|m| m.override_values.iter())
                            .find(|(id, _)| *id == project.entry.project_id.0)
                            .map(|(_, v)| shown(v))
                            .unwrap_or_default();
                        let machine = if multiple {
                            members
                                .iter()
                                .find(|m| m.environment_id == project.entry.environment_id.0)
                                .map(|m| m.machine_label.clone())
                        } else {
                            None
                        };
                        let open = project.open.clone();
                        projects = projects.child(
                            view()
                                .app_class(tw("grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 px-2 py-1"))
                                .child(
                                    view()
                                        .app_class(tw("flex min-w-0 flex-col items-start"))
                                        .child(
                                            nsh_base::button()
                                                .accessibility_label(Some(SharedString::from(project.label.clone())))
                                                .on_press(move || {
                                                    handle.set_open(false);
                                                    context.select_scope(open.clone());
                                                })
                                                .app_class(tw("min-w-0 max-w-full text-left font-medium text-foreground underline-offset-4 hover:underline"))
                                                .child(
                                                    text(project.label.clone())
                                                        .number_of_lines(1u32)
                                                        .app_class(tw("truncate")),
                                                ),
                                        )
                                        .child(machine.map(|m| {
                                            text(m)
                                                .number_of_lines(1u32)
                                                .app_class(tw("max-w-full truncate text-xs text-muted-foreground"))
                                        })),
                                )
                                .child(
                                    text(value)
                                        .number_of_lines(1u32)
                                        .app_class(tw("max-w-32 truncate text-muted-foreground")),
                                ),
                        );
                    }
                    let count = entries.len();
                    let reset_label = if count == 1 {
                        t("Reset it")
                    } else {
                        t("Reset all")
                    };
                    section = section.child(
                        view()
                            .app_class(tw("mt-2 border-t border-border/60 pt-2"))
                            .child(
                                view()
                                    .app_class(tw("flex flex-row items-center justify-between gap-3 px-2 text-xs text-muted-foreground"))
                                    .child(text(t("Overridden by")))
                                    .child(can_clear.then(|| {
                                        let entries = entries.clone();
                                        let scoped = scoped.clone();
                                        nsh_base::button()
                                            .accessibility_label(Some(SharedString::from(reset_label.clone())))
                                            .on_press(move || {
                                                context.clear_project_overrides(entries.clone(), scoped.clone())
                                            })
                                            .app_class(tw("text-xs font-medium text-foreground underline-offset-4 hover:underline"))
                                            .child(text(reset_label.clone()))
                                    })),
                            )
                            .child(projects),
                    );
                }
                sections = sections.child(section);
            }
            sections.into_element()
        },
    )
}
