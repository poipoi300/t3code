//! Port of apps/web/src/components/settings/SettingsSidebarNav.tsx's search: the field, its keys
//! and the results list. Ranking, availability, moved anchors and target scopes are
//! `t3_client::services::settings::search`.

use std::rc::Rc;

use nsh_core::{KeyEvent, prelude::*};
use t3_client::services::settings::search::{
    SettingsSearchItem, SettingsSearchPlatform, page_of_path, search_settings,
    settings_section_label,
};

use super::shell::section_icon;
use crate::i18n::{Text, t};
use crate::theme::{Scope, cn, palette, tw, tw_with};
use crate::ui::icon::{Icon, icon};
use crate::ui::kbd::{Variant as KbdVariant, kbd};

/// The nav's search state (`query`, `activeResultIndex`, `compactSearchOpen`).
#[derive(Clone, Copy)]
pub struct NavSearch {
    pub query: Signal<String>,
    pub active: Signal<usize>,
    pub compact_open: Signal<bool>,
    pub input: ElementRef,
    pub results: Memo<Vec<&'static SettingsSearchItem>>,
}

impl NavSearch {
    /// `items` are the searchable items (`useAvailableSettingsSearchItems`).
    pub fn new(
        items: impl Fn() -> Vec<&'static SettingsSearchItem> + 'static,
        platform: SettingsSearchPlatform,
    ) -> Self {
        let query = signal(String::new());
        let results = computed(move || {
            let query = query.get();
            let items = items();
            // The labels as displayed, so search also works in the active language.
            search_settings(&query, &items, platform, |key| {
                Text::client_key(key.to_owned()).resolve().to_string()
            })
        });
        Self {
            query,
            active: signal(0),
            compact_open: signal(false),
            input: element_ref(),
            results,
        }
    }

    pub fn is_searching(self) -> bool {
        !self.query.with(|query| query.trim().is_empty())
    }

    /// The highlighted index, clamped as the web clamps it on render.
    pub fn active_index(self) -> usize {
        let max = self.results.with(|results| results.len().saturating_sub(1));
        self.active.get().min(max)
    }

    pub fn clear(self) {
        self.query.set(String::new());
        self.active.set(0);
    }

    /// The Find and `/` shortcuts' action: open the compact field and focus it.
    pub fn focus(self) {
        self.compact_open.set(true);
        self.input.focus();
    }

    /// `handleSearchKeyDown`. Returns whether the key was handled.
    pub fn key_down(self, event: &KeyEvent, pick: &dyn Fn(&'static SettingsSearchItem)) -> bool {
        let count = self.results.with_untracked(Vec::len);
        match event.key.as_ref() {
            "Escape" if self.is_searching() => {
                self.clear();
                true
            }
            _ if count == 0 => false,
            "ArrowDown" => {
                self.active
                    .update(|index| *index = ((*index).min(count - 1) + 1) % count);
                true
            }
            "ArrowUp" => {
                self.active
                    .update(|index| *index = ((*index).min(count - 1) + count - 1) % count);
                true
            }
            "Enter" => {
                let index = self.active.get_untracked().min(count - 1);
                if let Some(item) = self
                    .results
                    .with_untracked(|results| results.get(index).copied())
                {
                    pick(item);
                }
                true
            }
            _ => false,
        }
    }
}

/// The search row at the top of the nav.
pub fn settings_search_field(
    search: NavSearch,
    narrow: impl Fn() -> bool + Copy + 'static,
    pick: Rc<dyn Fn(&'static SettingsSearchItem)>,
) -> impl IntoElement {
    let expanded = move || search.is_searching() && search.results.with(|r| !r.is_empty());
    view()
        .app_class(move || {
            Some(tw_with(
                Scope::Sidebar,
                &cn([
                    Some("flex h-8 items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-sidebar-muted-foreground hover:bg-sidebar-row-hover hover:text-sidebar-foreground"),
                    // `!compactSearchOpen && "max-md:hidden"`: md is the nav's own breakpoint.
                    (!search.compact_open.get() && narrow()).then_some("hidden"),
                ]),
                &[],
            ))
        })
        .child(icon(Icon::Search, 16.0, move || {
            palette().sidebar_muted_foreground.opacity(0.8)
        }))
        // SidebarInput's child classes on NSH's input; a native field clears on Escape itself.
        .child(
            nsh_base::input()
                .element_ref(&search.input)
                .value(move || search.query.get())
                .role(AccessibilityRole::Combobox)
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(expanded()),
                    ..Default::default()
                })
                .on_change(move |value: &str| {
                    search.query.set(value.to_owned());
                    search.active.set(0);
                })
                .on_key_down({
                    let pick = pick.clone();
                    move |event: &KeyEvent| {
                        search.key_down(event, &*pick);
                    }
                })
                .on_focus(move || search.compact_open.set(true))
                .on_blur(move || {
                    if !search.is_searching() {
                        search.compact_open.set(false);
                    }
                })
                .placeholder(t("Search"))
                .label(t("Search settings"))
                .app_class(tw_with(
                    Scope::Sidebar,
                    "h-auto min-w-0 flex-1 bg-transparent p-0 text-sm font-medium leading-normal text-sidebar-foreground placeholder:text-sidebar-muted-foreground",
                    &[],
                )),
        )
        .child(dynamic(move || {
            if search.is_searching() {
                // UI-BUTTON has no icon-micro size; its 20px ghost-muted look is written here.
                nsh_base::button()
                    .accessibility_label(t("Clear settings search"))
                    .on_press(move || {
                        search.clear();
                        search.input.focus();
                    })
                    .app_class(tw("flex size-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground outline-hidden focus-visible:ring-2 focus-visible:ring-ring"))
                    .child(icon(Icon::X, 12.0, move || palette().muted_foreground))
                    .into_element()
            } else {
                kbd(KbdVariant::Default, vec![crate::ui::badge::Child::Text(Text::content("/"))])
                    .into_element()
            }
        }))
}

