//! Port of apps/web/src/shortcuts/dispatcher.ts and keybindings.ts (NSH event bindings).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::element::desktop::{WindowRef, current_window};
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;
use t3_client::ClientStore;
use t3_client::connection::Phase as ConnectionPhase;
use t3_client::store::{EnvironmentState, Scope as StoreScope};
use t3_client::types::EnvironmentId;
use t3_client::view_logic::shell::keybindings::{
    self, KeyEvent, MatchContext, ModifierState, Modifiers, effective_shortcuts_for_command,
};
use t3_client::view_logic::shell::shortcut_config::{ShortcutSource, effective_config};
use t3_client::view_logic::shell::shortcuts::{self, CommandScope, Dispatcher};
pub use t3_client::view_logic::shell::shortcuts::{
    ChatRouteContext, DispatchEvent, Phase, RouteAction,
};
use t3_contracts::keybindings::{KeybindingShortcut, ResolvedKeybindingRule};

use crate::runtime::{backend::EnvironmentEntry, rt};
use crate::state::nav::{Navigator, navigator};
use crate::state::panels::Panels;

use crate::runtime::tasks::watch;

type Guard = Rc<dyn Fn(&DispatchEvent) -> bool>;
type ContextReader = Rc<dyn Fn() -> MatchContext>;
type RulesReader = Rc<dyn Fn() -> Vec<ResolvedKeybindingRule>>;
type Handler = Rc<dyn Fn(&mut DispatchEvent) -> bool>;
type Observer = Rc<dyn Fn(&DispatchEvent, bool)>;
type Menus = BTreeMap<u64, (String, Rc<dyn Fn()>)>;

/// A mounted web command scope: returning false gives the next scope its turn.
#[derive(Clone)]
pub struct Scope {
    pub phase: Phase,
    pub commands: BTreeMap<String, Handler>,
    pub when: Guard,
    pub context: ContextReader,
    pub keybindings: Option<RulesReader>,
    pub on_keydown: Option<Handler>,
    pub observe: Option<Observer>,
    pub raw_keys: Vec<KeybindingShortcut>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            phase: Phase::Bubble,
            commands: BTreeMap::new(),
            when: Rc::new(|_| true),
            context: Rc::new(MatchContext::default),
            keybindings: None,
            on_keydown: None,
            observe: None,
            raw_keys: Vec::new(),
        }
    }
}

impl Scope {
    pub fn command(
        mut self,
        command: &str,
        handler: impl Fn(&mut DispatchEvent) -> bool + 'static,
    ) -> Self {
        self.commands.insert(command.into(), Rc::new(handler));
        self
    }

    fn client(&self) -> CommandScope<'static> {
        let scope = self.clone();
        let when = scope.when;
        let context = scope.context;
        CommandScope {
            phase: scope.phase,
            commands: scope
                .commands
                .into_iter()
                .map(|(name, handler)| {
                    (
                        name,
                        Box::new(move |event: &mut DispatchEvent| handler(event))
                            as shortcuts::Handler<'static>,
                    )
                })
                .collect(),
            when: Box::new(move |event| when(event)),
            context: Box::new(move || context()),
            keybindings: scope.keybindings.map(|read| {
                Box::new(move || read()) as Box<dyn Fn() -> Vec<ResolvedKeybindingRule>>
            }),
            on_keydown: scope.on_keydown.map(|handle| {
                Box::new(move |event: &mut DispatchEvent| handle(event))
                    as shortcuts::Handler<'static>
            }),
            observe: scope.observe.map(|observe| {
                Box::new(move |event: &DispatchEvent, consumed| observe(event, consumed))
                    as shortcuts::Observer<'static>
            }),
        }
    }
}

