//! Port of ServerBrowserSurface.tsx, using the managed browser's JPEG stream.

use futures::channel::mpsc;
use nsh_core::{
    KeyEvent, TouchEvent, TouchPhase,
    element::ImageSrc,
    keymap::Modifiers,
    platform::clipboard::{image_mime, image_size},
    prelude::*,
};
use t3_client::{
    platform::Platform,
    preview::{
        PreviewControl, PreviewEvent,
        browser::BrowserPreview,
        preview_modifiers,
        socket::{BrowserInput, KeyAction, MouseAction, MouseButton, PagePoint, Viewport},
    },
    workspace::project_files::assets::preview_data_url,
};
use t3_contracts::preview::PreviewTabId;

use crate::{
    i18n::t,
    runtime::tasks::watch,
    state::layout::{Pointer, pointer},
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};

const MAX_JPEG_BYTES: usize = 12 * 1024 * 1024;
const INPUT_CAPACITY: usize = 96;

#[derive(Clone, Copy)]
pub(super) struct StreamControls {
    sender: Stored<Option<mpsc::Sender<BrowserInput>>>,
    control: Signal<Option<PreviewControl>>,
}

impl StreamControls {
    pub(super) fn new() -> Self {
        Self {
            sender: stored(None),
            control: signal(None),
        }
    }

    pub(super) fn controls_page(self) -> bool {
        self.control.with(controlled)
    }

    pub(super) fn send(self, input: BrowserInput) {
        self.sender.with(|sender| {
            if let Some(tx) = sender.as_mut()
                && tx.try_send(input).is_err_and(|error| error.is_full())
            {
                // A stalled connection must release input rather than lose a key-up.
                tx.close_channel();
            }
        });
    }
}

fn controlled(control: &Option<PreviewControl>) -> bool {
    control
        .as_ref()
        .is_some_and(|control| control.can_operate && control.controller == "you")
}

