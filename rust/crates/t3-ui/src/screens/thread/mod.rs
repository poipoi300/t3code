//! `/$environmentId/$threadId`: one thread, in PNL-HOST's workspace.

pub mod blocks;
pub mod composer;
pub mod find;
pub mod guest_pending;
pub mod header;
pub mod markdown;
pub mod minimap;
pub mod rewind;
pub mod rows;
pub mod selection;
pub mod subagents;
pub mod timeline;
pub mod viewport;

use nsh_core::prelude::*;
use t3_client::types::ThreadKey;

/// PNL-HOST's thread parent: header, timeline, composer, details card and workspace panels, with
/// the thread's open/close demand.
pub fn screen(key: ThreadKey) -> AnyElement {
    // The header's "New thread in {project}" navigates through the client from this thread.
    if let Some(client) = crate::runtime::rt().backend().client() {
        provide_context(crate::runtime::services::thread_ops::thread_header_navigation(
            &client,
            key.clone(),
        ));
    }
    crate::screens::panel::thread_workspace(key).into_element()
}
