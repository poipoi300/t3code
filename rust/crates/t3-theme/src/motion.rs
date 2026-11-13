//! Motion: panel animation settings, the presence and navigation rules behind panel
//! transitions, the sidebar list animation planner, and the rule that nothing in the
//! interface repaints continuously.
//!
//! Ports of `apps/web/src/panelAnimations.ts` and `apps/web/src/components/Sidebar.motion.ts`
//! with the DOM taken out: both become pure state that reports what to animate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::tokens::table::TokenTable;

// ------------------------------------------------------------------ timing functions

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Timing {
    Linear,
    CubicBezier { p: [f64; 4] },
    Steps { count: u32, jump: StepJump },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepJump {
    Start,
    End,
    Both,
    None,
}

impl Timing {
    pub const EASE: Timing = Timing::CubicBezier {
        p: [0.25, 0.1, 0.25, 1.0],
    };
    pub const EASE_IN: Timing = Timing::CubicBezier {
        p: [0.42, 0.0, 1.0, 1.0],
    };
    pub const EASE_OUT: Timing = Timing::CubicBezier {
        p: [0.0, 0.0, 0.58, 1.0],
    };
    pub const EASE_IN_OUT: Timing = Timing::CubicBezier {
        p: [0.42, 0.0, 0.58, 1.0],
    };

    /// A timing function keyword or function as written in CSS.
    pub fn parse(text: &str) -> Option<Timing> {
        let text = text.trim().to_ascii_lowercase();
        match text.as_str() {
            "linear" => return Some(Timing::Linear),
            "ease" => return Some(Timing::EASE),
            "ease-in" => return Some(Timing::EASE_IN),
            "ease-out" => return Some(Timing::EASE_OUT),
            "ease-in-out" => return Some(Timing::EASE_IN_OUT),
            "step-start" => {
                return Some(Timing::Steps {
                    count: 1,
                    jump: StepJump::Start,
                });
            }
            "step-end" => {
                return Some(Timing::Steps {
                    count: 1,
                    jump: StepJump::End,
                });
            }
            _ => {}
        }
        let inner = |name: &str| {
            text.strip_prefix(name)?
                .strip_prefix('(')?
                .strip_suffix(')')
        };
        if let Some(args) = inner("cubic-bezier") {
            let p: Vec<f64> = args
                .split(',')
                .map(|v| v.trim().parse().ok())
                .collect::<Option<_>>()?;
            return Some(Timing::CubicBezier {
                p: p.try_into().ok()?,
            });
        }
        if let Some(args) = inner("steps") {
            let mut parts = args.split(',').map(str::trim);
            let count = parts.next()?.parse().ok()?;
            let jump = match parts.next().unwrap_or("end") {
                "start" | "jump-start" => StepJump::Start,
                "end" | "jump-end" => StepJump::End,
                "jump-both" => StepJump::Both,
                "jump-none" => StepJump::None,
                _ => return None,
            };
            return Some(Timing::Steps { count, jump });
        }
        None
    }

    pub fn is_stepped(self) -> bool {
        matches!(self, Timing::Steps { .. })
    }
}

fn split_top_level_spaces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut depth = 0;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn parse_time(token: &str) -> Option<f64> {
    if let Some(ms) = token.strip_suffix("ms") {
        return ms.parse().ok();
    }
    token
        .strip_suffix('s')
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| s * 1000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Iterations {
    Count(f64),
    Infinite,
}

/// One entry of an `animation` shorthand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationSpec {
    pub name: String,
    pub duration_ms: f64,
    pub delay_ms: f64,
    pub timing: Timing,
    pub iterations: Iterations,
}

/// Parses an `animation` shorthand (comma separated list).
pub fn parse_animation(value: &str) -> Vec<AnimationSpec> {
    split_top_level_commas(value)
        .into_iter()
        .filter_map(|entry| {
            let mut spec = AnimationSpec {
                name: String::new(),
                duration_ms: 0.0,
                delay_ms: 0.0,
                timing: Timing::EASE,
                iterations: Iterations::Count(1.0),
            };
            let mut times = 0;
            for token in split_top_level_spaces(entry) {
                if let Some(time) = parse_time(&token) {
                    if times == 0 {
                        spec.duration_ms = time
                    } else {
                        spec.delay_ms = time
                    }
                    times += 1;
                } else if token == "infinite" {
                    spec.iterations = Iterations::Infinite;
                } else if let Some(timing) = Timing::parse(&token) {
                    spec.timing = timing;
                } else if let Ok(count) = token.parse::<f64>() {
                    spec.iterations = Iterations::Count(count);
                } else if !matches!(
                    token.as_str(),
                    "none"
                        | "forwards"
                        | "backwards"
                        | "both"
                        | "normal"
                        | "reverse"
                        | "alternate"
                        | "alternate-reverse"
                        | "paused"
                        | "running"
                ) && spec.name.is_empty()
                {
                    spec.name = token;
                }
            }
            (!spec.name.is_empty() || spec.duration_ms > 0.0).then_some(spec)
        })
        .collect()
}

/// One entry of a `transition` shorthand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransitionSpec {
    pub property: String,
    pub duration_ms: f64,
    pub delay_ms: f64,
    pub timing: Timing,
}

