//! `/$environmentId/$threadId`: one thread, in PNL-HOST's workspace.

pub mod blocks;
pub mod composer;
pub mod find;
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
    crate::screens::panel::thread_workspace(key).into_element()
}
