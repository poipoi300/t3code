//! Port of apps/web/src/components/ui/middle-truncate.tsx MiddleTruncate.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use nsh_core::element::text_measurer;
use nsh_core::prelude::*;

use crate::theme::{self, cn, tw};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    pub head: String,
    pub tail: String,
}

pub fn split_for_middle_truncate(value: &str, tail: Option<usize>) -> Option<Split> {
    let chars: Vec<_> = value.chars().collect();
    let mut keep = tail.unwrap_or(10);
    if tail.is_none()
        && let Some(slash) = chars.iter().rposition(|ch| *ch == '/')
        && slash > 0
        && slash < chars.len() - 1
    {
        let segment = chars.len() - slash - 1;
        keep = if segment <= 16 { segment } else { 10 };
    }
    if keep == 0 || chars.len().saturating_sub(keep) <= 4 {
        return None;
    }
    let mut cut = chars.len() - keep;
    let word_break = |ch: char| matches!(ch, '-' | '_' | '.' | '/');
    if tail.is_none() && !word_break(chars[cut - 1]) {
        let later = (cut + 1..=chars.len() - 4).find(|at| word_break(chars[at - 1]));
        let earlier = || {
            ((chars.len().saturating_sub(16).max(5))..cut)
                .rev()
                .find(|at| word_break(chars[at - 1]))
        };
        if let Some(snapped) = later.or_else(earlier) {
            cut = snapped;
        }
    }
    Some(Split {
        head: chars[..cut].iter().collect(),
        tail: chars[cut..].iter().collect(),
    })
}

fn collapse_segments(split: Split) -> Split {
    fn collapse(value: &str) -> String {
        let mut previous_space = false;
        value
            .chars()
            .filter_map(|ch| {
                if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000c}') {
                    let first = !previous_space;
                    previous_space = true;
                    first.then_some(' ')
                } else {
                    previous_space = false;
                    Some(ch)
                }
            })
            .collect()
    }
    let head = collapse(&split.head).trim_start_matches(' ').to_owned();
    let tail = collapse(&split.tail).trim_end_matches(' ').to_owned();
    let tail = if head.ends_with(' ') {
        tail.trim_start_matches(' ').to_owned()
    } else {
        tail
    };
    Split { head, tail }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Default,
    CapAligned,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub tail: Option<usize>,
    pub show_title: bool,
    pub variant: Variant,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tail: None,
            show_title: true,
            variant: Variant::Default,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub family: SharedString,
    pub size: f32,
    pub weight: FontWeight,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: theme::INTER.family.into(),
            size: theme::metrics().text,
            weight: FontWeight::NORMAL,
        }
    }
}

impl Font {
    fn shorthand(&self) -> String {
        format!("{} {}px {}", self.weight.0, self.size, self.family)
    }
}

type Measure = Rc<dyn Fn(&str, &str) -> Points>;

struct Prepared {
    raw: SharedString,
    head: SharedString,
    tail: Option<SharedString>,
    font: Font,
    shorthand: String,
    head_width: f32,
    tail_width: f32,
    ellipsis_width: f32,
    boundaries: Vec<usize>,
    widths: RefCell<BTreeMap<usize, f32>>,
    measure: Measure,
}

impl PartialEq for Prepared {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
            && self.head == other.head
            && self.tail == other.tail
            && self.font == other.font
    }
}

impl Prepared {
    fn new(raw: SharedString, tail: Option<usize>, font: Font) -> Self {
        Self::with_measurer(raw, tail, font, Rc::new(text_measurer()))
    }

    fn with_measurer(raw: SharedString, tail: Option<usize>, font: Font, measure: Measure) -> Self {
        let split = split_for_middle_truncate(&raw, tail).map(collapse_segments);
        let (head, tail) = match split {
            Some(split) => (
                SharedString::from(split.head),
                Some(SharedString::from(split.tail)),
            ),
            // The fallback uses the same ASCII collapsing as CSS whitespace-normal, not Unicode trim.
            None => (
                SharedString::from(
                    collapse_segments(Split {
                        head: raw.to_string(),
                        tail: String::new(),
                    })
                    .head
                    .trim_end_matches(' ')
                    .to_owned(),
                ),
                None,
            ),
        };
        let shorthand = font.shorthand();
        let head_width = measure(&shorthand, &head).0;
        let tail_width = tail
            .as_ref()
            .map_or(0.0, |tail| measure(&shorthand, tail).0);
        let ellipsis_width = measure(&shorthand, "…").0;
        let boundaries = std::iter::once(0)
            .chain(head.char_indices().map(|(at, ch)| at + ch.len_utf8()))
            .collect();
        Self {
            raw,
            head,
            tail,
            font,
            shorthand,
            head_width,
            tail_width,
            ellipsis_width,
            boundaries,
            widths: RefCell::new(BTreeMap::new()),
            measure,
        }
    }

