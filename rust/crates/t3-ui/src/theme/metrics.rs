use nsh_core::prelude::*;

use crate::state::layout::{Pointer, SM_BREAKPOINT, pointer};

/// The web app's two size scales: below its `sm` breakpoint controls and text are larger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    /// Below `sm` (640 pt): phones and narrow windows.
    Large,
    /// `sm` and wider.
    Regular,
}

impl Density {
    pub fn of(width: f32) -> Self {
        if width >= SM_BREAKPOINT {
            Self::Regular
        } else {
            Self::Large
        }
    }
}

/// Sizes in points. Heights and text sizes are the Tailwind utilities `components/ui` uses
/// (`h-8 sm:h-7` is `control_sm` 32 / 28), not CSS tokens; radii and insets are tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub control: f32,
    pub control_sm: f32,
    pub control_xs: f32,
    pub control_lg: f32,
    pub icon_button: f32,
    pub icon_button_sm: f32,
    pub text: f32,
    pub text_line: f32,
    pub text_sm: f32,
    pub text_xs: f32,
    pub text_xs_line: f32,
    pub text_2xs: f32,
    pub text_lg: f32,
    pub icon: f32,
    pub sidebar_row: f32,
    pub radius: f32,
    pub radius_md: f32,
    pub radius_lg: f32,
    pub radius_xl: f32,
    pub sidebar_row_inset: f32,
    pub sidebar_inset: f32,
    pub gutter: f32,
    pub topbar: f32,
    pub chat_max_width: f32,
    /// The smallest hit target: 44 on a coarse pointer (`pointer-coarse:min-h-11`), else none.
    pub hit_target: f32,
}

const REGULAR: Metrics = Metrics {
    control: 32.0,
    control_sm: 28.0,
    control_xs: 24.0,
    control_lg: 36.0,
    icon_button: 32.0,
    icon_button_sm: 28.0,
    text: 14.0,
    text_line: 20.0,
    text_sm: 14.0,
    text_xs: 12.0,
    text_xs_line: 16.0,
    text_2xs: 11.0,
    text_lg: 18.0,
    icon: 16.0,
    sidebar_row: 32.0,
    radius: 8.0,
    radius_md: 6.0,
    radius_lg: 10.0,
    radius_xl: 14.0,
    sidebar_row_inset: 10.0,
    sidebar_inset: 8.0,
    gutter: 12.0,
    topbar: 52.0,
    chat_max_width: 736.0,
    hit_target: 0.0,
};

const LARGE: Metrics = Metrics {
    control: 36.0,
    control_sm: 32.0,
    control_xs: 28.0,
    control_lg: 40.0,
    icon_button: 36.0,
    icon_button_sm: 32.0,
    text: 16.0,
    text_line: 24.0,
    text_sm: 14.0,
    text_xs: 14.0,
    text_xs_line: 20.0,
    text_2xs: 12.0,
    text_lg: 18.0,
    icon: 18.0,
    ..REGULAR
};

impl Metrics {
    pub fn of(density: Density, pointer: Pointer) -> Self {
        let mut metrics = match density {
            Density::Regular => REGULAR,
            Density::Large => LARGE,
        };
        if pointer == Pointer::Coarse {
            metrics.hit_target = 44.0;
            metrics.sidebar_row = 44.0;
        }
        metrics
    }
}

/// The metrics for the current window and pointer; reading it follows a resize.
pub fn metrics() -> Metrics {
    Metrics::of(Density::of(window().width.0), pointer())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sm_breakpoint_switches_scales_like_the_web_app() {
        assert_eq!(Density::of(639.9), Density::Large);
        assert_eq!(Density::of(640.0), Density::Regular);
        assert_eq!(Metrics::of(Density::Large, Pointer::Fine).control, 36.0);
        assert_eq!(Metrics::of(Density::Regular, Pointer::Fine).control, 32.0);
    }

    #[test]
    fn a_coarse_pointer_raises_hit_targets_and_list_rows_only() {
        let fine = Metrics::of(Density::Regular, Pointer::Fine);
        let coarse = Metrics::of(Density::Regular, Pointer::Coarse);
        assert_eq!(fine.hit_target, 0.0);
        assert_eq!(coarse.hit_target, 44.0);
        assert_eq!(coarse.sidebar_row, 44.0);
        assert_eq!(coarse.control, fine.control);
    }
}
