//! Port of ServerBrowserSurface.tsx, using the managed browser's JPEG stream.

use std::time::Duration;

use futures::channel::mpsc;
use nsh_core::{
    KeyEvent, TouchEvent, TouchPhase,
    element::ImageSrc,
    keymap::{Keystroke, Modifiers},
    platform::clipboard::{image_mime, image_size},
    prelude::*,
    time::{Instant, now},
};
use t3_client::{
    platform::Platform,
    preview::{
        PreviewControl, PreviewEvent,
        browser::BrowserPreview,
        host_setup_message, preview_modifiers,
        socket::{BrowserInput, KeyAction, MouseAction, MouseButton, PagePoint, Viewport},
    },
    workspace::project_files::assets::preview_data_url,
};
use t3_contracts::preview::{PreviewStreamHostSetup, PreviewTabId};

use crate::{
    i18n::t,
    runtime::tasks::{TaskOwner, watch},
    state::layout::{Pointer, pointer},
    theme::tw,
    ui::button::{ButtonContent, ButtonSize, ButtonVariant, button},
};

const RESIZE_DELAY: Duration = Duration::from_millis(150);
const SEND_DEADLINE: Duration = Duration::from_secs(5);
const COPY_DEADLINE: Duration = Duration::from_secs(3);
const MAX_QUEUED_INPUTS: usize = 2048;
const RELEASE_RESERVE: usize = 128;

#[derive(Clone, Copy)]
pub(super) struct StreamControls {
    sender: Stored<Option<mpsc::UnboundedSender<BrowserInput>>>,
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
        self.try_send(input);
    }

    pub(super) fn try_send(self, input: BrowserInput) -> bool {
        self.sender
            .with(|sender| sender.as_ref().is_some_and(|tx| enqueue(tx, input)))
    }
}

fn enqueue(sender: &mpsc::UnboundedSender<BrowserInput>, input: BrowserInput) -> bool {
    // Keep room to release keys/buttons under backpressure. The writer owns the stall deadline.
    let limit = match input {
        BrowserInput::Key {
            action: KeyAction::Up,
            ..
        }
        | BrowserInput::Mouse {
            action: MouseAction::Up,
            ..
        }
        | BrowserInput::ReleaseControl => MAX_QUEUED_INPUTS,
        _ => MAX_QUEUED_INPUTS - RELEASE_RESERVE,
    };
    sender.len() < limit && sender.unbounded_send(input).is_ok()
}

#[derive(Default)]
struct PendingMotion {
    pointer: Option<BrowserInput>,
    wheel: Option<WheelMotion>,
    scheduled: bool,
}

struct WheelMotion {
    at: PagePoint,
    delta_x: f64,
    delta_y: f64,
    modifiers: u8,
}

#[derive(Clone, Copy)]
struct Motion {
    state: Stored<PendingMotion>,
    controls: StreamControls,
    tasks: TaskOwner,
}

impl Motion {
    fn pointer(self, input: BrowserInput) {
        self.state.with(|pending| pending.pointer = Some(input));
        self.schedule();
    }

    fn wheel(self, mut wheel: WheelMotion) {
        self.state.with(|pending| {
            if let Some(previous) = &pending.wheel {
                wheel.delta_x += previous.delta_x;
                wheel.delta_y += previous.delta_y;
            }
            pending.wheel = Some(wheel);
        });
        self.schedule();
    }

    fn schedule(self) {
        if self
            .state
            .with(|pending| std::mem::replace(&mut pending.scheduled, true))
        {
            return;
        }
        self.tasks.spawn(async move {
            next_frame().await;
            self.state.with(|pending| pending.scheduled = false);
            self.flush();
        });
    }

    fn flush(self) {
        let (pointer, wheel) = self
            .state
            .with(|pending| (pending.pointer.take(), pending.wheel.take()));
        if !untracked(|| self.controls.controls_page()) {
            return;
        }
        if let Some(input) = pointer {
            self.controls.send(input);
        }
        if let Some(wheel) = wheel {
            self.controls.send(BrowserInput::Wheel {
                x: wheel.at.x,
                y: wheel.at.y,
                delta_x: wheel.delta_x,
                delta_y: wheel.delta_y,
                modifiers: wheel.modifiers,
            });
        }
    }
}

