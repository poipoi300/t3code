//! Settings lane: binds `t3_client::services::settings::service::SettingsService` to the
//! settings shell's `SettingsService` contract. Pure delegation; the rules are the client's.
use crate::screens::settings::shell::{SettingsRuntime, SettingsService};
use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
use std::rc::Rc;
use t3_client::{
    composition::ClientRuntime,
    navigation::SettingsPage,
    platform::Platform,
    services::settings::{
        scope::SettingsScopeSearch,
        scoped::SettingsPatch,
        search::SettingsSearchAvailability,
        service::{
            SettingsScopeView, SettingsService as ClientSettings, SettingsUpdateOutcome,
            SettingsWriteAccess,
        },
    },
};
use t3_contracts::settings::ClientSettingsSchema;

struct SettingsBinding<P: Platform + Clone + 'static>(ClientSettings<P>);

/// The settings pages' services over a client runtime (the host's, or a test's).
pub fn settings<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> SettingsRuntime {
    SettingsRuntime(Rc::new(SettingsBinding(client.settings())))
}

impl<P: Platform + Clone + 'static> SettingsService for SettingsBinding<P> {
    fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.0.changes()
    }
    fn scope_view(&self, page: SettingsPage, search: &SettingsScopeSearch) -> SettingsScopeView {
        self.0.scope_view(page, search)
    }
    fn client_settings(&self) -> Rc<ClientSettingsSchema> {
        self.0.client_settings()
    }
    fn write_access(&self, view: &SettingsScopeView) -> SettingsWriteAccess {
        self.0.write_access(view)
    }
    fn update(
        &self,
        view: &SettingsScopeView,
        patch: SettingsPatch,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        self.0.update(view, patch)
    }
    fn clear(
        &self,
        view: &SettingsScopeView,
        keys: Vec<&'static str>,
    ) -> LocalBoxFuture<'static, SettingsUpdateOutcome> {
        self.0.clear(view, &keys)
    }
    fn last_enabled_grouping_mode(&self) -> LocalBoxFuture<'static, &'static str> {
        self.0.last_enabled_grouping_mode()
    }
    fn remember_enabled_grouping_mode(&self, mode: &str) -> LocalBoxFuture<'static, ()> {
        self.0.remember_enabled_grouping_mode(mode)
    }
    fn search_availability(&self) -> SettingsSearchAvailability {
        let environments = self.0.environments();
        SettingsSearchAvailability::from_environments(
            &environments,
            self.0.primary_environment().is_some(),
        )
    }
}