/// Parses a `transition` shorthand (comma separated list).
pub fn parse_transition(value: &str) -> Vec<TransitionSpec> {
    split_top_level_commas(value)
        .into_iter()
        .map(|entry| {
            let mut spec = TransitionSpec {
                property: "all".into(),
                duration_ms: 0.0,
                delay_ms: 0.0,
                timing: Timing::EASE,
            };
            let mut times = 0;
            for token in split_top_level_spaces(entry) {
                if let Some(time) = parse_time(&token) {
                    if times == 0 {
                        spec.duration_ms = time
                    } else {
                        spec.delay_ms = time
                    }
                    times += 1;
                } else if let Some(timing) = Timing::parse(&token) {
                    spec.timing = timing;
                } else if token != "none" {
                    spec.property = token;
                }
            }
            spec
        })
        .collect()
}

// ------------------------------------------------------------------ nothing repaints continuously

/// How often an animation asks the compositor for a new frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repaint {
    /// Runs a bounded number of times.
    Finite,
    /// Loops, but only through stepped timing: a handful of discrete frames per cycle.
    Stepped,
    /// Loops with smooth timing, repainting every frame for as long as it is visible.
    Continuous,
}

/// Classifies an animation against the project rule that nothing repaints continuously.
pub fn classify_repaint(
    spec: &AnimationSpec,
    keyframes: Option<&crate::tokens::table::Keyframes>,
) -> Repaint {
    if !matches!(spec.iterations, Iterations::Infinite) {
        return Repaint::Finite;
    }
    let Some(keyframes) = keyframes else {
        return if spec.timing.is_stepped() {
            Repaint::Stepped
        } else {
            Repaint::Continuous
        };
    };
    // Expand the keyframes into (offset, properties, easing-out) in time order.
    let offset = |stop: &str| match stop {
        "from" => Some(0.0),
        "to" => Some(100.0),
        other => other.strip_suffix('%').and_then(|v| v.parse::<f64>().ok()),
    };
    type Frame<'a> = (f64, Vec<(&'a String, &'a String)>, Option<Timing>);
    let mut frames: Vec<Frame<'_>> = Vec::new();
    for frame in &keyframes.frames {
        let timing = frame
            .properties
            .get("animation-timing-function")
            .and_then(|t| Timing::parse(t));
        let properties: Vec<(&String, &String)> = frame
            .properties
            .iter()
            .filter(|(name, _)| name.as_str() != "animation-timing-function")
            .collect();
        for stop in &frame.stops {
            if let Some(at) = offset(stop) {
                frames.push((at, properties.clone(), timing));
            }
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    // A smooth segment between two different keyframes repaints every frame; a segment between
    // identical keyframes is a hold, and a stepped one draws a handful of discrete frames.
    let smooth_segment = |pair: &[Frame<'_>]| {
        let (from, to) = (&pair[0], &pair[1]);
        from.1 != to.1 && !from.2.unwrap_or(spec.timing).is_stepped()
    };
    let implicit_edges =
        frames.first().is_some_and(|f| f.0 > 0.0) || frames.last().is_some_and(|f| f.0 < 100.0);
    let implicit_smooth = implicit_edges && !spec.timing.is_stepped();
    if frames.windows(2).any(smooth_segment) || implicit_smooth {
        Repaint::Continuous
    } else {
        Repaint::Stepped
    }
}

/// An infinite animation the stylesheet declares and how it repaints.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopingAnimation {
    pub token: String,
    pub origin: String,
    pub spec: AnimationSpec,
    pub repaint: Repaint,
}

/// Every `--animate-*` token that loops, with its repaint class.
pub fn looping_animations(table: &TokenTable) -> Vec<LoopingAnimation> {
    use crate::tokens::expr::Expr;
    let mut out = Vec::new();
    for declaration in table
        .declarations
        .iter()
        .filter(|d| d.name.starts_with("--animate-"))
    {
        let text = match &declaration.value {
            Expr::Raw { text } => text.clone(),
            Expr::List { items, .. } => items.iter().map(expr_text).collect::<Vec<_>>().join(" "),
            other => expr_text(other),
        };
        for spec in parse_animation(&text) {
            if matches!(spec.iterations, Iterations::Infinite) {
                let keyframes = table.keyframes.iter().find(|k| k.name == spec.name);
                let repaint = classify_repaint(&spec, keyframes);
                out.push(LoopingAnimation {
                    token: declaration.name.clone(),
                    origin: declaration.origin.clone(),
                    spec,
                    repaint,
                });
            }
        }
    }
    out
}

/// A place the stylesheet starts an animation that never stops.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopingUse {
    /// `@utility name` or the selector of the rule.
    pub source: String,
    pub spec: AnimationSpec,
    pub repaint: Repaint,
    /// The play state follows `--visible-animation-state`, so it pauses while offscreen.
    pub visibility_gated: bool,
    /// A `prefers-reduced-motion: reduce` rule turns it off.
    pub off_for_reduced_motion: bool,
}

