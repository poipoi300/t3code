//! Port of apps/web/src/components/chat/ComposerCommandMenu.tsx ComposerCommandMenu.

use std::rc::Rc;

use nsh_base::rich_editor::state::KeyEvent;
use nsh_core::{AccessibilityState, element::text_measurer, prelude::*};
use t3_client::thread_views::composer::policy::{self, SubmissionIntent, Trigger, TriggerKind};

use super::editor::{ComposerEditor, ComposerSnapshot, EditorMode};
use crate::i18n::{Text, t, tr};
use crate::theme::{cn, tw};
use crate::ui::alert::composer_banner;
use crate::ui::badge::{self, Child, Props, Variant};
use crate::ui::icon::{Icon, icon_with_class, provider_icon};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Presentation only. The client retains the operation payload indexed by `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerCommandItem {
    pub id: String,
    pub label: Text,
    pub description: Text,
    pub kind: ComposerCommandItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerCommandItemKind {
    Path {
        directory: bool,
    },
    SlashCommand,
    ProviderSlashCommand,
    Skill {
        source: SkillSourceKind,
        display_name: String,
    },
    Thread,
    Model {
        driver: String,
        provider_name: String,
    },
}

/// Already resolved by the client's provider-skill projection; never inferred from a path here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSourceKind {
    App,
    Repo,
    Project,
    Personal,
    System,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerSuggestionResults {
    pub request: ComposerSuggestionRequest,
    pub items: Vec<ComposerCommandItem>,
    pub loading: bool,
    pub empty_state: Option<Text>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerSuggestionRequest {
    pub trigger: Trigger,
    pub generation: u64,
}

type TriggerHandler = Rc<dyn Fn(Option<ComposerSuggestionRequest>)>;
type SelectHandler = Rc<dyn Fn(&str, &Trigger, &ComposerSnapshot) -> bool>;

/// Port of chat/ChatComposer.tsx composerMenuSearchKey (highlight identity only).
fn same_menu_search(left: &Trigger, right: &Trigger) -> bool {
    policy::composer_menu_search_key(left.kind, &left.query)
        == policy::composer_menu_search_key(right.kind, &right.query)
}

/// Port of chat/useComposerTriggerState.ts and ChatComposer.tsx's controlled menu state.
/// Querying, result ordering and selection operations are client calls.
#[derive(Clone, Copy)]
pub struct ComposerSuggestions {
    editor: Stored<ComposerEditor>,
    pub trigger: Memo<Option<Trigger>>,
    pub request: Memo<Option<ComposerSuggestionRequest>>,
    generation: Signal<u64>,
    scope: Stored<String>,
    candidate: Signal<Option<Trigger>>,
    dismissed: Signal<Option<Trigger>>,
    pub results: Signal<Option<ComposerSuggestionResults>>,
    menu_items: Memo<Rc<Vec<ComposerCommandItem>>>,
    active: Memo<Option<String>>,
    highlighted: Signal<Option<(Trigger, String)>>,
    on_trigger: Stored<TriggerHandler>,
    on_select: Stored<SelectHandler>,
    pub enabled: Signal<bool>,
}

impl ComposerSuggestions {
    pub fn new(
        editor: ComposerEditor,
        on_trigger: impl Fn(Option<ComposerSuggestionRequest>) + 'static,
        on_select: impl Fn(&str, &Trigger, &ComposerSnapshot) -> bool + 'static,
    ) -> Self {
        let candidate = signal(None::<Trigger>);
        let dismissed = signal(None::<Trigger>);
        let enabled = signal(true);
        let generation = signal(0);
        let trigger = computed(move || {
            let candidate = candidate.get()?;
            (enabled.get()
                && !dismissed.with(|dismissed| {
                    dismissed.as_ref().is_some_and(|dismissed| {
                        dismissed.kind == candidate.kind
                            && dismissed.range.anchor == candidate.range.anchor
                    })
                }))
            .then_some(candidate)
        });
        let request = computed(move || {
            trigger.get().map(|trigger| ComposerSuggestionRequest {
                trigger,
                generation: generation.get(),
            })
        });
        let results = signal(None::<ComposerSuggestionResults>);
        let highlighted = signal(None::<(Trigger, String)>);
        let menu_items = computed(move || {
            let request = request.get();
            Rc::new(results.with(|results| {
                results
                    .as_ref()
                    .filter(|r| Some(&r.request) == request.as_ref())
                    .map_or_else(Vec::new, |r| r.items.clone())
            }))
        });
        let active = computed(move || {
            let items = menu_items.get();
            let trigger = trigger.get();
            highlighted
                .get()
                .filter(|(search, id)| {
                    trigger
                        .as_ref()
                        .is_some_and(|trigger| same_menu_search(search, trigger))
                        && items.iter().any(|i| &i.id == id)
                })
                .map(|(_, id)| id)
                .or_else(|| items.first().map(|i| i.id.clone()))
        });
        let state = Self {
            editor: stored(editor),
            trigger,
            request,
            generation,
            scope: stored(String::new()),
            candidate,
            dismissed,
            results,
            menu_items,
            active,
            highlighted,
            on_trigger: stored(Rc::new(on_trigger)),
            on_select: stored(Rc::new(on_select)),
            enabled,
        };
        effect(move || {
            let request = state.request.get();
            untracked(|| {
                state.editor.get().set_path_query_active(
                    request
                        .as_ref()
                        .is_some_and(|r| r.trigger.kind == TriggerKind::Path),
                );
                state.on_trigger.get()(request);
            });
        });
        let editor = state.editor.get();
        let on_trigger = state.on_trigger.get();
        cleanup(move || {
            editor.set_path_query_active(false);
            on_trigger(None);
        });
        state.observe();
        state
    }

    /// The host calls this with the draft/environment/provider identity before syncing its source.
    pub fn set_scope(self, scope: String) {
        if self.scope.get() == scope {
            return;
        }
        self.scope.with(|value| *value = scope);
        batch(|| {
            self.generation.update(|g| *g += 1);
            self.results.set(None);
            self.dismissed.set(None);
            self.highlighted.set(None);
        });
        self.observe();
    }

    /// Call from the editor's source/selection notification and after a controlled draft change.
    pub fn observe(self) {
        let editor = self.editor.get();
        let snapshot = editor.read_snapshot();
        let candidate = (editor.mode() != EditorMode::Literal
            && !snapshot.cursor_adjacent_to_mention)
            .then(|| policy::detect_trigger(&snapshot.value, snapshot.expanded_cursor))
            .flatten();
        let suppressed = self.dismissed.with_untracked(|dismissed| {
            candidate
                .as_ref()
                .zip(dismissed.as_ref())
                .is_some_and(|(a, b)| a.kind == b.kind && a.range.anchor == b.range.anchor)
        });
        batch(|| {
            if candidate.is_none() || !suppressed {
                self.dismissed.set(None);
            }
            let previous = self.candidate.get_untracked();
            if previous != candidate {
                if !previous
                    .as_ref()
                    .zip(candidate.as_ref())
                    .is_some_and(|(previous, candidate)| same_menu_search(previous, candidate))
                {
                    self.highlighted.set(None);
                }
                self.generation.update(|g| *g += 1);
                self.candidate.set(candidate);
            }
        });
    }

    pub fn dismiss(self) {
        self.dismissed.set(self.candidate.get_untracked());
        self.highlighted.set(None);
    }

    /// Late responses never replace another query's rows or become selectable in another draft.
    pub fn publish(self, results: ComposerSuggestionResults) -> bool {
        if self.request.get_untracked().as_ref() != Some(&results.request) {
            return false;
        }
        if results.items.is_empty() {
            self.highlighted.set(None);
        }
        self.results.set(Some(results));
        true
    }

    fn items(self) -> Rc<Vec<ComposerCommandItem>> {
        self.menu_items.get()
    }

    pub fn active_item_id(self) -> Option<String> {
        self.active.get()
    }

    pub fn highlight(self, id: &str) {
        if let Some(trigger) = self.trigger.get_untracked()
            && self.items().iter().any(|item| item.id == id)
        {
            self.highlighted.set(Some((trigger, id.to_owned())));
        }
    }

    pub fn choose(self, id: &str) -> bool {
        self.observe();
        let Some(trigger) = self.trigger.get_untracked() else {
            return false;
        };
        if !self.items().iter().any(|item| item.id == id) {
            return false;
        }
        let editor = self.editor.get();
        if !editor.session().is_editable() || editor.session().composing() {
            return false;
        }
        let accepted = self.on_select.get()(id, &trigger, &editor.read_snapshot());
        if accepted {
            self.highlighted.set(None);
            self.observe();
        }
        accepted
    }

    /// `composerCommandKey.ts`: invoke after Shift+Tab outdent, before submit/history/list keys.
    pub fn handle_key(self, event: &KeyEvent, intent: Option<SubmissionIntent>) -> bool {
        let editor = self.editor.get();
        if !editor.session().is_editable() {
            return false;
        }
        // NSH's input controller holds all native keys during pre-edit, before plugin dispatch.
        if editor.session().composing() {
            return event.key == "Enter";
        }
        self.observe();
        if self.trigger.get_untracked().is_none() {
            return false;
        }
        if event.key == "Escape" {
            self.dismiss();
            return true;
        }
        if event.key == "Tab" && event.shift
            || matches!(
                intent,
                Some(SubmissionIntent::Background | SubmissionIntent::Alternate)
            )
        {
            return false;
        }
        let items = self.items();
        if items.is_empty() {
            return false;
        }
        match event.key.as_str() {
            "ArrowDown" | "ArrowUp" => {
                let current = self.active_item_id();
                let at = current
                    .as_ref()
                    .and_then(|id| items.iter().position(|i| &i.id == id));
                let next = match (at, event.key == "ArrowDown") {
                    (None, true) => 0,
                    (None, false) => items.len() - 1,
                    (Some(at), true) => (at + 1) % items.len(),
                    (Some(at), false) => (at + items.len() - 1) % items.len(),
                };
                self.highlight(&items[next].id);
                true
            }
            "Enter" | "Tab" => {
                if let Some(id) = self.active_item_id() {
                    self.choose(&id);
                }
                true
            }
            _ => false,
        }
    }
}

pub fn composer_suggestion_option_id(list_id: &str, item_id: &str) -> String {
    let json = serde_json::to_string(item_id).expect("string serialization is infallible");
    let mut encoded = String::new();
    for byte in json.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("{list_id}-{encoded}")
}

pub fn composer_command_menu(state: ComposerSuggestions, list_id: String) -> impl IntoElement {
    when(
        move || state.trigger.get().is_some(),
        move || {
            untracked(|| {
                let list_id = list_id.clone();
                composer_banner::surface(
                    Default::default(),
                    view()
                        .test_id("composer-command-menu")
                        // Slot selectors move to rows; native scrollbars expose no CSS my-4 margin.
                        .app_class(tw("flex min-h-0 w-full flex-col overflow-hidden pb-[17px]"))
                        .child(dynamic(move || {
                            let items = state.items();
                            if items.is_empty() {
                                return view()
                                    .app_class(tw("px-5 pt-3.5 pb-7"))
                                    .child(
                                        text(move || empty_state(state))
                                            .app_class(tw("text-secondary-label text-xs")),
                                    )
                                    .into_element();
                            }
                            let list = element_ref();
                            scroll()
                                .element_ref(&list)
                                .test_id(&list_id)
                                .role(AccessibilityRole::Listbox)
                                .label(move || {
                                    listbox_label(state.trigger.get().as_ref().map(|t| t.kind))
                                })
                                // Native intrinsic scrolling replaces size-full inside this capped drawer.
                                .app_class(tw("relative max-h-72 min-h-0 p-2 outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"))
                                .child(
                                    view().role(AccessibilityRole::Group).children(
                                        items
                                            .iter()
                                            .cloned()
                                            .map(|item| {
                                                composer_command_menu_item(state, &list_id, item)
                                                    .into_element()
                                            })
                                            .collect::<Vec<_>>(),
                                    ),
                                )
                                .into_element()
                        })),
                )
            })
        },
    )
}

/// Port of chat/ComposerCommandMenuLayer.tsx, anchored to ComposerSurface.Main.
/// `anchor_width` is fed by that surface's native on_layout, in points.
pub fn composer_command_menu_layer(
    state: ComposerSuggestions,
    anchor: ElementRef,
    anchor_width: Memo<f32>,
    list_id: String,
) -> impl IntoElement {
    when(
        move || state.trigger.get().is_some() && anchor_width.get() > 44.0,
        move || {
            untracked(|| {
                let available = signal(288.0_f32 + 17.0);
                // Native anchoring follows ancestor scrolling/resizes; it also flips when space runs out.
                // The web instead fixes top placement. NSH exposes no disable-flip policy.
                overlay()
                    .anchor(&anchor)
                    .placement(Placement::Top)
                    .align(Align::Center)
                    .offset(px(-17.0))
                    .margin(px(24.0))
                    .w(move || px(anchor_width.get() - 44.0))
                    .max_h(move || px(available.get().max(96.0)))
                    .on_position(move |position: &nsh_core::element::OverlayPosition| {
                        available.set(position.available.height.0);
                    })
                    .app_class(tw("pointer-events-auto fixed z-40 flex flex-col"))
                    .child(composer_command_menu(state, list_id.clone()))
            })
        },
    )
}

fn listbox_label(kind: Option<TriggerKind>) -> SharedString {
    match kind {
        Some(TriggerKind::Path) => t("Files and folders"),
        Some(TriggerKind::Skill) => t("Skills"),
        Some(TriggerKind::PullRequest) => t("Pull requests"),
        _ => t("Commands"),
    }
    .into()
}

fn empty_state(state: ComposerSuggestions) -> SharedString {
    let trigger = state.trigger.get();
    let kind = trigger.as_ref().map(|t| t.kind);
    let request = state.request.get();
    let result = state
        .results
        .get()
        .filter(|r| Some(&r.request) == request.as_ref());
    if result.as_ref().is_none_or(|r| r.loading) {
        return match kind {
            Some(TriggerKind::Skill | TriggerKind::SlashCommand) => {
                t("Searching workspace skills...")
            }
            Some(TriggerKind::PullRequest) => t("Finding pull request..."),
            _ => t("Searching workspace files..."),
        }
        .into();
    }
    if let Some(text) = result.and_then(|r| r.empty_state) {
        return text.resolve();
    }
    match kind {
        Some(TriggerKind::Skill) => t("No skills found. Try / to browse provider commands."),
        Some(TriggerKind::Path) => t("No matching files or folders."),
        _ => t("No matching command."),
    }
    .into()
}

fn composer_command_menu_item(
    state: ComposerSuggestions,
    list_id: &str,
    item: ComposerCommandItem,
) -> impl IntoElement {
    let id = item.id.clone();
    let active_id = id.clone();
    let hover_id = id.clone();
    let active = computed(move || state.active_item_id().as_ref() == Some(&active_id));
    let reveal_area = element_ref();
    let row_height = signal(0.0_f32);
    effect(move || {
        if active.get() {
            reveal_area.reveal(
                RevealOptions::default()
                    .vertical(ScrollAlignment::Nearest)
                    .animated(false),
            );
        }
    });
    let label = item.label.clone();
    let icon = match &item.kind {
        // UI-ICONS has no PierreEntryIcon yet; use the source's File/Folder fallback meanwhile.
        ComposerCommandItemKind::Path { directory } => Some(
            icon_with_class(
                if *directory { Icon::Folder } else { Icon::File },
                "size-4 text-icon-muted",
            )
            .into_element(),
        ),
        ComposerCommandItemKind::Thread => Some(
            icon_with_class(Icon::MessagesSquare, "size-4 shrink-0 text-secondary-label")
                .into_element(),
        ),
        ComposerCommandItemKind::Model {
            driver,
            provider_name,
        } => Some(provider_icon(driver, provider_name, "size-4 shrink-0").into_element()),
        _ => None,
    };
    let skill = if let ComposerCommandItemKind::Skill {
        source,
        display_name,
    } = item.kind
    {
        Some((source, display_name))
    } else {
        None
    };
    let skill_label = skill.clone();
    // CommandItem's current frozen API lacks Text slots, aria-selected, hover callbacks and radius.
    // This feature row transcribes its active=true/false classes without translating content twice.
    view().relative().test_id(&composer_suggestion_option_id(list_id, &id))
        .on_layout(move |bounds: &Bounds<Points>| row_height.set(bounds.size.height.0))
        .hit_slop(move || {
            let slack = if crate::state::layout::pointer() == crate::state::layout::Pointer::Coarse {
                (44.0 - row_height.get()).max(0.0) / 2.0
            } else { 0.0 };
            EdgeInsets { top: px(slack), bottom: px(slack), ..Default::default() }
        })
        .role(AccessibilityRole::Option).focusable(false)
        .label(move || label.resolve())
        .accessibility_state(move || AccessibilityState { selected: active.get(), ..Default::default() })
        .on_mouse_move(move |_: &MouseEvent| state.highlight(&hover_id))
        .on_press(move || { state.choose(&id); })
        .app_class(move || tw(&cn([
            Some("flex min-h-8 cursor-pointer select-none items-center gap-2 rounded-lg px-2 py-1.5 text-base outline-none hover:bg-transparent hover:text-inherit sm:min-h-7 sm:text-sm"),
            active.get().then_some("bg-accent! text-accent-foreground!"),
        ])))
        // NSH has no scroll-padding-bottom: reveal an unpainted rectangle extended by scroll-pb-6.
        .child(view().element_ref(&reveal_area).accessible(false)
            .app_class(tw("pointer-events-none absolute inset-x-0 top-0 -bottom-6")))
        .child(icon)
        .child(view().app_class(tw("flex min-w-0 flex-1 items-center gap-2"))
            .child(view().app_class(tw("min-w-0 max-w-[45%] shrink-0 truncate font-sans text-xs font-medium"))
                .child(dynamic(move || {
                    let slash = state.trigger.get().is_some_and(|t| t.kind == TriggerKind::SlashCommand);
                    if let Some((_, name)) = skill_label.as_ref().filter(|_| slash) {
                        return view().flex_row()
                            .child(text(SharedString::from("/skill:")).app_class(tw("text-secondary-label")))
                            .child(text(SharedString::from(name.clone()))).into_element();
                    }
                    text(item.label.resolve()).into_element()
                })))
            .child(composer_command_description(item.description, active))
            .child(skill.map(|(source, _)| view().app_class(tw("ms-auto")).child(
                skill_source_badge(source, state.trigger.get().is_some_and(|t| t.kind == TriggerKind::Skill))
            ))))
}

fn composer_command_description(description: Text, active: Memo<bool>) -> impl IntoElement {
    let width = signal(0.0_f32);
    let value = computed(move || description.resolve());
    let measure = text_measurer();
    let natural_width = computed(move || {
        let tokens = tokens();
        let family = tokens
            .font(nsh_core::element::FONT_SANS)
            .map_or(crate::theme::INTER.family, |font| font.as_ref());
        value.with(|value| measure(&format!("400 12px {family}"), value).0)
    });
    let handle = tooltip_create_handle();
    effect(move || {
        // Native font measurement replaces DOM scrollWidth/clientWidth on the source span.
        let open = active.get() && width.get() > 0.0 && natural_width.get() > width.get();
        if handle.is_open() != open {
            handle.set_open(open);
        }
    });
    text(move || value.get())
        .on_layout(move |bounds: &Bounds<Points>| width.set(bounds.size.width.0))
        .app_class(tw(
            "min-w-0 flex-1 truncate text-left text-secondary-label text-xs",
        ))
        .tooltip(tooltip(
            handle,
            TooltipOptions::default(),
            text(move || value.get()),
        ))
}

fn skill_source_badge(kind: SkillSourceKind, show_skill_suffix: bool) -> impl IntoElement {
    let (glyph, label) = match kind {
        SkillSourceKind::App => (Icon::Blocks, Text::from("App")),
        SkillSourceKind::Repo => (Icon::Folder, Text::from("Repo")),
        SkillSourceKind::Project => (Icon::Folder, Text::from("Project")),
        SkillSourceKind::Personal => (Icon::UserRound, Text::from("Personal")),
        SkillSourceKind::System => (Icon::Settings, Text::from("System")),
        SkillSourceKind::Other => (Icon::Package, Text::from("Provider")),
    };
    badge::badge(
        Props {
            variant: Variant::Secondary,
            ..Default::default()
        },
        vec![
            Child::StyledIcon {
                icon: glyph,
                class_name: "text-current".into(),
            },
            Child::Element(
                text(move || {
                    if show_skill_suffix {
                        tr(
                            "{source} Skill",
                            &[(
                                "source",
                                crate::i18n::Value::Text(label.resolve().to_string()),
                            )],
                        )
                    } else {
                        label.resolve().to_string()
                    }
                })
                .into_element(),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn highlight_search_identity_uses_the_webs_trimmed_case_folded_query_and_kind() {
        let first = policy::detect_trigger("@src", 4).expect("path trigger");
        let mut next = first.clone();
        next.query = "\u{feff}SRC\u{feff}".into();
        next.range.anchor = 20;
        next.range.head = 24;
        assert!(same_menu_search(&first, &next));
        next.kind = TriggerKind::Skill;
        assert!(!same_menu_search(&first, &next));
        next.kind = first.kind;
        next.query = "source".into();
        assert!(!same_menu_search(&first, &next));
        next.query = "\u{0085}src".into();
        assert!(!same_menu_search(&first, &next));
    }

    #[test]
    fn option_ids_preserve_whitespace_escapes_unicode_and_list_identity() {
        let paths = [
            "docs/my file.md",
            "docs/my_file.md",
            "docs/my%20file.md",
            "docs/my\tfile.md",
            "docs/�.md",
            "docs/\\ud800.md",
            "docs/🚀.md",
        ];
        let ids = paths
            .map(|path| composer_suggestion_option_id("suggestions", &format!("path:file:{path}")));
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), paths.len());
        assert!(
            ids.iter()
                .all(|id| id.is_ascii() && !id.contains(char::is_whitespace))
        );
        assert_eq!(
            composer_suggestion_option_id("list", "path:file:my file.md"),
            "list-%22path%3Afile%3Amy%20file.md%22"
        );
        assert_ne!(
            composer_suggestion_option_id("one", paths[0]),
            composer_suggestion_option_id("two", paths[0])
        );
    }
}