/// "No settings found" and the results listbox.
pub fn settings_search_results(
    search: NavSearch,
    pick: Rc<dyn Fn(&'static SettingsSearchItem)>,
) -> impl IntoElement {
    view()
        .child(when(
            move || search.is_searching() && search.results.with(Vec::is_empty),
            move || {
                // NSH has no status role; the message is plain text.
                text(t("No settings found")).app_class(tw_with(
                    Scope::Sidebar,
                    "px-2 py-6 text-center text-xs text-sidebar-muted-foreground",
                    &[],
                ))
            },
        ))
        .child(dynamic(move || {
            let results = search.results.get();
            let mut list = view()
                .app_class(tw("flex w-full min-w-0 flex-col gap-1"))
                .when(!results.is_empty(), |list| {
                    list.role(AccessibilityRole::List)
                        .label(t("Settings search results"))
                });
            for (index, item) in results.into_iter().enumerate() {
                list = list.child(search_result_row(search, index, item, pick.clone()));
            }
            list.into_element()
        }))
}

fn search_result_row(
    search: NavSearch,
    index: usize,
    item: &'static SettingsSearchItem,
    pick: Rc<dyn Fn(&'static SettingsSearchItem)>,
) -> impl IntoElement {
    let active = move || search.active_index() == index;
    let section = page_of_path(item.to).map_or("", settings_section_label);
    let title = Text::client_key(item.title);
    let row = element_ref();
    // Attached only to the highlighted row, so it scrolls whenever the highlight lands on it.
    effect(move || {
        if active() {
            row.reveal(
                nsh_core::RevealOptions::default()
                    .vertical(nsh_core::ScrollAlignment::Nearest)
                    .animated(false),
            );
        }
    });
    nsh_base::button()
        .element_ref(&row)
        .role(AccessibilityRole::Option)
        .selected(active)
        .accessibility_label({
            let title = title.clone();
            move || Some(title.resolve())
        })
        .on_mouse_move(move |_: &MouseEvent| search.active.set(index))
        .on_press(move || pick(item))
        .app_class(move || {
            let attrs: &[(&str, &str)] = if active() { &[("data-active", "true")] } else { &[] };
            Some(tw_with(
                Scope::Sidebar,
                "flex w-full cursor-pointer items-center gap-(--sidebar-control-gap) overflow-hidden text-left outline-hidden ring-ring hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-inset active:bg-sidebar-row-active active:text-sidebar-foreground data-[active=true]:bg-sidebar-row-selected data-[active=true]:font-medium data-[active=true]:text-sidebar-foreground rounded-(--control-radius) px-(--sidebar-row-content-inset) py-2 text-xs font-medium text-sidebar-muted-foreground/80 h-auto min-h-10 items-start",
                attrs,
            ))
        })
        .child(section_icon_for(item))
        .child(
            view()
                .app_class(tw("min-w-0 flex-1"))
                .child(
                    text(move || title.resolve())
                        .number_of_lines(1u32)
                        .app_class(tw_with(Scope::Sidebar, "block truncate text-sm font-medium text-sidebar-foreground", &[])),
                )
                .child(
                    text(move || Text::client_key(section).resolve())
                        .number_of_lines(1u32)
                        .app_class(tw_with(Scope::Sidebar, "block truncate text-2xs text-sidebar-muted-foreground/75", &[])),
                ),
        )
}

fn section_icon_for(item: &SettingsSearchItem) -> impl IntoElement + use<> {
    let glyph = page_of_path(item.to).map_or(Icon::Settings2, section_icon);
    icon(glyph, 16.0, move || palette().sidebar_icon)
}
