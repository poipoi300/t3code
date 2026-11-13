//! `/$environmentId/$threadId`: header, timeline and composer of one thread.

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
use t3_client::state::selectors::{project, thread_shell};
use t3_client::types::ThreadKey;

use crate::runtime::rt;
use crate::theme::palette;

pub fn screen(key: ThreadKey) -> AnyElement {
    let runtime = rt();
    let backend = runtime.backend();
    backend.open_thread(&key);
    let closing = key.clone();
    cleanup(move || backend.close_thread(&closing));
    let thread_id = key.thread.clone();
    let titles = runtime
        .store
        .shell_state(key.environment.clone(), move |shell| {
            let shell = shell?;
            let thread = thread_shell(shell, &thread_id)?;
            let project = project(shell, &thread.project_id).map(|p| p.title.0.clone());
            Some((thread.title.clone(), project))
        });
    view()
        .flex_1()
        .min_w(px(0.0))
        .bg(move || palette().background)
        .child(header::header(titles))
        .child(timeline::timeline(key.clone()))
        .child(composer::composer(key))
        .into_element()
}
