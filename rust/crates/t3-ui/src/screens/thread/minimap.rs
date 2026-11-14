//! Port of apps/web/src/components/chat/TimelineMinimap.tsx TimelineMinimap.

use super::viewport::{TimelineNavigation, TimelineViewport};
use crate::i18n::{Value, t, tr, ts};
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use nsh_core::keymap::{Key, Keystroke};
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};
use std::rc::Rc;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow,
    viewport::{
        TIMELINE_MINIMAP_MIN_ITEMS, TIMELINE_MINIMAP_TARGET_OFFSET,
        resolve_timeline_minimap_has_persistent_gutter, resolve_timeline_minimap_hit_strip_width,
        resolve_timeline_minimap_index_from_pointer,
        resolve_timeline_minimap_navigation_interactive, resolve_timeline_minimap_top_percent,
    },
};
use t3_client::thread_views::threads::minimap::{TimelineMinimapItems, TimelineMinimapPreview};

// Strips overlap below two points. A pixel-bounded rail preserves every navigation target
// without mounting one native node or running one binding per turn on every scroll tick.
const MAX_STRIPS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Geometry {
    height: f32,
    width: f32,
    persistent: bool,
    hit_width: f32,
}

#[derive(Clone, Copy)]
pub struct TimelineMinimap {
    model: Stored<TimelineMinimapItems>,
    count: Signal<usize>,
    revision: Signal<u64>,
    active: Signal<Option<usize>>,
    current: Signal<Option<usize>>,
    visible: Signal<Option<(usize, usize)>>,
    visible_turns: Signal<std::ops::Range<usize>>,
    at_end: Signal<bool>,
    geometry: Signal<Geometry>,
    content_width: Signal<Option<f32>>,
    hover: Signal<bool>,
    focused: Signal<bool>,
    pressed: Signal<bool>,
    pass_through: Signal<bool>,
    scroll: Stored<f32>,
    rail_height: Signal<f32>,
    viewport: TimelineViewport,
}

impl TimelineMinimap {
    pub fn new(viewport: TimelineViewport) -> Self {
        let this = Self {
            model: stored(TimelineMinimapItems::default()),
            count: signal(0),
            revision: signal(0),
            active: signal(None),
            current: signal(None),
            visible: signal(None),
            visible_turns: signal(0..0),
            at_end: signal(false),
            geometry: signal(Geometry::default()),
            content_width: signal(None),
            hover: signal(false),
            focused: signal(false),
            pressed: signal(false),
            pass_through: signal(false),
            scroll: stored(0.0),
            rail_height: signal(0.0),
            viewport,
        };
        // Watches aggregate native scroll geometry, never message text or every list item.
        watch_scroll(
            move || viewport.element.node(),
            move |metrics| {
                this.scroll.replace(metrics.offset.y.0);
                this.at_end.set(
                    metrics.content_size.height.0 - metrics.offset.y.0 - metrics.size.height.0
                        <= 1.0,
                );
                this.measure(metrics.size.width.0, metrics.size.height.0);
            },
        );
        this
    }

    /// Structural changes only; the client projection's changed row feeds update_row on a delta.
    pub fn replace_rows(self, rows: &[Rc<MessagesTimelineRow>]) {
        self.model.with(|model| model.replace(rows));
        self.count.set(self.model.with(|model| model.len()));
        self.revision.update(|revision| *revision += 1);
        self.active.set(None);
        self.update_current();
    }

    pub fn update_row(self, row_index: usize, row: &MessagesTimelineRow) {
        self.model.with(|model| model.update_row(row_index, row));
        self.revision.update(|revision| *revision += 1);
    }

    /// The measured chat column, excluding its side gutters; follows chat-width settings.
    pub fn set_content_width(self, width: f32) {
        self.content_width.set(Some(width));
        let geometry = self.geometry.get_untracked();
        self.measure(geometry.width, geometry.height);
    }

    pub fn attach(self, list: VirtualList) -> VirtualList {
        list.on_viewable_items_change(move |items: &ViewableItems| {
            self.visible.set(items.viewable);
            self.update_current();
        })
    }

    fn measure(self, width: f32, height: f32) {
        let content = self.content_width.get_untracked().unwrap_or(width);
        self.geometry.set(Geometry {
            width,
            height,
            persistent: resolve_timeline_minimap_has_persistent_gutter(
                width.into(),
                content.into(),
            ),
            hit_width: resolve_timeline_minimap_hit_strip_width(width.into(), content.into())
                as f32,
        });
    }

