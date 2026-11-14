//! The root: reads the launch address, builds the runtime and UI state once, then shows the
//! standalone or workspace layout.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::ClientStore;

use crate::runtime::Rt;
use crate::runtime::host::{self, Started};
use crate::screens::{router, shell};
use crate::state::clock;
use crate::state::nav::{Navigator, initial_route};
use crate::state::panels::Panels;
use crate::theme::{self, ThemeChoice, palette};

#[component]
pub fn App() -> impl IntoElement {
    // Booting waits for the launch address (the page URL, a deep link) and the backend it chooses
    // (the real client, or the demo when asked). This also keeps token installation and task
    // spawns out of the root's first synchronous render, which aborts NSH's web runtime ("a prop
    // read or written after its owner was disposed").
    let address = signal(None::<Option<String>>);
    let slot: Rc<RefCell<Option<(ClientStore, Started)>>> = Rc::new(RefCell::new(None));
    let filling = slot.clone();
    spawn(async move {
        let launched = app_url().await;
        let store = ClientStore::default();
        let started = host::start(store.clone(), host::launch_choice(launched.as_deref())).await;
        *filling.borrow_mut() = Some((store, started));
        address.set(Some(launched));
    });
    // Untracked: root builds the runtime once; a signal it happens to read must not rebuild it.
    // `address` is set once, so this runs once with the slot filled.
    view().w_full().h_full().child(dynamic(move || {
        address.get().and_then(|launched| {
            let (store, started) = slot.borrow_mut().take()?;
            Some(untracked(|| root(launched.as_deref(), store, started)))
        })
    }))
}

fn root(address: Option<&str>, store: ClientStore, started: Started) -> AnyElement {
    theme::install(&ThemeChoice::default());
    provide_context(theme::text_view_parts());
    // NSH's built-in highlighter answers with a `dynamic` region inside the code's text node,
    // which the native platforms do not paint (ui-plan.md, NSH finding 10): plain code there.
    #[cfg(not(target_arch = "wasm32"))]
    provide_context(nsh_base::text_view::CodeHighlighter::new(|_| Vec::new()));
    crate::i18n::set_language("system", &[locale().to_string()]);
    let paired = started.paired;
    let runtime = Rt::boot(store, started);
    let nav = Navigator::new(initial_route(
        address,
        paired || !runtime.environments().is_empty(),
    ));
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
        // ConfirmDialogHost at the root, as the web mounts it: every confirm request needs one.
        .child(crate::ui::confirm::confirm_dialog_host())
        .into_element()
}
