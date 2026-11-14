//! Port of apps/web/src/components/CommandPalette.tsx CommandPalette and CommandPaletteDialog.

// Declared by the integrator (ui-packages.py); the owner edits below this block.
pub mod files;
pub mod results;
// End of declarations.

use std::{collections::BTreeMap, rc::Rc};

use nsh_core::{keymap::When, prelude::*};
use t3_client::view_logic::shell::palette::{
    self as client, Group, MountState, SearchAction, SearchState, View,
};
pub use t3_client::view_logic::shell::palette::{
    Intent, OverlayMode, ReturnContext, UiAction, UiState,
};
use t3_client::view_logic::shell::shortcuts::DispatchEvent;

use crate::i18n::{Text, t};
use crate::theme::{cn, palette, tw};
use crate::ui::command::{self, CommandOptions, CommandState, InputOptions};
use crate::ui::icon::{Icon, icon};

#[derive(Clone, Default)]
struct Presence(usize);

/// Capture this surface-local handle in handlers, including SH-KEYS' command handlers.
#[derive(Clone, Copy)]
pub struct Palette {
    store: Signal<MountState>,
    presence: Signal<Presence>,
}

pub type PaletteCommandHandler = Rc<dyn Fn(&mut DispatchEvent) -> bool>;

pub fn palette_controller() -> Palette {
    Palette {
        store: global(),
        presence: global(),
    }
}

impl Palette {
    pub fn state(self) -> UiState {
        self.store.with(|store| store.state.clone())
    }

    pub fn is_mounted(self) -> bool {
        self.store.with(|store| store.mounted > 0)
    }

    /// Includes the command primitive's retained exit, as commandPaletteBus.ts does.
    pub fn is_open(self) -> bool {
        self.presence.with(|presence| presence.0 > 0)
    }

    pub fn dispatch(self, action: UiAction) {
        if self.is_mounted() {
            self.store.update(|store| store.state.apply(action));
        }
    }

    pub fn close(self) {
        self.dispatch(UiAction::SetOpen(false));
    }

    pub fn escape(self) {
        if self.state().mode == OverlayMode::Command {
            self.close();
        } else {
            self.dispatch(UiAction::BackToCommand);
        }
    }

    pub fn open_child_search(self, mode: OverlayMode, context: ReturnContext) {
        if mode != OverlayMode::Command {
            self.dispatch(UiAction::OpenChild { mode, context });
        }
    }

