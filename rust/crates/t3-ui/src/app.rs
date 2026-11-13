//! The root: reads the launch address, builds the runtime and UI state once, then shows the
//! standalone or workspace layout.

use nsh_core::prelude::*;

use crate::runtime::Rt;
use crate::screens::{router, shell};
use crate::state::clock;
use crate::state::nav::{Navigator, initial_route};
use crate::state::panels::Panels;
use crate::theme::{self, ThemeChoice, palette};

#[component]
pub fn App() -> impl IntoElement {
    // Booting waits for the launch address (the page URL, a deep link). This also keeps token
    // installation and task spawns out of the root's first synchronous render, which aborts
    // NSH's web runtime ("a prop read or written after its owner was disposed").
    let address = signal(None::<Option<String>>);
    spawn(async move {
        let launched = app_url().await;
        address.set(Some(launched));
    });
    // Untracked: root builds the runtime once; a signal it happens to read must not rebuild it.
    view().w_full().h_full().child(dynamic(move || {
        address
            .get()
            .map(|launched| untracked(|| root(launched.as_deref())))
    }))
}

fn root(address: Option<&str>) -> AnyElement {
    theme::install(&ThemeChoice::default());
    provide_context(theme::text_view_parts());
    // NSH's built-in highlighter answers with a `dynamic` region inside the code's text node,
    // which the native platforms do not paint (ui-plan.md, NSH finding 10): plain code there.
    #[cfg(not(target_arch = "wasm32"))]
    provide_context(nsh_base::text_view::CodeHighlighter::new(|_| Vec::new()));
    crate::i18n::set_language("system", &[locale().to_string()]);
    let runtime = Rt::boot();
    let nav = Navigator::new(initial_route(address, !runtime.environments().is_empty()));
    provide_context(nav);
    let panels = Panels::new();
    provide_context(panels);
    clock::start();
    crate::keys::install(panels);

    let top = computed(move || nav.top());
    let standalone = computed(move || top.get().is_standalone());
    view()
        .flex_1()
        .w_full()
        .bg(move || palette().background)
        .text_color(move || palette().foreground)
        .pt(move || safe_area().top)
        .child(dynamic(move || {
            if standalone.get() {
                router::screen(&top.get())
            } else {
                shell::app_shell().into_element()
            }
        }))
        .into_element()
}
