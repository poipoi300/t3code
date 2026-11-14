//! `components/ui/button.tsx`: variants and sizes by the web app's names.

use nsh_core::prelude::*;

use super::icon::{Icon, icon};
use crate::theme::{Metrics, Palette, metrics, palette};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Destructive,
    DestructiveOutline,
    Ghost,
    GhostMuted,
    GhostDestructive,
    Link,
    Outline,
    Secondary,
    WarningOutline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Default,
    Compact,
    Xs,
    Sm,
    Lg,
    Icon,
    IconSm,
    IconXs,
}

impl ButtonSize {
    pub fn height(self, m: &Metrics) -> f32 {
        match self {
            Self::Default | Self::Icon => m.control,
            Self::Sm | Self::IconSm => m.control_sm,
            Self::Compact => 28.0,
            Self::Xs | Self::IconXs => m.control_xs,
            Self::Lg => m.control_lg,
        }
    }

    pub fn is_icon(self) -> bool {
        matches!(self, Self::Icon | Self::IconSm | Self::IconXs)
    }

    fn padding(self) -> f32 {
        match self {
            Self::Default => 11.0,
            Self::Sm => 9.0,
            Self::Compact | Self::Xs => 7.0,
            Self::Lg => 13.0,
            Self::Icon | Self::IconSm | Self::IconXs => 0.0,
        }
    }

    fn text_size(self, m: &Metrics) -> f32 {
        match self {
            Self::Compact => 12.0,
            Self::Xs => m.text_xs,
            _ => m.text,
        }
    }
}

/// The colours of a variant: rest background, hover background, border, text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VariantColors {
    pub background: AdaptiveColor,
    pub hover: AdaptiveColor,
    pub border: AdaptiveColor,
    pub foreground: AdaptiveColor,
    pub icon: AdaptiveColor,
}

pub fn variant_colors(variant: ButtonVariant, p: &Palette) -> VariantColors {
    let clear = AdaptiveColor::TRANSPARENT;
    let outline_bg = AdaptiveColor {
        light: p.popover.light,
        dark: p.input.opacity(0.32).dark,
    };
    let outline_hover = AdaptiveColor {
        light: p.accent.opacity(0.5).light,
        dark: p.input.opacity(0.64).dark,
    };
    match variant {
        ButtonVariant::Default => VariantColors {
            background: p.primary,
            hover: p.primary.opacity(0.9),
            border: p.primary,
            foreground: p.primary_foreground,
            icon: p.primary_foreground,
        },
        ButtonVariant::Destructive => VariantColors {
            background: p.destructive,
            hover: p.destructive.opacity(0.9),
            border: p.destructive,
            foreground: AdaptiveColor::rgb(0xffffff, 0xffffff),
            icon: AdaptiveColor::rgb(0xffffff, 0xffffff),
        },
        ButtonVariant::DestructiveOutline => VariantColors {
            background: outline_bg,
            hover: p.destructive.opacity(0.04),
            border: p.input,
            foreground: p.destructive_foreground,
            icon: p.destructive_foreground,
        },
        ButtonVariant::Ghost => VariantColors {
            background: clear,
            hover: p.accent,
            border: clear,
            foreground: p.foreground,
            icon: p.muted_foreground,
        },
        ButtonVariant::GhostMuted => VariantColors {
            background: clear,
            hover: p.accent,
            border: clear,
            foreground: p.muted_foreground,
            icon: p.muted_foreground,
        },
        ButtonVariant::GhostDestructive => VariantColors {
            background: clear,
            hover: p.accent,
            border: clear,
            foreground: p.muted_foreground,
            icon: p.muted_foreground,
        },
        ButtonVariant::Link => VariantColors {
            background: clear,
            hover: clear,
            border: clear,
            foreground: p.foreground,
            icon: p.foreground,
        },
        ButtonVariant::Outline => VariantColors {
            background: outline_bg,
            hover: outline_hover,
            border: p.input,
            foreground: p.foreground,
            icon: p.muted_foreground,
        },
        ButtonVariant::Secondary => VariantColors {
            background: p.secondary,
            hover: p.secondary.opacity(0.9),
            border: clear,
            foreground: p.secondary_foreground,
            icon: p.secondary_foreground,
        },
        ButtonVariant::WarningOutline => VariantColors {
            background: p.warning_surface,
            hover: p.warning.opacity(0.16),
            border: p.warning.opacity(0.32),
            foreground: p.warning_foreground,
            icon: p.warning_foreground,
        },
    }
}

/// What a button shows: a label, an icon, or both. The label is already translated.
pub enum ButtonContent {
    Label(String),
    Icon { icon: Icon, label: String },
    IconLabel { icon: Icon, label: String },
    /// The caller's own element in the button's frame (a glyph at its own size, with its own
    /// motion); `label` is the button's accessible name, already translated.
    Custom { label: String, child: AnyElement },
}

