//! ThemeImportDialog.tsx and the editable fields of ThemeEditorPanel.tsx.
use super::{library::Form, *};
use crate::runtime::tasks::TaskOwner;
use crate::theme::{Edit, SaveKind, ThemeMerge, ThemeState};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::dialog::{
    PopupOptions, dialog, dialog_header, dialog_panel, dialog_popup, dialog_title,
};
use nsh_core::picker::{FilePickRequest, PickedStatus};
use t3_theme::import::{pair_vscode_themes, parse_theme_import, resolve_theme_label_collisions};
use t3_theme::palette::{
    Appearance as Half, ThemeColorRole as Role, ThemeDefinition, ThemeVariants,
    create_vivid_theme_colors, parse_theme_file, standard_theme_colors, theme_id_from_name,
    to_canonical_theme_color, update_theme_color_family,
};

pub(super) fn show(form: Signal<Option<Form>>, state: ThemeState, request: Form) -> AnyElement {
    if let Form::Editor {
        seed,
        editing,
        name,
    } = request
    {
        return editor_panel(state, form, seed, editing, name);
    }
    let open = signal(true);
    dialog(open, move |dialog_state| {
        dialog_state.on_open_change(move |next| {
            if !next {
                form.set(None);
            }
        });
        dialog_popup(dialog_state, PopupOptions::default(), move || {
            let title = match &request {
                Form::Import => Text::from("Add a theme"),
                Form::Editor { editing: true, .. } => Text::from("Edit theme"),
                Form::Editor { .. } => Text::from("Create theme"),
                Form::Remove(t,group) if !group.is_empty() => Text::content(format!("Remove themes from “{}”?",t.collection.as_ref().map_or(t.label.as_str(),|c| &c.label))),
                Form::Remove(t,_) => Text::content(tr(
                    "Remove “{name}”?",
                    &[("name", t3_i18n::Value::Text(t.label.clone()))],
                )),
            };
            let body = match request.clone() {
                Form::Import => import(state, form),
                Form::Editor { .. } => unreachable!("editor opens in its modeless panel"),
                Form::Remove(t,group) => {
                    let submit = Submission::new(state, form);
                    let collection=!group.is_empty();
                    let selected=signal(if collection {Vec::new()} else {vec![t.id]});
                    let variants = group.into_iter().map(|t| {
                        let id=stored(t.id);
                        nsh_base::checkbox().accessibility_label(t.label.clone()).checked(move||selected.with(|ids|ids.contains(&id.get())))
                            .on_change(move|on:&bool|selected.update(|ids|{ids.retain(|i|i!=&id.get());if *on {ids.push(id.get());}}))
                            .child(text(t.label)).into_element()
                    }).collect::<Vec<_>>();
                    view()
                        .child(text(crate::i18n::t(if collection {"Select the variants you want to remove. You can restore them by importing the extension again."} else {"You can bring it back anytime by importing its JSON file."})))
                        .child(variants)
                        .child(control("Cancel", move || form.set(None)))
                        .child(control("Remove theme", move || {
                            submit.save(Edit::Remove(selected.get_untracked()))
                        }))
                        .child(submit.message())
                        .into_element()
                }
            };
            view()
                .child(dialog_header(
                    dialog_state,
                    dialog_title(dialog_state, move || title.clone()),
                ))
                .child(dialog_panel(dialog_state, body))
        })
    })
    .into_element()
}

#[derive(Clone, Copy)]
pub(super) struct Submission {
    state: ThemeState,
    form: Signal<Option<Form>>,
    pub(super) busy: Signal<bool>,
    pub(super) error: Signal<String>,
    session: Option<Memo<Option<EditorDraft>>>,
}

impl Submission {
    fn new(state: ThemeState, form: Signal<Option<Form>>) -> Self {
        Self {
            state,
            form,
            busy: signal(false),
            error: signal(String::new()),
            session: None,
        }
    }
    pub(super) fn save(self, edit: Edit) {
        if self.busy.get_untracked() {
            return;
        }
        self.busy.set(true);
        self.state.save(edit, move |result| {
            if let Some(session) = self.session
                && session
                    .try_get()
                    .flatten()
                    .is_none_or(|draft| draft.busy != self.busy)
            {
                return;
            }
            if !self.busy.try_set(false) {
                return;
            }
            match result {
                Ok(()) => {
                    let _ = self.form.try_set(None);
                }
                Err(error) => {
                    let _ = self.error.try_set(error);
                }
            }
        });
    }
    fn message(self) -> AnyElement {
        text(move || SharedString::from(self.error.get()))
            .app_class(tw("text-sm text-destructive"))
            .into_element()
    }
}

