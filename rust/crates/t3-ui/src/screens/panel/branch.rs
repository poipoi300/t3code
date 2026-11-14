//! Port of apps/web/src/components/BranchToolbarBranchSelector.tsx BranchToolbarBranchSelector.

use nsh_core::prelude::*;
use t3_client::types::ThreadKey;

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::rt;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::middle_truncate::{Font, Options, Variant, middle_truncate_with_font};
use crate::ui::popover::dropdown_glass_background;

/// Mount once per scoped thread in `VersionControlSlots::branch`.
pub fn branch_toolbar_branch_selector(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let shell = runtime.store.shell(key.environment);
    let branch = computed(move || {
        shell.with(|shell| {
            let shell = shell.as_ref()?;
            let thread = shell
                .threads
                .iter()
                .find(|thread| thread.id == key.thread)?;
            shell
                .projects
                .iter()
                .find(|project| project.id == thread.project_id)?;
            Some(thread.branch.as_ref().map(|branch| branch.0.clone()))
        })
    });
    let label = computed(move || match branch.get().flatten() {
        Some(branch) if !branch.is_empty() => Text::content(branch).resolve(),
        _ => Text::from("Select ref").resolve(),
    });
    view().app_class(tw("flex w-full flex-col")).child(when(
        move || branch.get().is_some(),
        move || {
            // Backend has no branch/ref lease yet; match the web's empty failed-query state.
            // Client/runtime requests are in briefs/client-seam-requests.md (GIT-2).
            branch_picker(label)
        },
    ))
}

/// Port of BranchPicker.tsx BranchPicker's empty query state; mutations await the client lease.
fn branch_picker(label: Memo<SharedString>) -> impl IntoElement {
    let open = signal(false);
    let query = signal(String::new());
    let anchor_width = signal(0.0_f32);
    let available = signal(nsh_core::Size::new(window().width, window().height));
    let search = element_ref();
    // UI-COMBOBOX is still a stub. Native Popover supplies outside/Escape/back dismissal.
    // NSH Combobox only supports an inline input trigger, unlike the source's select row.
    nsh_base::popover()
        .open(move || Some(open.get()))
        .on_open_change(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(next, _) = event {
                open.set(*next);
                if !next {
                    query.set(String::new());
                }
            }
        })
        .on_open(move || search.focus())
        .on_position(move |event: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::Position(_, _, room) = event {
                available.set(*room);
            }
        })
        .placement(Placement::Bottom)
        .align(Align::Start)
        .offset(px(4.0))
        .popup_class(tw("z-[130]"))
        // The native trigger slot attaches its focus/expanded state directly to the button;
        // source layout-only trigger wrappers are flattened to preserve that focus target.
        .trigger(thread_details_control(label, open, anchor_width))
        .child(dynamic(move || {
            untracked(|| {
                // Positioner variables become measured native bounds; Electron's drag-region
                // and transform-origin hints have no meaning in this shared tree.
                view()
                    .test_id("branch-picker-popup")
                    .app_class(tw("relative flex max-h-full rounded-lg border backdrop-blur-(--glass-blur) dark:backdrop-blur-[16px] backdrop-saturate-[1.14] dark:backdrop-saturate-[1.08] shadow-[0_16px_40px_-18px_rgb(0_0_0/55%)] dark:shadow-[0_18px_44px_-18px_rgb(0_0_0/80%)]"))
                    .w(move || px(anchor_width.get().min(available.get().width.0).max(0.0)))
                    .max_h(move || px(available.get().height.0.clamp(0.0, 368.0)))
                    .bg(dropdown_glass_background)
                    .border_color(move || tokens().color("contrast-foreground").unwrap_or_default().opacity(0.1))
                    .child(
                        view()
                            .app_class(tw("flex min-w-0 flex-1 flex-col overflow-hidden text-foreground"))
                            .child(combobox_search_input(query, search))
                            .child(
                                view().app_class(tw("flex min-h-0 flex-1 flex-col overflow-hidden"))
                                    .child(text(move || t("No refs found."))
                                        .app_class(tw("p-2 text-center text-base text-muted-foreground sm:text-sm")))
                                    .child(view().role(AccessibilityRole::Listbox)
                                        .app_class(tw("relative flex min-h-0 w-full max-h-56 flex-1 flex-col overflow-hidden"))),
                            ),
                    )
                    .into_element()
            })
        }))
}