    /// Port of shortcuts/commandPaletteCommands.ts's overlay and theme-picker handlers.
    /// SH-KEYS supplies effective bindings; the integrator installs these in its mounted Scope.
    pub fn command_handlers(self) -> BTreeMap<String, PaletteCommandHandler> {
        [
            "commandPalette.toggle",
            "filePicker.toggle",
            "projectSearch.toggle",
            "theme.select",
        ]
        .into_iter()
        .map(|command| {
            let handler: PaletteCommandHandler = Rc::new(move |event| {
                if !self.is_mounted() {
                    return false;
                }
                event.claim();
                if let Some(mode) = client::overlay_mode(Some(command)) {
                    self.dispatch(UiAction::ToggleMode(mode));
                } else if !event.key.repeat {
                    self.dispatch(UiAction::OpenIntent(Intent::ChangeTheme));
                }
                true
            });
            (command.into(), handler)
        })
        .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OpenPage {
    AddProject,
    NewThreadIn,
}

/// Port of commandPaletteBus.ts CommandPaletteOpenDetail; linked rows stay client-owned.
#[derive(Default)]
pub struct OpenDetail {
    pub open: Option<OpenPage>,
    pub query: Option<String>,
    pub linked_threads: Option<Vec<t3_client::types::ThreadKey>>,
}

pub fn open_command_palette(detail: OpenDetail) {
    let action = match (detail.open, detail.query) {
        (Some(OpenPage::AddProject), _) => UiAction::OpenIntent(Intent::AddProject),
        (Some(OpenPage::NewThreadIn), _) => UiAction::OpenIntent(Intent::NewThreadIn),
        (None, Some(query)) => UiAction::OpenIntent(Intent::Search {
            query,
            linked_threads: detail.linked_threads,
        }),
        (None, None) => UiAction::SetOpen(true),
    };
    palette_controller().dispatch(action);
}

pub fn is_command_palette_mounted() -> bool {
    palette_controller().is_mounted()
}

pub fn is_command_palette_open() -> bool {
    palette_controller().is_open()
}

/// Mount only around the workspace shell, never around standalone/pairing routes.
pub fn command_palette(children: impl IntoElement) -> impl IntoElement {
    let palette = palette_controller();
    palette.store.update(MountState::mount);
    cleanup(move || {
        if let Some(mut store) = palette.store.try_get() {
            store.unmount();
            let _ = palette.store.try_set(store);
        }
    });
    let open = signal(untracked(|| palette.state().open));
    // The primitive owns native outside/system dismissal; its signal feeds the client reducer.
    effect(move || {
        let requested = palette.state().open;
        if open.get_untracked() != requested {
            open.set(requested);
        }
    });
    effect(move || {
        let open = open.get();
        untracked(|| {
            if palette.state().open != open {
                palette.close();
            }
        });
    });
    // NSH's modal owns trapping/inertness; a transparent region replaces HTML `contents`.
    let contents = stored(Some(vec![
        children.into_element(),
        command::command_dialog(open, "Command palette", move || {
            untracked(|| command_palette_dialog(palette))
        })
        .into_element(),
    ]));
    dynamic(move || {
        untracked(|| {
            contents
                .with(|contents| contents.take())
                .unwrap_or_default()
        })
    })
}

fn command_palette_dialog(palette: Palette) -> impl IntoElement {
    palette.presence.update(|presence| presence.0 += 1);
    cleanup(move || {
        if let Some(presence) = palette.presence.try_get() {
            let _ = palette
                .presence
                .try_set(Presence(presence.0.saturating_sub(1)));
        }
    });
    let mode_session = computed(move || {
        let state = palette.state();
        (state.mode, state.session)
    });
    // Child searches may have no input yet; Escape still returns to the parent command mode.
    effect(move || {
        let state = palette.state();
        if state.open && state.mode != OverlayMode::Command {
            on_key_with(
                "escape",
                KeyOptions {
                    when: When::Always,
                    ..Default::default()
                },
                move |_| palette.escape(),
            );
        }
    });
    command::command_dialog_popup(
        // The opaque popup has no class slot; this layout child carries the feature classes.
        view()
            .test_id("command-palette")
            .app_class(move || {
                tw(&cn([
                    Some("overflow-hidden"),
                    (mode_session.get().0 == OverlayMode::Content).then_some("h-105"),
                ]))
            })
            .child(dynamic(move || {
                let (mode, _) = mode_session.get();
                Some(untracked(|| match mode {
                    OverlayMode::Command => open_command_palette_dialog(palette).into_element(),
                    OverlayMode::Files => files::project_file_picker().into_element(),
                    OverlayMode::Content => files::project_content_search_dialog().into_element(),
                }))
            })),
        move || palette.close(),
    )
}

/// The client search stack plus the native input's current query, scoped to one palette session.
#[derive(Clone, Copy)]
pub struct PaletteSession {
    pub query: Signal<String>,
    search: Stored<SearchState>,
    revision: Signal<u64>,
    linked_search: Stored<Option<Intent>>,
}

impl PaletteSession {
    pub fn new(
        query: Signal<String>,
        intent: Option<Intent>,
        restored: Option<ReturnContext>,
    ) -> Self {
        let initial = restored.clone().unwrap_or_else(|| match &intent {
            Some(intent @ Intent::Search { query, .. }) => ReturnContext {
                query: query.clone(),
                linked_search: Some(intent.clone()),
            },
            _ => ReturnContext {
                query: String::new(),
                linked_search: None,
            },
        });
        let mut search = SearchState {
            query: initial.query,
            ..Default::default()
        };
        if restored.is_none()
            && let Some(view) = open_intent_view(intent.as_ref())
        {
            search.apply(SearchAction::Push(view));
        }
        query.set(search.query.clone());
        Self {
            query,
            search: stored(search),
            revision: signal(0),
            linked_search: stored(initial.linked_search),
        }
    }

    pub fn current_view(self) -> Option<View> {
        self.revision.get();
        self.search
            .with(|search| search.views.last().map(|(view, _)| view.clone()))
    }

    pub fn is_submenu(self) -> bool {
        self.revision.get();
        self.search.with(|search| !search.views.is_empty())
    }

    pub fn apply(self, action: SearchAction) {
        self.search.with(|search| {
            search.apply(SearchAction::Query(self.query.get_untracked()));
            search.apply(action);
            self.query.set(search.query.clone());
        });
        self.revision.update(|revision| *revision += 1);
    }

    pub fn return_context(self) -> ReturnContext {
        ReturnContext {
            query: self.query.get(),
            linked_search: self.linked_search.get(),
        }
    }
}

fn open_intent_view(intent: Option<&Intent>) -> Option<View> {
    let (value, label) = match intent? {
        Intent::AddProject => ("add-project", ""),
        Intent::NewThreadIn => ("projects", "Projects"),
        Intent::ChangeTheme => ("themes", "Change theme"),
        Intent::Search { .. } => return None,
    };
    Some(View {
        groups: vec![Group {
            value: value.into(),
            label: label.into(),
            items: Vec::new(),
        }],
        initial_query: None,
    })
}

fn open_command_palette_dialog(palette: Palette) -> impl IntoElement {
    let initial = palette.state();
    // SH-PALETTE-2 supplies the client-ranked catalogue and execution; no UI-invented commands.
    let state = CommandState::new(
        Vec::new,
        CommandOptions {
            filter: false,
            ..Default::default()
        },
        |_| {},
    );
    let session = PaletteSession::new(state.query, initial.intent, initial.restored_search);
    // View depth changes remount the input, as CommandPalette.tsx's keyed content does.
    let depth = computed(move || {
        session.revision.get();
        session.search.with(|search| search.views.len())
    });
    dynamic(move || {
        depth.get();
        Some(untracked(move || {
            let submenu = session.is_submenu();
            let mut input = InputOptions::new(
                "Command palette",
                client::placeholder(client::mode(submenu, false)),
            );
            input.on_cancel = Some(Rc::new(move || palette.close()));
            if submenu {
                input.start_addon = Some(
                    nsh_base::button()
                        .accessibility_label(move || Some(SharedString::from(t("Back"))))
                        .disabled(move || !palette.state().open)
                        .on_press(move || session.apply(SearchAction::Back))
                        .app_class(tw("flex cursor-pointer items-center"))
                        // autocomplete.tsx's addon svg selectors must reach the icon directly.
                        .child(
                            image(Icon::ArrowLeft.asset())
                                .app_class(tw("-mx-0.5 size-4.5 sm:size-4"))
                                .tint_color(|| crate::theme::palette().foreground)
                                .accessible(false),
                        )
                        .into_element(),
                );
            }
            effect(move || {
                if palette.state().open
                    && session.is_submenu()
                    && state.query.with(|query| query.is_empty())
                {
                    on_key_with(
                        "backspace",
                        KeyOptions {
                            when: When::Always,
                            ..Default::default()
                        },
                        move |_| session.apply(SearchAction::Back),
                    );
                }
            });
            command_palette_content(
                state,
                ContentOptions {
                    input,
                    show_back_hint: submenu,
                    footer_action_label: Some("Select".into()),
                    ..Default::default()
                },
                view(),
            )
        }))
    })
}

#[derive(Clone, Copy, Default)]
pub enum PanelSize {
    #[default]
    List,
    TallList,
    Fill,
}

pub struct ContentOptions {
    pub input: InputOptions,
    pub escape_label: Text,
    pub footer_action_label: Option<Text>,
    pub footer_trailing: Option<AnyElement>,
    pub input_accessory: Option<AnyElement>,
    pub panel_size: PanelSize,
    pub show_back_hint: bool,
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            input: InputOptions::new(
                "Command palette",
                "Search commands, projects, and threads...",
            ),
            escape_label: "Close".into(),
            footer_action_label: None,
            footer_trailing: None,
            input_accessory: None,
            panel_size: PanelSize::List,
            show_back_hint: false,
        }
    }
}