fn animation_text(table: &TokenTable, expr: &crate::tokens::expr::Expr) -> String {
    use crate::tokens::expr::Expr;
    match expr {
        Expr::Var { name, .. } => table
            .declarations
            .iter()
            .find(|d| d.name == *name)
            .map(|d| animation_text(table, &d.value))
            .unwrap_or_default(),
        other => expr_text(other),
    }
}

/// Every `animation` the stylesheet applies (utilities and rules, not tokens) that loops forever.
///
/// The project rule is that nothing repaints continuously: each of these must either be
/// [`Repaint::Stepped`] or, if smooth, be gated by visibility.
pub fn looping_animation_uses(table: &TokenTable) -> Vec<LoopingUse> {
    use crate::tokens::expr::Expr;
    let mut out = Vec::new();
    for utility in &table.utilities {
        let gated = utility.declarations.iter().any(|d| {
            d.prop == "animation-play-state"
                && matches!(&d.value, Expr::Var { name, .. } if name == "--visible-animation-state")
        });
        let off = utility.declarations.iter().any(|d| {
            d.prop == "animation" && matches!(&d.value, Expr::Keyword { v } if v == "none")
        });
        for declaration in utility
            .declarations
            .iter()
            .filter(|d| d.prop == "animation")
        {
            for spec in parse_animation(&animation_text(table, &declaration.value)) {
                if matches!(spec.iterations, Iterations::Infinite) {
                    let repaint = classify_repaint(
                        &spec,
                        table.keyframes.iter().find(|k| k.name == spec.name),
                    );
                    out.push(LoopingUse {
                        source: format!("@utility {}", utility.name),
                        spec,
                        repaint,
                        visibility_gated: gated,
                        off_for_reduced_motion: off,
                    });
                }
            }
        }
    }
    for rule in table.motion.iter().filter(|m| m.property == "animation") {
        for spec in parse_animation(&rule.value) {
            if matches!(spec.iterations, Iterations::Infinite) {
                let repaint =
                    classify_repaint(&spec, table.keyframes.iter().find(|k| k.name == spec.name));
                out.push(LoopingUse {
                    source: rule.selector.clone(),
                    spec,
                    repaint,
                    visibility_gated: false,
                    off_for_reduced_motion: false,
                });
            }
        }
    }
    out
}