    fn update_current(self) {
        self.visible_turns
            .set(self.visible.get_untracked().map_or(0..0, |(start, end)| {
                self.model.with(|model| model.visible_turns(start, end))
            }));
        self.current.set(
            self.visible
                .get_untracked()
                .and_then(|(start, end)| self.model.with(|model| model.current_index(start, end))),
        );
    }

    fn interactive(self) -> bool {
        self.geometry
            .with(|g| resolve_timeline_minimap_navigation_interactive(g.hit_width.into()))
            && !self.pass_through.get()
    }

    pub fn select(self, index: usize) {
        if index >= self.count.get_untracked() {
            return;
        }
        if let Some(row) = self
            .model
            .with(|model| model.item(index).map(|item| item.row_index))
        {
            self.viewport
                .scroll_to_index(row, px(-(TIMELINE_MINIMAP_TARGET_OFFSET as f32)));
        }
    }

    fn move_active(self, delta: isize) {
        let count = self.count.get_untracked();
        if count == 0 {
            return;
        }
        let index = self
            .active
            .get_untracked()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(count - 1);
        self.active.set(Some(index));
    }

    fn pointer_index(self, event: &MouseEvent) -> Option<usize> {
        resolve_timeline_minimap_index_from_pointer(
            self.count.get_untracked(),
            0.0,
            self.rail_height.get_untracked().into(),
            event.position.y.0.into(),
        )
    }

    pub fn row_visits(self) -> usize {
        self.model.with(|model| model.row_visits)
    }
    pub fn preview_bytes(self) -> usize {
        self.model.with(|model| model.preview_bytes)
    }
}

pub fn timeline_minimap(map: TimelineMinimap) -> impl IntoElement {
    let preview = computed(move || {
        map.revision.get();
        map.active.get().and_then(|index| {
            map.model
                .with(|model| model.preview_with_thread_label(index, &t("a thread")))
        })
    });
    when(
        move || map.count.get() >= TIMELINE_MINIMAP_MIN_ITEMS,
        move || {
            view()
                .test_id("timeline-minimap")
                .pointer_events(PointerEvents::BoxNone)
                // The TSX pointer:fine media query follows the shared pointer capability.
                .app_class(move || {
                    Some(tw(&cn([
                        Some("group/minimap absolute inset-y-0 left-0 z-40 w-18"),
                        (!map.geometry.get().persistent && pointer() == Pointer::Coarse)
                            .then_some("hidden"),
                    ])))
                })
                .opacity(move || {
                    if map.geometry.get().persistent || map.hover.get() || map.focused.get() {
                        1.0_f32
                    } else {
                        0.0_f32
                    }
                })
                .on_mouse_enter(move |_: &MouseEvent| map.hover.set(true))
                .on_mouse_leave(move |_: &MouseEvent| {
                    map.hover.set(false);
                    map.active.set(None);
                })
                .on_wheel(move |event: &WheelEvent| {
                    let delta = event.delta.points(px(16.0)).y.0;
                    if event.modifiers.ctrl || delta == 0.0 {
                        return;
                    }
                    map.viewport
                        .navigate(TimelineNavigation::Wheel { up: delta < 0.0 });
                    map.viewport
                        .element
                        .scroll_to_offset(px((map.scroll.get() + delta).max(0.0)), false);
                })
                .child(
                    view()
                        .app_class(tw("relative h-full w-full select-none"))
                        .pointer_events(PointerEvents::BoxNone)
                        .child(
                            view()
                                .app_class(tw("absolute top-1/2 left-3 -translate-y-1/2"))
                                .w(move || px(map.geometry.get().hit_width))
                                .h(move || {
                                    px(((map.count.get().saturating_sub(1) * 8).max(1) as f32)
                                        .min((map.geometry.get().height - 236.0).max(64.0)))
                                })
                                .on_layout(move |bounds: &Bounds<Points>| {
                                    map.rail_height.set(bounds.size.height.0)
                                })
                                .pointer_events(move || {
                                    if map.geometry.get().hit_width > 0.0 && !map.pass_through.get()
                                    {
                                        PointerEvents::Auto
                                    } else {
                                        PointerEvents::None
                                    }
                                })
                                .on_mouse_move(move |event: &MouseEvent| {
                                    if event.pressed_button.is_none() {
                                        map.pressed.set(false);
                                        map.pass_through.set(false);
                                    } else if !map.pressed.get_untracked() {
                                        map.pass_through.set(true);
                                    }
                                })
                                .child(timeline_minimap_navigation_button(map, true))
                                .child(minimap_rail(map, preview))
                                .child(timeline_minimap_navigation_button(map, false)),
                        ),
                )
        },
    )
}

