use super::*;
use futures::future::LocalBoxFuture;
use t3_contracts::settings::ClientSettingsSchema;

pub const DETAILS_ITEMS: &[(&str, &str, bool)] = &[
    ("environment", "Environment and worktree", true),
    ("openIn", "Open in editor", true),
    ("scripts", "Project scripts", true),
    ("branch", "Branch and pull request", true),
    ("gitActions", "Git actions", true),
    ("changes", "Changes", true),
    ("servers", "Servers", true),
    ("automations", "Automations", true),
    ("lineage", "Lineage", true),
    ("sideChats", "Side chats", true),
    ("usage", "Usage limits", false),
    ("context", "Context window", false),
];
fn default_visible(id: &str, settings: &ClientSettingsSchema) -> bool {
    if matches!(id, "usage" | "context") {
        settings.thread_details_usage_enabled
    } else {
        DETAILS_ITEMS
            .iter()
            .find(|i| i.0 == id)
            .is_some_and(|i| i.2)
    }
}
pub fn resolve_thread_details_visibility(
    settings: &ClientSettingsSchema,
) -> BTreeMap<String, bool> {
    DETAILS_ITEMS
        .iter()
        .map(|i| {
            (
                i.0.into(),
                settings
                    .thread_details_items
                    .get(i.0)
                    .copied()
                    .unwrap_or_else(|| default_visible(i.0, settings)),
            )
        })
        .collect()
}
pub fn set_thread_details_item(
    settings: &ClientSettingsSchema,
    id: &str,
    visible: bool,
) -> BTreeMap<String, bool> {
    let mut items = settings.thread_details_items.clone();
    items.remove(id);
    if visible != default_visible(id, settings) {
        items.insert(id.into(), visible);
    }
    items
}
pub fn is_default_thread_details_visibility(visibility: &BTreeMap<String, bool>) -> bool {
    DETAILS_ITEMS
        .iter()
        .all(|i| visibility.get(i.0) == Some(&i.2))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailsDensity {
    Full,
    Compact,
    Essential,
}
pub fn resolve_thread_details_density(height: f64, full: f64, compact: f64) -> DetailsDensity {
    if full == 0.0 || full <= height {
        DetailsDensity::Full
    } else if compact == 0.0 || compact <= height {
        DetailsDensity::Compact
    } else {
        DetailsDensity::Essential
    }
}
pub use super::super::chat_canvas::{
    DetailsCardPlacement as DetailsLayout, Frame,
    resolve_thread_details_card_layout as resolve_thread_details_layout,
};
pub fn environment_config(
    store: &crate::ClientStore,
    environment: &EnvironmentId,
) -> Option<Rc<t3_contracts::server::ServerConfig>> {
    store
        .snapshot()
        .environments
        .get(environment)?
        .config
        .clone()
}
/// `isScratchProject`: the shared No project folder must never offer git.
pub fn is_scratch_project(workspace_root: &str, scratch_workspace_root: Option<&str>) -> bool {
    scratch_workspace_root.is_some_and(|root| {
        super::super::project_ops::paths::find_project_by_path(&[workspace_root], root, |p| *p)
            .is_some()
    })
}
impl<P: Platform + Clone + 'static> GitPanel<P> {
    /// ThreadDetailsPanel's `offersGit`: a checkout, a loaded config and a project that is not
    /// the scratch folder. Until the config loads, a No project thread looks like any folder.
    pub fn offers_git(&self, key: &ThreadKey) -> bool {
        let Some(context) = self.workspace_context(key, None) else {
            return false;
        };
        environment_config(self.runtime.store(), &key.environment).is_some_and(|config| {
            !is_scratch_project(
                &context.project.workspace_root.0,
                config.scratch_workspace_root.as_ref().map(|p| p.0.as_str()),
            )
        })
    }
    pub fn details_visibility(&self) -> BTreeMap<String, bool> {
        resolve_thread_details_visibility(&self.runtime.composer_preferences().snapshot())
    }
    pub fn details_changes(&self) -> LocalBoxStream<'static, ()> {
        self.runtime.composer_preferences().changes()
    }
    pub fn set_details_item(
        &self,
        id: String,
        visible: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            if !DETAILS_ITEMS.iter().any(|i| i.0 == id) {
                return Err(ClientError::InvalidPayload(
                    "Unknown thread details item.".into(),
                ));
            }
            service
                .runtime
                .composer_preferences()
                .update(|settings| {
                    settings.thread_details_items = set_thread_details_item(settings, &id, visible);
                })
                .await
        }
        .boxed_local()
    }
    pub fn reset_details_items(&self) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            service
                .runtime
                .composer_preferences()
                .update(|settings| {
                    settings.thread_details_items.clear();
                    settings.thread_details_usage_enabled = false;
                })
                .await
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_scratch_root_itself_is_a_scratch_project() {
        assert!(is_scratch_project(
            "/home/me/.t3/scratch",
            Some("/home/me/.t3/scratch/")
        ));
        assert!(!is_scratch_project(
            "/home/me/repo",
            Some("/home/me/.t3/scratch")
        ));
        assert!(!is_scratch_project("/home/me/.t3/scratch", None));
    }

    #[test]
    fn density_falls_back_from_full_to_compact_to_essential() {
        assert_eq!(
            resolve_thread_details_density(300.0, 0.0, 0.0),
            DetailsDensity::Full
        );
        assert_eq!(
            resolve_thread_details_density(300.0, 280.0, 0.0),
            DetailsDensity::Full
        );
        assert_eq!(
            resolve_thread_details_density(300.0, 320.0, 0.0),
            DetailsDensity::Compact
        );
        assert_eq!(
            resolve_thread_details_density(300.0, 320.0, 240.0),
            DetailsDensity::Compact
        );
        assert_eq!(
            resolve_thread_details_density(200.0, 320.0, 240.0),
            DetailsDensity::Essential
        );
    }
}
