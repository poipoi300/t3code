//! Theme palettes: roles, built-in themes, derivation from seeds, theme files and
//! the saved library. A port of `apps/web/src/themePalette.ts` and
//! `packages/shared/src/themePalettes.ts`.

mod definition;
mod derive;
mod files;
mod library;
pub mod math;
mod preference;
mod roles;

pub use definition::*;
pub use derive::{
    CONTEXT_CHIP_INVALID_VARIABLE, CONTEXT_CHIP_MESSAGE_VARIABLE, context_chip_variables,
    create_vivid_theme_colors, update_theme_color_family,
};
pub use files::*;
pub use library::*;
pub use math::{readable_theme_color_on, theme_color_to_hex, to_canonical_theme_color};
pub use preference::*;
pub use roles::{ROLE_COUNT, ThemeColorRole};