fn minimap_rail(
    map: TimelineMinimap,
    preview: Memo<Option<TimelineMinimapPreview>>,
) -> impl IntoElement {
    let user_text = computed(move || preview.get().and_then(|preview| preview.user_text));
    let rail = element_ref();
    rail.on_focus_in(move || {
        map.focused.set(true);
        if map.active.get_untracked().is_none() {
            map.active
                .set(Some(map.current.get_untracked().unwrap_or(0)));
        }
    });
    rail.on_focus_out(move || {
        map.focused.set(false);
        map.active.set(None);
    });
    // Base Button exports presses only; this transparent parent receives native key/mouse events.
    view().app_class(tw("absolute inset-y-0 left-0 w-full"))
        .capture_keys(KeyCapture::keys([Key::Up, Key::Down, Key::Home, Key::End, Key::Enter, Key::Char(' ')].map(|key| Keystroke { key, modifiers: Default::default() })))
        .on_mouse_up(move |_: &MouseEvent| map.pressed.set(false))
        .on_mouse_move(move |event: &MouseEvent| {
            if event.pressed_button.is_none() || map.pressed.get_untracked() { map.active.set(map.pointer_index(event)); }
        })
        .on_mouse_leave(move |_: &MouseEvent| { map.active.set(None); map.pass_through.set(false); })
        .on_touch(move |event: &TouchEvent| {
            if event.phase == TouchPhase::Start && let Some(point) = event.points.iter().find(|point| point.changed) {
                map.active.set(resolve_timeline_minimap_index_from_pointer(map.count.get_untracked(), 0.0, map.rail_height.get_untracked().into(), point.location.y.0.into()));
            }
        })
        .on_key_down(move |event: &KeyEvent| match event.key.as_str() {
            "ArrowDown" => map.move_active(1), "ArrowUp" => map.move_active(-1),
            "Home" => map.active.set(Some(0)), "End" => map.active.set(map.count.get().checked_sub(1)),
            "Enter" | " " => { if let Some(index) = map.active.get_untracked() { map.select(index); } }
            _ => {}
        })
        .child(nsh_base::button().element_ref(&rail).test_id("timeline-minimap-rail")
            .accessibility_label(move || Some(tr("Jump to message: {message}", &[("message", Value::Text(user_text.get().map_or_else(|| t("User message"), |text| text.to_string())))]).into()))
            // The before: hit extension is native hit slop rather than a pseudo-element.
            .hit_slop(EdgeInsets { top: px(4.0), bottom: px(4.0), left: px(0.0), right: px(0.0) })
            .app_class(tw("absolute inset-y-0 left-0 w-full cursor-pointer bg-transparent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"))
            .on_press_down(move || map.pressed.set(true))
            .on_press_up(move || map.pressed.set(false))
            .on_press(move || { if let Some(index) = map.active.get_untracked() { map.select(index); } rail.blur(); })
            .child(view().app_class(tw("absolute top-0 left-3 h-full w-px bg-border/15")))
            .child(each(move || (0..map.count.get().min(MAX_STRIPS)).collect::<Vec<_>>(), |index| *index, move |bin, _| minimap_strip(map, bin)))
            .child(when(move || preview.with(|preview| preview.is_some()), move || minimap_preview(map, preview))))
}

fn minimap_strip(map: TimelineMinimap, bin: ReadSignal<usize>) -> impl IntoElement {
    let covered = computed(move || {
        let count = map.count.get();
        let strips = count.min(MAX_STRIPS);
        (bin.get() * count).checked_div(strips).unwrap_or_default()
            ..((bin.get() + 1) * count)
                .checked_div(strips)
                .unwrap_or_default()
    });
    let index =
        computed(move || covered.with(|range| (range.start + range.end.saturating_sub(1)) / 2));
    let distance = computed(move || {
        map.active.get().map(|active| {
            covered.with(|range| {
                if active < range.start {
                    range.start - active
                } else {
                    active.saturating_sub(range.end.saturating_sub(1))
                }
            })
        })
    });
    let visible = computed(move || {
        let visible = map.visible_turns.get();
        covered.with(|range| visible.start < range.end && range.start < visible.end)
    });
    view().test_id("minimap-strip").accessible(false)
        // after: is the final overlay child; no perpetual transition or animation runs.
        .app_class(move || Some(tw(&cn([Some("pointer-events-none absolute left-0 h-0.5 w-6 origin-left -translate-y-1/2 rounded-full transition-transform duration-150"),
            Some(if distance.get() == Some(0) { "bg-muted-foreground/60" } else { "bg-muted-foreground/25" }),
            Some(match distance.get() { Some(0) => "scale-x-100", Some(1) => "scale-x-[0.667]", Some(2) => "scale-x-[0.417]", _ => "scale-x-[0.333]" })]))))
        .top(move || Percentage(resolve_timeline_minimap_top_percent(index.get() as f64, map.count.get()) as f32 / 100.0))
        .child(view().app_class(tw("absolute inset-0 rounded-full bg-foreground/55 transition-opacity duration-150"))
            .opacity(move || if visible.get() { 1.0_f32 } else { 0.0_f32 }))
}