pub(super) fn server_browser_surface<P: Platform + Clone + 'static>(
    service: BrowserPreview<P>,
    tab: PreviewTabId,
    controls: StreamControls,
) -> impl IntoElement {
    let service = stored(service);
    let tab = stored(tab);
    let size = signal((0.0_f32, 0.0_f32));
    let page = signal(None::<Viewport>);
    let frame = signal(None::<SharedString>);
    let failed = signal(false);
    let entry_ref = element_ref();
    let entry_focused = signal(false);
    let pending_probe = signal(None::<(f64, f64)>);
    let revision = signal(0_u64);
    let measured = computed(move || {
        let (width, height) = size.get();
        Viewport::for_container(width, height)
    });
    effect(move || {
        if let Some(size) = measured.get()
            && controls.controls_page()
        {
            controls.send(BrowserInput::Resize {
                width: size.width,
                height: size.height,
            });
        }
    });

    effect(move || {
        revision.get();
        let service = service.get();
        let tab = tab.get();
        let bounds = untracked(window);
        frame.set(None);
        page.set(None);
        failed.set(false);
        controls.control.set(None);
        let (send, receive) = mpsc::channel(INPUT_CAPACITY);
        let _ = controls.sender.replace(Some(send));
        watch(async move {
            let result = service
                .connect_stream(tab, (bounds.width.0 as u32, bounds.height.0 as u32))
                .await;
            if let Ok(socket) = result {
                let _ = socket
                    .run(receive, |event| match event {
                        PreviewEvent::Control(current) => controls.control.set(Some(current)),
                        PreviewEvent::Viewport { width, height } => {
                            page.set(Viewport::from_page(width, height))
                        }
                        PreviewEvent::Probe { x, y, editable } => {
                            if pending_probe.get_untracked() == Some((x, y)) {
                                pending_probe.set(None);
                                if editable && controlled(&controls.control.get_untracked()) {
                                    entry_ref.focus();
                                }
                            }
                        }
                        PreviewEvent::Frame(bytes) => {
                            if let Some(jpeg) = jpeg_source(&bytes) {
                                frame.set(Some(SharedString::from(jpeg)));
                            }
                        }
                        _ => {}
                    })
                    .await;
            }
            controls.control.set(None);
            let _ = controls.sender.replace(None);
            frame.set(None);
            page.set(None);
            failed.set(true);
        });
    });
    cleanup(move || {
        controls.control.try_set(None);
        let _ = controls.sender.try_with(|sender| *sender = None);
    });

    let focus = element_ref();
    let coarse = pointer() == Pointer::Coarse;
    let touch = signal(None::<TouchPan>);
    let down = signal_with(None::<(PagePoint, u32)>, SignalEquals::Never);
    effect(move || {
        if !controls.controls_page() {
            down.set(None);
            touch.set(None);
            pending_probe.set(None);
            if entry_focused.get_untracked() {
                entry_ref.blur();
            }
        }
    });
    let position = move |at: Point<Points>| {
        if !frame.with_untracked(Option::is_some) {
            return None;
        }
        page.get_untracked()?
            .project(size.get_untracked(), (at.x.0, at.y.0))
    };
    let mouse =
        move |at: PagePoint, action, button: MouseButton, buttons, click_count, modifiers| {
            controls.send(BrowserInput::Mouse {
                action,
                x: at.x,
                y: at.y,
                button,
                buttons,
                click_count,
                modifiers,
            });
        };

    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(when(move || controls.control.get().is_some(), move || control_bar(controls)))
        .child(when(move || failed.get(), move || {
            view().app_class(tw("flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center"))
                .child(text(t("Browser unavailable")).app_class(tw("text-sm text-muted-foreground")))
                .child(button(ButtonContent::Label(t("Try again")), ButtonVariant::Outline, ButtonSize::Sm,
                    || false, move || revision.update(|value| *value += 1)))
        }).otherwise(move || {
            view()
                .element_ref(&focus)
                .focusable(move || controls.controls_page())
                .capture_keys(move || if controls.controls_page() && !entry_focused.get() {
                    KeyCapture::All
                } else {
                    KeyCapture::None
                })
                .test_id("preview-page")
                .label(t("Streamed browser page"))
                .on_layout(move |bounds: &Bounds<Points>| size.set((bounds.size.width.0, bounds.size.height.0)))
                .on_press(move |event: &PressEvent| {
                    if coarse && controlled(&controls.control.get_untracked())
                        && let Some(at) = position(event.position)
                    {
                        focus.focus();
                        mouse(at, MouseAction::Down, MouseButton::Left, 1, 1, 0);
                        mouse(at, MouseAction::Up, MouseButton::Left, 0, 1, 0);
                        pending_probe.set(Some((at.x, at.y)));
                        controls.send(BrowserInput::Probe { x: at.x, y: at.y });
                    }
                })
                .on_touch(move |event: &TouchEvent| {
                    if coarse && controlled(&controls.control.get_untracked()) {
                        scroll_touch(touch, position, controls, event);
                    }
                })
                .on_press_down(move |event: &PressEvent| {
                    if coarse || !controlled(&controls.control.get_untracked()) {
                        return;
                    }
                    if let Some(at) = position(event.position) {
                        focus.focus();
                        let clicks = event.clicks.max(1);
                        down.set(Some((at, clicks)));
                        mouse(at, MouseAction::Down, MouseButton::Left, MouseButton::Left.mask(), clicks, mods(event.modifiers));
                    }
                })
                .on_press_up(move |event: &PressEvent| {
                    if let Some((start, clicks)) = down.get_untracked() {
                        down.set(None);
                        let at = position(event.position).unwrap_or(start);
                        mouse(at, MouseAction::Up, MouseButton::Left, 0, clicks, mods(event.modifiers));
                    }
                })
                .on_mouse_move(move |event: &MouseEvent| {
                    if controlled(&controls.control.get_untracked())
                        && let Some(at) = position(event.position)
                    {
                        let button = if down.get_untracked().is_some() { MouseButton::Left } else { MouseButton::None };
                        mouse(at, MouseAction::Move, button, button.mask(), 0, mods(event.modifiers));
                    }
                })
                .on_wheel(move |event: &WheelEvent| {
                    if controlled(&controls.control.get_untracked())
                        && let Some(at) = position(event.position)
                    {
                        let delta = event.delta.points(px(16.0));
                        controls.send(BrowserInput::Wheel {
                            x: at.x, y: at.y,
                            delta_x: f64::from(delta.x.0) / at.scale,
                            delta_y: f64::from(delta.y.0) / at.scale,
                            modifiers: mods(event.modifiers),
                        });
                    }
                })
                .on_key_down(move |event: &KeyEvent| {
                    if event.key.as_str() == "Escape" && event.modifiers.shift {
                        focus.blur();
                    } else if !entry_focused.get_untracked() {
                        forward_key(controls, KeyAction::Down, event);
                    }
                })
                .on_key_up(move |event: &KeyEvent| {
                    if !entry_focused.get_untracked() {
                        forward_key(controls, KeyAction::Up, event);
                    }
                })
                .app_class(tw("relative flex min-h-0 flex-1 items-center justify-center overflow-hidden"))
                .child(coarse.then(|| mobile_entry(controls, entry_ref, entry_focused)))
                .child(when(move || frame.with(Option::is_some), move || {
                    image(move || ImageSrc::from(frame.get().unwrap_or_default()))
                        .object_fit(ResizeMode::Contain)
                        .pointer_events(PointerEvents::None)
                        .w_full()
                        .h_full()
                        .test_id("preview-jpeg")
                }).otherwise(|| text(t("Connecting to browser…")).app_class(tw("text-sm text-muted-foreground"))))
        }))
}

