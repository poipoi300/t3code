use super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    thread_views::shelves::{PickDecision, SnoozeCalendar, SnoozePreset},
    types::{EnvironmentId, ThreadKey},
};
use t3_contracts::settings::{ServerSettings, SnoozePresetSettings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnoozeMenuState {
    pub supported: bool,
    pub can_operate: bool,
    pub can_snooze: bool,
    pub snoozed: bool,
    pub reason: Option<String>,
}
impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn snooze_menu(&self, key: &ThreadKey) -> SnoozeMenuState {
        let menu = self.thread_action_menu(key);
        let item = menu
            .items
            .iter()
            .find(|i| i.id == "snooze" || i.id == "unsnooze");
        SnoozeMenuState {
            supported: item.is_some(),
            can_operate: self.can_operate(key),
            can_snooze: item.is_some_and(|i| i.id == "snooze" && i.disabled != Some(true)),
            snoozed: item.is_some_and(|i| i.id == "unsnooze"),
            reason: menu.unavailable_reason,
        }
    }
    pub fn snooze_settings(&self, environment: &EnvironmentId) -> SnoozePresetSettings {
        self.runtime
            .store()
            .snapshot()
            .environments
            .get(environment)
            .and_then(|e| e.config.as_ref())
            .map_or_else(
                || ServerSettings::default().snooze_presets,
                |c| c.settings.snooze_presets.clone(),
            )
    }
    pub fn resolve_snooze_presets(
        &self,
        environment: &EnvironmentId,
        calendar: &SnoozeCalendar<'_>,
    ) -> Vec<SnoozePreset> {
        crate::thread_views::shelves::resolve_snooze_presets(
            &self.snooze_settings(environment),
            calendar,
        )
    }
    pub fn pick_snooze_preset(
        &self,
        environment: &EnvironmentId,
        shown: &[SnoozePreset],
        id: &str,
        calendar: &SnoozeCalendar<'_>,
    ) -> Result<SnoozePreset, ClientError> {
        match crate::thread_views::shelves::pick_snooze_preset(
            shown,
            id,
            &self.snooze_settings(environment),
            calendar,
        ) {
            PickDecision::Picked(preset) => Ok(preset),
            PickDecision::Expired => Err(Self::refused(
                "That snooze time has passed. Choose another time.",
            )),
        }
    }
}
