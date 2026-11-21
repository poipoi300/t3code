use super::*;
use crate::{
    i18n::Text,
    ui::{
        dialog::*,
        input::{InputSize, text_field},
    },
};
use t3_client::{
    navigation::SettingsPage,
    services::{
        settings::scope::SettingsScopeSearch,
        usage::prices::{PriceEdit, cells, patch},
    },
};

pub(super) fn dialog_for<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    selected: Memo<Vec<UsageTarget>>,
    open: Signal<bool>,
    initial: Signal<String>,
    refresh: Signal<u64>,
) -> AnyElement {
    region(
        move || open.get(),
        move |visible| {
            if !visible {
                return None;
            }
            let initial_model = initial.get_untracked();
            initial.set(String::new());
            let state = EditorState {
                edit: signal((!initial_model.is_empty()).then_some(initial_model)),
                busy: signal(false),
                messages: signal(Vec::new()),
                refresh,
            };
            let service = client.clone();
            let popup = dialog(open, move |dialog_state| {
                dialog_state.on_open_change(move |next| {
                    if !state.busy.get_untracked() {
                        open.set(next);
                    }
                });
                let service = service.clone();
                let options = PopupOptions {
                    max_width: MaxWidth::FourXl,
                    ..Default::default()
                };
                dialog_popup(dialog_state, options, move || {
                    let header = view()
                        .child(dialog_title(dialog_state, || Text::from("Custom model prices")))
                        .child(dialog_description(dialog_state, || Text::from("Prices and mappings apply to all past and future usage on the environments you select.")));
                    let service = service.clone();
                    let panel = region(
                        move || {
                            let edit = state.edit.get();
                            // A settings notification can arrive while a draft is being edited.
                            let targets = if edit.is_some() {
                                selected.get_untracked()
                            } else {
                                selected.get()
                            };
                            (edit, targets)
                        },
                        move |(model, targets)| price_rows(&service, model, targets, state),
                    );
                    view()
                        .child(dialog_header(dialog_state, header))
                        .child(dialog_panel(dialog_state, panel))
                })
            });
            Some(popup.into_element())
        },
    )
}
fn price_rows<P: Platform + Clone + 'static>(
    service: &ClientRuntime<P>,
    model: Option<String>,
    targets: Vec<UsageTarget>,
    state: EditorState,
) -> AnyElement {
    let keys: BTreeSet<_> = targets
        .iter()
        .filter_map(|t| t.config.as_ref())
        .flat_map(|c| {
            c.settings
                .usage_price_overrides
                .keys()
                .chain(c.settings.usage_model_aliases.keys())
                .map(|k| k.0.clone())
        })
        .collect();
    let labels = targets
        .iter()
        .map(|t| t.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut body = view()
        .app_class(tw("flex min-w-0 flex-col gap-3"))
        .child(text(t("Apply to")))
        .child(text(labels))
        .child(text(t("USD / million tokens")));
    if targets.is_empty() {
        body = body.child(text(t(
            "Select an environment to see and change its model prices.",
        )));
    } else if let Some(model) = model {
        body = body.child(editor(service.clone(), targets, model, &keys, state));
    } else {
        if keys.is_empty() {
            body = body.child(text(t(
                "No custom prices or mappings. Add a row to set one.",
            )));
        }
        for model in keys {
            body = body.child(button(
                ButtonContent::Label(model.clone()),
                ButtonVariant::Ghost,
                ButtonSize::Sm,
                || false,
                move || state.open(model.clone()),
            ));
        }
        body = body.child(button(
            ButtonContent::Icon {
                icon: Icon::Plus,
                label: t("Add model price"),
            },
            ButtonVariant::Ghost,
            ButtonSize::IconXs,
            || false,
            move || state.open(String::new()),
        ));
    }
    body.child(text(move || state.messages.get().join("\n")))
        .into_element()
}
#[derive(Clone, Copy)]
struct EditorState {
    edit: Signal<Option<String>>,
    busy: Signal<bool>,
    messages: Signal<Vec<String>>,
    refresh: Signal<u64>,
}
impl EditorState {
    fn open(&self, model: String) {
        self.messages.set(Vec::new());
        self.edit.set(Some(model));
    }
}
fn price_field(
    value: Signal<String>,
    placeholder: String,
    label: String,
    locked: impl Fn() -> bool + 'static,
) -> AnyElement {
    region(locked, move |locked| {
        if locked {
            text(move || value.get()).into_element()
        } else {
            text_field(
                value,
                placeholder.clone(),
                label.clone(),
                InputSize::Default,
                || {},
            )
            .into_element()
        }
    })
}
fn editor<P: Platform + Clone + 'static>(
    client: ClientRuntime<P>,
    targets: Vec<UsageTarget>,
    initial: String,
    keys: &BTreeSet<String>,
    state: EditorState,
) -> AnyElement {
    let EditorState {
        edit,
        busy,
        messages,
        refresh,
    } = state;
    let existing = keys.contains(&initial);
    let name = signal(initial.clone());
    let current: Vec<_> = targets
        .iter()
        .filter_map(|t| t.config.as_ref())
        .map(|c| cells(c, &initial))
        .collect();
    let originals: [Option<String>; 5] = std::array::from_fn(|i| {
        current
            .first()
            .map(|row| row[i].clone())
            .filter(|v| current.iter().all(|row| row[i] == *v))
    });
    let fields = originals.clone().map(|v| signal(v.unwrap_or_default()));
    let original = stored(originals);
    let keys = stored(keys.clone());
    let retry = signal(BTreeSet::<EnvironmentId>::new());

    let locked = move || busy.get() || retry.with(|r| !r.is_empty());
    let mut body = view()
        .app_class(tw("flex flex-col gap-2"))
        .child(text(t("Model ID")));
    if existing {
        body = body.child(text(initial.clone()));
    } else {
        body = body.child(price_field(name, String::new(), t("Model ID"), locked));
    }
    for (i, key) in ["Input", "Output", "Cache read", "Cache write", "Map to"]
        .into_iter()
        .enumerate()
    {
        if i == 4
            && !targets.iter().any(|t| {
                t.config
                    .as_ref()
                    .is_some_and(|c| c.environment.capabilities.usage_model_aliases == Some(true))
            })
        {
            continue;
        }
        body = body.child(text(t(key))).child(price_field(
            fields[i],
            if original.with(|o| o[i].is_none()) {
                t("Mixed")
            } else {
                String::new()
            },
            t(key),
            locked,
        ));
    }
    let staged = stored(None::<PriceEdit>);
    let tasks = TaskOwner::here();
    let submit = Rc::new(move |removed: bool| {
        if busy.get_untracked() {
            return;
        }
        let original = original.get();
        let draft = if let Some(saved) = staged.get() {
            saved
        } else {
            let field = |i: usize| {
                let v = fields[i].get_untracked();
                (original[i].as_deref().unwrap_or_default() != v).then_some(v)
            };
            let model = name.get_untracked().trim().to_owned();
            if !existing && keys.with(|keys| keys.contains(&model)) {
                messages.set(vec![t(
                    "This model already has a row. Edit its prices there.",
                )]);
                return;
            }
            PriceEdit {
                model,
                rates: std::array::from_fn(field),
                alias: field(4),
                removed,
            }
        };
        let failed = retry.get_untracked();
        let mut writes = Vec::new();
        let settings = client.settings();
        let live = client.usage_targets();
        for target in targets
            .iter()
            .filter(|t| failed.is_empty() || failed.contains(&t.id))
        {
            let here = live.iter().find(|t| t.id == target.id);
            let plan = match here {
                None => Err(t("Environment removed")),
                Some(target) if !target.connected => Err(t("Offline")),
                Some(target) if !target.settings_write => Err(t("Read-only access")),
                Some(target) => match &target.config {
                    None => Err(t("Prices not loaded")),
                    Some(c) => match patch(c, &draft) {
                        Ok(p) => Ok(p),
                        Err(msg) => {
                            messages.set(vec![msg]);
                            return;
                        }
                    },
                },
            };
            let id = target.id.clone();
            let label = target.label.clone();
            let settings = settings.clone();
            writes.push(async move {
                let result = match plan {
                    Err(e) => Err(e),
                    Ok(p) if p.is_empty() => Ok(()),
                    Ok(p) => {
                        let scope = settings.scope_view(
                            SettingsPage::Chat,
                            &SettingsScopeSearch {
                                machine: Some(id.0.clone()),
                                ..Default::default()
                            },
                        );
                        let outcome = settings.update(&scope, p).await;
                        if outcome.failed || outcome.saved_environments.is_empty() {
                            Err("Could not save. Try again.".to_owned())
                        } else {
                            Ok(())
                        }
                    }
                };
                (id, label, result)
            });
        }
        busy.set(true);
        staged.replace(Some(draft));
        tasks.spawn(async move {
            let results = futures::future::join_all(writes).await;
            let mut failed = BTreeSet::new();
            let mut status = Vec::new();
            for (id, label, result) in results {
                match result {
                    Ok(()) => status.push(format!("{label}: {}", t("Saved"))),
                    Err(error) => {
                        failed.insert(id);
                        status.push(format!("{label}: {error}"));
                    }
                }
            }
            let success = failed.is_empty();
            retry.set(failed);
            messages.set(status);
            busy.set(false);
            if success {
                refresh.update(|r| *r += 1);
                edit.set(None);
            }
        });
    });
    if existing {
        let submit = submit.clone();
        body = body.child(button(
            ButtonContent::Label(t("Remove")),
            ButtonVariant::GhostDestructive,
            ButtonSize::Sm,
            locked,
            move || submit(true),
        ));
    }
    body.child(region(
        move || retry.with(BTreeSet::is_empty),
        move |first| {
            let submit = submit.clone();
            button(
                ButtonContent::Label(if first { t("Save") } else { t("Retry failed") }),
                ButtonVariant::Default,
                ButtonSize::Sm,
                move || busy.get(),
                move || submit(false),
            )
        },
    ))
    .child(button(
        ButtonContent::Label(t("Discard")),
        ButtonVariant::Ghost,
        ButtonSize::Sm,
        move || busy.get(),
        move || edit.set(None),
    ))
    .into_element()
}