#[derive(Clone, PartialEq)]
enum StreamFailure {
    Unavailable,
    Unauthorized,
    Gone,
    HostSetup(PreviewStreamHostSetup),
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
    let failed = signal(None::<StreamFailure>);
    let tasks = TaskOwner::here();
    let motion = Motion {
        state: stored(PendingMotion::default()),
        controls,
        tasks,
    };
    let clipboard = ClipboardShortcuts {
        controls,
        tasks,
        paste_keys: stored([false; 2]),
        pending_copy: stored(None),
        interaction: stored(0),
    };
    let entry_ref = element_ref();
    let entry_focused = signal(false);
    let pending_probe = signal(None::<(f64, f64)>);
    let revision = signal(0_u64);
    let measured = computed(move || {
        let (width, height) = size.get();
        Viewport::for_container(width, height)
    });
    let owns_page = computed(move || controls.controls_page());
    effect(move || {
        if let Some(size) = measured.get()
            && owns_page.get()
        {
            let task = watch(async move {
                timer(RESIZE_DELAY).await;
                controls.send(BrowserInput::Resize {
                    width: size.width,
                    height: size.height,
                });
            });
            cleanup(move || task.cancel());
        }
    });

    effect(move || {
        revision.get();
        let service = service.get();
        let tab = tab.get();
        let bounds = untracked(window);
        frame.set(None);
        page.set(None);
        failed.set(None);
        controls.control.set(None);
        let (send, receive) = mpsc::unbounded();
        clipboard.reset();
        clipboard.pending_copy.replace(None);
        let _ = controls.sender.replace(Some(send));
        watch(async move {
            let result = service
                .connect_stream(tab, (bounds.width.0 as u32, bounds.height.0 as u32))
                .await;
            let mut failure = StreamFailure::Unavailable;
            match result {
                Ok(socket) => {
                    let _ = socket
                        .run(
                            receive,
                            || timer(SEND_DEADLINE),
                            |event| match event {
                                PreviewEvent::Control(current) => {
                                    if controls.control.with_untracked(|previous| {
                                        previous.as_ref().is_none_or(|previous| {
                                            previous.generation != current.generation
                                                || previous.controller != current.controller
                                        })
                                    }) {
                                        clipboard.reset();
                                    }
                                    controls.control.set(Some(current));
                                }
                                PreviewEvent::Viewport { width, height } => {
                                    page.set(Viewport::from_page(width, height))
                                }
                                PreviewEvent::Probe { x, y, editable } => {
                                    if pending_probe.get_untracked() == Some((x, y)) {
                                        pending_probe.set(None);
                                        if editable && untracked(|| controls.controls_page()) {
                                            entry_ref.focus();
                                        }
                                    }
                                }
                                PreviewEvent::Frame(bytes) => {
                                    if let Some(jpeg) = jpeg_source(&bytes) {
                                        frame.set(Some(SharedString::from(jpeg)));
                                    }
                                }
                                PreviewEvent::Clipboard(value) => clipboard.copy_out(value),
                                PreviewEvent::Unauthorized => failure = StreamFailure::Unauthorized,
                                PreviewEvent::Gone => failure = StreamFailure::Gone,
                                PreviewEvent::HostSetup(setup) => {
                                    failure = StreamFailure::HostSetup(setup)
                                }
                                _ => {}
                            },
                        )
                        .await;
                }
                Err(problem) if problem.is_unauthorized() => failure = StreamFailure::Unauthorized,
                Err(_) => {}
            }
            controls.control.set(None);
            let _ = controls.sender.replace(None);
            frame.set(None);
            page.set(None);
            failed.set(Some(failure));
        });
    });
    cleanup(move || {
        controls.control.try_set(None);
        let _ = controls.sender.try_with(|sender| *sender = None);
    });

