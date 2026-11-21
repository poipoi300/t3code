//! Region selection on a frozen frame from the managed browser viewer.
//!
//! The web editor (`PickPreload.ts`) is pointer-driven: it has no keyboard way to choose a
//! region, so this editor has none either. Escape cancels and Enter in the note attaches, as on
//! the web.

use std::{future::Future, pin::pin, rc::Rc, time::Duration};

use futures::{
    FutureExt,
    future::{AbortHandle, Either, LocalBoxFuture, abortable, select},
};
use nsh_core::{
    TouchEvent, TouchPhase, element::ImageSrc, keymap::When, prelude::*, time::unix_ms,
};
use t3_client::{
    ClientError,
    platform::Platform,
    preview::{
        annotation::{AnnotationRegion, AnnotationRequest, NOTE_MAX_UTF16},
        browser::BrowserPreview,
        socket::Viewport,
    },
    time::iso_from_epoch_ms,
};

use crate::{
    i18n::t,
    runtime::tasks::TaskOwner,
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};

use super::crop::crop_png;

/// How long an attach may take before the editor reports a failure.
const SAVE_TIMEOUT: Duration = Duration::from_secs(30);

/// The page a frame was captured from, read when the frame arrived.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageIdentity {
    pub thread_id: String,
    pub url: String,
    pub title: Option<String>,
    /// The page viewport in CSS pixels at that moment.
    pub viewport: Viewport,
}

#[derive(Clone)]
pub(super) struct Capture {
    pub source: SharedString,
    pub jpeg: Rc<Vec<u8>>,
    pub pixels: Viewport,
    /// `None` until the page and its viewport were known when the frame arrived.
    pub page: Option<PageIdentity>,
}

pub(super) struct Annotation {
    pub capture: Capture,
    pub page: PageIdentity,
}

type CapturePoint = (f64, f64);

/// The region between two points, in frame pixels. `None` for a tap, a line, or a point that is
/// not a finite number.
fn region(start: CapturePoint, end: CapturePoint) -> Option<AnnotationRegion> {
    if ![start.0, start.1, end.0, end.1]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let x = start.0.min(end.0).floor() as u32;
    let y = start.1.min(end.1).floor() as u32;
    let width = start.0.max(end.0).ceil() as u32 - x;
    let height = start.1.max(end.1).ceil() as u32 - y;
    (start.0 != end.0 && start.1 != end.1).then_some(AnnotationRegion {
        x,
        y,
        width,
        height,
    })
}

/// A pointer position as a frame pixel, pinned to the frame edge when it is outside the image
/// (or in the letterbox bars). `bounds` is the picker's size.
fn clamped_point(pixels: Viewport, bounds: (f32, f32), at: (f32, f32)) -> Option<CapturePoint> {
    let (frame_w, frame_h) = (f64::from(pixels.width), f64::from(pixels.height));
    let scale = (f64::from(bounds.0) / frame_w).min(f64::from(bounds.1) / frame_h);
    if !scale.is_finite() || scale <= 0.0 || !at.0.is_finite() || !at.1.is_finite() {
        return None;
    }
    let x = (f64::from(at.0) - (f64::from(bounds.0) - frame_w * scale) / 2.0) / scale;
    let y = (f64::from(at.1) - (f64::from(bounds.1) - frame_h * scale) / 2.0) / scale;
    Some((x.clamp(0.0, frame_w), y.clamp(0.0, frame_h)))
}

/// The drag in progress and the region it has chosen.
#[derive(Clone, Debug, Default, PartialEq)]
struct Picker {
    drag: Option<(u32, CapturePoint)>,
    selected: Option<AnnotationRegion>,
}

impl Picker {
    /// A new press always begins a fresh drag: a drag whose `End` never arrived (pointer released
    /// outside the element, window blur) must not block the next one.
    fn start(&mut self, id: u32, at: CapturePoint) {
        self.selected = None;
        self.drag = Some((id, at));
    }

    fn update(&mut self, id: u32, at: CapturePoint, done: bool) {
        let Some((held, origin)) = self.drag else {
            return;
        };
        if held != id {
            return;
        }
        self.selected = region(origin, at);
        if done {
            self.drag = None;
        }
    }

