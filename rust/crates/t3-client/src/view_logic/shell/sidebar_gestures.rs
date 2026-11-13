use super::{ListItem, Marker, Section, resolve_drop_target};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn bottom(self) -> f64 {
        self.top + self.height
    }
    pub fn right(self) -> f64 {
        self.left + self.width
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
}
pub const STATIONARY: Transform = Transform {
    x: 0.0,
    y: 0.0,
    scale_x: 1.0,
    scale_y: 1.0,
};
pub fn restrict_below_sidebar_label(
    mut transform: Transform,
    container: Option<Rect>,
    dragged: Option<Rect>,
    offset: f64,
) -> Transform {
    if let (Some(container), Some(dragged)) = (container, dragged) {
        transform.y = transform.y.max(container.top + offset - dragged.top);
    }
    transform
}
#[derive(Debug, Clone, Default)]
pub struct CollisionState {
    pub previous_y: Option<f64>,
    pub boundary_section: Option<Section>,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct CollisionGeometry {
    pub pointer: Option<Point>,
    pub boundary: Option<Rect>,
    pub active_bottom: Option<f64>,
}
impl CollisionState {
    pub fn resolve(
        &mut self,
        mut collisions: Vec<String>,
        active: &str,
        items: &[ListItem],
        geometry: CollisionGeometry,
        valid: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let CollisionGeometry {
            pointer,
            boundary,
            active_bottom,
        } = geometry;
        if let (Some(pointer), Some(boundary), Some(ListItem::Thread { section, .. })) =
            (pointer, boundary, items.iter().find(|i| i.id() == active))
        {
            let section = self
                .boundary_section
                .get_or_insert(if *section == Section::Pinned {
                    Section::Pinned
                } else {
                    Section::Active
                });
            let previous = self.previous_y.unwrap_or(pointer.y);
            self.previous_y = Some(pointer.y);
            if pointer.x >= boundary.left && pointer.x <= boundary.right() {
                if pointer.y < previous && pointer.y <= boundary.bottom() {
                    *section = Section::Pinned;
                } else if pointer.y > previous && pointer.y >= boundary.top {
                    *section = Section::Active;
                }
                if (*section == Section::Pinned || active_bottom.is_some_and(|b| pointer.y < b))
                    && let Some(i) = collisions.iter().position(|id| {
                        resolve_drop_target(items, active, id)
                            .is_some_and(|t| t.section == *section)
                    })
                {
                    let target = collisions.remove(i);
                    collisions.insert(0, target);
                }
            }
        }
        if collisions
            .first()
            .is_some_and(|id| id != active && !valid(id))
        {
            collisions.retain(|id| id == active);
        }
        collisions
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerPhase {
    Pending,
    Dragging,
    Finished,
}
#[derive(Debug, Clone, PartialEq)]
pub enum PointerEffect {
    Pending { origin: Point, offset: Point },
    Start(Point),
    Move(Point),
    Drop(Point),
    Finish { started: bool },
    Abort,
    Cancel,
    End,
}
#[derive(Debug, Clone)]
pub struct PointerSensor {
    pub phase: PointerPhase,
    pub pointer_id: i64,
    pub origin: Point,
    pub distance: f64,
    pub suppress_click: bool,
}
impl PointerSensor {
    pub fn activate(
        primary: bool,
        button: i32,
        pointer_id: i64,
        origin: Point,
        distance: f64,
    ) -> Option<Self> {
        (primary && button == 0).then_some(Self {
            phase: PointerPhase::Pending,
            pointer_id,
            origin,
            distance,
            suppress_click: false,
        })
    }
    pub fn move_pointer(
        &mut self,
        id: i64,
        point: Point,
        buttons: u32,
        claimed: bool,
    ) -> Vec<PointerEffect> {
        if id != self.pointer_id || self.phase == PointerPhase::Finished {
            return vec![];
        }
        if buttons & 1 == 0 {
            if self.phase == PointerPhase::Pending {
                return self.finish(false);
            }
            let mut effects = vec![PointerEffect::Drop(point)];
            effects.extend(self.finish(true));
            return effects;
        }
        let mut effects = vec![];
        if self.phase == PointerPhase::Pending {
            let offset = Point {
                x: point.x - self.origin.x,
                y: point.y - self.origin.y,
            };
            if offset.x.hypot(offset.y) <= self.distance {
                return vec![PointerEffect::Pending {
                    origin: self.origin,
                    offset,
                }];
            }
            self.phase = PointerPhase::Dragging;
            self.suppress_click = true;
            effects.push(PointerEffect::Start(self.origin));
        }
        if !claimed {
            effects.push(PointerEffect::Move(point));
        }
        effects
    }
    pub fn end(&mut self, id: i64, point: Point, dropped: bool) -> Vec<PointerEffect> {
        if id != self.pointer_id || self.phase == PointerPhase::Finished {
            return vec![];
        }
        let mut effects = if self.phase == PointerPhase::Dragging {
            vec![PointerEffect::Drop(point)]
        } else {
            vec![]
        };
        effects.extend(self.finish(dropped));
        effects
    }
    pub fn cancel(&mut self) -> Vec<PointerEffect> {
        self.finish(true)
    }
    fn finish(&mut self, cancelled: bool) -> Vec<PointerEffect> {
        if self.phase == PointerPhase::Finished {
            return vec![];
        }
        let aborted = self.phase == PointerPhase::Pending;
        self.phase = PointerPhase::Finished;
        self.suppress_click |= !aborted || cancelled;
        let mut effects = vec![PointerEffect::Finish { started: !aborted }];
        if aborted {
            effects.push(PointerEffect::Abort);
        }
        effects.push(if cancelled {
            PointerEffect::Cancel
        } else {
            PointerEffect::End
        });
        effects
    }
    pub fn clear_click_suppression(&mut self) {
        self.suppress_click = false;
    }
}
#[derive(Debug, Clone, Default)]
pub struct ShiftGuard {
    tops: BTreeMap<String, f64>,
    moved_at: BTreeMap<String, i64>,
    last_input: Option<i64>,
    held_drops: BTreeSet<i64>,
    press_dropped: bool,
    released_drop: Option<i64>,
}
impl ShiftGuard {
    pub fn new(rows: &[(String, f64, f64)]) -> Self {
        Self {
            tops: rows.iter().map(|(id, top, _)| (id.clone(), *top)).collect(),
            ..Default::default()
        }
    }
    pub fn update(&mut self, rows: &[(String, f64, f64)], now: i64) {
        let own_move = self.tops.is_empty()
            || self
                .last_input
                .is_some_and(|at| i128::from(now) - i128::from(at) < 500);
        for (id, top, height) in rows {
            let previous = self.tops.get(id);
            if previous == Some(top) {
                continue;
            }
            if previous.is_some_and(|old| (top - old).abs() * 2.0 < *height) {
                continue;
            }
            if own_move {
                self.moved_at.remove(id);
            } else {
                self.moved_at.insert(id.clone(), now);
            }
        }
        self.tops = rows.iter().map(|(id, top, _)| (id.clone(), *top)).collect();
    }
    pub fn pointer_down(&mut self, id: i64, row: Option<&str>, now: i64) -> bool {
        self.held_drops.remove(&id);
        if self.released_drop == Some(id) {
            self.released_drop = None;
        }
        if row
            .and_then(|r| self.moved_at.get(r))
            .is_some_and(|at| i128::from(now) - i128::from(*at) < 500)
        {
            self.held_drops.insert(id);
            self.press_dropped = true;
            return true;
        }
        self.press_dropped = false;
        self.last_input = Some(now);
        false
    }
    pub fn pointer_up(&mut self, id: i64, in_row: bool, now: i64) -> bool {
        let dropped = self.held_drops.remove(&id);
        self.released_drop = dropped.then_some(id);
        self.press_dropped = dropped;
        if !dropped {
            self.last_input = Some(now);
        }
        dropped && in_row
    }
    pub fn pointer_cancel(&mut self, id: i64) {
        self.held_drops.remove(&id);
    }
    pub fn key_down(
        &mut self,
        editable: bool,
        key: &str,
        ctrl: bool,
        meta: bool,
        alt: bool,
        now: i64,
    ) {
        self.press_dropped = false;
        if !(editable && key != "Enter" && !ctrl && !meta && !alt) {
            self.last_input = Some(now);
        }
    }
    pub fn follow_up(
        &self,
        is_click: bool,
        detail: i32,
        pointer: Option<i64>,
        in_row: bool,
    ) -> bool {
        if is_click && detail == 0 {
            return false;
        }
        in_row
            && pointer
                .filter(|id| *id >= 0)
                .map_or(self.press_dropped, |id| {
                    self.held_drops.contains(&id) || self.released_drop == Some(id)
                })
    }
}
pub struct SortingInput<'a> {
    pub items: &'a [ListItem],
    pub rects: &'a [Option<Rect>],
    pub active_index: usize,
    pub over_index: usize,
    pub enabled: bool,
    pub settled_order: &'a [String],
    pub active_order: Option<&'a [String]>,
    pub settled_expanded: bool,
    pub settled_visible_count: Option<usize>,
    pub route_key: Option<&'a str>,
    pub snoozed_count: usize,
    pub card_height: Option<f64>,
    pub slim_height: Option<f64>,
    pub label_height: f64,
}
pub fn sorting_transforms(input: SortingInput<'_>) -> Vec<Transform> {
    let items = input.items;
    let stationary = || vec![STATIONARY; items.len()];
    if !input.enabled {
        return stationary();
    }
    let Some(ListItem::Thread {
        key: active,
        section: source,
    }) = items.get(input.active_index)
    else {
        return stationary();
    };
    let Some(over) = items
        .get(input.over_index)
        .or_else(|| items.get(input.active_index))
    else {
        return stationary();
    };
    let Some(first) = input.rects.first().copied().flatten() else {
        return stationary();
    };
    let Some(target) = resolve_drop_target(items, active, &over.id()) else {
        return stationary();
    };
    let mut groups: [Vec<ListItem>; 5] = Default::default();
    let index = |s: Section| match s {
        Section::Pinned => 0,
        Section::Active => 1,
        Section::Working => 2,
        Section::Snoozed => 3,
        Section::Settled => 4,
    };
    let mut card = input.card_height;
    let mut slim = input.slim_height;
    let mut header_scale = None;
    for (i, item) in items.iter().enumerate() {
        match item {
            ListItem::Marker(m) => {
                if m.shelf()
                    && let Some(h) = input
                        .rects
                        .get(i)
                        .copied()
                        .flatten()
                        .map(|r| r.height)
                        .filter(|h| *h != 0.0)
                {
                    header_scale.get_or_insert(h / 32.0);
                }
            }
            ListItem::Thread { key, section } => {
                let height = input.rects.get(i).copied().flatten().map(|r| r.height);
                if matches!(
                    section,
                    Section::Pinned | Section::Active | Section::Working
                ) {
                    card = card.or(height);
                } else {
                    slim = slim.or(height);
                }
                if key != active {
                    groups[index(*section)].push(item.clone());
                }
            }
        }
    }
    let scale = slim
        .map(|s| s / 36.0)
        .unwrap_or_else(|| header_scale.unwrap_or(card.unwrap_or(82.0) / 82.0));
    let card = card.unwrap_or(82.0 * scale);
    let slim = slim.unwrap_or(36.0 * scale);
    let order = match target.section {
        Section::Pinned => target.pinned_order.as_slice(),
        Section::Settled => input.settled_order,
        _ => input.active_order.unwrap_or(&target.active_order),
    };
    let rank = |key: &str| order.iter().position(|id| id == key).unwrap_or(usize::MAX);
    let group = &mut groups[index(target.section)];
    let at = group
        .iter()
        .position(|i| rank(&i.id()) > rank(active))
        .unwrap_or(group.len());
    group.insert(
        at,
        ListItem::Thread {
            key: active.clone(),
            section: target.section,
        },
    );
    let settled = if input.settled_order.is_empty() {
        groups[4].iter().map(ListItem::id).collect::<Vec<_>>()
    } else {
        input.settled_order.to_vec()
    }
    .into_iter()
    .filter(|k| k != active || target.section == Section::Settled)
    .collect::<Vec<_>>();
    let mut visible = if input.settled_expanded {
        settled
            .iter()
            .take(input.settled_visible_count.unwrap_or(settled.len()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    if let Some(key) = input
        .route_key
        .filter(|key| settled.iter().any(|k| k == key) && !visible.iter().any(|k| k == key))
    {
        visible.push(key.into());
    }
    groups[4] = visible
        .into_iter()
        .map(|key| ListItem::Thread {
            key,
            section: Section::Settled,
        })
        .collect();
    let has_marker = |m| items.contains(&ListItem::Marker(m));
    let mut projected = vec![ListItem::Marker(Marker::PinnedHeader)];
    projected.extend(groups[0].clone());
    projected.push(ListItem::Marker(Marker::PinnedDivider));
    if groups[1].is_empty() {
        projected.push(ListItem::Marker(Marker::ActivePlaceholder));
    } else {
        projected.extend(groups[1].clone());
    }
    if has_marker(Marker::WorkingHeader) {
        projected.push(ListItem::Marker(Marker::WorkingHeader));
        projected.extend(groups[2].clone());
    }
    if !groups[3].is_empty()
        || ((*source != Section::Snoozed || input.snoozed_count > 1)
            && has_marker(Marker::SnoozedHeader))
    {
        projected.push(ListItem::Marker(Marker::SnoozedHeader));
        projected.extend(groups[3].clone());
    }
    projected.push(ListItem::Marker(Marker::SettledHeader));
    if groups[4].is_empty() {
        projected.push(ListItem::Marker(Marker::SettledPlaceholder));
    } else {
        projected.extend(groups[4].clone());
    }
    let indices = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id(), i))
        .collect::<BTreeMap<_, _>>();
    let heights = projected
        .iter()
        .map(|item| {
            let fallback = if matches!(
                item,
                ListItem::Thread {
                    section: Section::Pinned | Section::Active | Section::Working,
                    ..
                }
            ) {
                card
            } else {
                slim
            };
            match item {
                ListItem::Marker(Marker::PinnedHeader | Marker::PinnedDivider) => {
                    input.label_height * scale
                }
                ListItem::Marker(Marker::ActivePlaceholder | Marker::SettledPlaceholder) => slim,
                ListItem::Thread { key, .. } if key == active => fallback,
                _ => indices
                    .get(&item.id())
                    .and_then(|i| input.rects.get(*i))
                    .copied()
                    .flatten()
                    .map_or(fallback, |r| r.height),
            }
        })
        .collect::<Vec<_>>();
    let mut shelf_space = 0.0;
    if let Some(i) = items
        .iter()
        .position(|item| matches!(item,ListItem::Marker(m)if m.shelf()))
        && let (Some(shelf), Some(before), Some(last)) = (
            input.rects.get(i).copied().flatten(),
            i.checked_sub(1)
                .and_then(|i| input.rects.get(i))
                .copied()
                .flatten(),
            input.rects.last().copied().flatten(),
        )
        && shelf.top > before.bottom() + 1.0
    {
        shelf_space = (last.bottom() - first.top - heights.iter().map(|h| h + 1.0).sum::<f64>()
            + 1.0)
            .max(0.0);
    }
    let mut result = vec![
        Transform {
            scale_y: 0.0,
            ..STATIONARY
        };
        items.len()
    ];
    let mut top = first.top;
    for (i, item) in projected.iter().enumerate() {
        if matches!(item,ListItem::Marker(m)if m.shelf()) {
            top += shelf_space;
            shelf_space = 0.0;
        }
        if let Some(&at) = indices.get(&item.id())
            && let Some(rect) = input.rects.get(at).copied().flatten()
        {
            result[at] = Transform {
                y: top - rect.top,
                ..STATIONARY
            };
        }
        top += heights[i] + 1.0;
    }
    result[input.active_index] = STATIONARY;
    result
}