/// A button. `disabled` follows a signal or closure; a disabled button is 64% opaque and inert.
pub fn button(
    content: ButtonContent,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let colors = move || variant_colors(variant, &palette());
    let m = metrics;
    let (label, glyph, show_label, custom) = match content {
        ButtonContent::Label(label) => (label, None, true, None),
        ButtonContent::Icon { icon, label } => (label, Some(icon), false, None),
        ButtonContent::IconLabel { icon, label } => (label, Some(icon), true, None),
        ButtonContent::Custom { label, child } => (label, None, false, Some(child)),
    };
    let disabled = std::rc::Rc::new(disabled);
    let disabled_for_opacity = disabled.clone();
    let accessibility = SharedString::from(label.clone());
    nsh_base::button()
        .accessibility_label(Some(accessibility))
        .disabled(move || disabled())
        .on_press(on_press)
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px(if size == ButtonSize::Compact {
            4.0
        } else {
            8.0
        }))
        .h(move || px(size.height(&m())))
        .min_w(move || {
            px(if size.is_icon() {
                size.height(&m())
            } else {
                0.0
            })
        })
        .px(px(size.padding()))
        .rounded(move || px(m().radius))
        .border_1()
        .border_color(move || colors().border)
        .bg(move || colors().background)
        .hover(move |s| s.bg(colors().hover))
        .active(|s| s.scale(0.97))
        .focus_visible(move |s| s.ring(px(2.0)).ring_inset().ring_color(palette().ring))
        .opacity(move || {
            if disabled_for_opacity() {
                0.64_f32
            } else {
                1.0
            }
        })
        .hit_slop(move || {
            let m = m();
            let slack = ((m.hit_target - size.height(&m)) / 2.0).max(0.0);
            EdgeInsets::all(px(slack))
        })
        .child(glyph.map(|glyph| icon(glyph, m().icon, move || colors().icon)))
        .child(show_label.then(|| {
            text(SharedString::from(label))
                .text_size(move || px(size.text_size(&m())))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || colors().foreground)
                .number_of_lines(1u32)
        }))
        .child(custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::layout::Pointer;
    use crate::theme::{Density, Metrics};

    #[test]
    fn sizes_follow_the_web_scale_on_both_densities() {
        let regular = Metrics::of(Density::Regular, Pointer::Fine);
        let large = Metrics::of(Density::Large, Pointer::Fine);
        assert_eq!(ButtonSize::Default.height(&regular), 32.0);
        assert_eq!(ButtonSize::Default.height(&large), 36.0);
        assert_eq!(ButtonSize::IconSm.height(&regular), 28.0);
        assert_eq!(ButtonSize::Compact.height(&large), 28.0);
    }

    mod custom_content {
        use super::super::*;
        use nsh_engine::{EngineConfig, Root};
        use nsh_testing::Headless;

        #[component]
        fn CustomButtonProbe() -> impl IntoElement {
            let presses = signal(0_u32);
            view()
                .child(button(
                    ButtonContent::Custom {
                        label: crate::i18n::t("Fork"),
                        child: text(SharedString::from("custom glyph")).into_element(),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::IconXs,
                    || false,
                    move || presses.update(|n| *n += 1),
                ))
                .child(button(
                    ButtonContent::Label(crate::i18n::t("Copy")),
                    ButtonVariant::Outline,
                    ButtonSize::Sm,
                    || false,
                    || {},
                ))
                .child(text(move || SharedString::from(format!("presses {}", presses.get()))))
        }

        #[test]
        fn custom_content_keeps_the_frame_label_and_press_and_leaves_labels_alone() {
            let mut sets = crate::registries();
            let mut fixtures = Registry::new();
            fixtures.register::<CustomButtonProbe>();
            sets.push(fixtures);
            let mut app = Headless::start(EngineConfig {
                ui_sets: sets,
                viewport: size(px(390.0), px(844.0)),
                ..EngineConfig::new(Root::of::<CustomButtonProbe>())
            })
            .expect("button probe starts");
            app.wait_for("custom glyph").expect("the caller's element is drawn");
            app.press("role:button[Fork]").expect("named by its label");
            app.wait_for("presses 1").expect("the press reaches the handler");
            assert!(app.screen().shows("Copy"), "a label button still draws its label");
            assert!(!app.screen().shows("Fork"), "a custom button draws no label text");
        }
    }

    #[test]
    fn variants_keep_their_roles_distinct() {
        let tokens = crate::theme::tokens_for(&crate::theme::ThemeChoice::default());
        let p = crate::theme::Palette::of(&tokens);
        let primary = variant_colors(ButtonVariant::Default, &p);
        let ghost = variant_colors(ButtonVariant::Ghost, &p);
        let outline = variant_colors(ButtonVariant::Outline, &p);
        assert_eq!(primary.background, p.primary);
        assert_eq!(primary.foreground, p.primary_foreground);
        assert_eq!(ghost.background, AdaptiveColor::TRANSPARENT);
        assert_eq!(ghost.hover, p.accent);
        assert_eq!(outline.border, p.input);
        assert_ne!(outline.background.light, outline.background.dark);
    }
}
