//! Application-shell presentation logic: sidebar, command palette, keybindings, model selection.
//!
//! Start the app shell with [`sidebar::presentation`], using WP6 environment-thread shells and
//! WP8 logical project snapshots. [`sidebar`] documents ordering, unread state and host scheduling.
//! [`palette`] describes actions and mode transitions; [`models`] owns instance-scoped picks and
//! composer model resolution. [`keybindings`] and [`shortcuts`] accept host-supplied focus/key data.
//!
//! The local stores expose plain state, transitions and legacy JSON codecs. The host owns storage
//! notifications, durability, DOM/browser listeners and rendering. Keep decode failures' source
//! bytes and old keys until a successful migration; stash writers report durability explicitly.
#[path = "shell/closed_views.rs"]
pub mod closed_views;
#[path = "shell/desktop_update.rs"]
pub mod desktop_update;
#[path = "shell/diff_state.rs"]
pub mod diff_state;
#[path = "shell/history_swipe.rs"]
pub mod history_swipe;
#[path = "shell/keybindings.rs"]
pub mod keybindings;
#[path = "shell/models.rs"]
pub mod models;
#[path = "shell/navigation.rs"]
pub mod navigation;
#[path = "shell/palette.rs"]
pub mod palette;
#[path = "shell/palette_files.rs"]
pub mod palette_files;
#[path = "shell/panels.rs"]
pub mod panels;
#[path = "shell/preview_state.rs"]
pub mod preview_state;
#[path = "shell/request_latency.rs"]
pub mod request_latency;
#[path = "shell/search.rs"]
pub mod search;
#[path = "shell/shortcuts.rs"]
pub mod shortcuts;
#[path = "shell/shortcut_config.rs"]
pub mod shortcut_config;
#[path = "shell/sidebar.rs"]
pub mod sidebar;
#[path = "shell/sidebar_width.rs"]
pub mod sidebar_width;
#[path = "shell/skill_search.rs"]
pub mod skill_search;
#[path = "shell/stash.rs"]
pub mod stash;
#[path = "shell/terminal_state.rs"]
pub mod terminal_state;
#[path = "shell/ui_state.rs"]
pub mod ui_state;