/// Port of apps/web/src/components/CommandPaletteContent.tsx CommandPaletteContent.
pub fn command_palette_content(
    state: CommandState,
    options: ContentOptions,
    children: impl IntoElement + 'static,
) -> impl IntoElement {
    let ContentOptions {
        mut input,
        escape_label,
        footer_action_label,
        footer_trailing,
        input_accessory,
        panel_size,
        show_back_hint,
    } = options;
    input.auto_focus = false;
    let claimed_focus = stored(false);
    command::command(state, move |state| {
        let input = view()
            .app_class(tw("relative"))
            .on_layout(move |_: &Bounds<Points>| {
                if !claimed_focus.get() {
                    claimed_focus.with(|claimed| *claimed = true);
                    state.focus();
                }
            })
            .child(command::command_input(state, input))
            .child(input_accessory);
        let panel_layout = match panel_size {
            PanelSize::List => command::PanelLayout::List,
            PanelSize::TallList => command::PanelLayout::TallList,
            PanelSize::Fill => command::PanelLayout::Fill,
        };
        let footer = command::command_footer(
            // Footer's responsive layout needs a child because primitives expose no class slot.
            view().app_class(tw("flex w-full items-center justify-between gap-2 max-sm:flex-col max-sm:items-start"))
                .child(view().app_class(tw("flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5"))
                    .child(view().app_class(tw("inline-flex items-center gap-1"))
                        .child(kbd(icon(Icon::ArrowUp, 12.0, || palette().foreground)))
                        .child(kbd(icon(Icon::ArrowDown, 12.0, || palette().foreground)))
                        .child(text(move || SharedString::from(t("Navigate")))))
                    .child(footer_action_label.map(|label| kbd_group(kbd(text(move || SharedString::from(t("Enter")))), label)))
                    .child(show_back_hint.then(|| kbd_group(kbd(text(move || SharedString::from(t("Backspace")))), "Back".into())))
                    .child(kbd_group(kbd(text(move || SharedString::from(t("Esc")))), escape_label)))
                .child(footer_trailing));
        vec![
            input.into_element(),
            command::command_panel_with_layout(true, panel_layout, children).into_element(),
            footer.into_element(),
        ]
    })
}

fn kbd_group(keys: impl IntoElement, label: Text) -> impl IntoElement {
    view()
        .app_class(tw("inline-flex items-center gap-1"))
        .child(keys)
        .child(text(move || label.resolve()))
}

fn kbd(children: impl IntoElement) -> impl IntoElement {
    // ui/kbd.tsx Kbd plus CommandFooter's descendant overrides; svg sizing is on each icon.
    view().app_class(tw("pointer-events-none inline-flex h-5 min-w-5 select-none items-center justify-center gap-1 rounded px-1 font-medium font-sans text-xs bg-foreground/8 text-foreground ring-0"))
        .child(children)
}