fn expr_text(expr: &crate::tokens::expr::Expr) -> String {
    use crate::tokens::expr::Expr;
    match expr {
        Expr::Keyword { v } => v.clone(),
        Expr::Duration { ms } => format!("{ms}ms"),
        Expr::Number { v } => v.to_string(),
        Expr::CubicBezier { p } => format!("cubic-bezier({},{},{},{})", p[0], p[1], p[2], p[3]),
        Expr::Steps { n, jump } => format!("steps({n},{jump})"),
        Expr::Raw { text } => text.clone(),
        Expr::List { items, sep } => items
            .iter()
            .map(expr_text)
            .collect::<Vec<_>>()
            .join(if sep == "," { "," } else { " " }),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ panel animation

pub const MIN_PANEL_ANIMATION_DURATION_MS: u32 = 0;
pub const MAX_PANEL_ANIMATION_DURATION_MS: u32 = 400;
/// Panels snap by default.
pub const DEFAULT_PANEL_ANIMATION_DURATION_MS: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelAnimationSettings {
    pub active: bool,
    pub duration_ms: u32,
}

/// `usePanelAnimationSettings`: panels animate only for a positive duration, without a
/// reduced-motion request, and not while a navigation is still painting.
pub fn panel_animation_settings(
    duration_ms: u32,
    prefers_reduced_motion: bool,
    suppressed: bool,
) -> PanelAnimationSettings {
    PanelAnimationSettings {
        active: duration_ms > 0 && !prefers_reduced_motion && !suppressed,
        duration_ms,
    }
}

/// Browser frames to wait after a navigation before panel animations come back.
pub const NAVIGATION_SUPPRESSION_FRAMES: u32 = 2;

/// `usePanelNavigationSuppression`: animations stay off until the new route has painted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavigationSuppression {
    painted: Option<String>,
}

impl NavigationSuppression {
    pub fn suppressed(&self, navigation_key: &str) -> bool {
        self.painted.as_deref() != Some(navigation_key)
    }

    /// Call after [`NAVIGATION_SUPPRESSION_FRAMES`] frames have rendered the route.
    pub fn mark_painted(&mut self, navigation_key: &str) {
        self.painted = Some(navigation_key.to_owned());
    }
}

/// What `usePanelPresence` renders for a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceFrame<T> {
    /// Whether the panel is mounted (open, or still playing its exit).
    pub present: bool,
    pub value: Option<T>,
    /// Unmount the panel after this long unless it reopens first.
    pub unmount_after_ms: Option<u32>,
}

/// `usePanelPresence`: keeps a closing panel and its last value mounted for its exit
/// transition, but only for the scope it was open in.
#[derive(Clone, Debug)]
pub struct PanelPresence<T> {
    present: bool,
    retained: Option<(Option<String>, Option<T>)>,
}

impl<T: Clone + PartialEq> PanelPresence<T> {
    pub fn new(open: bool, value: Option<T>, scope_key: Option<&str>) -> Self {
        Self {
            present: open,
            retained: open.then(|| (scope_key.map(str::to_owned), value)),
        }
    }