/// UI-owned focus facts; all matching and modifier transitions stay in the client.
#[derive(Clone, Copy)]
pub struct KeyContext {
    window: Stored<WindowRef>,
    pub values: Signal<MatchContext>,
    pub selection_collapsed: Signal<Option<bool>>,
    pub modifiers: Signal<ModifierState>,
    pub modifiers_outside_editable: Signal<ModifierState>,
}

impl Default for KeyContext {
    fn default() -> Self {
        Self {
            window: stored(current_window()),
            values: signal(MatchContext::default()),
            selection_collapsed: signal(None),
            modifiers: signal(ModifierState::default()),
            modifiers_outside_editable: signal(ModifierState {
                ignore_editable: true,
                ..Default::default()
            }),
        }
    }
}

impl KeyContext {
    pub fn read(self) -> MatchContext {
        let mut context = MatchContext::new(self.window.with(|window| window.state().is_some()));
        context.0.extend(self.values.get().0);
        context
    }

    pub fn set(self, name: &str, value: bool) {
        self.values.update(|context| {
            context.0.insert(name.into(), value);
        });
        if name == "editableFocus" {
            self.modifiers_outside_editable
                .update(|state| state.focus(value));
        }
    }

    /// Register on the actual editable, terminal, preview or model-picker focus boundary.
    pub fn focus_within(self, element: ElementRef, name: &'static str) {
        element.on_focus_in(move || self.set(name, true));
        element.on_focus_out(move || self.set(name, false));
        cleanup(move || self.set(name, false));
    }

    /// Port of shortcutModifierState.ts: the host also calls reset on paste and window blur.
    pub fn keyboard(self, event: &nsh_core::KeyEvent, down: bool) {
        let event = client_event(event, down, self.read().get("editableFocus"));
        self.modifiers.update(|state| state.keyboard(&event));
        self.modifiers_outside_editable
            .update(|state| state.keyboard(&event));
    }

    pub fn reset_modifiers(self) {
        self.modifiers.update(ModifierState::reset);
        self.modifiers_outside_editable.update(ModifierState::reset);
    }
}

#[derive(Clone, Copy)]
pub struct Keybindings {
    pub context: KeyContext,
    pub primary: Signal<Option<EnvironmentId>>,
    pub rules: Memo<Vec<ResolvedKeybindingRule>>,
    scopes: Stored<BTreeMap<u64, Rc<Scope>>>,
    next_scope: Stored<u64>,
    revision: Signal<u64>,
    menus: Stored<Menus>,
    dispatcher: Stored<Option<(u64, Rc<Dispatcher<'static>>)>>,
}

impl Keybindings {
    /// Subscribe before reading config; shell and thread deltas never rebuild the shortcut table.
    pub fn attach(
        store: ClientStore,
        catalogue: Signal<Vec<EnvironmentEntry>>,
        primary: Signal<Option<EnvironmentId>>,
    ) -> Self {
        let mut changes = store.subscribe();
        let environments = signal(store.snapshot().environments);
        watch(async move {
            while let Some(change) = changes.next().await {
                if change
                    .scopes
                    .iter()
                    .any(|scope| matches!(scope, StoreScope::All | StoreScope::Environment(_)))
                {
                    let next = store.snapshot().environments;
                    environments.set(next);
                }
            }
        });
        let rules = computed(move || {
            catalogue.with(|catalogue| {
                environments.with(|environments| {
                    let sources = catalogue
                        .iter()
                        .map(|entry| ShortcutSource {
                            environment: &entry.id,
                            connected: entry.state.phase == ConnectionPhase::Connected,
                            keybindings: environments
                                .get(&entry.id)
                                .and_then(|state: &Rc<EnvironmentState>| state.config.as_ref())
                                .map(|config| config.keybindings.0.as_slice()),
                        })
                        .collect::<Vec<_>>();
                    effective_config(primary.get().as_ref(), &sources)
                })
            })
        });
        Self {
            context: KeyContext::default(),
            primary,
            rules,
            scopes: stored(BTreeMap::new()),
            next_scope: stored(0),
            revision: signal(0),
            menus: stored(BTreeMap::new()),
            dispatcher: stored(None),
        }
    }