fn minimap_preview(
    map: TimelineMinimap,
    preview: Memo<Option<TimelineMinimapPreview>>,
) -> impl IntoElement {
    let user_text = computed(move || preview.get().and_then(|preview| preview.user_text));
    let assistant_text = computed(move || preview.get().and_then(|preview| preview.assistant_text));
    view().app_class(tw("pointer-events-none absolute left-8 w-80 -translate-y-1/2"))
        .top(move || Percentage(resolve_timeline_minimap_top_percent(map.active.get().unwrap_or(0) as f64, map.count.get()) as f32 / 100.0))
        // index.css @utility dropdown-glass: token background, contrast border and backdrop filter.
        .child(view().app_class(tw("rounded-xl p-3 text-left text-popover-foreground shadow-xl shadow-black/25 border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08]"))
            .bg(crate::ui::popover::dropdown_glass_background)
            .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
            .child(text(move || user_text.get().map_or_else(|| ts("User message"), |s| SharedString::from(s.to_string())))
                .app_class(tw("max-w-full overflow-hidden text-ellipsis whitespace-nowrap text-sm font-medium leading-5")).number_of_lines(1u32))
            .child(when(move || assistant_text.with(|text| text.is_some()), move || text(move || assistant_text.get().map_or_else(SharedString::default, |text| text.to_string().into()))
                .app_class(tw("mt-1 max-h-[3.75rem] overflow-hidden text-muted-foreground text-sm leading-5")).number_of_lines(3u32))))
}