    pub fn render(
        &mut self,
        open: bool,
        value: Option<T>,
        animated: bool,
        scope_key: Option<&str>,
        duration_ms: u32,
    ) -> PresenceFrame<T> {
        let scope = scope_key.map(str::to_owned);
        if open
            && self
                .retained
                .as_ref()
                .is_none_or(|(s, v)| *s != scope || *v != value)
        {
            self.retained = Some((scope.clone(), value.clone()));
        }
        if open && !self.present {
            self.present = true;
        }
        if !open && !animated && self.present {
            self.present = false;
        }
        let retained_value = self
            .retained
            .as_ref()
            .filter(|(s, _)| *s == scope)
            .and_then(|(_, v)| v.clone());
        let visible = open
            || (animated
                && self.present
                && self.retained.as_ref().is_some_and(|(s, _)| *s == scope));
        PresenceFrame {
            present: visible,
            value: if open {
                value
            } else if visible {
                retained_value
            } else {
                None
            },
            unmount_after_ms: (!open && animated).then_some(duration_ms),
        }
    }

    /// The exit timer fired.
    pub fn exit_elapsed(&mut self) {
        self.present = false;
    }
}

// ------------------------------------------------------------------ sidebar list motion

/// The one timing every sidebar row movement shares.
pub const SIDEBAR_MOTION_DURATION_MS: f64 = 150.0;
pub const SIDEBAR_MOTION_TIMING: Timing = Timing::EASE_OUT;
/// Rows travel at most this far when they fade in or out.
pub const MAX_ROW_TRAVEL_PX: f64 = 40.0;
/// Fades clone removed rows and animate clones and entries; more than this per update would
/// force a reflow per row, so larger changes skip animation.
pub const MAX_FADED_ROWS_PER_UPDATE: usize = 40;