    fn width_to(&self, count: usize) -> f32 {
        if count == 0 {
            return 0.0;
        }
        *self.widths.borrow_mut().entry(count).or_insert_with(|| {
            (self.measure)(&self.shorthand, &self.head[..self.boundaries[count]]).0
        })
    }

    fn fit(&self, width: f32) -> Fit {
        let width = width.max(0.0);
        // Layout frames are pixel-rounded, like the DOM's integer client/scroll dimensions.
        let overflowing =
            self.tail.is_some() && (self.head_width + self.tail_width).round() > width.round();
        let capacity = if self.tail.is_some() {
            (width
                - self.tail_width
                - if overflowing {
                    self.ellipsis_width
                } else {
                    0.0
                })
            .max(0.0)
        } else {
            width
        };
        let clip_width = if overflowing {
            let (mut fits, mut over) = (0, self.boundaries.len() - 1);
            while over - fits > 1 {
                let mid = (fits + over) / 2;
                if self.width_to(mid) <= capacity {
                    fits = mid;
                } else {
                    over = mid;
                }
            }
            self.width_to(fits)
        } else {
            capacity
        };
        Fit {
            overflowing,
            capacity,
            clip_width,
            slack: (capacity - clip_width).max(0.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Fit {
    overflowing: bool,
    capacity: f32,
    clip_width: f32,
    slack: f32,
}

pub fn middle_truncate(
    value: impl Fn() -> SharedString + 'static,
    options: Options,
) -> impl IntoElement {
    middle_truncate_with_font(value, options, Font::default)
}

/// NSH exposes no inherited computed font; bind the feature's font here for both layout and measuring.
pub fn middle_truncate_with_font(
    value: impl Fn() -> SharedString + 'static,
    options: Options,
    font: impl Fn() -> Font + 'static,
) -> impl IntoElement {
    let value = computed(value);
    let font = computed(font);
    let prepared = computed(move || Rc::new(Prepared::new(value.get(), options.tail, font.get())));
    let width = signal(0.0_f32);
    let fitted = computed(move || prepared.with(|prepared| prepared.fit(width.get())));
    let split = computed(move || prepared.with(|prepared| prepared.tail.is_some()));
    let root = view()
        .app_class(tw(
            "inline-flex min-w-0 max-w-full overflow-hidden whitespace-nowrap",
        ))
        // Stable intrinsic width avoids feeding the fitted text's shorter width back into layout.
        .w(move || px(prepared.with(|prepared| prepared.head_width + prepared.tail_width)))
        .font_family(move || prepared.with(|prepared| prepared.font.family.clone()))
        .style_with(move |style| style.font_weight(prepared.with(|prepared| prepared.font.weight)))
        .text_size(move || px(prepared.with(|prepared| prepared.font.size)))
        .accessible(true)
        .label(move || prepared.with(|prepared| prepared.raw.clone()))
        .on_layout(move |bounds: &Bounds<Points>| {
            if width.get_untracked() != bounds.size.width.0 {
                width.set(bounds.size.width.0);
            }
        })
        .child(dynamic(move || {
            let split = split.get();
            untracked(|| {
                if !split {
                    return text(move || prepared.with(|prepared| prepared.head.clone()))
                        .app_class(tw("min-w-0 truncate"))
                        .number_of_lines(1_u32)
                        .into_element();
                }
                // NSH has no clip-path inset; a clipped inner viewport retains the full head text.
                // text-box trim and independent x/y clip are unavailable; retain cap-aligned descender padding.
                let padding = cn([
                    Some("shrink-0"),
                    (options.variant == Variant::CapAligned).then_some("-mb-1 pb-1"),
                ]);
                view()
                    .app_class(tw("flex min-w-0"))
                    .child(
                        view()
                            .app_class(tw("min-w-0 overflow-hidden"))
                            .w(move || px(fitted.get().capacity))
                            .child(
                                view()
                                    .app_class(tw("overflow-hidden"))
                                    .w(move || px(fitted.get().clip_width))
                                    .child(
                                        text(move || {
                                            prepared.with(|prepared| prepared.head.clone())
                                        })
                                        .app_class(tw(&padding))
                                        .w(
                                            move || {
                                                px(prepared.with(|prepared| prepared.head_width))
                                            },
                                        )
                                        .number_of_lines(1_u32)
                                        .text_overflow(nsh_core::EllipsizeMode::Clip),
                                    ),
                            ),
                    )
                    .child(
                        text(move || {
                            prepared.with(|prepared| {
                                let tail = prepared.tail.as_deref().unwrap_or_default();
                                SharedString::from(if fitted.get().overflowing {
                                    format!("…{tail}")
                                } else {
                                    tail.to_owned()
                                })
                            })
                        })
                        .app_class(tw(&padding))
                        .translate_x(move || px(-fitted.get().slack))
                        .number_of_lines(1_u32),
                    )
                    .into_element()
            })
        }));
    if options.show_title {
        root.tooltip(text(move || prepared.with(|prepared| prepared.raw.clone())))
    } else {
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn keeps_a_short_last_path_segment_whole() {
        assert_eq!(
            split_for_middle_truncate("feature/ui-polish", None),
            Some(Split {
                head: "feature/".into(),
                tail: "ui-polish".into()
            })
        );
    }

    #[test]
    fn starts_a_long_segments_tail_on_a_word() {
        for (value, tail) in [
            ("t3/clone-repo-to-other-machine", "machine"),
            ("t3/investigate-release-build-times", "times"),
        ] {
            assert_eq!(
                split_for_middle_truncate(value, None).expect("split").tail,
                tail
            );
        }
    }

    #[test]
    fn falls_back_to_an_earlier_word_when_the_last_is_too_short() {
        assert_eq!(
            split_for_middle_truncate("t3/compare-long-featurenames-ab", None)
                .expect("split")
                .tail,
            "featurenames-ab"
        );
    }

    #[test]
    fn keeps_the_fixed_tail_when_no_word_break_is_near() {
        assert_eq!(
            split_for_middle_truncate("t3/abcdefghijklmnopqrstuvwxyz", None)
                .expect("split")
                .tail,
            "qrstuvwxyz"
        );
    }

    #[test]
    fn respects_an_explicit_tail_length() {
        assert_eq!(
            split_for_middle_truncate("t3/clone-repo-to-other-machine", Some(6))
                .expect("split")
                .tail,
            "achine"
        );
    }

    #[test]
    fn thresholds_and_unicode_code_points_are_preserved() {
        for (value, tail) in [
            ("", None),
            ("main", None),
            ("abcdefghijklmno", Some(11)),
            ("a/short", None),
            ("long-enough", Some(0)),
            ("value", Some(usize::MAX)),
        ] {
            assert!(
                split_for_middle_truncate(value, tail).is_none(),
                "{value:?}/{tail:?}"
            );
        }
        let split = split_for_middle_truncate("abcdef😀😀😀😀😀", Some(4)).expect("unicode split");
        assert_eq!(split.head, "abcdef😀");
        assert_eq!(split.tail, "😀😀😀😀");
        assert_eq!(format!("{}{}", split.head, split.tail), "abcdef😀😀😀😀😀");
    }

    #[test]
    fn collapses_only_css_ascii_spaces_after_splitting_the_raw_tail() {
        let split = split_for_middle_truncate("longprefix a  b.txt", None)
            .map(collapse_segments)
            .expect("split");
        assert_eq!(split.tail, "x a b.txt");
        let split = collapse_segments(Split {
            head: "  head \t".into(),
            tail: " \r\nend  ".into(),
        });
        assert_eq!(
            split,
            Split {
                head: "head ".into(),
                tail: "end".into()
            }
        );
        let split = collapse_segments(Split {
            head: "report\u{2003}".into(),
            tail: "\u{2003}\u{a0}draft.txt".into(),
        });
        assert_eq!(split.head, "report\u{2003}");
        assert_eq!(split.tail, "\u{2003}\u{a0}draft.txt");
    }

    #[test]
    fn fitting_caches_measurements_and_clips_after_the_last_whole_code_point() {
        let measured = Rc::new(Cell::new(0));
        let calls = measured.clone();
        let prepared = Prepared::with_measurer(
            "abcdef😀gh/long-tail".into(),
            Some(9),
            Font {
                family: "sans-serif".into(),
                size: 10.0,
                weight: FontWeight::NORMAL,
            },
            Rc::new(move |_, text| {
                calls.set(calls.get() + 1);
                px(text
                    .chars()
                    .map(|ch| if ch == '😀' { 12.0 } else { 10.0 })
                    .sum())
            }),
        );
        let fitted = prepared.fit(167.0);
        assert!(fitted.overflowing);
        assert_eq!(fitted.capacity, 67.0);
        assert_eq!(fitted.clip_width, 60.0, "emoji cannot be partially visible");
        assert_eq!(fitted.slack, 7.0);
        let count = measured.get();
        for _ in 0..20 {
            assert_eq!(prepared.fit(167.0).clip_width, 60.0);
        }
        assert_eq!(
            measured.get(),
            count,
            "unchanged inputs perform no new measurements"
        );
        assert!(
            !prepared
                .fit(prepared.head_width + prepared.tail_width)
                .overflowing
        );
        let count = measured.get();
        assert!(!prepared.fit(1000.0).overflowing);
        assert_eq!(
            measured.get(),
            count,
            "wide layout needs no prefix measurements"
        );
        let fractional = Prepared::with_measurer(
            "abcdefghi/tail".into(),
            Some(4),
            prepared.font.clone(),
            Rc::new(|_, text| px(text.chars().count() as f32 * 10.16)),
        );
        assert!(
            !fractional
                .fit((fractional.head_width + fractional.tail_width).round())
                .overflowing,
            "an unconstrained natural width must not overflow after pixel rounding"
        );
    }
}