/// Port of TimelineMinimap.tsx TimelineMinimapNavigationButton.
fn timeline_minimap_navigation_button(map: TimelineMinimap, previous: bool) -> impl IntoElement {
    let label = if previous {
        t("Previous turn")
    } else {
        t("Next turn")
    };
    let hint = crate::ui::tooltip::tooltip_create_handle();
    let popup = crate::ui::tooltip::tooltip(
        hint,
        crate::ui::tooltip::TooltipOptions {
            side: if previous {
                Placement::Top
            } else {
                Placement::Bottom
            },
            ..Default::default()
        },
        text(SharedString::from(label.clone())),
    );
    let target = computed(move || {
        map.current.get().and_then(|index| {
            if previous {
                index.checked_sub(1)
            } else if map.at_end.get() {
                None
            } else {
                index
                    .checked_add(1)
                    .filter(|index| *index < map.count.get())
            }
        })
    });
    // NSH cannot parse calc(100%+6px); use the measured rail height for the same offset.
    view().app_class(tw("absolute left-1 z-10 inline-flex -translate-x-1/2 opacity-0 transition-opacity duration-150 hover:opacity-100 focus-within:opacity-100"))
        .test_id(if previous { "minimap-previous" } else { "minimap-next" })
        .tooltip(popup)
        .style_with(move |style| if previous { style.bottom(px(map.rail_height.get() + 6.0)) } else { style.top(px(map.rail_height.get() + 6.0)) })
        .pointer_events(move || if map.interactive() { PointerEvents::Auto } else { PointerEvents::None })
        // The shared primitive has no icon-micro size; icon-xs is its smallest supported size.
        .child(button(ButtonContent::Icon { icon: if previous { Icon::ChevronUp } else { Icon::ChevronDown }, label },
            ButtonVariant::GhostMuted, ButtonSize::IconXs, move || target.get().is_none() || !map.interactive(), move || { if map.interactive() && let Some(index) = target.get_untracked() { map.select(index); } }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: usize, role: &str, text: &str) -> Rc<MessagesTimelineRow> {
        Rc::new(serde_json::from_value(json!({
            "id":format!("m-{id}"),"kind":"message","createdAt":"2026-11-14T12:00:00Z",
            "showAssistantMeta":false,"showAssistantCopyButton":false,"assistantCopyStreaming":false,
            "message":{"id":format!("m-{id}"),"role":role,"text":text,"runId":null,"streaming":false,
                "createdAt":"2026-11-14T12:00:00Z","updatedAt":"2026-11-14T12:00:00Z"}
        })).expect("client row"))
    }

    #[test]
    fn bootstrap_prompts_and_thread_link_previews_match_upstream_cases() {
        for prefix in ["", "Ultrathink:\n"] {
            let mut model = TimelineMinimapItems::default();
            model.replace(&[row(0, "user", &format!("{prefix}{}", t3_client::thread_views::user_message::ATTACHMENT_ONLY_BOOTSTRAP_PROMPT)),
                row(1, "assistant", "Started <t3-thread://v2/env-1/thread-1> and [Fix CI](t3-thread://v2/env-1/t2).")]);
            let preview = model.preview(0).expect("turn preview");
            assert!(preview.user_text.is_none());
            assert_eq!(
                preview.assistant_text.as_deref(),
                Some("Started a thread and Fix CI.")
            );
        }
    }

    #[test]
    fn final_assistant_belongs_to_its_user_turn_and_old_deltas_cannot_replace_it() {
        let rows = [
            row(0, "user", "First"),
            row(1, "assistant", "Working"),
            row(2, "assistant", "Final"),
            row(3, "user", "Second"),
            row(4, "assistant", "Next"),
        ];
        let mut model = TimelineMinimapItems::default();
        model.replace(&rows);
        assert_eq!(model.len(), 2);
        assert_eq!(
            model.preview(0).expect("first").assistant_text.as_deref(),
            Some("Final")
        );
        model.update_row(1, &row(1, "assistant", "older changed"));
        assert_eq!(
            model.preview(0).expect("first").assistant_text.as_deref(),
            Some("Final")
        );
        model.update_row(2, &row(2, "assistant", "Final grew"));
        assert_eq!(
            model.preview(0).expect("first").assistant_text.as_deref(),
            Some("Final grew")
        );
        assert_eq!(
            model.preview(1).expect("second").assistant_text.as_deref(),
            Some("Next")
        );
        assert_eq!(model.current_index(1, 2), Some(0));
        assert_eq!(model.current_index(2, 3), Some(1));
        assert_eq!(model.current_index(4, 4), Some(1));
    }

    #[test]
    fn streaming_updates_and_preview_work_are_independent_of_thread_and_reply_length() {
        fn run(turns: usize, tail: usize) -> (usize, usize) {
            let rows: Vec<_> = (0..turns * 2)
                .map(|index| {
                    row(
                        index,
                        if index % 2 == 0 { "user" } else { "assistant" },
                        "Text",
                    )
                })
                .collect();
            let mut model = TimelineMinimapItems::default();
            model.replace(&rows);
            let visits = model.row_visits;
            for _ in 0..32 {
                model.update_row(
                    turns * 2 - 1,
                    &row(turns * 2 - 1, "assistant", &"x".repeat(tail)),
                );
                assert_eq!(
                    model
                        .preview(turns - 1)
                        .expect("preview")
                        .assistant_text
                        .expect("text")
                        .len(),
                    1500
                );
            }
            (model.row_visits - visits, model.preview_bytes)
        }
        assert_eq!(run(4, 2_000), run(2_000, 200_000));
    }

    #[test]
    fn gutter_pointer_and_current_index_match_web_geometry() {
        assert_eq!(
            resolve_timeline_minimap_index_from_pointer(5, 100.0, 400.0, -1.0),
            Some(0)
        );
        assert_eq!(
            resolve_timeline_minimap_index_from_pointer(5, 100.0, 400.0, 300.0),
            Some(2)
        );
        assert_eq!(
            resolve_timeline_minimap_index_from_pointer(5, 100.0, 400.0, 900.0),
            Some(4)
        );
        assert_eq!(
            resolve_timeline_minimap_index_from_pointer(0, 100.0, 400.0, 300.0),
            None
        );
        assert!(!resolve_timeline_minimap_has_persistent_gutter(
            800.0, 736.0
        ));
        assert!(resolve_timeline_minimap_has_persistent_gutter(832.0, 736.0));
        assert_eq!(resolve_timeline_minimap_hit_strip_width(832.0, 736.0), 36.0);
        assert_eq!(resolve_timeline_minimap_top_percent(2.0, 5), 50.0);
    }
}