    fn cancel(&mut self) {
        *self = Self::default();
    }

    fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveFailure {
    Rejected,
    TimedOut,
}

/// Run `work` until it finishes or `deadline` does. The losing future is dropped, which is what
/// rolls back a half-finished attach.
async fn within<T, E>(
    work: impl Future<Output = Result<T, E>>,
    deadline: impl Future<Output = ()>,
) -> Result<T, SaveFailure> {
    match select(pin!(work), pin!(deadline)).await {
        Either::Left((result, _)) => result.map_err(|_| SaveFailure::Rejected),
        Either::Right(_) => Err(SaveFailure::TimedOut),
    }
}

type Save = Rc<dyn Fn(AnnotationRequest) -> LocalBoxFuture<'static, Result<(), ClientError>>>;

pub(super) fn editor<P: Platform + Clone + 'static>(
    service: BrowserPreview<P>,
    annotation: Rc<Annotation>,
    tasks: TaskOwner,
    close: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    let save: Save = Rc::new(move |request| {
        let service = service.clone();
        async move { service.attach_annotation(request).await }.boxed_local()
    });
    editor_with(save, annotation, tasks, close)
}

/// The editor over any save function: production attaches through the client, tests substitute
/// a save they can hold open.
fn editor_with(
    save_with: Save,
    annotation: Rc<Annotation>,
    // The owner of the stream surface, which outlives the editor: a save started here must reach
    // its rollback or its commit even when the editor is closed or rebuilt under it.
    tasks: TaskOwner,
    close: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    let pixels = annotation.capture.pixels;
    let annotation = stored(annotation);
    let save_with = stored(save_with);
    let bounds = signal((0.0_f32, 0.0_f32));
    let picker = signal_with(Picker::default(), SignalEquals::Never);
    let note = signal(String::new());
    let saving = signal(false);
    let failure = signal(None::<SaveFailure>);
    let save = signal_with(None::<AbortHandle>, SignalEquals::Never);
    let selected = move || picker.with(|picker| picker.selected);
    let note_fits = move || note.with(|note| note.encode_utf16().count() <= NOTE_MAX_UTF16);
    let start = move |id: u32, at: Point<Points>| {
        if saving.get_untracked() {
            return;
        }
        // A press has to land on the image; only the drag may leave it.
        if pixels
            .project(bounds.get_untracked(), (at.x.0, at.y.0))
            .is_none()
        {
            return;
        }
        if let Some(point) = clamped_point(pixels, bounds.get_untracked(), (at.x.0, at.y.0)) {
            picker.update(|picker| picker.start(id, point));
        }
    };
    let update = move |id: u32, at: Point<Points>, done: bool| {
        if let Some(point) = clamped_point(pixels, bounds.get_untracked(), (at.x.0, at.y.0)) {
            picker.update(|picker| picker.update(id, point, done));
        } else if done {
            picker.update(|picker| picker.cancel());
        }
    };
    // Leaving mid-save ends the save: the dropped future removes what it staged.
    let cancel = move || {
        if let Some(handle) = save.get_untracked() {
            handle.abort();
        }
        close();
    };
    // The web editor takes Escape for the whole overlay (PickPreload.ts), not only from the note.
    // NSH's raw key listener is not cross-platform; its keymap handles Escape.
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..Default::default()
        },
        move |_| cancel(),
    );
    let attach = move || {
        let Some(region) = selected() else {
            return;
        };
        if saving.get_untracked() || picker.with_untracked(Picker::dragging) || !note_fits() {
            return;
        }
        let annotation = annotation.get();
        let save_with = save_with.get();
        let Some(png) = crop_png(annotation.capture.jpeg.as_ref(), region) else {
            failure.set(Some(SaveFailure::Rejected));
            return;
        };
        let request = AnnotationRequest {
            thread_id: annotation.page.thread_id.clone(),
            url: annotation.page.url.clone(),
            title: annotation.page.title.clone(),
            page: annotation.page.viewport,
            frame: annotation.capture.pixels,
            region,
            note: note.get_untracked(),
            png,
            created_at: iso_from_epoch_ms(unix_ms() as u64),
        };
        saving.set(true);
        failure.set(None);
        let (work, handle) =
            abortable(async move { within(save_with(request), timer(SAVE_TIMEOUT)).await });
        save.set(Some(handle));
        tasks.spawn(async move {
            match work.await {
                Ok(Ok(())) => close(),
                Ok(Err(reason)) => {
                    failure.try_set(Some(reason));
                    saving.try_set(false);
                }
                Err(_aborted) => {}
            }
        });
    };

    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(
            view()
                .test_id("preview-region-picker")
                .label(t("Highlight a region on the preview"))
                .on_layout(move |bounds_in_view: &Bounds<Points>| {
                    bounds.set((bounds_in_view.size.width.0, bounds_in_view.size.height.0));
                })
                .on_touch(move |event: &TouchEvent| {
                    if event.phase == TouchPhase::Cancel {
                        picker.update(|picker| picker.cancel());
                        return;
                    }
                    for point in event.points.iter().filter(|point| point.changed) {
                        match event.phase {
                            TouchPhase::Start => start(point.id, point.location),
                            TouchPhase::Move | TouchPhase::End => {
                                update(point.id, point.location, event.phase == TouchPhase::End)
                            }
                            TouchPhase::Cancel => {}
                        }
                    }
                })
                .app_class(tw("relative min-h-0 flex-1 overflow-hidden"))
                .child(
                    image(ImageSrc::from(annotation.get().capture.source.clone()))
                        .object_fit(ResizeMode::Contain)
                        .h_full()
                        .w_full()
                        .pointer_events(PointerEvents::None),
                )
                .child(dynamic(move || {
                    let Some(region) = selected() else {
                        return view().into_element();
                    };
                    let (width, height) = bounds.get();
                    let scale = (width / pixels.width as f32).min(height / pixels.height as f32);
                    let left = (width - pixels.width as f32 * scale) / 2.0;
                    let top = (height - pixels.height as f32 * scale) / 2.0;
                    view()
                        .test_id("preview-region-mark")
                        .pointer_events(PointerEvents::None)
                        .left(px(left + region.x as f32 * scale))
                        .top(px(top + region.y as f32 * scale))
                        .w(px(region.width as f32 * scale))
                        .h(px(region.height as f32 * scale))
                        .app_class(tw("absolute border-2 border-primary bg-primary/20"))
                        .into_element()
                })),
        )
        .child(
            view()
                .app_class(tw(
                    "flex shrink-0 flex-col gap-2 border-t border-border p-2",
                ))
                .child(
                    nsh_base::input()
                        .label(t("Annotation note"))
                        .placeholder(t("Annotation note"))
                        .disabled(saving)
                        .on_change(move |value: &str| note.set(value.to_owned()))
                        .submit_behavior(SubmitBehavior::Submit)
                        .on_submit(move |_: &str| attach())
                        .app_class(tw("h-8 rounded-md border border-border px-2 text-sm")),
                )
                .child(when(
                    move || !note_fits(),
                    || {
                        text(t("The note is too long to attach."))
                            .role(AccessibilityRole::Alert)
                            .app_class(tw("text-sm text-destructive"))
                    },
                ))
                .child(when(
                    move || failure.get().is_some(),
                    move || {
                        text(match failure.get() {
                            Some(SaveFailure::TimedOut) => {
                                t("Attaching the preview annotation timed out.")
                            }
                            _ => t("Could not attach the preview annotation."),
                        })
                        .role(AccessibilityRole::Alert)
                        .app_class(tw("text-sm text-destructive"))
                    },
                ))
                .child(
                    view()
                        .app_class(tw("flex items-center justify-end gap-2"))
                        .child(button(
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Ghost,
                            ButtonSize::Sm,
                            || false,
                            cancel,
                        ))
                        .child(button(
                            ButtonContent::Label(t("Attach annotation")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || {
                                selected().is_none()
                                    || picker.with(Picker::dragging)
                                    || saving.get()
                                    || !note_fits()
                            },
                            attach,
                        )),
                ),
        )
}
#[cfg(test)]
mod tests {
    use std::{cell::Cell, future::pending, rc::Rc};