fn control(label: &'static str, press: impl Fn() + 'static) -> AnyElement {
    button(
        ButtonContent::Label(t(label)),
        ButtonVariant::Outline,
        ButtonSize::Sm,
        || false,
        press,
    )
    .into_element()
}

fn import(state: ThemeState, form: Signal<Option<Form>>) -> AnyElement {
    const LIMIT: u64 = 256 * 1024;
    let json = signal(String::new());
    let name = signal(None::<String>);
    let conflicts = signal(Vec::<ThemeDefinition>::new());
    let submit = Submission::new(state, form);
    let reading = signal(false);
    let owner = TaskOwner::here();
    let accept = move |themes: Vec<ThemeDefinition>| {
        if state.choice.with_untracked(|c| {
            themes
                .iter()
                .any(|t| c.custom.iter().any(|saved| saved.id == t.id))
        }) {
            conflicts.set(themes);
        } else {
            let kind = if themes.len() == 1 {
                SaveKind::Import
            } else {
                SaveKind::InstallMany
            };
            submit.save(Edit::Save(themes, kind));
        }
    };
    let consume = move |files: Vec<(String, String)>| {
        if let [(filename, contents)] = files.as_slice() {
            name.set(Some(filename.clone()));
            json.set(contents.clone());
            submit.error.set(String::new());
            return;
        }
        if files.is_empty() {
            return;
        }
        let parsed = files
            .iter()
            .map(|(filename, text)| {
                parse_theme_import(text).map(|theme| (theme, Some(filename.as_str())))
            })
            .collect::<Result<Vec<_>, _>>();
        match parsed {
            Ok(themes) => accept(pair_vscode_themes(
                &resolve_theme_label_collisions(&themes),
                None,
            )),
            Err(error) => submit.error.set(error.to_string()),
        }
    };
    let choose = move || {
        if reading.get_untracked() {
            return;
        }
        reading.set(true);
        let request = FilePickRequest::new(true, 0).max_size(LIMIT).filter(
            nsh_core::file_dialog::FileFilter::new("Theme JSON", ["json", "jsonc"]),
        );
        let answer = pick_files(request);
        owner.spawn(async move {
            let result = answer.await;
            if !reading.try_set(false) {
                return;
            }
            let files = result
                .map_err(|error| error.to_string())
                .and_then(import_files);
            match files {
                Ok(files) => consume(files),
                Err(error) => submit.error.set(error),
            }
        });
    };
    let drop_area = move || {
        let area = view().app_class(tw("flex flex-row flex-wrap items-center justify-between gap-3 rounded-xl border border-dashed px-3 py-3"))
            .child(view().child(text(t("Theme file"))).child(text(move ||SharedString::from(name.get().unwrap_or_else(|| {
                if cfg!(target_arch="wasm32") {t("T3 Code or VS Code .json files")} else {t("Drop T3 Code or VS Code .json files")}
            })))))
            .child(control("Choose files", choose));
        // NSH supplies local file: URLs for native drops. Browser pickers supply bytes instead.
        #[cfg(not(target_arch = "wasm32"))]
        let area = area.on_file_drop(move |event: &nsh_core::element::FileDropEvent| {
            if reading.get_untracked() {
                return;
            }
            let uris = event.uris.clone();
            reading.set(true);
            owner.spawn(async move {
                let files = nsh_core::element::blocking(move || read_dropped(&uris, LIMIT)).await;
                if !reading.try_set(false) {
                    return;
                }
                match files {
                    Ok(Ok(files)) => consume(files),
                    Ok(Err(error)) => submit.error.set(error),
                    Err(_) => submit
                        .error
                        .set(t("Could not read that file. Paste the JSON below instead.")),
                }
            });
        });
        area
    };
    view().app_class(tw("flex flex-col gap-4"))
        .child(super::community::section(state, submit))
        .child(text(t("or import a file")).app_class(tw("text-2xs uppercase tracking-wider text-muted-foreground")))
        .child(region(move || conflicts.with(|c| !c.is_empty()),move |conflict| {
            if conflict {
                return view().child(text(t("Already installed")))
                    .child(text(move || SharedString::from(conflicts.with(|items|items.iter().map(|t|t.label.as_str()).collect::<Vec<_>>().join(", ")))))
                    .child(control("Update existing",move ||submit.save(Edit::Save(conflicts.get_untracked(),SaveKind::Replace))))
                    .child(control("Keep both",move || {
                        let mut items=conflicts.get_untracked();
                        let mut used=state.choice.with_untracked(|c|c.custom.iter().map(|t|t.id.clone()).collect::<Vec<_>>());
                        for item in &mut items {
                            if !used.contains(&item.id) { used.push(item.id.clone());continue; }
                            let Some((label,id))=(1..100).find_map(|i| {
                                let suffix=format!(" ({i})");
                                let mut base=item.label.clone();
                                while base.encode_utf16().count()+suffix.len()>48 {base.pop();}
                                let label=format!("{base}{suffix}"); let id=theme_id_from_name(&label);
                                (!used.contains(&id)).then_some((label,id))
                            }) else {submit.error.set(t("Too many copies.")); return;};
                            item.id=id.clone();item.label=label;used.push(id);
                        }
                        submit.save(Edit::Save(items,SaveKind::InstallMany));
                    })).child(control("Back",move ||conflicts.set(vec![]))).child(control("Cancel",move ||form.set(None))).into_element();
            }
            view().app_class(tw("flex flex-col gap-4"))
                .child(drop_area())
                .child(text(t("Theme JSON")).app_class(tw("text-sm font-medium")))
                .child(nsh_base::input().label(t("Theme JSON")).multiline(true).value(move ||json.get())
                    .on_change(move |s:&str| json.set(s.into())).app_class(tw("h-32 w-full rounded-lg border border-input bg-background p-2 font-mono text-xs")))
                .child(view().app_class(tw("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end"))
                    .child(control("Cancel",move ||form.set(None)))
                    .child(nsh_base::button().test_id("theme-import-submit").accessibility_label(t("Add theme"))
                        .disabled(move || json.with(|s|s.trim().is_empty()) || reading.get() || submit.busy.get())
                        .on_press(move || {
                            let source=json.get_untracked();
                            if source.encode_utf16().count()>LIMIT as usize {submit.error.set(t("Theme file exceeds 256 KB."));return;}
                            match parse_theme_import(&source) {Ok(t)=>accept(vec![t]), Err(e)=>submit.error.set(e.to_string())}
                        }).app_class(tw("rounded-lg bg-primary px-3 py-2 text-primary-foreground")).child(text(t("Add theme")))))
                .into_element()
        })).child(submit.message()).into_element()
}

