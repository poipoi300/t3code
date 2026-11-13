use std::cell::RefCell;
use std::sync::Arc;

use nsh_core::prelude::*;

macro_rules! palette {
    ($( $field:ident = $token:literal ),* $(,)?) => {
        /// The colours components draw with, each a light and a dark value. Names follow the web
        /// app's tokens; `sidebar_*` fields are the values inside the sidebar subtree.
        #[derive(Clone, Debug, PartialEq)]
        pub struct Palette {
            $(pub $field: AdaptiveColor,)*
        }

        impl Palette {
            pub fn of(tokens: &Tokens) -> Self {
                Self {
                    $($field: lookup(tokens, $token).unwrap_or(AdaptiveColor::TRANSPARENT),)*
                }
            }

            /// Token names the palette reads that the resolved theme does not define.
            pub fn missing(tokens: &Tokens) -> Vec<&'static str> {
                [$($token),*].into_iter().filter(|name| lookup(tokens, name).is_none()).collect()
            }
        }
    };
}

palette! {
    background = "background",
    foreground = "foreground",
    card = "card",
    card_foreground = "card-foreground",
    popover = "popover",
    popover_foreground = "popover-foreground",
    primary = "primary",
    primary_foreground = "primary-foreground",
    secondary = "secondary",
    secondary_foreground = "secondary-foreground",
    muted = "muted",
    muted_foreground = "muted-foreground",
    accent = "accent",
    accent_foreground = "accent-foreground",
    destructive = "destructive",
    destructive_foreground = "destructive-foreground",
    border = "border",
    input = "input",
    ring = "ring",
    placeholder = "placeholder",
    icon_muted = "icon-muted",
    secondary_label = "secondary-label",
    success = "success",
    warning = "warning",
    warning_surface = "warning-surface",
    warning_foreground = "warning-foreground",
    info = "info",
    error = "error",
    error_surface = "error-surface",
    error_foreground = "error-foreground",
    message_surface = "message",
    message_foreground = "message-foreground",
    message_action_foreground = "message-action-foreground",
    code_background = "code",
    code_foreground = "code-foreground",
    toolbar_background = "toolbar-background",
    toolbar_border = "toolbar-border",
    app_chrome_background = "app-chrome-background",
    surface_raised = "surface-raised",
    sidebar = "sb-sidebar",
    sidebar_foreground = "sb-sidebar-foreground",
    sidebar_muted_foreground = "sb-sidebar-muted-foreground",
    sidebar_border = "sb-sidebar-border",
    sidebar_row_hover = "sb-sidebar-row-hover",
    sidebar_row_selected = "sb-sidebar-row-selected",
    sidebar_row_active = "sb-sidebar-row-active",
    sidebar_icon = "sb-sidebar-icon-color",
    sidebar_accent = "sb-accent",
    sidebar_input = "sb-input",
}

/// What a Tailwind class of that name paints (see `pair`).
fn lookup(tokens: &Tokens, name: &str) -> Option<AdaptiveColor> {
    tokens.color(name)
}

thread_local! {
    static CURRENT: RefCell<Option<(Arc<Tokens>, Arc<Palette>)>> = const { RefCell::new(None) };
}

/// The palette of the installed tokens. Reading it subscribes, so bindings follow a theme change;
/// it is rebuilt only when the tokens change.
pub fn palette() -> Arc<Palette> {
    let tokens = super::current_tokens();
    CURRENT.with_borrow_mut(|current| match current {
        Some((source, palette)) if Arc::ptr_eq(source, &tokens) => palette.clone(),
        _ => {
            let palette = Arc::new(Palette::of(&tokens));
            *current = Some((tokens, palette.clone()));
            palette
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_reads_what_classes_paint_including_contrast_and_sidebar_scope() {
        let stock = Palette::of(&super::super::tokens_for(
            &super::super::ThemeChoice::default(),
        ));
        let high = Palette::of(&super::super::tokens_for(&super::super::ThemeChoice {
            contrast: 200,
            ..super::super::ThemeChoice::default()
        }));
        assert_ne!(stock.muted_foreground, high.muted_foreground);
        assert_ne!(stock.foreground, high.foreground);
        assert_eq!(stock.primary, high.primary);
        let tokens = super::super::tokens_for(&super::super::ThemeChoice::default());
        assert_ne!(
            stock.sidebar_row_selected,
            tokens
                .color("color-sidebar-row-selected")
                .expect("root alias")
        );
    }

    #[test]
    fn the_resolved_stock_theme_defines_every_palette_colour() {
        let tokens = super::super::tokens_for(&super::super::ThemeChoice::default());
        assert_eq!(Palette::missing(&tokens), Vec::<&str>::new());
    }
}
