use super::*;

impl<P: Platform + Clone + 'static> ClientRuntime<P> {
    /// The settings pages' service (`services::settings::service`).
    pub fn settings(&self) -> crate::services::settings::service::SettingsService<P> {
        crate::services::settings::service::SettingsService::new(
            self.clone(),
            Rc::new(self.inner.platform.clone()),
            self.inner.spawn.clone(),
        )
    }
}