fn row_travel(height: f64) -> f64 {
    height.min(MAX_ROW_TRAVEL_PX)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowRect {
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
}

/// A running animation, identified so the host can report its progress.
pub type AnimationId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum MotionCommand<K> {
    /// Stop everything this row is animating with a position offset.
    CancelMove(K),
    /// Stop a row's entry animation.
    CancelEnter(K),
    /// Stop all entry and exit animations and remove the exit clones.
    ClearFades,
    /// Slide `node` from `offset` pixels away to its place.
    Move {
        node: K,
        offset: f64,
        animation: AnimationId,
    },
    /// Fade `node` in while it travels `travel` pixels into place.
    Enter {
        node: K,
        travel: f64,
        animation: AnimationId,
    },
    /// Show a clone of a removed row at `rect` and fade it out while it travels.
    Exit {
        rect: RowRect,
        from_opacity: f64,
        travel: f64,
        animation: AnimationId,
    },
}

struct Running {
    offset: f64,
    animation: AnimationId,
}

struct Entering {
    travel: f64,
    animation: AnimationId,
}

/// The animation planner for one sidebar list.
pub struct SidebarListMotion<K> {
    positions: Option<Vec<(K, RowRect)>>,
    running: BTreeMap<K, Running>,
    entering: BTreeMap<K, Entering>,
    exiting: Vec<AnimationId>,
    released: Option<BTreeMap<K, f64>>,
    next_animation: AnimationId,
}

impl<K: Clone + Ord> Default for SidebarListMotion<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + Ord> SidebarListMotion<K> {
    pub fn new() -> Self {
        Self {
            positions: None,
            running: BTreeMap::new(),
            entering: BTreeMap::new(),
            exiting: Vec::new(),
            released: None,
            next_animation: 0,
        }
    }

    fn animation(&mut self) -> AnimationId {
        self.next_animation += 1;
        self.next_animation
    }

    /// The host reports an animation finished (or cancelled).
    pub fn finished(&mut self, animation: AnimationId) {
        self.running.retain(|_, r| r.animation != animation);
        self.entering.retain(|_, e| e.animation != animation);
        self.exiting.retain(|a| *a != animation);
    }

    fn remaining_offset(&self, node: &K, progress: &dyn Fn(AnimationId) -> f64) -> f64 {
        let run = self
            .running
            .get(node)
            .map_or(0.0, |r| r.offset * (1.0 - progress(r.animation)));
        let enter = self
            .entering
            .get(node)
            .map_or(0.0, |e| e.travel * (1.0 - progress(e.animation)));
        run + enter
    }

    /// `suspend`: forget positions and cancel everything (for example when the list is hidden).
    pub fn suspend(&mut self) -> Vec<MotionCommand<K>> {
        let mut commands: Vec<MotionCommand<K>> = self
            .running
            .keys()
            .cloned()
            .map(MotionCommand::CancelMove)
            .collect();
        self.running.clear();
        commands.push(MotionCommand::ClearFades);
        self.entering.clear();
        self.exiting.clear();
        self.positions = None;
        self.released = None;
        commands
    }

    /// `release`: a drag ended; rows slide from where the drag left them (`tops` are offsets
    /// from the list's top) to their next positions on the following update.
    pub fn release(&mut self, tops: Vec<(K, f64)>) -> Vec<MotionCommand<K>> {
        let commands = self.suspend();
        self.released = Some(tops.into_iter().collect());
        commands
    }

    /// `update`: the list was laid out again with `next` (in document order).
    pub fn update(
        &mut self,
        next: Vec<(K, RowRect)>,
        animate: bool,
        reduced_motion: bool,
        progress: &dyn Fn(AnimationId) -> f64,
    ) -> Vec<MotionCommand<K>> {
        let mut commands = Vec::new();
        let next_lookup: BTreeMap<&K, &RowRect> = next.iter().map(|(k, r)| (k, r)).collect();
        let previous = self.positions.take();
        let previous_lookup: BTreeMap<&K, &RowRect> =
            previous.iter().flatten().map(|(k, r)| (k, r)).collect();

        let mut fade_count = 0;
        if previous.is_some() {
            fade_count += previous_lookup
                .iter()
                .filter(|(k, r)| !next_lookup.contains_key(*k) && r.height > 0.0)
                .count();
            fade_count += next_lookup
                .iter()
                .filter(|(k, r)| !previous_lookup.contains_key(*k) && r.height > 0.0)
                .count();
        }
        let should_animate = animate
            && previous.is_some()
            && !reduced_motion
            && fade_count <= MAX_FADED_ROWS_PER_UPDATE;

        let mut moved_delta: BTreeMap<K, f64> = BTreeMap::new();
        let next_order: Vec<&K> = next.iter().map(|(k, _)| k).collect();
        let old_order: Vec<&K> = previous.iter().flatten().map(|(k, _)| k).collect();

        if !should_animate {
            commands.push(MotionCommand::ClearFades);
            self.entering.clear();
            self.exiting.clear();
        } else if let Some(previous_rows) = &previous {
            for (node, rect) in &next {
                let Some(previous_rect) = previous_lookup.get(node) else {
                    continue;
                };
                if previous_rect.top == rect.top {
                    continue;
                }
                moved_delta.insert(
                    node.clone(),
                    previous_rect.top + self.remaining_offset(node, progress) - rect.top,
                );
            }
            for (index, (node, rect)) in previous_rows.iter().enumerate() {
                if next_lookup.contains_key(node) {
                    continue;
                }
                let riding = self.riding_delta(&old_order, index, &moved_delta, progress, &|n| {
                    next_lookup.contains_key(n)
                });
                if rect.height == 0.0 {
                    continue;
                }
                let travel = riding.map_or_else(|| row_travel(rect.height), |d| -d);
                let from_opacity = self
                    .entering
                    .get(node)
                    .map_or(1.0, |e| progress(e.animation));
                let top = rect.top + self.remaining_offset(node, progress);
                let animation = self.animation();
                self.exiting.push(animation);
                commands.push(MotionCommand::Exit {
                    rect: RowRect { top, ..*rect },
                    from_opacity,
                    travel,
                    animation,
                });
            }
        }

        let gone: Vec<K> = self
            .entering
            .keys()
            .filter(|k| !next_lookup.contains_key(k))
            .cloned()
            .collect();
        for node in gone {
            self.entering.remove(&node);
            commands.push(MotionCommand::CancelEnter(node));
        }
        let cancelled: Vec<K> = self
            .running
            .keys()
            .filter(|k| !should_animate || !next_lookup.contains_key(k))
            .cloned()
            .collect();
        for node in cancelled {
            self.running.remove(&node);
            commands.push(MotionCommand::CancelMove(node));
        }

        if should_animate {
            for (index, (node, rect)) in next.iter().enumerate() {
                match previous_lookup.get(node) {
                    None => {
                        if rect.height > 0.0 {
                            let riding = self.riding_delta(
                                &next_order,
                                index,
                                &moved_delta,
                                progress,
                                &|n| previous_lookup.contains_key(n),
                            );
                            let travel = riding.unwrap_or(-row_travel(rect.height));
                            let animation = self.animation();
                            self.entering
                                .insert(node.clone(), Entering { travel, animation });
                            commands.push(MotionCommand::Enter {
                                node: node.clone(),
                                travel,
                                animation,
                            });
                        }
                    }
                    Some(_) => {
                        if let Some(delta) = moved_delta.get(node).copied() {
                            self.start_move(node, delta, &mut commands);
                        }
                    }
                }
            }
        }

        if let Some(released) = self.released.take()
            && !reduced_motion
        {
            for (node, rect) in &next {
                if let Some(top) = released.get(node) {
                    self.start_move(node, top - rect.top, &mut commands);
                }
            }
        }
        self.positions = Some(next);
        commands
    }

    fn start_move(&mut self, node: &K, offset: f64, commands: &mut Vec<MotionCommand<K>>) {
        if self.running.remove(node).is_some() {
            commands.push(MotionCommand::CancelMove(node.clone()));
        }
        let entry_exists = if let Some(entry) = self.entering.get_mut(node) {
            entry.travel = 0.0;
            true
        } else {
            false
        };
        if offset == 0.0 && !entry_exists {
            return;
        }
        let animation = self.animation();
        self.running
            .insert(node.clone(), Running { offset, animation });
        commands.push(MotionCommand::Move {
            node: node.clone(),
            offset,
            animation,
        });
    }

    /// The offset a neighbouring row is still travelling, so an inserted or removed row rides along.
    fn riding_delta(
        &self,
        order: &[&K],
        index: usize,
        moved_delta: &BTreeMap<K, f64>,
        progress: &dyn Fn(AnimationId) -> f64,
        retained: &dyn Fn(&K) -> bool,
    ) -> Option<f64> {
        for cursor in (0..index).rev() {
            let node = order[cursor];
            if let Some(delta) = moved_delta.get(node) {
                return Some(*delta);
            }
            if retained(node) {
                return Some(self.remaining_offset(node, progress));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(keys: &[&'static str]) -> Vec<(&'static str, RowRect)> {
        keys.iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    *k,
                    RowRect {
                        top: i as f64 * 30.0,
                        left: 0.0,
                        width: 200.0,
                        height: 28.0,
                    },
                )
            })
            .collect()
    }

    fn done(_: AnimationId) -> f64 {
        1.0
    }

    #[test]
    fn first_layout_only_records_positions() {
        let mut motion = SidebarListMotion::new();
        let commands = motion.update(rows(&["a", "b"]), true, false, &done);
        assert_eq!(commands, vec![MotionCommand::ClearFades]);
    }

    #[test]
    fn an_inserted_row_fades_in_and_the_rows_below_slide_down() {
        let mut motion = SidebarListMotion::new();
        motion.update(rows(&["a", "b"]), true, false, &done);
        let commands = motion.update(rows(&["a", "x", "b"]), true, false, &done);
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, MotionCommand::Enter { node: "x", .. }))
        );
        assert!(commands.iter().any(|c| matches!(c, MotionCommand::Move { node: "b", offset, .. } if (*offset + 30.0).abs() < 1e-9)));
    }

    #[test]
    fn a_removed_row_leaves_a_clone_that_fades_out() {
        let mut motion = SidebarListMotion::new();
        motion.update(rows(&["a", "b", "c"]), true, false, &done);
        let commands = motion.update(rows(&["a", "c"]), true, false, &done);
        let exit = commands.iter().find_map(|c| match c {
            MotionCommand::Exit { rect, travel, .. } => Some((rect.top, *travel)),
            _ => None,
        });
        assert_eq!(exit.map(|(top, _)| top), Some(30.0));
        assert!(commands.iter().any(|c| matches!(c, MotionCommand::Move { node: "c", offset, .. } if (*offset - 30.0).abs() < 1e-9)));
    }

    #[test]
    fn reduced_motion_and_large_changes_skip_animation() {
        let mut motion = SidebarListMotion::new();
        motion.update(rows(&["a", "b"]), true, false, &done);
        assert_eq!(
            motion.update(rows(&["b", "a"]), true, true, &done),
            vec![MotionCommand::ClearFades]
        );
        let many: Vec<&'static str> = (0..45)
            .map(|i| Box::leak(format!("row{i}").into_boxed_str()) as &'static str)
            .collect();
        let mut motion = SidebarListMotion::new();
        motion.update(rows(&["a"]), true, false, &done);
        let commands = motion.update(rows(&many), true, false, &done);
        assert!(
            commands
                .iter()
                .all(|c| !matches!(c, MotionCommand::Enter { .. }))
        );
    }

    #[test]
    fn presence_keeps_the_exit_mounted_for_its_scope_only() {
        let mut presence = PanelPresence::new(true, Some(1), Some("thread-a"));
        let open = presence.render(true, Some(1), true, Some("thread-a"), 200);
        assert!(open.present);
        let closing = presence.render(false, None, true, Some("thread-a"), 200);
        assert_eq!(
            (closing.present, closing.value, closing.unmount_after_ms),
            (true, Some(1), Some(200))
        );
        let elsewhere = presence.render(false, None, true, Some("thread-b"), 200);
        assert!(!elsewhere.present);
        presence.exit_elapsed();
        assert!(
            !presence
                .render(false, None, true, Some("thread-a"), 200)
                .present
        );
        let mut snapping = PanelPresence::new(true, Some(1), None);
        assert!(!snapping.render(false, None, false, None, 0).present);
    }

    #[test]
    fn panels_animate_only_when_asked_and_allowed() {
        assert!(panel_animation_settings(200, false, false).active);
        assert!(!panel_animation_settings(0, false, false).active);
        assert!(!panel_animation_settings(200, true, false).active);
        assert!(!panel_animation_settings(200, false, true).active);
        let mut navigation = NavigationSuppression::default();
        assert!(navigation.suppressed("/a"));
        navigation.mark_painted("/a");
        assert!(!navigation.suppressed("/a"));
        assert!(navigation.suppressed("/b"));
    }

    #[test]
    fn timing_functions_parse() {
        assert_eq!(Timing::parse("ease-out"), Some(Timing::EASE_OUT));
        assert_eq!(
            Timing::parse("cubic-bezier(0.32, 0.72, 0, 1)"),
            Some(Timing::CubicBezier {
                p: [0.32, 0.72, 0.0, 1.0]
            })
        );
        assert_eq!(
            Timing::parse("steps(4)"),
            Some(Timing::Steps {
                count: 4,
                jump: StepJump::End
            })
        );
        assert_eq!(Timing::parse("nope"), None);
        let spec = &parse_animation("status-pulse 2s infinite")[0];
        assert_eq!(
            (spec.name.as_str(), spec.duration_ms, spec.iterations),
            ("status-pulse", 2000.0, Iterations::Infinite)
        );
        let transition = &parse_transition("opacity 600ms ease-out 20ms")[0];
        assert_eq!(
            (
                transition.property.as_str(),
                transition.duration_ms,
                transition.delay_ms
            ),
            ("opacity", 600.0, 20.0)
        );
    }
}
