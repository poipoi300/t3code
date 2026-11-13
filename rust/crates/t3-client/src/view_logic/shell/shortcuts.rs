//! Scope dispatch and route command decisions. Hosts install listeners and execute returned actions.
use super::keybindings::{KeyEvent, MatchContext, resolve_shortcut_command};
use std::collections::BTreeMap;
use t3_contracts::keybindings::ResolvedKeybindingRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Capture,
    Bubble,
    Observe,
}
#[derive(Debug, Clone)]
pub struct DispatchEvent {
    pub key: KeyEvent,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
    pub inside_confirmation: bool,
}
impl DispatchEvent {
    pub fn claim(&mut self) {
        self.default_prevented = true;
        self.propagation_stopped = true;
    }
}
pub type Handler<'a> = Box<dyn Fn(&mut DispatchEvent) -> bool + 'a>;
pub type Observer<'a> = Box<dyn Fn(&DispatchEvent, bool) + 'a>;
type MenuHandlers<'a> = BTreeMap<String, Vec<(u64, Box<dyn Fn() + 'a>)>>;
pub struct CommandScope<'a> {
    pub phase: Phase,
    pub commands: BTreeMap<String, Handler<'a>>,
    pub when: Box<dyn Fn(&DispatchEvent) -> bool + 'a>,
    pub context: Box<dyn Fn() -> MatchContext + 'a>,
    pub keybindings: Option<Box<dyn Fn() -> Vec<ResolvedKeybindingRule> + 'a>>,
    pub on_keydown: Option<Handler<'a>>,
    pub observe: Option<Observer<'a>>,
}
impl Default for CommandScope<'_> {
    fn default() -> Self {
        Self {
            phase: Phase::Bubble,
            commands: BTreeMap::new(),
            when: Box::new(|_| true),
            context: Box::new(MatchContext::default),
            keybindings: None,
            on_keydown: None,
            observe: None,
        }
    }
}
#[derive(Default)]
pub struct Dispatcher<'a> {
    next_id: u64,
    scopes: Vec<(u64, CommandScope<'a>)>,
    menus: MenuHandlers<'a>,
}
impl<'a> Dispatcher<'a> {
    pub fn register(&mut self, scope: CommandScope<'a>) -> u64 {
        self.next_id += 1;
        self.scopes.push((self.next_id, scope));
        self.next_id
    }
    pub fn unregister(&mut self, id: u64) {
        self.scopes.retain(|(key, _)| *key != id);
        for handlers in self.menus.values_mut() {
            handlers.retain(|(key, _)| *key != id);
        }
    }
    pub fn register_menu(&mut self, action: String, handler: Box<dyn Fn() + 'a>) -> u64 {
        self.next_id += 1;
        self.menus
            .entry(action)
            .or_default()
            .push((self.next_id, handler));
        self.next_id
    }
    pub fn dispatch_menu(&self, action: &str) {
        if let Some((_, handler)) = self.menus.get(action).and_then(|h| h.last()) {
            handler();
        }
    }
    fn command<'s>(
        scope: &'s CommandScope<'a>,
        event: &DispatchEvent,
        primary: &[ResolvedKeybindingRule],
        platform: &str,
        focus: &MatchContext,
    ) -> Option<&'s Handler<'a>> {
        let mut context = focus.clone();
        context.0.extend((scope.context)().0);
        let bindings = scope.keybindings.as_ref().map(|read| read());
        let command = resolve_shortcut_command(
            &event.key,
            bindings.as_deref().unwrap_or(primary),
            platform,
            &context,
        )?;
        scope.commands.get(&command.0)
    }
    pub fn has_command(
        &self,
        event: &DispatchEvent,
        primary: &[ResolvedKeybindingRule],
        platform: &str,
        focus: &MatchContext,
    ) -> bool {
        self.scopes.iter().any(|(_, scope)| {
            scope.phase == Phase::Bubble
                && (scope.when)(event)
                && Self::command(scope, event, primary, platform, focus).is_some()
        })
    }
    pub fn dispatch(
        &self,
        phase: Phase,
        event: &mut DispatchEvent,
        primary: &[ResolvedKeybindingRule],
        platform: &str,
        focus: &MatchContext,
    ) -> bool {
        let mut consumed = false;
        for (_, scope) in self
            .scopes
            .iter()
            .rev()
            .filter(|(_, s)| s.phase == phase && s.observe.is_none())
        {
            if event.default_prevented {
                consumed = true;
                break;
            }
            if !(scope.when)(event) {
                continue;
            }
            if scope
                .on_keydown
                .as_ref()
                .is_some_and(|handle| handle(event))
            {
                consumed = true;
                break;
            }
            if let Some(handler) = Self::command(scope, event, primary, platform, focus) {
                if event.inside_confirmation {
                    event.default_prevented = true;
                    consumed = true;
                    break;
                }
                if handler(event) {
                    consumed = true;
                    break;
                }
            }
        }
        consumed |= event.default_prevented;
        if phase == Phase::Bubble {
            for (_, scope) in &self.scopes {
                if let Some(observer) = &scope.observe {
                    observer(event, consumed);
                }
            }
        }
        consumed
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChatRouteContext {
    pub chat_route: bool,
    pub modal_page: bool,
    pub palette_open: bool,
    pub selection_present: bool,
    pub open_projects: usize,
    pub total_projects: usize,
    pub has_thread: bool,
    pub can_operate_preview: bool,
    pub preview_open: bool,
    pub preview_available: bool,
    pub scratch_environment_available: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAction {
    ClearSelection,
    StartLocalThread,
    PickNewThreadProject,
    StartScratchThread,
    PreviewUnavailable,
    Preview(&'static str),
}
pub fn chat_route_enabled(context: &ChatRouteContext) -> bool {
    context.chat_route && !context.modal_page && !context.palette_open
}
pub fn chat_route_escape(key: &str, context: &ChatRouteContext) -> Option<RouteAction> {
    (chat_route_enabled(context) && key == "Escape" && context.selection_present)
        .then_some(RouteAction::ClearSelection)
}
/// `Some(None)` consumes the chord without taking an action, matching the installed handlers.
pub fn chat_route_command(
    command: &str,
    context: &ChatRouteContext,
) -> Option<Option<RouteAction>> {
    if !chat_route_enabled(context) {
        return None;
    }
    let action = match command {
        "chat.newLocal" => Some(RouteAction::StartLocalThread),
        "chat.new" => Some(
            if context.open_projects == 1 || context.total_projects == 0 {
                RouteAction::StartLocalThread
            } else {
                RouteAction::PickNewThreadProject
            },
        ),
        "chat.newWithoutProject" => context
            .scratch_environment_available
            .then_some(RouteAction::StartScratchThread),
        "preview.toggle" => {
            if !context.has_thread || (!context.can_operate_preview && !context.preview_open) {
                None
            } else if !context.preview_available {
                Some(RouteAction::PreviewUnavailable)
            } else {
                Some(RouteAction::Preview("toggle-panel"))
            }
        }
        "preview.refresh" | "preview.focusUrl" | "preview.zoomIn" | "preview.zoomOut"
        | "preview.resetZoom" | "preview.find" => (context.has_thread
            && context.can_operate_preview)
            .then_some(RouteAction::Preview(match command {
                "preview.refresh" => "refresh",
                "preview.focusUrl" => "focus-url",
                "preview.zoomIn" => "zoom-in",
                "preview.zoomOut" => "zoom-out",
                "preview.resetZoom" => "reset-zoom",
                _ => "find",
            })),
        _ => return None,
    };
    Some(action)
}
pub fn palette_escape(
    mounted: bool,
    open: bool,
    child_mode: bool,
    key: &str,
    composing: bool,
    key_code: u32,
) -> bool {
    mounted && open && child_mode && key == "Escape" && !composing && key_code != 229
}
pub fn next_appearance(mode: &str) -> &'static str {
    match mode {
        "system" => "light",
        "light" => "dark",
        _ => "system",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommandAction {
    ToggleMode(super::palette::OverlayMode),
    SetAppearance(&'static str),
    ChangeTheme,
    ToggleThemeEditor,
    /// Close the palette before navigating to the usage page.
    OpenUsage,
}
/// `Some(None)` claims a repeated theme/appearance chord without repeating its action.
pub fn palette_command(
    mounted: bool,
    command: &str,
    repeat: bool,
    appearance: &str,
) -> Option<Option<PaletteCommandAction>> {
    if !mounted {
        return None;
    }
    if let Some(mode) = super::palette::overlay_mode(Some(command)) {
        return Some(Some(PaletteCommandAction::ToggleMode(mode)));
    }
    Some(match command {
        "appearance.cycle" => {
            (!repeat).then(|| PaletteCommandAction::SetAppearance(next_appearance(appearance)))
        }
        "theme.select" => (!repeat).then_some(PaletteCommandAction::ChangeTheme),
        "themeEditor.toggle" => Some(PaletteCommandAction::ToggleThemeEditor),
        "usage.open" => Some(PaletteCommandAction::OpenUsage),
        _ => return None,
    })
}
