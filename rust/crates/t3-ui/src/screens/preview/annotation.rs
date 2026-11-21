//! Region selection on a frozen frame from the managed browser viewer.

use std::rc::Rc;

use nsh_core::{TouchEvent, TouchPhase, element::ImageSrc, prelude::*};
use t3_client::{
    platform::Platform,
    preview::{
        annotation::AnnotationRegion,
        browser::BrowserPreview,
        socket::{PagePoint, Viewport},
    },
};
use t3_contracts::preview::PreviewSessionSnapshot;

use crate::{
    i18n::t,
    runtime::tasks::TaskOwner,
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};

#[derive(Clone)]
pub(super) struct Capture {
    pub source: SharedString,
    pub jpeg: Rc<Vec<u8>>,
    pub pixels: Viewport,
}

pub(super) struct Annotation {
    pub capture: Capture,
    pub snapshot: PreviewSessionSnapshot,
}

type CapturePoint = (f64, f64);

fn region(start: CapturePoint, end: CapturePoint) -> Option<AnnotationRegion> {
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

pub(super) fn editor<P: Platform + Clone + 'static>(
    service: BrowserPreview<P>,
    annotation: Rc<Annotation>,
    close: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    let pixels = annotation.capture.pixels;
    let annotation = stored(annotation);
    let service = stored(service);
    let tasks = TaskOwner::here();
    let bounds = signal((0.0_f32, 0.0_f32));
    let selected = signal(None::<AnnotationRegion>);
    let drag = signal(None::<(u32, CapturePoint)>);
    let note = signal(String::new());
    let saving = signal(false);
    let failed = signal(false);
    let start = move |id, at: Point<Points>| {
        if saving.get_untracked() || drag.get_untracked().is_some() {
            return;
        }
        if let Some(PagePoint { x, y, .. }) =
            pixels.project(bounds.get_untracked(), (at.x.0, at.y.0))
        {
            selected.set(None);
            drag.set(Some((id, (x, y))));
        }
    };
    let update = move |id, at: Point<Points>, done| {
        let Some((held, origin)) = drag.get_untracked() else {
            return;
        };
        if held != id {
            return;
        }
        if let Some(PagePoint { x, y, .. }) =
            pixels.project(bounds.get_untracked(), (at.x.0, at.y.0))
        {
            selected.set(region(origin, (x, y)));
        }
        if done {
            drag.set(None);
        }
    };
    let attach = move || {
        let Some(region) = selected.get_untracked() else {
            return;
        };
        if saving.get_untracked() || drag.get_untracked().is_some() {
            return;
        }
        let annotation = annotation.get();
        let service = service.get();
        let note = note.get_untracked();
        saving.set(true);
        failed.set(false);
        tasks.spawn(async move {
            let result = service
                .attach_annotation(
                    &annotation.snapshot,
                    annotation.capture.pixels,
                    region,
                    &note,
                    annotation.capture.jpeg.as_ref().clone(),
                )
                .await;
            if result.is_ok() {
                close();
            } else {
                failed.set(true);
                saving.set(false);
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
                        drag.set(None);
                        selected.set(None);
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
                    let Some(region) = selected.get() else {
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
                        .max_length(1000)
                        .disabled(saving)
                        .on_change(move |value: &str| note.set(value.to_owned()))
                        .app_class(tw("h-8 rounded-md border border-border px-2 text-sm")),
                )
                .child(when(failed, || {
                    text(t("Could not attach the preview annotation."))
                        .app_class(tw("text-sm text-destructive"))
                }))
                .child(
                    view()
                        .app_class(tw("flex items-center justify-end gap-2"))
                        .child(button(
                            ButtonContent::Label(t("Cancel")),
                            ButtonVariant::Ghost,
                            ButtonSize::Sm,
                            move || saving.get(),
                            close,
                        ))
                        .child(button(
                            ButtonContent::Label(t("Attach annotation")),
                            ButtonVariant::Default,
                            ButtonSize::Sm,
                            move || {
                                selected.get().is_none() || drag.get().is_some() || saving.get()
                            },
                            attach,
                        )),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_in_any_direction_makes_the_same_region() {
        let forward = region((10.2, 20.7), (30.1, 40.0));
        let backward = region((30.1, 40.0), (10.2, 20.7));
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            Some(AnnotationRegion {
                x: 10,
                y: 20,
                width: 21,
                height: 20
            })
        );
    }

    #[test]
    fn a_tap_or_a_line_selects_nothing() {
        assert_eq!(region((5.0, 5.0), (5.0, 5.0)), None);
        assert_eq!(region((5.0, 5.0), (50.0, 5.0)), None);
        assert_eq!(region((5.0, 5.0), (5.0, 50.0)), None);
    }
}
