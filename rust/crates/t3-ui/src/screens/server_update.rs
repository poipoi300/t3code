//! Server version skew for connected environments. The launcher owns self-update execution.
use crate::{i18n::t, theme::tw};
use nsh_core::prelude::*;
use t3_client::{
    composition::ClientRuntime,
    platform::Platform,
    view_logic::session::version::{
        manual_server_update_command, resolve_server_config_version_mismatch,
    },
};
pub(super) fn notices<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    version: Option<&str>,
) -> Vec<AnyElement> {
    let Some(version) = version else {
        return Vec::new();
    };
    client
        .usage_targets()
        .into_iter()
        .filter(|t| t.connected)
        .filter_map(|tgt| {
            let config = tgt.config?;
            let mismatch = resolve_server_config_version_mismatch(Some(&config), version)?;
            let mut row = view()
                .app_class(tw("flex min-w-0 flex-col gap-1 px-2 py-1"))
                .child(
                    text(format!(
                        "{} · {} → {}",
                        tgt.label, mismatch.server_version, mismatch.client_version
                    ))
                    .app_class(tw("text-xs font-medium")),
                )
                .child(text(t(mismatch.hint)).app_class(tw("text-xs text-muted-foreground")));
            if config.environment.capabilities.server_self_update.is_none()
                && config
                    .managed_policy
                    .as_ref()
                    .and_then(|p| p.limits.as_ref())
                    .and_then(|limits| limits.updates.as_ref())
                    .is_none_or(|p| p.mode != t3_contracts::managed_policy::ManagedUpdatesMode::Off)
            {
                row = row.child(
                    text(manual_server_update_command(
                        version,
                        config.environment.capabilities.server_installation.as_ref(),
                    ))
                    .app_class(tw("font-mono text-xs")),
                );
            }
            Some(row.into_element())
        })
        .collect()
}