    pub fn register(self, scope: Scope) {
        let id = self.next_scope.with(|id| {
            *id += 1;
            *id
        });
        self.scopes.with(|scopes| {
            scopes.insert(id, Rc::new(scope));
        });
        self.revision.update(|revision| *revision += 1);
        cleanup(move || {
            self.scopes.with(|scopes| {
                scopes.remove(&id);
            });
            self.revision.update(|revision| *revision += 1);
        });
    }

    fn dispatcher(self) -> Rc<Dispatcher<'static>> {
        let revision = self.revision.get();
        if let Some((_, dispatcher)) = self
            .dispatcher
            .with(|cached| cached.as_ref().filter(|(at, _)| *at == revision).cloned())
        {
            return dispatcher;
        }
        let scopes = self
            .scopes
            .with(|scopes| scopes.values().cloned().collect::<Vec<_>>());
        let mut dispatcher = Dispatcher::default();
        for scope in scopes {
            dispatcher.register(scope.client());
        }
        for (action, handler) in self
            .menus
            .with(|menus| menus.values().cloned().collect::<Vec<_>>())
        {
            dispatcher.register_menu(action, Box::new(move || handler()));
        }
        let dispatcher = Rc::new(dispatcher);
        let _ = self
            .dispatcher
            .replace(Some((revision, dispatcher.clone())));
        dispatcher
    }

    pub fn register_menu(self, action: &str, handler: impl Fn() + 'static) {
        let id = self.next_scope.with(|id| {
            *id += 1;
            *id
        });
        self.menus.with(|menus| {
            menus.insert(id, (action.into(), Rc::new(handler)));
        });
        self.revision.update(|revision| *revision += 1);
        cleanup(move || {
            self.menus.with(|menus| {
                menus.remove(&id);
            });
            self.revision.update(|revision| *revision += 1);
        });
    }

    pub fn dispatch_menu(self, action: &str) {
        self.dispatcher().dispatch_menu(action);
    }

    pub fn dispatch(self, event: &mut DispatchEvent) -> bool {
        let dispatcher = self.dispatcher();
        let context = self.context.read();
        if context.get("keybindingCapture") {
            return false;
        }
        let platform = shortcut_platform();
        self.rules.with(|rules| {
            let capture = dispatcher.dispatch(Phase::Capture, event, rules, platform, &context);
            if capture {
                event.default_prevented = true;
            }
            dispatcher.dispatch(Phase::Bubble, event, rules, platform, &context) || capture
        })
    }

    pub fn shortcut_label(self, command: &str) -> Option<String> {
        self.rules.with(|rules| {
            effective_shortcuts_for_command(
                rules,
                command,
                shortcut_platform(),
                &self.context.read(),
            )
            .into_iter()
            .find(|shortcut| native_stroke(shortcut).is_some())
            .map(|shortcut| keybindings::format_shortcut_label(&shortcut, shortcut_platform()))
        })
    }

    fn strokes(self) -> Vec<String> {
        self.revision.get();
        let context = self.context.read();
        if context.get("keybindingCapture") {
            return Vec::new();
        }
        let rules = self.rules.get();
        let platform = shortcut_platform();
        let scopes = self
            .scopes
            .with(|scopes| scopes.values().cloned().collect::<Vec<_>>());
        let mut strokes = BTreeSet::new();
        for scope in scopes.iter().filter(|scope| scope.observe.is_none()) {
            let mut context = context.clone();
            context.0.extend((scope.context)().0);
            let scoped_rules = scope.keybindings.as_ref().map(|read| read());
            let rules = scoped_rules.as_deref().unwrap_or(&rules);
            for shortcut in &scope.raw_keys {
                if (scope.when)(&event_for_shortcut(shortcut, &context))
                    && let Some(stroke) = native_stroke(shortcut)
                {
                    strokes.insert(stroke);
                }
            }
            for command in scope.commands.keys() {
                for shortcut in effective_shortcuts_for_command(rules, command, platform, &context)
                {
                    let event = event_for_shortcut(&shortcut, &context);
                    if !(scope.when)(&event) {
                        continue;
                    }
                    if let Some(stroke) = native_stroke(&shortcut) {
                        strokes.insert(stroke);
                    }
                }
            }
        }
        strokes.into_iter().collect()
    }
}