fn mobile_entry(
    controls: StreamControls,
    entry: ElementRef,
    focused: Signal<bool>,
) -> impl IntoElement {
    // A sentinel keeps native Backspace observable when the remote field owns the text.
    const SENTINELS: [char; 2] = ['\u{200b}', '\u{200c}'];
    let alternate = signal(false);
    nsh_base::input()
        .element_ref(&entry)
        .test_id("preview-mobile-input")
        .label(t("Browser page input"))
        .value(move || SENTINELS[usize::from(alternate.get())].to_string())
        .on_focus(move || focused.set(true))
        .on_blur(move || focused.set(false))
        .on_change(move |value: &str| {
            if !focused.get_untracked() {
                return;
            }
            let text = value.trim_start_matches(SENTINELS);
            if value.is_empty() {
                entry_key(controls, "Backspace", None);
            } else if !text.is_empty() {
                controls.send(BrowserInput::Text {
                    text: text.to_owned(),
                });
            }
            alternate.update(|state| *state = !*state);
        })
        .submit_behavior(SubmitBehavior::Submit)
        .on_submit(move |_: &str| entry_key(controls, "Enter", Some("\r")))
        .pointer_events(PointerEvents::None)
        .app_class(tw("absolute bottom-0 left-0"))
        .w(px(1.0))
        .h(px(1.0))
        .opacity(0.01)
}

fn entry_key(controls: StreamControls, key: &str, text: Option<&str>) {
    for action in [KeyAction::Down, KeyAction::Up] {
        controls.send(BrowserInput::Key {
            action,
            key: key.to_owned(),
            code: key.to_owned(),
            text: if matches!(action, KeyAction::Down) {
                text.map(str::to_owned)
            } else {
                None
            },
            modifiers: 0,
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
struct TouchPan {
    id: u32,
    previous: Point<Points>,
    panning: bool,
}

fn scroll_touch(
    pan: Signal<Option<TouchPan>>,
    position: impl Fn(Point<Points>) -> Option<PagePoint>,
    controls: StreamControls,
    event: &TouchEvent,
) {
    if event.phase == TouchPhase::Cancel {
        pan.set(None);
        return;
    }
    let previous = pan.get_untracked();
    let Some(point) = event.points.iter().find(|point| {
        point.changed
            && point.pointer_type == PointerType::Touch
            && previous.is_none_or(|pan| pan.id == point.id)
    }) else {
        return;
    };
    match event.phase {
        TouchPhase::Start => {
            if previous.is_none() && position(point.location).is_some() {
                pan.set(Some(TouchPan {
                    id: point.id,
                    previous: point.location,
                    panning: false,
                }));
            }
        }
        TouchPhase::Move => {
            let Some(mut current) = previous else { return };
            let delta = current.previous - point.location;
            if !current.panning && delta.x.0.hypot(delta.y.0) < 6.0 {
                return;
            }
            if let Some(at) = position(point.location) {
                controls.send(BrowserInput::Wheel {
                    x: at.x,
                    y: at.y,
                    delta_x: f64::from(delta.x.0) / at.scale,
                    delta_y: f64::from(delta.y.0) / at.scale,
                    modifiers: 0,
                });
            }
            current.previous = point.location;
            current.panning = true;
            pan.set(Some(current));
        }
        TouchPhase::End | TouchPhase::Cancel => pan.set(None),
    }
}

fn jpeg_source(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_JPEG_BYTES || image_mime(bytes) != Some("image/jpeg") {
        return None;
    }
    let (width, height) = image_size(bytes)?;
    Viewport::from_page(f64::from(width), f64::from(height))?;
    Some(preview_data_url("image/jpeg", bytes))
}

fn mods(modifiers: Modifiers) -> u8 {
    preview_modifiers(
        modifiers.alt,
        modifiers.ctrl,
        modifiers.cmd,
        modifiers.shift,
    )
}

fn forward_key(controls: StreamControls, action: KeyAction, event: &KeyEvent) {
    if !controlled(&controls.control.get_untracked())
        || matches!(event.key.as_str(), "Process" | "Unidentified")
    {
        return;
    }
    let key = event.key.to_string();
    let text = if matches!(action, KeyAction::Down) && !event.modifiers.ctrl && !event.modifiers.cmd
    {
        match key.as_str() {
            "Enter" => Some(String::from("\r")),
            key if key.chars().count() == 1 => Some(key.to_owned()),
            _ => None,
        }
    } else {
        None
    };
    controls.send(BrowserInput::Key {
        action,
        key,
        code: event.code.to_string(),
        text,
        modifiers: mods(event.modifiers),
    });
}

fn control_bar(controls: StreamControls) -> impl IntoElement {
    view()
        .app_class(tw(
            "flex shrink-0 items-center justify-between border-b border-border/60 px-2",
        ))
        .child(
            text(move || {
                controls.control.with(|control| {
                    control.as_ref().map_or_else(String::new, |control| {
                        crate::i18n::Text::client_key(control.label())
                            .resolve()
                            .to_string()
                    })
                })
            })
            .app_class(tw("text-xs text-muted-foreground")),
        )
        .child(when(
            move || {
                controls
                    .control
                    .with(|control| control.as_ref().is_some_and(|state| state.can_operate))
            },
            move || {
                dynamic(move || {
                    let own = controls.controls_page();
                    button(
                        ButtonContent::Label(if own {
                            t("Release control")
                        } else {
                            t("Take control")
                        }),
                        ButtonVariant::Ghost,
                        ButtonSize::Xs,
                        || false,
                        move || {
                            controls.send(if own {
                                BrowserInput::ReleaseControl
                            } else {
                                BrowserInput::TakeControl
                            });
                        },
                    )
                })
            },
        ))
}
