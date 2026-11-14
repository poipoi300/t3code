pub use crate::state::selectors::{sidebar_threads, timeline};
pub mod code;
pub mod composer;
pub mod document;
pub mod drafts;
pub mod embedded_content;
pub mod entities;
pub mod languages;
pub mod lists;
pub mod messages_timeline;
pub mod outbox;
pub mod questions;
pub mod relationships;
pub mod requests;
pub mod rich_text;
pub mod shelves;
pub mod subagent_inspector;
pub mod tag_filter;
pub mod thread_ops;
pub mod threads;
pub mod timeline_events;
pub mod undo;
pub mod user_message;
pub mod visits;
pub mod worktree_setup;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadShelf {
    Active,
    Working,
    Settled,
    Snoozed,
    Archived,
}
