//! Lucide icons (the set the web app uses), tinted from the palette.

use nsh_core::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    ArrowLeft,
    ArrowUp,
    Check,
    ChevronDown,
    ChevronRight,
    CircleAlert,
    Copy,
    Ellipsis,
    Folder,
    FolderPlus,
    GitBranch,
    Link,
    MessageSquare,
    PanelLeft,
    Plus,
    Search,
    Settings,
    Square,
    SquarePen,
    WifiOff,
    X,
}

impl Icon {
    pub fn asset(self) -> ImageAsset {
        match self {
            Self::ArrowLeft => asset!("icons/arrow-left.svg"),
            Self::ArrowUp => asset!("icons/arrow-up.svg"),
            Self::Check => asset!("icons/check.svg"),
            Self::ChevronDown => asset!("icons/chevron-down.svg"),
            Self::ChevronRight => asset!("icons/chevron-right.svg"),
            Self::CircleAlert => asset!("icons/circle-alert.svg"),
            Self::Copy => asset!("icons/copy.svg"),
            Self::Ellipsis => asset!("icons/ellipsis.svg"),
            Self::Folder => asset!("icons/folder.svg"),
            Self::FolderPlus => asset!("icons/folder-plus.svg"),
            Self::GitBranch => asset!("icons/git-branch.svg"),
            Self::Link => asset!("icons/link.svg"),
            Self::MessageSquare => asset!("icons/message-square.svg"),
            Self::PanelLeft => asset!("icons/panel-left.svg"),
            Self::Plus => asset!("icons/plus.svg"),
            Self::Search => asset!("icons/search.svg"),
            Self::Settings => asset!("icons/settings.svg"),
            Self::Square => asset!("icons/square.svg"),
            Self::SquarePen => asset!("icons/square-pen.svg"),
            Self::WifiOff => asset!("icons/wifi-off.svg"),
            Self::X => asset!("icons/x.svg"),
        }
    }
}

/// An icon drawn at `size` points in `color`. Decorative: the control around it carries the label.
pub fn icon(
    which: Icon,
    size: f32,
    color: impl Fn() -> AdaptiveColor + 'static,
) -> impl IntoElement {
    image(which.asset())
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .tint_color(color)
        .accessible(false)
}
