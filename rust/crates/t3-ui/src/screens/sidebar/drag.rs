//! The sidebar's pointer gesture over virtualized thread and shelf rows.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Duration};

use nsh_core::{TouchEvent, TouchPhase, element::desktop::current_window, prelude::*};

use crate::{
    runtime::tasks::TaskOwner,
    theme::{palette, tw},
    ui::icon::{Icon, icon},
};

struct DropNode {
    owner: u64,
    element: ElementRef,
    min_height: f32,
}

type Nodes = Rc<RefCell<BTreeMap<String, DropNode>>>;
type CanDrop = Rc<dyn Fn(&str, &str) -> bool>;
type DropHandler = Rc<dyn Fn(String, String)>;
type Targets = Vec<(String, Bounds<Points>)>;

#[derive(Clone)]
struct Gesture {
    source: String,
    point: Point<Points>,
    dragging: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    nodes: Stored<Nodes>,
    sequence: Stored<u64>,
    generation: Stored<u64>,
    candidate: Stored<Option<String>>,
    gesture: Stored<Option<Gesture>>,
    viewport: ElementRef,
    held: Signal<bool>,
    active: Signal<bool>,
    over: Signal<Option<String>>,
    can_drop: Stored<CanDrop>,
    drop: Stored<DropHandler>,
    tasks: TaskOwner,
}

impl Drag {
    pub fn new(
        viewport: ElementRef,
        tasks: TaskOwner,
        can_drop: impl Fn(&str, &str) -> bool + 'static,
        drop: impl Fn(String, String) + 'static,
    ) -> Self {
        Self {
            nodes: stored(Rc::default()),
            sequence: stored(0),
            generation: stored(0),
            candidate: stored(None),
            gesture: stored(None),
            viewport,
            held: signal(false),
            active: signal(false),
            over: signal(None),
            can_drop: stored(Rc::new(can_drop) as CanDrop),
            drop: stored(Rc::new(drop) as DropHandler),
            tasks,
        }
    }

    pub fn attach(self, key: Memo<Option<String>>, node: ElementRef, min_height: Memo<u32>) {
        let nodes = self.nodes.get();
        let sequence = self.sequence.with(|next| {
            *next += 1;
            *next
        });
        effect(move || {
            if let Some(key) = key.get() {
                nodes.borrow_mut().insert(
                    key.clone(),
                    DropNode {
                        owner: sequence,
                        element: node,
                        min_height: min_height.get() as f32,
                    },
                );
                let nodes = nodes.clone();
                cleanup(move || {
                    let mut nodes = nodes.borrow_mut();
                    if nodes.get(&key).is_some_and(|node| node.owner == sequence) {
                        nodes.remove(&key);
                    }
                });
            }
        });
    }

    pub fn active(self) -> bool {
        self.active.get()
    }

    pub fn over(self, key: &str) -> bool {
        self.over.with(|over| over.as_deref() == Some(key))
    }

    pub fn clear(self) {
        self.generation.with(|generation| *generation += 1);
        self.candidate.replace(None);
        self.finish();
    }

    fn finish(self) -> Option<Gesture> {
        self.held.set(false);
        self.active.set(false);
        self.over.set(None);
        self.gesture.replace(None)
    }

    async fn measure(self) -> Option<(Bounds<Points>, Targets)> {
        let nodes = self.nodes.get();
        let pending = nodes
            .borrow()
            .iter()
            .map(|(key, node)| (key.clone(), node.min_height, node.element.measure()))
            .collect::<Vec<_>>();
        let viewport = self.viewport.measure().await?;
        let viewport = Bounds {
            origin: viewport.page,
            size: viewport.frame.size,
        };
        let mut targets = vec![];
        for (key, min_height, response) in pending {
            if let Some(node) = response.await {
                let bounds = Bounds {
                    origin: node.page,
                    size: size(
                        node.frame.size.width,
                        px(node.frame.size.height.0.max(min_height)),
                    ),
                };
                if let Some(bounds) = intersect(bounds, viewport) {
                    targets.push((key, bounds));
                }
            }
        }
        Some((viewport, targets))
    }

    async fn track(self, generation: u64) {
        let can_drop = self.can_drop.get();
        let mut hover = None;
        while self.generation.get() == generation && self.active.get_untracked() {
            if let Some((viewport, targets)) = self.measure().await {
                if self.generation.get() != generation || !self.active.get_untracked() {
                    return;
                }
                let Some(gesture) = self.gesture.get() else {
                    return;
                };
                let target = hit(&targets, &gesture.source, gesture.point, can_drop.as_ref());
                if target != hover {
                    hover = target;
                    self.over.set(hover.clone());
                }
                let speed = edge_scroll(viewport, gesture.point);
                if speed != 0.0 {
                    let pos = self.viewport.measure_scroll_position().await;
                    if let Some(pos) = pos.filter(|_| {
                        self.generation.get() == generation && self.active.get_untracked()
                    }) {
                        self.viewport.scroll_to_index(
                            pos.index,
                            percentage(0.0),
                            -pos.offset + px(speed),
                            false,
                        );
                    }
                }
            }
            timer(Duration::from_millis(40)).await;
        }
    }

    pub fn cancel_on_interrupt(self) {
        let previous = stored((window(), lifecycle()));
        effect(move || {
            let next = (window(), lifecycle());
            if previous.replace(next) != next {
                self.clear();
            }
        });
        current_window().on_focus_change(move |focused| {
            if !focused {
                self.clear();
            }
        });
        effect(move || {
            if self.held.get() {
                on_key_with(
                    "escape",
                    KeyOptions {
                        when: nsh_core::keymap::When::Always,
                        ..Default::default()
                    },
                    move |_| self.clear(),
                );
            }
        });
    }