fn import_files(
    picked: nsh_core::picker::FilePickSelection,
) -> Result<Vec<(String, String)>, String> {
    if picked.truncated {
        return Err(t("Too many files."));
    }
    picked
        .files
        .into_iter()
        .map(|file| {
            if file.status == PickedStatus::TooLarge {
                return Err(t("Theme file exceeds 256 KB."));
            }
            if file.status != PickedStatus::Read {
                return Err(t("Could not read that file. Paste the JSON below instead."));
            }
            let text = std::str::from_utf8(file.data.bytes())
                .map_err(|_| t("Could not read that file. Paste the JSON below instead."))?;
            Ok((file.name.to_string(), text.to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(not(target_arch = "wasm32"))]
fn read_dropped(uris: &[String], limit: u64) -> Result<Vec<(String, String)>, String> {
    use std::path::PathBuf;
    use t3_client::view_logic::session::links::{parse_file_url_href, safe_decode_uri_component};
    let unreadable = || t("Could not read that file. Paste the JSON below instead.");
    if uris.len() > nsh_core::picker::MAX_PICKED as usize {
        return Err(t("Too many files."));
    }
    let paths = uris
        .iter()
        .map(|uri| {
            if !(uri.starts_with("file:///") || uri.starts_with("file://localhost/")) {
                return Err(unreadable());
            }
            let (path, _) = parse_file_url_href(uri).ok_or_else(unreadable)?;
            Ok(PathBuf::from(safe_decode_uri_component(&path)))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let request = FilePickRequest::new(true, 0).max_size(limit);
    import_files(nsh_core::picker::read_picked_files(&paths, &request))
}

/// NSH overlays are positioned against the root window and pass input through their empty area.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct EditorDraft {
    name: Signal<String>,
    busy: Signal<bool>,
    error: Signal<String>,
    half: Signal<Half>,
    colors: Signal<[t3_theme::palette::ThemeColors; 2]>,
    advanced: Signal<bool>,
    filter: Signal<String>,
    css: Signal<String>,
    css_open: Signal<bool>,
    minimized: Signal<bool>,
    width: Signal<f32>,
    height: Signal<f32>,
    right: Signal<f32>,
    bottom: Signal<f32>,
}

impl EditorDraft {
    pub(super) fn new(seed: Option<&ThemeDefinition>, editing: bool, initial_name: String) -> Self {
        let name = signal(initial_name);
        let initial = if dark() { Half::Dark } else { Half::Light };
        let half = signal(
            seed.filter(|t| editing && t.colors_for(initial).is_none())
                .map_or(initial, |t| t.appearance),
        );
        let colors = signal(Half::ALL.map(|h| {
            seed.and_then(|t| t.colors_for(h))
                .unwrap_or_else(|| standard_theme_colors(h))
                .clone()
        }));
        let advanced = signal(seed.is_some_and(|t| !t.is_managed()));
        let filter = signal(String::new());
        let css = signal(seed.and_then(|t| t.css.clone()).unwrap_or_default());
        let css_open = signal(!css.get_untracked().is_empty());
        Self {
            name,
            busy: signal(false),
            error: signal(String::new()),
            half,
            colors,
            advanced,
            filter,
            css,
            css_open,
            minimized: signal(false),
            width: signal(416.),
            height: signal(672.),
            right: signal(16.),
            bottom: signal(16.),
        }
    }
    pub(super) fn preview(self) -> ThemeDefinition {
        let appearance = self.half.get();
        ThemeDefinition {
            id: t3_theme::palette::THEME_PREVIEW_ID.into(),
            label: String::new(),
            appearance,
            colors: self
                .colors
                .with(|all| all[usize::from(appearance == Half::Dark)].clone()),
            variants: None,
            collection: None,
            sidebar_artwork: None,
            managed: None,
            css: Some(self.css.get()),
        }
    }
}

fn editor_panel(
    state: ThemeState,
    form: Signal<Option<Form>>,
    seed: Option<ThemeDefinition>,
    editing: bool,
    name: String,
) -> AnyElement {
    use nsh_core::element::{ClaimRequest, ResponderEvent, TouchClaim};
    let draft = use_context::<super::ThemeForms>()
        .and_then(|host| host.draft.get_untracked())
        .unwrap_or_else(|| {
            let draft = EditorDraft::new(seed.as_ref(), editing, name);
            effect(move || state.preview.set(Some(draft.preview())));
            cleanup(move || {
                let _ = state.preview.try_set(None);
            });
            draft
        });
    let EditorDraft {
        minimized,
        width,
        height,
        right,
        bottom,
        ..
    } = draft;
    let origin = signal((16.0f32, 16.0f32));
    let extent = signal((416.0f32, 672.0f32));
    let panel_width = move || width.get().min((window().width.0 - 32.0).max(1.0));
    let panel_height = move || {
        if minimized.get() {
            44.0
        } else {
            height.get().min((window().height.0 - 96.0).max(48.0))
        }
    };
    let panel_right = move || {
        right
            .get()
            .clamp(0.0, (window().width.0 - panel_width()).max(0.0))
    };
    let panel_bottom = move || {
        bottom
            .get()
            .clamp(0.0, (window().height.0 - panel_height()).max(0.0))
    };
    let claim = || TouchClaim::on_start().ask(|r: &ClaimRequest| r.claim());
    let title = if editing {
        "Edit theme"
    } else {
        "Create theme"
    };
    let handle = view()
        .flex_1()
        .claim_touches(claim())
        .on_responder_grant(move |_: &ResponderEvent| origin.set((panel_right(), panel_bottom())))
        .on_responder_move(move |e: &ResponderEvent| {
            let (x, y) = origin.get_untracked();
            right.set(x - e.gesture.delta.x.0);
            bottom.set(y - e.gesture.delta.y.0);
        })
        .child(text(t(title)).app_class(tw("text-sm font-medium")));
    let resize = view()
        .label(t("Resize theme editor"))
        .claim_touches(claim())
        .on_responder_grant(move |_: &ResponderEvent| {
            extent.set((panel_width(), panel_height()));
            origin.set((panel_right(), panel_bottom()));
        })
        .on_responder_move(move |e: &ResponderEvent| {
            let (w, h) = extent.get_untracked();
            let (x, y) = origin.get_untracked();
            let next_w = (w + e.gesture.delta.x.0).clamp(224.0, window().width.0.max(224.0));
            let next_h = (h + e.gesture.delta.y.0).clamp(160.0, window().height.0.max(160.0));
            width.set(next_w);
            height.set(next_h);
            right.set(x + w - next_w);
            bottom.set(y + h - next_h);
        })
        .app_class(move || {
            Some(tw(if minimized.get() {
                "hidden"
            } else {
                "absolute bottom-0 right-0 size-5 items-center justify-center"
            }))
        })
        .child(crate::ui::icon::icon(Icon::Grip, 12.0, || {
            palette().foreground
        }));
    let minimize = nsh_base::button()
        .accessibility_label(move || {
            Some(SharedString::from(t(if minimized.get() {
                "Expand the theme editor"
            } else {
                "Minimize the theme editor"
            })))
        })
        .on_press(move || minimized.update(|value| *value = !*value))
        .child(text(move || if minimized.get() { "+" } else { "−" }));
    let close = nsh_base::button()
        .accessibility_label(t("Close the theme editor"))
        .on_press(move || form.set(None))
        .child(crate::ui::icon::icon(Icon::X, 16.0, || {
            palette().foreground
        }));
    let header = view()
        .app_class(tw("flex flex-row items-center gap-2 border-b px-3 py-2"))
        .child(handle)
        .child(minimize)
        .child(close);
    let body = scroll()
        .app_class(move || {
            Some(tw(if minimized.get() {
                "hidden"
            } else {
                "min-h-0 flex-1 px-3 py-3"
            }))
        })
        .child(editor(state, form, seed, editing, draft));
    let panel = view()
        .absolute()
        .test_id("theme-editor-panel")
        .label(t(title))
        .w(move || px(panel_width()))
        .h(move || px(panel_height()))
        .right(move || px(panel_right()))
        .bottom(move || px(panel_bottom()))
        .app_class(tw(
            "flex flex-col overflow-hidden rounded-xl border bg-popover text-popover-foreground",
        ))
        .child(header)
        .child(body)
        .child(resize);
    overlay()
        .absolute()
        .inset_0()
        .pointer_events(nsh_core::PointerEvents::BoxNone)
        .child(panel)
        .into_element()
}

fn editor(
    state: ThemeState,
    form: Signal<Option<Form>>,
    seed: Option<ThemeDefinition>,
    editing: bool,
    draft: EditorDraft,
) -> AnyElement {
    let EditorDraft {
        name,
        half,
        colors,
        advanced,
        filter,
        css,
        css_open,
        ..
    } = draft;
    let seed = stored(seed);
    let submit = Submission {
        state,
        form,
        busy: draft.busy,
        error: draft.error,
        session: use_context::<super::ThemeForms>().map(|host| host.draft),
    };
    let name_field = nsh_base::input()
        .label(t("Theme name"))
        .value(move || name.get())
        .placeholder(if editing {
            t("Theme name")
        } else {
            t("e.g. Aurora")
        })
        .on_change(move |s: &str| name.set(s.into()))
        .app_class(tw(
            "h-8 w-full rounded-lg border border-input bg-background px-3",
        ));
    let choices = Half::ALL.map(|h| {
        let label = t(if h == Half::Light { "Light" } else { "Dark" });
        nsh_base::button()
            .accessibility_label(label.clone())
            .selected(move || half.get() == h)
            .disabled(move || {
                editing && seed.with(|s| s.as_ref().is_some_and(|t| t.colors_for(h).is_none()))
            })
            .on_press(move || half.set(h))
            .child(text(label))
            .into_element()
    });
    let save = move || {
        let h = half.get_untracked();
        let colors = colors.get_untracked();
        let source = seed.get();
        let base = source
            .as_ref()
            .filter(|_| editing)
            .map_or(h, |t| t.appearance);
        let mut file = json!({"version":1,"name":name.get_untracked(),"appearance":base,"colors":colors[usize::from(base==Half::Dark)],"css":css.get_untracked()});
        if !advanced.get_untracked() {
            file["managed"] = json!(true);
        }
        if editing && let Some(source) = &source {
            file["id"] = json!(source.id);
            if let Some(collection) = &source.collection {
                file["collection"] = json!(collection);
            }
            if source.colors_for(base.other()).is_some() {
                file["variants"] =
                    json!({base.other().as_str():colors[usize::from(base.other()==Half::Dark)]});
            }
        }
        match parse_theme_file(&file) {
            Ok(theme) => {
                if let Some(existing) = state.choice.with_untracked(|c| {
                    c.custom
                        .iter()
                        .find(|t| {
                            t.label.eq_ignore_ascii_case(&theme.label)
                                && (!editing || t.id != theme.id)
                        })
                        .cloned()
                }) {
                    if let Some(conflict) = Half::ALL.into_iter().find(|half| {
                        theme.colors_for(*half).is_some() && existing.colors_for(*half).is_some()
                    }) {
                        submit.error.set(format!(
                            "“{}” already has a {} palette. Pick another name.",
                            existing.label,
                            conflict.as_str()
                        ));
                        return;
                    }
                    if let (Some(a), Some(b)) = (&existing.css, &theme.css)
                        && a != b
                    {
                        submit.error.set(format!(
                            "“{}” has different custom CSS. Use the same CSS or pick another name.",
                            existing.label
                        ));
                        return;
                    }
                    let mut merged = existing.clone();
                    for half in Half::ALL {
                        if let Some(colors) = theme.colors_for(half) {
                            merged
                                .variants
                                .get_or_insert_with(ThemeVariants::default)
                                .set(half, colors.clone());
                        }
                    }
                    merged.css = theme.css.or(merged.css);
                    if advanced.get_untracked() {
                        merged.managed = None;
                    }
                    submit.save(Edit::Merge(Box::new(ThemeMerge {
                        target: merged,
                        previous: existing,
                        retired: source.filter(|_| editing),
                    })));
                } else {
                    submit.save(Edit::Save(
                        vec![theme],
                        if editing {
                            SaveKind::Update
                        } else {
                            SaveKind::Create
                        },
                    ));
                }
            }
            Err(e) => submit.error.set(e.to_string()),
        }
    };
    view()
        .app_class(tw("flex flex-col gap-3"))
        .child(text(t("Theme name")))
        .child(name_field)
        .child(text(t("Appearance")))
        .child(
            view()
                .app_class(tw("flex flex-row gap-2"))
                .child(Vec::from(choices)),
        )
        .child(text(t("Colors")))
        .child(region(move ||advanced.get(), move |on| {
            if on {
                nsh_base::input().label(t("Filter colors")).placeholder(t("Filter colors"))
                    .value(move || filter.get()).on_change(move |s:&str| filter.set(s.into())).into_element()
            } else {text(t("Two colors, rest derived")).into_element()}
        }))
        .child(view().app_class(tw("flex flex-row items-center gap-2"))
            .child(text(t("Advanced")))
            .child(switch("Use advanced theme colors", ControlState { checked: Prop::bind(move ||advanced.get()), ..Default::default() }, Size::Default, move |on| advanced.set(on))))
        .child(region(
            move || (half.get(), advanced.get(), filter.get().trim().to_lowercase()),
            move |(h, adv, filter)| {
                let roles = if adv {
                    Role::ALL.into_iter().filter(|role|role.name().to_lowercase().contains(&filter) ||
                        (*role == Role::Canvas && "background".contains(&filter))).collect()
                } else {
                    vec![Role::Canvas, Role::Accent]
                };
                let index = usize::from(h == Half::Dark);
                let active_role = signal(None);
                let last_input = signal(None);
                let rows = roles
                    .into_iter()
                    .map(|role| {
                        let label = match role {
                            Role::Canvas => t("Background"),
                            Role::Accent => t("Accent"),
                            _ => role.name().to_owned(),
                        };
                        let draft =
                            signal(colors.with_untracked(|c| c[index].get(role).to_owned()));
                        let set_color = move |s: &str| {
                            draft.set(s.into());
                            last_input.set(Some(role));
                            colors.update(|c| {
                                c[index] = if adv {
                                    update_theme_color_family(h, &c[index], role, s)
                                } else {
                                    let next = c[index].clone().with(role, s);
                                    if to_canonical_theme_color(s).is_some() {
                                        create_vivid_theme_colors(h, next.get(Role::Canvas), next.get(Role::Accent))
                                    } else { next }
                                };
                            });
                        };
                        let displayed = move || if last_input.get()==Some(role) {draft.get()} else {colors.with(|all|all[index].get(role).to_owned())};
                        let picker_label = label.clone();
                        view().app_class(tw("flex flex-col gap-2"))
                            .child(view().app_class(tw("flex flex-row items-center gap-2"))
                                .child(nsh_base::button().accessibility_label(SharedString::from(format!("Pick {label} color")))
                                    .on_press(move || active_role.update(|open| *open = (*open != Some(role)).then_some(role)))
                                    .child(text(SharedString::from(label.clone()))))
                                .child(nsh_base::input().label(SharedString::from(label))
                                    .value(displayed).on_change(set_color)
                                    .app_class(tw("h-8 min-w-0 flex-1 rounded-lg border border-input px-2"))))
                            .child(region(move || active_role.get()==Some(role), move |open|
                                open.then(|| color_controls(&picker_label, displayed, set_color))
                            ))
                            .into_element()
                    })
                    .collect::<Vec<_>>();
                scroll()
                    .app_class(tw("max-h-72 w-full"))
                    .child(view().child(rows))
            },
        ))
        .child(control("Custom CSS", move || css_open.update(|o| *o = !*o)))
        .child(region(
            move || css_open.get(),
            move |open| {
                open.then(|| {
                    view().child(text(t("Root and sidebar color variables are applied here. Other CSS is preserved on export.")))
                      .child(nsh_base::input()
                        .label(t("Custom CSS"))
                        .multiline(true)
                        .value(move || css.get())
                        .on_change(move |s: &str| css.set(s.into())))
                })
            },
        ))
        .child(submit.message())
        .child(
            view()
                .app_class(tw("flex flex-row justify-end gap-2"))
                .child(control("Cancel", move || form.set(None)))
                .child(button(
                    ButtonContent::Label(t(if editing {
                        "Save changes"
                    } else {
                        "Create theme"
                    })),
                    ButtonVariant::Default,
                    ButtonSize::Sm,
                    move || name.with(|n| n.trim().is_empty()) || submit.busy.get(),
                    save,
                )),
        )
        .into_element()
}

fn color_controls(
    label: &str,
    displayed: impl Fn() -> String + Copy + 'static,
    set_color: impl Fn(&str) + Copy + 'static,
) -> AnyElement {
    use crate::ui::color_picker::{PlaneVariant, color_hue_slider, color_saturation_value_plane};
    use t3_client::view_logic::color::{HsvColor, hex_to_hsv, hsv_to_hex};
    use t3_theme::palette::theme_color_to_hex;

    let external = computed(move || theme_color_to_hex(&displayed()));
    let initial = external.get_untracked();
    let hsv = signal(hex_to_hsv(initial.as_deref().unwrap_or("#000000")));
    let last = signal(initial);
    effect(move || {
        if let Some(hex) = external.get()
            && last.with_untracked(|previous| previous.as_ref() != Some(&hex))
        {
            hsv.set(hex_to_hsv(&hex));
            last.set(Some(hex));
        }
    });
    let update = move |color: HsvColor| {
        let mut hex = hsv_to_hex(color.h, color.s, color.v);
        if let Some(alpha) = external
            .get_untracked()
            .and_then(|value| value.get(7..9).map(str::to_owned))
        {
            hex.push_str(&alpha);
        }
        // Choosing a different hue while gray may produce the same RGB. Keep that hue for the next drag.
        last.set(Some(hex.clone()));
        hsv.set(color);
        set_color(&hex);
    };
    view()
        .app_class(tw("flex flex-col gap-2"))
        .child(color_saturation_value_plane(
            Text::content(label.to_owned()),
            move || hsv.get(),
            PlaneVariant::Inset,
            update,
            None,
        ))
        .child(color_hue_slider(
            Text::content(format!("{label} hue")),
            move || hsv.get().h,
            move |h| {
                update(HsvColor {
                    h,
                    ..hsv.get_untracked()
                })
            },
            None,
        ))
        .into_element()
}