    let focus = element_ref();
    let coarse = pointer() == Pointer::Coarse;
    let web_text_field = cfg!(target_arch = "wasm32");
    let web_hardware_keyboard = web_text_field && !coarse;
    let touch = signal(None::<TouchPan>);
    let down = signal_with(None::<(PagePoint, u32)>, SignalEquals::Never);
    effect(move || {
        if !controls.controls_page() {
            down.set(None);
            touch.set(None);
            pending_probe.set(None);
            motion.flush();
            clipboard.reset();
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
            let input = BrowserInput::Mouse {
                action,
                x: at.x,
                y: at.y,
                button,
                buttons,
                click_count,
                modifiers,
            };
            if matches!(action, MouseAction::Move) {
                motion.pointer(input);
            } else {
                if matches!(action, MouseAction::Down) {
                    clipboard.invalidate();
                }
                motion.flush();
                controls.send(input);
            }
        };

    view()
        .app_class(tw("flex min-h-0 flex-1 flex-col"))
        .child(when(
            move || controls.control.get().is_some(),
            move || control_bar(controls),
        ))
        .child(
            when(
                move || failed.with(Option::is_some),
                move || {
                    dynamic(move || match failed.get() {
                        Some(failure) => stream_failure(failure, revision).into_element(),
                        None => view().into_element(),
                    })
                },
            )
            .otherwise(move || {
                view()
                    .element_ref(&focus)
                    .focusable(move || controls.controls_page())
                    .capture_keys(move || {
                        if controls.controls_page() && !entry_focused.get() {
                            KeyCapture::All
                        } else {
                            KeyCapture::None
                        }
                    })
                    .test_id("preview-page")
                    .on_blur(move || clipboard.reset())
                    .label(t("Streamed browser page"))
                    .on_layout(move |bounds: &Bounds<Points>| {
                        size.set((bounds.size.width.0, bounds.size.height.0))
                    })
                    .on_press(move |event: &PressEvent| {
                        if coarse
                            && controlled(&controls.control.get_untracked())
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
                            scroll_touch(touch, position, motion, event);
                        }
                    })
                    .on_press_down(move |event: &PressEvent| {
                        if coarse || !controlled(&controls.control.get_untracked()) {
                            return;
                        }
                        if let Some(at) = position(event.position) {
                            if web_hardware_keyboard {
                                entry_ref.focus();
                            } else {
                                focus.focus();
                            }
                            let clicks = event.clicks.max(1);
                            down.set(Some((at, clicks)));
                            mouse(
                                at,
                                MouseAction::Down,
                                MouseButton::Left,
                                MouseButton::Left.mask(),
                                clicks,
                                mods(event.modifiers),
                            );
                        }
                    })
                    .on_press_up(move |event: &PressEvent| {
                        if let Some((start, clicks)) = down.get_untracked() {
                            down.set(None);
                            let at = position(event.position).unwrap_or(start);
                            mouse(
                                at,
                                MouseAction::Up,
                                MouseButton::Left,
                                0,
                                clicks,
                                mods(event.modifiers),
                            );
                        }
                    })
                    .on_mouse_move(move |event: &MouseEvent| {
                        if controlled(&controls.control.get_untracked())
                            && let Some(at) = position(event.position)
                        {
                            let button = if down.get_untracked().is_some() {
                                MouseButton::Left
                            } else {
                                MouseButton::None
                            };
                            mouse(
                                at,
                                MouseAction::Move,
                                button,
                                button.mask(),
                                0,
                                mods(event.modifiers),
                            );
                        }
                    })
                    .on_wheel(move |event: &WheelEvent| {
                        if controlled(&controls.control.get_untracked())
                            && let Some(at) = position(event.position)
                        {
                            let delta = event.delta.points(px(16.0));
                            motion.wheel(WheelMotion {
                                at,
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
                            clipboard.forward(KeyAction::Down, event);
                        }
                    })
                    .on_key_up(move |event: &KeyEvent| {
                        if !entry_focused.get_untracked() {
                            clipboard.forward(KeyAction::Up, event);
                        }
                    })
                    .app_class(tw(
                        "relative flex min-h-0 flex-1 items-center justify-center overflow-hidden",
                    ))
                    .child((coarse || web_text_field).then(|| {
                        browser_entry(clipboard, entry_ref, entry_focused, web_hardware_keyboard)
                    }))
                    .child(
                        when(
                            move || frame.with(Option::is_some),
                            move || {
                                image(move || ImageSrc::from(frame.get().unwrap_or_default()))
                                    .object_fit(ResizeMode::Contain)
                                    .pointer_events(PointerEvents::None)
                                    .w_full()
                                    .h_full()
                                    .test_id("preview-jpeg")
                            },
                        )
                        .otherwise(|| {
                            text(t("Connecting to browser…"))
                                .app_class(tw("text-sm text-muted-foreground"))
                        }),
                    )
            }),
        )
}

fn stream_failure(failure: StreamFailure, revision: Signal<u64>) -> impl IntoElement {
    let message = match &failure {
        StreamFailure::Unavailable => t("Browser unavailable"),
        StreamFailure::Unauthorized => {
            t("Pair this client again with preview access to control browser previews.")
        }
        StreamFailure::Gone => t("This browser tab is closed."),
        StreamFailure::HostSetup(setup) => t(host_setup_message(setup)),
    };
    let retry = matches!(
        failure,
        StreamFailure::Unavailable | StreamFailure::HostSetup(_)
    );
    let command = match failure {
        StreamFailure::HostSetup(setup) => Some(setup.command),
        _ => None,
    };
    view()
        .app_class(tw(
            "flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center",
        ))
        .child(text(message).app_class(tw("text-sm text-muted-foreground")))
        .child(command.map(|command| text(command).app_class(tw("select-text font-mono text-sm"))))
        .child(retry.then(|| {
            button(
                ButtonContent::Label(t("Try again")),
                ButtonVariant::Outline,
                ButtonSize::Sm,
                || false,
                move || revision.update(|value| *value += 1),
            )
        }))
}

const SENTINELS: [char; 2] = ['\u{200b}', '\u{200c}'];

fn mobile_text(value: &str, sentinel: char) -> &str {
    value
        .strip_prefix(sentinel)
        .or_else(|| value.strip_suffix(sentinel))
        .unwrap_or(value)
}

fn browser_entry(
    clipboard: ClipboardShortcuts,
    entry: ElementRef,
    focused: Signal<bool>,
    hardware_keyboard: bool,
) -> impl IntoElement {
    let controls = clipboard.controls;
    // A sentinel keeps native Backspace observable when the remote field owns the text.
    let alternate = signal(false);
    let shortcuts = KeyCapture::keys(
        ["ctrl+c", "cmd+c", "ctrl+x", "cmd+x", "shift+escape"]
            .into_iter()
            .chain(
                (!cfg!(target_arch = "wasm32"))
                    .then_some(["ctrl+v", "cmd+v", "shift+insert"])
                    .into_iter()
                    .flatten(),
            )
            .filter_map(|key| Keystroke::parse(key, os()).ok()),
    );
    nsh_base::input()
        .element_ref(&entry)
        .test_id("preview-mobile-input")
        .label(t("Browser page input"))
        .value(move || SENTINELS[usize::from(alternate.get())].to_string())
        .auto_capitalize(AutoCapitalize::None)
        .auto_correct(false)
        .paste_behavior(PasteBehavior::Report)
        .on_paste(move |event: &nsh_core::PasteEvent| {
            if untracked(|| controls.controls_page()) && !event.text.is_empty() {
                controls.send(BrowserInput::Text {
                    text: event.text.clone(),
                });
            }
        })
        .capture_keys(shortcuts)
        .on_key_down(move |event: &KeyEvent| {
            if event.key.as_str() == "Escape" && event.modifiers.shift {
                entry.blur();
            } else if hardware_keyboard || event.default_prevented {
                clipboard.forward(KeyAction::Down, event);
            }
        })
        .on_key_up(move |event: &KeyEvent| {
            if hardware_keyboard || event.default_prevented {
                clipboard.forward(KeyAction::Up, event);
            }
        })
        .on_focus(move || {
            focused.set(true);
            entry.set_selection(1, 1);
        })
        .on_blur(move || {
            clipboard.reset();
            focused.set(false);
        })
        .on_change(move |value: &str| {
            if !focused.get_untracked() || !untracked(|| controls.controls_page()) {
                return;
            }
            if !hardware_keyboard {
                clipboard.invalidate();
                let text = mobile_text(value, SENTINELS[usize::from(alternate.get_untracked())]);
                if value.is_empty() {
                    entry_key(controls, "Backspace", None);
                } else if !text.is_empty() {
                    controls.send(BrowserInput::Text {
                        text: text.to_owned(),
                    });
                }
            }
            alternate.update(|state| *state = !*state);
            after_answer(move || entry.set_selection(1, 1));
        })
        .submit_behavior(SubmitBehavior::Submit)
        .on_submit(move |_: &str| {
            if !hardware_keyboard && untracked(|| controls.controls_page()) {
                entry_key(controls, "Enter", Some("\r"));
            }
        })
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
    motion: Motion,
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
                motion.wheel(WheelMotion {
                    at,
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
    if image_mime(bytes) != Some("image/jpeg") {
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

#[derive(Clone, Copy)]
struct ClipboardShortcuts {
    controls: StreamControls,
    tasks: TaskOwner,
    paste_keys: Stored<[bool; 2]>,
    pending_copy: Stored<Option<CopyRequest>>,
    interaction: Stored<u64>,
}

#[derive(Clone, Copy)]
struct CopyRequest {
    expires: Instant,
    interaction: u64,
}

impl ClipboardShortcuts {
    fn invalidate(self) {
        self.interaction
            .with(|epoch| *epoch = epoch.wrapping_add(1));
    }

    fn reset(self) {
        self.invalidate();
        self.paste_keys.replace([false; 2]);
    }

    fn forward(self, action: KeyAction, event: &KeyEvent) {
        if !untracked(|| self.controls.controls_page())
            || matches!(event.key.as_str(), "Process" | "Unidentified")
        {
            return;
        }
        let key = event.key.as_str();
        let shortcut = event.modifiers.ctrl || event.modifiers.cmd;
        let down = matches!(action, KeyAction::Down);
        if down && !event.repeat {
            self.invalidate();
        }
        let paste = if key.eq_ignore_ascii_case("v") {
            Some((0, shortcut))
        } else if key == "Insert" {
            Some((1, event.modifiers.shift))
        } else {
            None
        };
        if let Some((index, gesture)) = paste {
            let claimed = self.paste_keys.with(|keys| {
                let pressed = keys[index];
                keys[index] = down && (gesture || pressed);
                pressed || gesture
            });
            if claimed {
                if down && !event.repeat && !cfg!(target_arch = "wasm32") {
                    self.paste();
                }
                return;
            }
        }
        let copying =
            down && shortcut && (key.eq_ignore_ascii_case("c") || key.eq_ignore_ascii_case("x"));
        if copying {
            // The protocol's clipboard replies have no request ID.
            if self
                .pending_copy
                .get()
                .is_some_and(|request| now() <= request.expires)
            {
                return;
            }
            if forward_key(self.controls, action, event) {
                self.pending_copy.replace(Some(CopyRequest {
                    expires: now() + COPY_DEADLINE,
                    interaction: self.interaction.get(),
                }));
            }
        } else {
            forward_key(self.controls, action, event);
        }
    }

    fn paste(self) {
        let Some(sender) = self.controls.sender.get() else {
            return;
        };
        let Some(control) = self.controls.control.get_untracked() else {
            return;
        };
        let clipboard = clipboard_string();
        let interaction = self.interaction.get();
        self.tasks.spawn(async move {
            let text = clipboard.await;
            if !text.is_empty()
                && self.interaction.get() == interaction
                && self.controls.control.with_untracked(|current| {
                    controlled(current)
                        && current
                            .as_ref()
                            .is_some_and(|current| current.generation == control.generation)
                })
            {
                enqueue(&sender, BrowserInput::Text { text });
            }
        });
    }

    fn copy_out(self, text: String) {
        if untracked(|| self.controls.controls_page())
            && self.pending_copy.replace(None).is_some_and(|request| {
                self.interaction.get() == request.interaction && now() <= request.expires
            })
        {
            set_clipboard(text);
        }
    }
}

fn forward_key(controls: StreamControls, action: KeyAction, event: &KeyEvent) -> bool {
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
    controls.try_send(BrowserInput::Key {
        action,
        key,
        code: event.code.to_string(),
        text,
        modifiers: mods(event.modifiers),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saturated_queue_reserves_room_for_releases_and_stays_open() {
        let (sender, mut receiver) = mpsc::unbounded();
        for _ in 0..(MAX_QUEUED_INPUTS - RELEASE_RESERVE) {
            assert!(enqueue(&sender, BrowserInput::Reload));
        }
        assert!(!enqueue(&sender, BrowserInput::Reload));
        assert!(enqueue(&sender, BrowserInput::ReleaseControl));
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_ok());
        assert!(enqueue(&sender, BrowserInput::Reload));
    }

    #[test]
    fn mobile_caret_artifact_is_removed_from_either_end() {
        for sentinel in SENTINELS {
            assert_eq!(mobile_text(&format!("{sentinel}a"), sentinel), "a");
            assert_eq!(mobile_text(&format!("b{sentinel}"), sentinel), "b");
            assert_eq!(mobile_text(&sentinel.to_string(), sentinel), "");
        }
        assert_eq!(mobile_text("a\u{200c}b", SENTINELS[1]), "a\u{200c}b");
    }

    #[test]
    fn jpeg_rejects_untrusted_formats_and_excessive_dimensions() {
        assert!(jpeg_source(b"<svg></svg>").is_none());
        assert!(jpeg_source(&[0xff, 0xd8, 0xff]).is_none());
        let mut jpeg = include_bytes!("../../../tests/ui/prv_2.jpg").to_vec();
        assert!(jpeg_source(&jpeg).is_some());
        let sof = jpeg
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .expect("SOF");
        jpeg[sof + 5..sof + 7].copy_from_slice(&3840_u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&3840_u16.to_be_bytes());
        assert!(
            jpeg_source(&jpeg).is_none(),
            "JPEG exceeds the viewport area"
        );
        jpeg[sof + 5..sof + 7].copy_from_slice(&1_u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&4096_u16.to_be_bytes());
        assert!(jpeg_source(&jpeg).is_none(), "JPEG exceeds the side limit");
    }
}
