//! Pull request view models shared by every client surface.
#[path = "pr_views/detail.rs"]
pub mod detail;
#[path = "pr_views/diff_http.rs"]
pub mod diff_http;
#[path = "pr_views/file_order.rs"]
pub mod file_order;
#[path = "pr_views/forge.rs"]
pub mod forge;
#[path = "pr_views/groups.rs"]
pub mod groups;
#[path = "pr_views/guide.rs"]
pub mod guide;
#[path = "pr_views/handoff.rs"]
pub mod handoff;
#[path = "pr_views/markdown.rs"]
pub mod markdown;
#[path = "pr_views/preferences.rs"]
pub mod preferences;
#[path = "pr_views/queries.rs"]
pub mod queries;
#[path = "pr_views/reviewers.rs"]
pub mod reviewers;
#[path = "pr_views/routing.rs"]
pub mod routing;
#[path = "pr_views/service.rs"]
pub mod service;

pub fn section_collapse_anchor_scroll_top(
    scroll_top: f64,
    viewport_top: f64,
    section_top: f64,
    heading_top: f64,
) -> Option<f64> {
    (section_top < viewport_top && heading_top <= viewport_top + 1.0)
        .then(|| (scroll_top + section_top - viewport_top).max(0.0))
}
