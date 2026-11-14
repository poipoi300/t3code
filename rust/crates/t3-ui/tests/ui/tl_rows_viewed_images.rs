//! The viewed-images row mounted from the client's row, with images whose URLs come from the
//! asset binding (a recording stand-in for the client's signed asset URLs).

use std::rc::Rc;

use futures::StreamExt;
use futures::stream::LocalBoxStream;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::ClientStore;
use t3_client::services::assets::AssetUrlState;
use t3_client::thread_views::messages_timeline::MessagesTimelineRow;
use t3_client::types::{EnvironmentId, ThreadId, ThreadKey};
use t3_contracts::assets::{AssetCreateUrlResult, AssetResource};
use t3_ui::i18n::t;
use t3_ui::runtime::services::assets::{AssetUrlBinding, AssetUrlClient};
use t3_ui::runtime::{Rt, Shared, backend::Unavailable, host::Choice};
use t3_ui::screens::thread::rows::viewed_images::{
    ViewedImagesContext, viewed_images_timeline_row,
};

use crate::support::{DESKTOP, PHONE};

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("environment".into()),
        thread: ThreadId("thread".into()),
    }
}

/// Answers each resource by its file name and records what was asked.
struct Assets(Signal<Vec<String>>);
impl AssetUrlClient for Assets {
    fn watch(
        &self,
        environment: EnvironmentId,
        resource: AssetResource,
    ) -> LocalBoxStream<'static, AssetUrlState> {
        let AssetResource::MediaFile {
            thread_id, path, ..
        } = &resource
        else {
            panic!("viewed images are media files, got {resource:?}");
        };
        let call = format!("{} {} {}", environment.0, thread_id.0, path.0);
        self.0.update(|calls| calls.push(call));
        let state = if path.0.ends_with("denied.png") {
            AssetUrlState::Failure {
                missing_file_access: true,
            }
        } else {
            AssetUrlState::Success {
                url: format!("https://server.test/api/assets/{}", path.0.trim_start_matches('/')),
                asset: Rc::new(
                    serde_json::from_value::<AssetCreateUrlResult>(
                        json!({"relativeUrl":"/api/assets/x","expiresAt":1}),
                    )
                    .expect("fixture"),
                ),
            }
        };
        futures::stream::once(async move { state })
            .chain(futures::stream::pending())
            .boxed_local()
    }
}

fn control(label: &'static str, id: &str, action: impl Fn() + 'static) -> impl IntoElement {
    nsh_base::button()
        .accessibility_label(Some(t(label).into()))
        .test_id(id)
        .on_press(action)
        .child(text(t(label)))
}

#[component]
fn ImagesFixture() -> impl IntoElement {
    t3_ui::theme::install(&Default::default());
    provide_context(Rt::new(
        ClientStore::default(),
        Rc::new(Unavailable("fixture".into())),
        Choice::Demo,
    ));
    let calls = signal(Vec::<String>::new());
    let with_assets = signal(true);
    let row = computed(|| {
        let row: MessagesTimelineRow = serde_json::from_value(json!({
            "id":"viewed","createdAt":"2026-11-13T12:00:00Z","kind":"viewed-images",
            "paths":["/repo/first.png","/repo/notes.txt","/repo/denied.png","https://example.com/remote.png"]
        }))
        .expect("fixture");
        Some(Shared(Rc::new(row)))
    });
    let with = ViewedImagesContext::with_assets(key(), Some(AssetUrlBinding(Rc::new(Assets(calls)))));
    let without = ViewedImagesContext::with_assets(key(), None);
    view()
        .w_full()
        .h_full()
        .child(control("No client", "no-client", move || {
            with_assets.set(false)
        }))
        .child(text(move || {
            SharedString::from(format!("Watched: {}", calls.get().join(", ")))
        }))
        .child(dynamic(move || {
            let context = if with_assets.get() {
                with.clone()
            } else {
                without.clone()
            };
            Some(untracked(|| viewed_images_timeline_row(row, context).into_element()))
        }))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut sets = t3_ui::registries();
    let mut fixtures = Registry::new();
    fixtures.register::<ImagesFixture>();
    sets.push(fixtures);
    Headless::start(EngineConfig {
        ui_sets: sets,
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<ImagesFixture>())
    })
    .expect("fixture")
}

#[test]
fn only_host_images_are_requested_each_as_a_media_file_of_the_thread() {
    for window in [PHONE, DESKTOP] {
        let mut app = fixture(window);
        app.wait_for(
            "Watched: environment thread /repo/first.png, environment thread /repo/denied.png",
        )
        .expect("one signed URL per host image, in row order; text and remote paths skipped");
        app.wait_for("text:/^Image unavailable · denied.png · This connection cannot read host files/")
            .expect("a connection without file access explains itself");
        assert_eq!(
            app.screen().count("id:viewed-images"),
            Ok(1),
            "one gallery"
        );
    }
}

#[test]
fn without_an_asset_service_images_are_unavailable_not_loading() {
    let mut app = fixture(DESKTOP);
    app.wait_for("No client").expect("mounted");
    app.press("id:no-client").expect("no client");
    app.wait_for("Image unavailable · first.png")
        .expect("no asset service: the fallback, never a frame that keeps loading");
    assert_eq!(app.screen().count("label:Loading image"), Ok(0));
}
