//! Shell lane (ui-l): the client's shell services behind the shell pieces. Pure delegation; the
//! rules are `t3_client::view_logic::shell::{sidebar_width, history_swipe}`.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::shell::sidebar_width::{
    THREAD_SIDEBAR_MIN_WIDTH, resolve_thread_sidebar_maximum_width,
    should_accept_thread_sidebar_width,
};

use futures::future::LocalBoxFuture;
use t3_client::composition::ClientRuntime;
use t3_client::platform::{Platform, PlatformError};
use t3_client::view_logic::shell::sidebar_width::SidebarWidth;
use t3_client::view_logic::shell::{sidebar::Marker, sidebar_list::ShelfExpansion};

use crate::ui::resizable::{RailResize, ResizeBounds};

use crate::runtime::tasks::watch;

/// The client's `chat_thread_sidebar_width` record (`ClientRuntime::sidebar_width`), provided at
/// the app root; absent on the demo backend.
#[derive(Clone)]
pub struct SidebarWidthStore(Rc<dyn SidebarWidthRecord>);

trait SidebarWidthRecord {
    fn load(&self, viewport: f64) -> LocalBoxFuture<'static, f64>;
    fn save(&self, width: f64) -> LocalBoxFuture<'static, Result<(), PlatformError>>;
    fn clear(&self) -> LocalBoxFuture<'static, Result<(), PlatformError>>;
}

impl<P: Platform + Clone + 'static> SidebarWidthRecord for SidebarWidth<P> {
    fn load(&self, viewport: f64) -> LocalBoxFuture<'static, f64> {
        SidebarWidth::load(self, viewport)
    }
    fn save(&self, width: f64) -> LocalBoxFuture<'static, Result<(), PlatformError>> {
        SidebarWidth::save(self, width)
    }
    fn clear(&self) -> LocalBoxFuture<'static, Result<(), PlatformError>> {
        SidebarWidth::clear(self)
    }
}

pub fn sidebar_width_store<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> SidebarWidthStore {
    SidebarWidthStore(Rc::new(client.sidebar_width()))
}

/// AppSidebarLayout's `resizable` options for the thread sidebar's rail: the web's minimum and
/// maximum, its main-content rule, and the commit persisted through the client's
/// `chat_thread_sidebar_width` record. Without a client (the demo backend) the width still
/// resizes and is not stored, as the web's failed `setLocalStorageItem` leaves it.
pub fn thread_sidebar_rail(width: Signal<f32>, store: Option<SidebarWidthStore>) -> RailResize {
    let tasks = crate::runtime::tasks::TaskOwner::here();
    RailResize {
        width,
        bounds: Rc::new(|| {
            let viewport = f64::from(window().width.0);
            ResizeBounds {
                min: THREAD_SIDEBAR_MIN_WIDTH as f32,
                max: resolve_thread_sidebar_maximum_width(viewport, THREAD_SIDEBAR_MIN_WIDTH)
                    as f32,
            }
        }),
        should_accept: Some(Rc::new(|current: f32, next: f32| {
            should_accept_thread_sidebar_width(
                f64::from(current),
                f64::from(next),
                f64::from(window().width.0),
            )
        })),
        on_resize: Some(Rc::new(move |final_width: f32| {
            if let Some(store) = &store {
                let save = store.0.save(f64::from(final_width));
                tasks.spawn(async move {
                    if let Err(error) = save.await {
                        log::error!("Could not persist sidebar width. {error}");
                    }
                });
            }
        })),
    }
}

/// The width the thread sidebar starts with (`readInitialSidebarWidth`): the stored width
/// resolved for the window, or the default.
pub fn load_thread_sidebar_width(width: Signal<f32>, store: Option<SidebarWidthStore>) {
    let Some(store) = store else {
        return;
    };
    let load = store.0.load(f64::from(window().width.0));
    watch(async move {
        let stored = load.await;
        let _ = width.try_set(stored as f32);
    });
}

/// `resetSidebarWidth`: forgets the stored width and applies the default for the window.
pub fn reset_thread_sidebar_width(
    width: Signal<f32>,
    store: Option<SidebarWidthStore>,
    tasks: crate::runtime::tasks::TaskOwner,
) {
    let viewport = f64::from(window().width.0);
    width.set(
        t3_client::view_logic::shell::sidebar_width::resolve_initial_thread_sidebar_width(
            None, viewport,
        ) as f32,
    );
    if let Some(store) = store {
        let clear = store.0.clear();
        tasks.spawn(async move {
            if let Err(error) = clear.await {
                log::error!("Could not clear persisted thread sidebar width. {error}");
            }
        });
    }
}

/// The three sidebar shelves' expanded flags (`ClientRuntime::sidebar_shelves`), provided at the
/// app root; absent on the demo backend, where the shelves start collapsed and are not stored.
#[derive(Clone)]
pub struct SidebarShelvesStore {
    load: Rc<dyn Fn() -> LocalBoxFuture<'static, ShelfExpansion>>,
    save: Rc<SaveShelf>,
}

type SaveShelf = dyn Fn(Marker, bool) -> LocalBoxFuture<'static, Result<(), PlatformError>>;

pub fn sidebar_shelves_store<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> SidebarShelvesStore {
    let shelves = client.sidebar_shelves();
    let saving = shelves.clone();
    SidebarShelvesStore {
        load: Rc::new(move || shelves.load()),
        save: Rc::new(move |marker, expanded| saving.save(marker, expanded)),
    }
}

/// The stored flags into `expansion`, once.
pub fn load_sidebar_shelves(expansion: Signal<ShelfExpansion>, store: Option<SidebarShelvesStore>) {
    let Some(store) = store else {
        return;
    };
    let load = (store.load)();
    watch(async move {
        let stored = load.await;
        let _ = expansion.try_set(stored);
    });
}

/// A shelf header's toggle: the flag changes at once and is written alone.
pub fn toggle_sidebar_shelf(
    expansion: Signal<ShelfExpansion>,
    store: Option<SidebarShelvesStore>,
    marker: Marker,
    expanded: bool,
    tasks: crate::runtime::tasks::TaskOwner,
) {
    expansion.update(|current| *current = current.with(marker, expanded));
    if let Some(store) = store {
        let save = (store.save)(marker, expanded);
        tasks.spawn(async move {
            if let Err(error) = save.await {
                log::error!("Could not persist the sidebar shelf. {error}");
            }
        });
    }
}
