//! Port of apps/web/src/components/ui/menu.tsx Menu and its slots.
//!
//! Public composition: `menu(trigger, options, popup)` / `context_menu(trigger, options,
//! disabled, popup)` own NSH's popup lifecycle; `menu_popup(children)` is its padded content.
//! `menu_button` is the same shared menu with a button trigger, including on web and Android.
//! `MenuOptions` controls the open signal, placement and `Width::{Default, Content}`.
//! Rows take `ItemOptions`: `Variant::{Default, Destructive, Ghost}`, `Density::{Default, Touch}`,
//! inset, reactive disabled state and close-on-select. There is no web `Size` prop: density is
//! independent of the window's `sm` breakpoint. Checkbox rows use `CheckboxVariant` instead.
//! `menu_sub(trigger, disabled, popup)` owns a submenu; `menu_sub_trigger` and `menu_sub_popup`
//! are its slots. Group, label, shortcut, detail, radio indicator and search functions mirror
//! the named TSX components. Labels and content passed by callers are already translated.
//!
//! NSH owns portals, available-height scrolling, keyboard navigation, focus return, outside
//! dismissal, long-press cancellation and the adaptive sheet with submenu Back on touch devices.
//! Popup appearance belongs on NSH's surface (not on the inner content); submenus inherit it.
