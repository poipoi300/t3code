//! The right panel's diff view without the drawing: DiffPanel.tsx's selection, queries and
//! file facts, diffPanelStore.ts's session state, and the changed-file kinds it filters by.
pub mod changed_file_kind;
pub mod comments;
pub mod files;
pub mod logic;
mod service;
pub mod tree;
pub use service::{
    BranchCommitsView, DiffPanelService, DiffPanelSnapshot, DiffPanelState, ShownFiles,
};