/// The platform service knows the browser host's keyboard convention as well as native hosts'.
fn shortcut_platform() -> &'static str {
    if apple_keys() { "MacIntel" } else { "Linux" }
}

fn event_for_shortcut(shortcut: &KeybindingShortcut, context: &MatchContext) -> DispatchEvent {
    DispatchEvent {
        key: KeyEvent {
            key: match shortcut.key.as_str() {
                "escape" => "Escape",
                "enter" => "Enter",
                "tab" => "Tab",
                "arrowup" => "ArrowUp",
                "arrowdown" => "ArrowDown",
                "arrowleft" => "ArrowLeft",
                "arrowright" => "ArrowRight",
                key => key,
            }
            .into(),
            modifiers: keybindings::shortcut_modifiers(shortcut, shortcut_platform()),
            editable_focus: context.get("editableFocus"),
            ..Default::default()
        },
        default_prevented: false,
        propagation_stopped: false,
        inside_confirmation: crate::ui::confirm::is_confirm_dialog_active(),
    }
}

fn native_stroke(shortcut: &KeybindingShortcut) -> Option<String> {
    // NSH's "plus" means the physical '=' key; TS's '+' is a distinct logical shortcut.
    if shortcut.key == "+" {
        return None;
    }
    let modifiers = keybindings::shortcut_modifiers(shortcut, shortcut_platform());
    let mut parts = Vec::new();
    if modifiers.ctrl {
        parts.push("ctrl");
    }
    if modifiers.meta {
        parts.push("cmd");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    parts.push(match shortcut.key.as_str() {
        " " => "space",
        key => key,
    });
    let stroke = parts.join("+");
    let parsed = Keystroke::parse_with(&stroke, apple_keys()).ok()?;
    (!crate::host::reserved_stroke(&parsed)).then_some(stroke)
}

pub fn client_event(event: &nsh_core::KeyEvent, down: bool, editable_focus: bool) -> KeyEvent {
    KeyEvent {
        key: event.key.to_string(),
        code: event.code.to_string(),
        modifiers: Modifiers {
            meta: event.modifiers.cmd,
            ctrl: event.modifiers.ctrl,
            alt: event.modifiers.alt,
            shift: event.modifiers.shift,
        },
        event_type: Some(if down { "keydown" } else { "keyup" }.into()),
        repeat: event.repeat,
        editable_focus,
        alt_graph: false,
    }
}

/// Mount once beside the root content; changes replace only bindings, never the workspace tree.
pub fn bindings_host(keys: Keybindings) -> impl IntoElement {
    let strokes = computed(move || keys.strokes());
    each(
        move || strokes.get(),
        |stroke| stroke.clone(),
        move |stroke, _| {
            let stroke = stroke.get_untracked();
            let shortcut = keybindings::parse_shortcut(&stroke);
            if let Some(shortcut) = shortcut {
                // NSH's callback carries only the chord, so repeats cannot be reported faithfully.
                on_key_with(
                    &stroke,
                    KeyOptions {
                        when: nsh_core::keymap::When::Always,
                        ..Default::default()
                    },
                    move |_| {
                        let mut event = event_for_shortcut(&shortcut, &keys.context.read());
                        keys.dispatch(&mut event);
                    },
                );
            }
            view().w(px(0.0)).h(px(0.0)).accessible(false)
        },
    )
}

/// Wrap the root so unbound keys reach observers and the shared modifier stores.
pub fn keyboard_events(keys: Keybindings, children: impl IntoElement) -> impl IntoElement {
    current_window().on_focus_change(move |focused| {
        if !focused {
            keys.context.reset_modifiers();
        }
    });
    view()
        .flex_1()
        .w_full()
        .h_full()
        .on_key_down(move |raw: &nsh_core::KeyEvent| {
            keys.context.keyboard(raw, true);
            let mut event = DispatchEvent {
                key: client_event(raw, true, keys.context.read().get("editableFocus")),
                default_prevented: raw.default_prevented,
                propagation_stopped: false,
                inside_confirmation: crate::ui::confirm::is_confirm_dialog_active(),
            };
            keys.dispatch(&mut event);
        })
        .on_key_up(move |raw: &nsh_core::KeyEvent| keys.context.keyboard(raw, false))
        .child(children)
        .child(bindings_host(keys))
}

pub fn keybindings() -> Keybindings {
    use_context::<Keybindings>().expect("App installs keybindings")
}

/// Root installation; the integrator supplies the catalogue's designated primary, not the route.
pub fn install(panels: Panels, primary: Signal<Option<EnvironmentId>>) -> Keybindings {
    let runtime = rt();
    let nav = navigator();
    let keys = Keybindings::attach(
        runtime.store.store(),
        runtime.environments_signal(),
        primary,
    );
    provide_context(keys);
    install_shell_commands(keys, panels, nav);
    keys
}

pub fn install_shell_commands(keys: Keybindings, panels: Panels, nav: Navigator) {
    keys.register(
        Scope {
            phase: Phase::Capture,
            when: Rc::new(move |event| {
                !nav.top().is_standalone()
                    && !(keys.context.read().get("composerRichTextFocus")
                        && keybindings::is_rich_text_bold_shortcut(
                            &event.key,
                            keys.context.selection_collapsed.get(),
                        ))
            }),
            ..Default::default()
        }
        .command("sidebar.toggle", move |event| {
            event.claim();
            panels.sidebar_open.update(|open| *open = !*open);
            true
        }),
    );
    keys.register_menu("sidebar.toggle", move || {
        if !nav.top().is_standalone() {
            panels.sidebar_open.update(|open| *open = !*open);
        }
    });
}

/// Port of chatRouteCommands.ts; destination controllers execute the client's route decisions.
pub fn install_chat_route_commands(
    keys: Keybindings,
    context: impl Fn() -> ChatRouteContext + 'static,
    action: impl Fn(RouteAction) + 'static,
) {
    let context = Rc::new(context);
    let action = Rc::new(action);
    let enabled = context.clone();
    let escape_context = context.clone();
    let escape_action = action.clone();
    let mut scope = Scope {
        when: Rc::new(move |event| {
            shortcuts::chat_route_enabled(&enabled())
                && (event.key.key != "Escape" || enabled().selection_present)
        }),
        raw_keys: keybindings::parse_shortcut("escape").into_iter().collect(),
        on_keydown: Some(Rc::new(move |event| {
            let Some(action) = shortcuts::chat_route_escape(&event.key.key, &escape_context())
            else {
                return false;
            };
            event.default_prevented = true;
            escape_action(action);
            true
        })),
        ..Default::default()
    };
    for command in [
        "chat.new",
        "chat.newLocal",
        "chat.newWithoutProject",
        "preview.toggle",
        "preview.refresh",
        "preview.focusUrl",
        "preview.zoomIn",
        "preview.zoomOut",
        "preview.resetZoom",
        "preview.find",
    ] {
        let context = context.clone();
        let action = action.clone();
        scope = scope.command(command, move |event| {
            let Some(result) = shortcuts::chat_route_command(command, &context()) else {
                return false;
            };
            event.claim();
            if let Some(result) = result {
                action(result);
            }
            true
        });
    }
    keys.register(scope);
}