/// Port of chat/ThreadDetailsControl.tsx ThreadDetailsControl's select part.
fn thread_details_control(
    label: Memo<SharedString>,
    open: Signal<bool>,
    anchor_width: Signal<f32>,
) -> impl IntoElement {
    // Selector classes move to their children; coarse after: becomes native hit slop.
    // text-box cap trimming is unavailable; UI-TRUNCATE owns that declared limitation.
    // NSH cannot parse the source shadow/scale transition list; scale stays 100 on this trigger.
    nsh_base::button()
        .role(AccessibilityRole::Combobox)
        .accessibility_label(move || Some(tr("Branch: {branch}", &[("branch", Value::Text(label.get().to_string()))]).into()))
        .on_layout(move |bounds: &Bounds<Points>| anchor_width.set(bounds.size.width.0))
        .app_class(tw("group/branch-select relative inline-flex shrink-0 cursor-pointer items-center whitespace-nowrap border outline-none transition-shadow focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-64 h-8 min-w-0 rounded-lg border-transparent text-left text-[13px] font-medium text-foreground/80 w-full justify-start bg-transparent shadow-none hover:!bg-black/[0.035] active:!bg-black/[0.035] dark:hover:!bg-white/[0.08] dark:active:!bg-white/[0.08] gap-0 px-0 max-w-full active:scale-100"))
        .hit_slop(move || EdgeInsets::all(px(if pointer() == Pointer::Coarse { 6.0 } else { 0.0 })))
        .child(
            view()
                .app_class(tw("flex h-full min-w-0 flex-1 items-center rounded-s-lg gap-2.5 px-2.5 text-left hover:bg-black/[0.035] dark:hover:bg-white/[0.08]"))
                .child(icon_with_class(Icon::GitBranch, "size-4 shrink-0 text-muted-foreground"))
                .child(composer_context_label(label)),
        )
        .child(
            view()
                .app_class(tw("rounded-e-lg hover:bg-black/[0.035] dark:hover:bg-white/[0.08] -me-px relative flex h-full w-8 shrink-0 items-center justify-center"))
                // Source select-icon::before is an explicitly positioned first child.
                .child(view().app_class(move || Some(tw(&cn([
                    Some("absolute -left-px top-1/2 h-4 w-px -translate-y-1/2 bg-transparent group-hover/branch-select:bg-border/65"),
                    open.get().then_some("bg-border/65"),
                ])))))
                .child(icon_with_class(Icon::ChevronDown, "size-4 shrink-0 text-muted-foreground opacity-100 me-0")),
        )
}

/// Port of ComposerContextLabel.tsx ComposerContextLabel with displayMode="panel".
fn composer_context_label(label: Memo<SharedString>) -> impl IntoElement {
    // Native text wraps by default; the shared primitive handles truncation and measurement.
    // Hide the duplicate visual value: the combobox's changing label serves browse mode.
    view()
        .app_class(tw("min-w-0 flex-1 overflow-hidden text-left"))
        .accessibility_elements_hidden(true)
        .child(
            view()
                .app_class(tw("block w-full min-w-0 overflow-hidden"))
                .child(middle_truncate_with_font(
                    move || label.get(),
                    Options {
                        variant: Variant::CapAligned,
                        ..Default::default()
                    },
                    || Font {
                        size: 13.0,
                        weight: FontWeight::MEDIUM,
                        ..Default::default()
                    },
                )),
        )
}

/// Port of ui/combobox.tsx ComboboxSearchInput; remove after UI-COMBOBOX lands its slots.
fn combobox_search_input(query: Signal<String>, search: ElementRef) -> impl IntoElement {
    view()
        .app_class(tw("min-w-0 shrink-0 px-3 pt-2.5"))
        .child(
            view().app_class(tw("relative -translate-y-px border-b border-border/70 pb-1.5 transition-colors focus-within:border-ring"))
                .child(view().app_class(tw("pointer-events-none absolute top-1.5 left-0"))
                    .child(icon_with_class(Icon::Search, "size-4 shrink-0 text-muted-foreground/55")))
                .child(nsh_base::input()
                    .element_ref(&search)
                    .value(move || query.get())
                    .on_change(move |value: &str| query.set(value.to_owned()))
                    .placeholder(move || t("Search refs..."))
                    .label(move || t("Search refs..."))
                    .auto_correct(false)
                    .app_class(tw("flex w-full h-6.5 ps-5 font-sans leading-6.5 rounded-none bg-transparent text-sm"))),
        )
}
