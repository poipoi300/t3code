//! Shell lane (ui-l): the client's shell services behind the shell pieces. Pure delegation; the
//! rules are `t3_client::view_logic::shell::{sidebar_width, history_swipe}`.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::shell::sidebar_width::{
    THREAD_SIDEBAR_MIN_WIDTH, resolve_thread_sidebar_maximum_width,
    should_accept_thread_sidebar_width,
};

use crate::runtime::backend::HostClient;
use crate::ui::resizable::{RailResize, ResizeBounds};

/// AppSidebarLayout's `resizable` options for the thread sidebar's rail: the web's minimum and
/// maximum, its main-content rule, and the commit persisted through the client's
/// `chat_thread_sidebar_width` record. Without a client (the demo backend) the width still
/// resizes and is not stored, as the web's failed `setLocalStorageItem` leaves it.
pub fn thread_sidebar_rail(width: Signal<f32>, client: Option<HostClient>) -> RailResize {
    let store = client.map(|client| client.sidebar_width());
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
                let save = store.save(f64::from(final_width));
                spawn(async move {
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
pub fn load_thread_sidebar_width(width: Signal<f32>, client: Option<HostClient>) {
    let Some(client) = client else {
        return;
    };
    let load = client.sidebar_width().load(f64::from(window().width.0));
    spawn(async move {
        let stored = load.await;
        let _ = width.try_set(stored as f32);
    });
}

/// `resetSidebarWidth`: forgets the stored width and applies the default for the window.
pub fn reset_thread_sidebar_width(width: Signal<f32>, client: Option<HostClient>) {
    let viewport = f64::from(window().width.0);
    width.set(
        t3_client::view_logic::shell::sidebar_width::resolve_initial_thread_sidebar_width(
            None, viewport,
        ) as f32,
    );
    if let Some(client) = client {
        let clear = client.sidebar_width().clear();
        spawn(async move {
            if let Err(error) = clear.await {
                log::error!("Could not clear persisted thread sidebar width. {error}");
            }
        });
    }
}