    use futures::executor::block_on;

    use super::*;

    const FRAME: Viewport = Viewport {
        width: 200,
        height: 100,
    };

    fn rect(x: u32, y: u32, width: u32, height: u32) -> Option<AnnotationRegion> {
        Some(AnnotationRegion {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn a_drag_in_any_direction_makes_the_same_region() {
        let forward = region((10.2, 20.7), (30.1, 40.0));
        let backward = region((30.1, 40.0), (10.2, 20.7));
        assert_eq!(forward, backward);
        assert_eq!(forward, rect(10, 20, 21, 20));
    }

    #[test]
    fn a_tap_a_line_or_a_non_number_selects_nothing() {
        assert_eq!(region((5.0, 5.0), (5.0, 5.0)), None);
        assert_eq!(region((5.0, 5.0), (50.0, 5.0)), None);
        assert_eq!(region((5.0, 5.0), (5.0, 50.0)), None);
        assert_eq!(region((f64::NAN, 5.0), (50.0, 50.0)), None);
        assert_eq!(region((5.0, 5.0), (50.0, f64::NAN)), None);
        assert_eq!(region((5.0, f64::INFINITY), (50.0, 50.0)), None);
    }

    #[test]
    fn a_drag_past_the_image_edge_ends_on_the_edge() {
        // A 400 x 200 picker over a 200 x 100 frame: scale 2, no letterbox.
        let bounds = (400.0, 200.0);
        assert_eq!(
            clamped_point(FRAME, bounds, (500.0, 250.0)),
            Some((200.0, 100.0))
        );
        assert_eq!(
            clamped_point(FRAME, bounds, (-30.0, -1.0)),
            Some((0.0, 0.0))
        );
        let end = clamped_point(FRAME, bounds, (900.0, 90.0)).expect("point");
        assert_eq!(region((100.0, 10.0), end), rect(100, 10, 100, 35));
        // Letterbox bars (a taller picker) pin to the frame, not past it.
        assert_eq!(
            clamped_point(FRAME, (400.0, 400.0), (200.0, 0.0)),
            Some((100.0, 0.0))
        );
    }

    #[test]
    fn a_non_number_pointer_position_is_ignored() {
        assert_eq!(clamped_point(FRAME, (400.0, 200.0), (f32::NAN, 1.0)), None);
        assert_eq!(clamped_point(FRAME, (0.0, 0.0), (1.0, 1.0)), None);
    }

    #[test]
    fn a_new_press_replaces_a_drag_whose_end_never_came() {
        let mut picker = Picker::default();
        picker.start(1, (10.0, 10.0));
        picker.update(1, (50.0, 50.0), false);
        assert!(picker.dragging());
        // The release was missed. A second press must not be ignored.
        picker.start(2, (100.0, 20.0));
        assert!(picker.dragging());
        assert_eq!(picker.selected, None);
        // The first pointer's late events no longer move the selection.
        picker.update(1, (60.0, 60.0), true);
        assert_eq!(picker.selected, None);
        picker.update(2, (150.0, 70.0), true);
        assert_eq!(picker.selected, rect(100, 20, 50, 50));
        assert!(!picker.dragging());
    }

    #[test]
    fn cancel_clears_the_drag_and_the_selection() {
        let mut picker = Picker::default();
        picker.start(1, (10.0, 10.0));
        picker.update(1, (50.0, 50.0), false);
        picker.cancel();
        assert_eq!(picker, Picker::default());
    }

    struct Dropped(Rc<Cell<bool>>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    #[test]
    fn a_save_that_misses_its_deadline_fails_and_is_dropped() {
        let dropped = Rc::new(Cell::new(false));
        let guard = Dropped(dropped.clone());
        let work = async move {
            let _guard = guard;
            pending::<Result<(), ()>>().await
        };
        let result = block_on(within(work, async {}));
        assert_eq!(result, Err(SaveFailure::TimedOut));
        assert!(
            dropped.get(),
            "the stalled save must be dropped to roll back"
        );
    }

    #[test]
    fn a_save_that_finishes_first_reports_its_own_outcome() {
        assert_eq!(block_on(within(async { Ok::<_, ()>(7) }, pending())), Ok(7));
        assert_eq!(
            block_on(within(async { Err::<(), _>("no") }, pending())),
            Err(SaveFailure::Rejected)
        );
    }

    mod editor {
        use nsh_engine::{EngineConfig, Root};
        use nsh_testing::Headless;

        use super::*;

        /// The editor over a save that never finishes. `dropped` flips when that save is dropped,
        /// which is what rolls back whatever a real save had staged (see `Staged`).
        #[component]
        fn HungSave() -> impl IntoElement {
            let open = signal(true);
            let started = signal(false);
            let dropped = signal(false);
            let save: Save = Rc::new(move |_| {
                started.set(true);
                async move {
                    let _signal = SignalOnDrop(dropped);
                    pending::<Result<(), ClientError>>().await
                }
                .boxed_local()
            });
            let annotation = Rc::new(Annotation {
                capture: Capture {
                    source: "data:image/jpeg;base64,".into(),
                    jpeg: Rc::new(include_bytes!("../../../tests/ui/prv_2.jpg").to_vec()),
                    pixels: Viewport {
                        width: 8,
                        height: 6,
                    },
                    page: None,
                },
                page: PageIdentity {
                    thread_id: "thread".into(),
                    url: "https://example.test/".into(),
                    title: None,
                    viewport: Viewport {
                        width: 800,
                        height: 600,
                    },
                },
            });
            let tasks = TaskOwner::here();
            view()
                .app_class(tw("flex h-full w-full flex-col"))
                .child(when(open, move || {
                    editor_with(save.clone(), annotation.clone(), tasks, move || {
                        open.set(false)
                    })
                    .into_element()
                }))
                .child(when(move || !open.get(), || text("Editor closed")))
                .child(when(started, || text("Save started")))
                .child(when(dropped, || text("Save dropped")))
        }

        struct SignalOnDrop(Signal<bool>);

        impl Drop for SignalOnDrop {
            fn drop(&mut self) {
                self.0.try_set(true);
            }
        }

        fn start() -> Headless {
            let mut sets = crate::registries();
            let mut fixtures = Registry::new();
            fixtures.register::<HungSave>();
            sets.push(fixtures);
            Headless::start(EngineConfig {
                ui_sets: sets,
                viewport: size(px(390.0), px(844.0)),
                ..EngineConfig::new(Root::of::<HungSave>())
            })
            .expect("editor probe starts")
        }

        #[test]
        fn cancel_during_a_save_aborts_it_and_closes_the_editor() {
            let mut app = start();
            app.wait_for("label:Highlight a region on the preview")
                .expect("editor");
            app.drag(
                "label:Highlight a region on the preview",
                point(px(40.0), px(30.0)),
            )
            .expect("mark a region");
            app.press("role:button[Attach annotation]").expect("attach");
            app.wait_for("Save started").expect("the save began");
            assert!(!app.screen().shows("Save dropped"), "it is still held open");
            app.press("role:button[Cancel]").expect("cancel mid-save");
            app.wait_for("Save dropped")
                .expect("cancelling dropped the save, so its staged image is rolled back");
            app.wait_for("Editor closed").expect("editor closed");
        }

        #[test]
        fn an_over_long_note_says_why_attach_is_unavailable() {
            let mut app = start();
            app.wait_for("label:Highlight a region on the preview")
                .expect("editor");
            app.drag(
                "label:Highlight a region on the preview",
                point(px(40.0), px(30.0)),
            )
            .expect("mark a region");
            app.type_text("label:Annotation note", &"x".repeat(NOTE_MAX_UTF16 + 1))
                .expect("type past the limit");
            app.wait_for("role:alert[The note is too long to attach.]")
                .expect("the reason is shown");
            app.press_key("enter").expect("Enter is refused");
            assert!(!app.screen().shows("Save started"));
        }
    }
}
