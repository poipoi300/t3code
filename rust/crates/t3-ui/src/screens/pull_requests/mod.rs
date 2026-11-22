//! Pull request workspace (PR-*).

pub mod detail;
pub mod files;
pub mod list;
pub mod review;

mod markdown;

mod actions;
mod guide;
mod pickers;
mod stack;

use std::time::Duration;

use nsh_core::prelude::*;

pub(super) fn host_display_label(provider: &str) -> String {
    match provider {
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "azure-devops" => "Azure DevOps",
        other => other,
    }
    .to_owned()
}

pub(super) fn rejection_reason(error: t3_client::rpc::RpcCallError) -> String {
    match error.into_client_error() {
        t3_client::ClientError::Rejected {
            detail, message, ..
        } => detail.unwrap_or(message),
        _ => String::new(),
    }
}

pub(super) fn update_if_alive<T: Clone + 'static>(sig: Signal<T>, update: impl FnOnce(&mut T)) {
    if let Some(mut value) = sig.try_get() {
        update(&mut value);
        sig.try_set(value);
    }
}

pub(super) fn bind_debounced_query(query: Signal<String>, debounced: Signal<String>) {
    let seq = signal(0u64);
    effect(move || {
        let trimmed = query.get().trim().to_owned();
        let next = seq.get_untracked() + 1;
        seq.set(next);
        if trimmed.is_empty() {
            if debounced.with_untracked(|cur| !cur.is_empty()) {
                debounced.set(String::new());
            }
            return;
        }
        set_timeout(Duration::from_millis(250), move || {
            if seq.try_get() == Some(next)
                && debounced.with_untracked(|cur| cur.as_str() != trimmed)
            {
                debounced.set(trimmed);
            }
        });
    });
}