    // The grip identifies the row in its touch event. The list claims the same
    // event afterwards, so it keeps the gesture when scrolling recycles the row.
    pub fn listen(self, list: VirtualList) -> VirtualList {
        list.claim_touches(
            TouchClaim::on_start()
                .capture()
                .ask(move |request: &ClaimRequest| {
                    if request.event.gesture.active_touches == 1
                        && self.candidate.with(|candidate| candidate.is_some())
                    {
                        request.claim();
                    }
                }),
        )
        .keep_touches(true)
        .on_responder_grant(move |event: &ResponderEvent| {
            let source = self.candidate.replace(None);
            self.clear();
            if let Some(source) = source {
                self.held.set(true);
                self.gesture.replace(Some(Gesture {
                    source,
                    point: event.page,
                    dragging: false,
                }));
            }
        })
        .on_responder_start(move |event: &ResponderEvent| {
            if event.gesture.active_touches != 1 {
                self.clear();
            }
        })
        .on_responder_move(move |event: &ResponderEvent| {
            let start = self.gesture.with(|gesture| {
                let Some(gesture) = gesture else {
                    return false;
                };
                gesture.point = event.page;
                if gesture.dragging || !drag_started(event.gesture.delta) {
                    return false;
                }
                gesture.dragging = true;
                true
            });
            if start {
                self.active.set(true);
                let generation = self.generation.get();
                self.tasks.spawn(self.track(generation));
            }
        })
        .on_responder_release(move |event: &ResponderEvent| {
            let Some(gesture) = self.finish().filter(|gesture| gesture.dragging) else {
                return;
            };
            let point = event.page;
            let generation = self.generation.get();
            self.tasks.spawn(async move {
                let Some((_, targets)) = self.measure().await else {
                    return;
                };
                if self.generation.get() == generation
                    && let Some(over) = hit(
                        &targets,
                        &gesture.source,
                        point,
                        self.can_drop.get().as_ref(),
                    )
                {
                    self.drop.get()(gesture.source, over);
                }
            });
        })
        .on_responder_terminate(move |_: &ResponderEvent| self.clear())
    }

    pub fn grip(self, key: Memo<Option<String>>) -> impl IntoElement {
        let id = format!(
            "sidebar-drag-grip-{}",
            key.get_untracked().unwrap_or_default()
        );
        // Claiming the whole row would steal touch scrolling and long-press
        // menus from the virtual list. The handle is pointer-only; row menus
        // provide the equivalent pin, settle, and wake actions to other inputs.
        view()
            .test_id(&id)
            .accessible(false)
            .hit_slop(EdgeInsets::all(px(6.0)))
            .app_class(tw("flex w-6 shrink-0 cursor-grab items-center justify-center self-stretch text-sidebar-muted-foreground/60 hover:text-sidebar-foreground"))
            .on_touch(move |event: &TouchEvent| {
                if event.phase == TouchPhase::Start && event.points.len() == 1 {
                    self.candidate.replace(key.get_untracked());
                }
            })
            .child(icon(Icon::GripVertical, 13.0, move || palette().muted_foreground))
    }
}

fn drag_started(delta: Point<Points>) -> bool {
    delta.x.0 * delta.x.0 + delta.y.0 * delta.y.0 >= 36.0
}

fn intersect(item: Bounds<Points>, viewport: Bounds<Points>) -> Option<Bounds<Points>> {
    let x = item.origin.x.0.max(viewport.origin.x.0);
    let y = item.origin.y.0.max(viewport.origin.y.0);
    let right =
        (item.origin.x.0 + item.size.width.0).min(viewport.origin.x.0 + viewport.size.width.0);
    let bottom =
        (item.origin.y.0 + item.size.height.0).min(viewport.origin.y.0 + viewport.size.height.0);
    (right > x && bottom > y).then(|| Bounds {
        origin: point(px(x), px(y)),
        size: size(px(right - x), px(bottom - y)),
    })
}

fn contains(bounds: Bounds<Points>, point: Point<Points>) -> bool {
    point.x >= bounds.origin.x
        && point.x < bounds.origin.x + bounds.size.width
        && point.y >= bounds.origin.y
        && point.y < bounds.origin.y + bounds.size.height
}

fn edge_scroll(bounds: Bounds<Points>, point: Point<Points>) -> f32 {
    if !contains(bounds, point) {
        return 0.0;
    }
    let from_top = point.y.0 - bounds.origin.y.0;
    let from_bottom = bounds.size.height.0 - from_top;
    if from_top < 28.0 {
        16.0
    } else if from_bottom < 28.0 {
        -16.0
    } else {
        0.0
    }
}

fn hit(
    targets: &Targets,
    source: &str,
    point: Point<Points>,
    can_drop: &dyn Fn(&str, &str) -> bool,
) -> Option<String> {
    targets
        .iter()
        .filter(|(key, bounds)| key != source && contains(*bounds, point) && can_drop(source, key))
        .min_by(|(_, a), (_, b)| a.size.height.0.total_cmp(&b.size.height.0))
        .map(|(key, _)| key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_requires_six_points_of_euclidean_motion() {
        assert!(!drag_started(point(px(4.0), px(4.0))));
        assert!(drag_started(point(px(-4.3), px(4.3))));
        assert!(drag_started(point(px(6.0), px(0.0))));
    }
}
