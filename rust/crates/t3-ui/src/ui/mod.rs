//! The component layer: `apps/web/src/components/ui` in Rust.
//!
//! A primitive takes a variant and a size enum, named as the web app names them, and returns
//! `impl IntoElement`, so callers cannot restyle it: layout (width, flex, margin) belongs to the
//! parent. If no variant fits a generic need, add one here; a feature's own look stays in the
//! feature's module.

pub mod alert;
pub mod avatar;
pub mod badge;
pub mod button;
pub mod calendar;
pub mod collapsible;
pub mod color_picker;
pub mod combobox;
pub mod command;
pub mod confirm;
pub mod dialog;
pub mod icon;
pub mod input;
pub mod kbd;
pub mod menu;
pub mod middle_truncate;
pub mod panel;
pub mod placeholder;
pub mod popover;
pub mod qr_code;
pub mod resizable;
pub mod scroll_area;
pub mod select;
pub mod sheet;
pub mod sidebar;
pub mod skeleton;
pub mod slider;
pub mod status;
pub mod surface;
pub mod switch;
pub mod table;
pub mod tabs;
pub mod text;
pub mod toast;
pub mod toggle;
pub mod tooltip;
pub mod wizard;
