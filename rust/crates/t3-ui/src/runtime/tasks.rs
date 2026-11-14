//! Where async work runs (rule 7 of the UI brief). A handler's task runs under the owner of the
//! screen, host or section that built the control, never under the control: the handler's own
//! state change can rebuild the control and would cancel it.

use std::future::Future;

use nsh_core::prelude::*;

/// The owner of a screen, host or section. Capture it with [`TaskOwner::here`] where that is
/// built, outside any `dynamic`/`when` that the handler's state changes rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskOwner(Owner);

impl TaskOwner {
    pub fn here() -> Self {
        Self(Owner::current())
    }

    /// Starts `future` under this owner: it ends when the work does or when this owner goes.
    pub fn spawn(self, future: impl Future<Output = ()> + 'static) {
        self.0.run(|| {
            spawn(future);
        });
    }
}

/// Build-time work that ends with the view it is started in (a stream loop, a load).
pub fn watch<T: 'static>(future: impl Future<Output = T> + 'static) -> Spawned<T> {
    spawn(future)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use futures::channel::oneshot;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;

    /// One task's reply: the test releases it, and the release says whether the task was still
    /// there to receive it.
    #[derive(Clone, Default)]
    struct Gate(Rc<RefCell<Option<oneshot::Sender<()>>>>);

    impl Gate {
        fn wait(&self) -> impl Future<Output = bool> + 'static {
            let (sender, receiver) = oneshot::channel();
            *self.0.borrow_mut() = Some(sender);
            async move { receiver.await.is_ok() }
        }
        fn release(&self) -> &'static str {
            match self.0.borrow_mut().take() {
                Some(sender) => match sender.send(()) {
                    Ok(()) => "delivered",
                    Err(()) => "refused",
                },
                None => "never started",
            }
        }
    }

    fn control(label: &'static str, on_press: impl Fn() + 'static) -> impl IntoElement {
        nsh_base::button()
            .accessibility_label(SharedString::from(label))
            .child(text(SharedString::from(label)))
            .on_press(on_press)
    }

    /// A screen (`when(open)`) holding a section whose controls a `dynamic` rebuilds on
    /// `generation`. Each start button waits on the gate, then counts a finish.
    #[component]
    fn TaskProbe() -> impl IntoElement {
        let open = signal(true);
        let generation = signal(0_u32);
        let finished = signal(0_u32);
        let released = signal("");
        let gate = Gate::default();
        let section = {
            let gate = gate.clone();
            move || {
                let screen = TaskOwner::here();
                let watching = gate.wait();
                watch(async move {
                    if watching.await {
                        finished.update(|n| *n += 1);
                    }
                });
                let gate = gate.clone();
                view().child(dynamic(move || {
                    let _ = generation.get();
                    let in_run = TaskOwner::here();
                    let (screen_gate, run_gate, bare_gate) =
                        (gate.clone(), gate.clone(), gate.clone());
                    view()
                        .child(control("Start under the screen", move || {
                            let wait = screen_gate.wait();
                            screen.spawn(async move {
                                if wait.await {
                                    finished.update(|n| *n += 1);
                                }
                            });
                        }))
                        .child(control("Start under the run", move || {
                            let wait = run_gate.wait();
                            in_run.spawn(async move {
                                if wait.await {
                                    finished.update(|n| *n += 1);
                                }
                            });
                        }))
                        .child(control("Start bare", move || {
                            let wait = bare_gate.wait();
                            spawn(async move {
                                if wait.await {
                                    finished.update(|n| *n += 1);
                                }
                            });
                        }))
                }))
            }
        };
        view()
            .child(control("Rebuild the controls", move || {
                generation.update(|g| *g += 1)
            }))
            .child(control("Close the screen", move || open.set(false)))
            .child(control("Release", move || released.set(gate.release())))
            .child(text(move || {
                SharedString::from(format!("released: {}", released.get()))
            }))
            .child(text(move || {
                SharedString::from(format!("finished: {}", finished.get()))
            }))
            .child(when(move || open.get(), section))
    }

    fn start() -> Headless {
        let mut sets = crate::registries();
        let mut fixtures = Registry::new();
        fixtures.register::<TaskProbe>();
        sets.push(fixtures);
        let mut app = Headless::start(EngineConfig {
            ui_sets: sets,
            viewport: size(px(390.0), px(844.0)),
            ..EngineConfig::new(Root::of::<TaskProbe>())
        })
        .expect("task probe starts");
        app.wait_for("text:finished: 0").expect("probe mounted");
        app
    }

    fn release(app: &mut Headless) {
        app.press("role:button[Release]").expect("release");
    }

    /// The first release goes to the build-time `watch` task; later starts replace the gate.
    fn started(app: &mut Headless, label: &str) {
        app.press(&format!("role:button[{label}]"))
            .expect("start from the handler");
    }

    #[test]
    fn a_task_under_the_screen_survives_its_control_being_rebuilt_and_completes() {
        let mut app = start();
        started(&mut app, "Start under the screen");
        app.press("role:button[Rebuild the controls]")
            .expect("rebuild");
        release(&mut app);
        app.wait_for("text:released: delivered")
            .expect("task still waiting");
        app.wait_for("text:finished: 1").expect("task completed");
    }

    #[test]
    fn a_task_under_the_screen_ends_with_the_screen() {
        let mut app = start();
        started(&mut app, "Start under the screen");
        app.press("role:button[Close the screen]").expect("close");
        release(&mut app);
        app.wait_for("text:released: refused")
            .expect("task cancelled");
        assert!(app.screen().shows("finished: 0"));
    }

    #[test]
    fn watch_ends_with_its_view() {
        let mut app = start();
        app.press("role:button[Close the screen]").expect("close");
        release(&mut app);
        app.wait_for("text:released: refused")
            .expect("watch cancelled");
        assert!(app.screen().shows("finished: 0"));
    }

    #[test]
    fn an_owner_captured_inside_the_rebuilt_dynamic_does_not_save_the_task() {
        let mut app = start();
        started(&mut app, "Start under the run");
        app.press("role:button[Rebuild the controls]")
            .expect("rebuild");
        release(&mut app);
        app.wait_for("text:released: refused")
            .expect("task cancelled with the run");
        assert!(app.screen().shows("finished: 0"));
    }

    #[test]
    fn a_bare_spawn_from_a_handler_dies_with_its_control() {
        let mut app = start();
        started(&mut app, "Start bare");
        app.press("role:button[Rebuild the controls]")
            .expect("rebuild");
        release(&mut app);
        app.wait_for("text:released: refused")
            .expect("the defect rule 7 forbids");
        assert!(app.screen().shows("finished: 0"));
    }
}
