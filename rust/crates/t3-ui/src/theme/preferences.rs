//! Device theme preferences and server-published palettes (useTheme / useEnvironmentTheme).
use std::rc::Rc;

use futures::{StreamExt, channel::mpsc, future::join_all};
use nsh_core::prelude::*;
use serde_json::{Value, json};
use t3_client::{
    composition::ClientRuntime, platform::Platform,
    thread_views::composer::preferences::SettingsHydration,
};
use t3_theme::palette::{
    Appearance as Half, AppearanceMode, CUSTOM_THEMES_STORAGE_KEY as LIBRARY, StorageFailure,
    THEME_APPEARANCE_MODE_STORAGE_KEY as MODE, THEME_FOLLOW_SYSTEM_STORAGE_KEY as FOLLOW,
    THEME_HALVES_STORAGE_KEY as HALVES, ThemeCatalog, ThemeDefinition, ThemeHalves, ThemeLibrary,
    ThemeStorage, canonical_theme_preference, published_theme_definitions,
};

use super::{ThemeChoice, install};
use crate::{
    i18n::Text,
    runtime::tasks::watch,
    ui::toast::{ToastOptions, Variant, toast_manager},
};

const BASE: &str = "t3code:theme";

#[derive(Clone, Default, PartialEq)]
struct Preferences {
    base: Option<String>,
    mode: Option<String>,
    follow: Option<bool>,
    halves: ThemeHalves,
    library: Option<String>,
}

impl ThemeStorage for Preferences {
    fn read(&self) -> Result<Option<String>, StorageFailure> {
        Ok(self.library.clone())
    }
    fn write(&mut self, raw: &str) -> Result<(), StorageFailure> {
        self.library = Some(raw.into());
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveKind {
    Import,
    InstallMany,
    Replace,
    Create,
    Update,
}

#[derive(Clone)]
pub(crate) struct ThemeMerge {
    pub target: ThemeDefinition,
    pub previous: ThemeDefinition,
    pub retired: Option<ThemeDefinition>,
}

#[derive(Clone)]
pub(crate) enum Edit {
    Mode(String),
    Whole(String),
    Half(Half, Option<String>),
    Save(Vec<ThemeDefinition>, SaveKind),
    Merge(Box<ThemeMerge>),
    Remove(Vec<String>),
    Collection(String, Vec<ThemeDefinition>, Vec<ThemeDefinition>),
}

#[derive(Default)]
struct ReadSet {
    conditions: Vec<t3_client::platform::StorageCondition>,
    errors: Vec<(&'static str, String)>,
}

impl Preferences {
    async fn load<P: Platform + Clone + 'static>(
        service: &t3_client::services::settings::service::SettingsService<P>,
    ) -> (Self, ReadSet) {
        let values = join_all(
            [BASE, MODE, FOLLOW, HALVES, LIBRARY].map(|key| service.read_theme_preference(key)),
        )
        .await;
        let mut prefs = Preferences::default();
        let mut guard = ReadSet::default();
        for (key, value) in [BASE, MODE, FOLLOW, HALVES, LIBRARY]
            .into_iter()
            .zip(values)
        {
            let value = value
                .map(|(value, records)| {
                    guard.conditions.extend(records);
                    value
                })
                .inspect_err(|e| guard.errors.push((key, e.to_string())));
            match (key, value) {
                (BASE, Ok(Some(Value::String(id)))) => prefs.base = Some(id),
                (MODE, Ok(Some(Value::String(mode)))) => prefs.mode = Some(mode),
                (FOLLOW, Ok(Some(Value::Bool(on)))) => prefs.follow = Some(on),
                (HALVES, Ok(Some(value))) => {
                    for half in Half::ALL {
                        prefs.set_half(half, value[half.as_str()].as_str().map(str::to_owned));
                    }
                }
                (LIBRARY, Ok(Some(Value::Array(stored)))) => {
                    prefs.library = Some(Value::Array(stored).to_string());
                }
                // A malformed library stays unreadable to ThemeLibrary; a later install cannot erase it.
                (LIBRARY, Err(_)) => prefs.library = Some("null".into()),
                _ => {}
            }
        }
        (prefs, guard)
    }

    fn base(&self) -> &str {
        self.base.as_deref().unwrap_or("system")
    }

    fn choice(
        &self,
        custom: Rc<[ThemeDefinition]>,
        environment: Rc<[ThemeDefinition]>,
        contrast: u32,
    ) -> ThemeChoice {
        let catalog = ThemeCatalog {
            custom: &custom,
            environment: &environment,
        };
        let base = self.base();
        let base = if catalog.is_known_preference(base) {
            canonical_theme_preference(base)
        } else {
            "system"
        };
        let mode = self
            .mode
            .as_deref()
            .and_then(AppearanceMode::parse)
            .unwrap_or_else(|| {
                if self.follow.unwrap_or(base == "system") {
                    AppearanceMode::System
                } else if catalog.preference_mode(base) == Some(Half::Dark) {
                    AppearanceMode::Dark
                } else {
                    AppearanceMode::Light
                }
            });
        let resolve = |half| {
            let theme = catalog.get(self.halves.get(half)?)?;
            theme.colors_for(half).map(|_| theme.id.clone())
        };
        let halves = ThemeHalves {
            light: resolve(Half::Light),
            dark: resolve(Half::Dark),
        };
        ThemeChoice {
            theme: base.into(),
            mode,
            halves: (halves != ThemeHalves::default()).then_some(halves),
            contrast,
            custom,
            environment,
        }
    }

    fn whole(&mut self, base: String, mode: AppearanceMode) {
        self.mode = Some(mode_key(mode).into());
        self.base = Some(base);
        self.halves = Default::default();
    }

    fn assign(&mut self, half: Half, id: Option<String>, choice: &ThemeChoice) {
        let base = choice.catalog().get(&choice.theme);
        if id.is_none()
            && let Some(base) = base
        {
            let other = half.other();
            let owner = self.halves.get(other).unwrap_or(&base.id).to_owned();
            self.whole(mode_key(choice.mode).into(), choice.mode);
            self.set_half(other, Some(owner));
        } else {
            self.set_half(half, id);
        }
    }

    fn set_half(&mut self, half: Half, id: Option<String>) {
        match half {
            Half::Light => self.halves.light = id,
            Half::Dark => self.halves.dark = id,
        }
    }

    fn edit(&mut self, edit: Edit, choice: &ThemeChoice) -> Result<(), String> {
        match edit {
            Edit::Mode(mode) => self.mode = Some(mode),
            Edit::Whole(base) => self.whole(base, choice.mode),
            Edit::Half(half, id) => self.assign(half, id, choice),
            Edit::Save(themes, kind) => {
                let mut library = ThemeLibrary::new(self.clone());
                for theme in &themes {
                    if matches!(kind, SaveKind::Update | SaveKind::Replace)
                        && library.custom_themes().iter().any(|old| old.id == theme.id)
                    {
                        library.update(theme)
                    } else {
                        library.install(theme)
                    }
                    .map_err(|e| e.to_string())?;
                }
                self.library.clone_from(&library.storage().library);
                if let [theme] = themes.as_slice()
                    && kind == SaveKind::Create
                {
                    self.whole(theme.id.clone(), choice.mode);
                } else if let [theme] = themes.as_slice()
                    && kind == SaveKind::Import
                {
                    if let Some(half) = theme.single_appearance() {
                        self.assign(half, Some(theme.id.clone()), choice);
                    } else {
                        self.whole(theme.id.clone(), choice.mode);
                    }
                }
            }
            Edit::Merge(merge) => {
                let ThemeMerge {
                    target,
                    previous,
                    retired,
                } = *merge;
                let mut library = ThemeLibrary::new(self.clone());
                let saved = library.custom_themes();
                if !saved.contains(&previous)
                    || retired.as_ref().is_some_and(|old| !saved.contains(old))
                {
                    return Err("This theme changed while you were editing it.".into());
                }
                library.update(&target).map_err(|err| err.to_string())?;
                if let Some(old) = retired {
                    library.remove(&[&old.id]).map_err(|err| err.to_string())?;
                }
                self.library.clone_from(&library.storage().library);
                self.whole(target.id, choice.mode);
            }
            Edit::Collection(id, themes, expected) => {
                let mut library = ThemeLibrary::new(self.clone());
                library
                    .replace_collection(&id, &themes, Some(&expected))
                    .map_err(|e| e.to_string())?;
                self.library.clone_from(&library.storage().library);
            }
            Edit::Remove(ids) => {
                let mut library = ThemeLibrary::new(self.clone());
                library
                    .remove(&ids.iter().map(String::as_str).collect::<Vec<_>>())
                    .map_err(|e| e.to_string())?;
                self.library.clone_from(&library.storage().library);
                if ids.iter().any(|id| id == self.base()) {
                    self.base = Some(mode_key(choice.mode).into());
                }
                for half in Half::ALL {
                    if self
                        .halves
                        .get(half)
                        .is_some_and(|id| ids.iter().any(|it| it == id))
                    {
                        self.set_half(half, None);
                    }
                }
            }
        }
        Ok(())
    }

    fn changes(&self, next: &Self) -> Result<Vec<(&'static str, Option<Value>)>, String> {
        let mut writes = Vec::new();
        if self.mode != next.mode {
            writes.push((MODE, next.mode.clone().map(Value::String)));
        }
        if self.base != next.base {
            writes.push((BASE, next.base.clone().map(Value::String)));
        }
        if self.halves != next.halves {
            let mut mix = json!({});
            for half in Half::ALL {
                if let Some(id) = next.halves.get(half) {
                    mix[half.as_str()] = json!(id);
                }
            }
            writes.push((HALVES, (mix != json!({})).then_some(mix)));
        }
        if self.library != next.library {
            writes.push((
                LIBRARY,
                next.library
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e| e.to_string())?,
            ));
        }
        Ok(writes)
    }
}

pub(crate) fn mode_key(mode: AppearanceMode) -> &'static str {
    match mode {
        AppearanceMode::Dark => "dark",
        AppearanceMode::Light => "light",
        AppearanceMode::System => "system",
    }
}

type Export = Box<dyn Fn(&ThemeDefinition)>;
type Completion = Box<dyn FnOnce(Result<(), String>)>;
type Request = (Edit, Completion);

#[derive(Clone, Copy)]
pub struct ThemeState {
    pub(crate) preview: Signal<Option<ThemeDefinition>>,
    export: Stored<Option<Export>>,
    pub choice: Memo<ThemeChoice>,
    ready: Memo<bool>,
    requests: Stored<mpsc::UnboundedSender<Request>>,
}

impl ThemeState {
    pub(crate) fn can_export(self) -> bool {
        self.export.with(|e| e.is_some())
    }
    pub(crate) fn export(self, theme: &ThemeDefinition) {
        self.export.with(|e| {
            if let Some(export) = e {
                export(theme);
            }
        });
    }

    pub(crate) fn save(self, edit: Edit, done: impl FnOnce(Result<(), String>) + 'static) {
        self.requests.with(|requests| {
            let _ = requests.unbounded_send((edit, Box::new(done)));
        });
    }

    pub(crate) fn choose(self, edit: Edit) {
        self.save(edit, |result| {
            if let Err(error) = result {
                save_error(error);
            }
        });
    }
}

fn save_error(error: String) {
    toast_manager().add(ToastOptions {
        variant: Variant::Error,
        title: Text::from("Couldn’t save theme selection"),
        description: Some(Text::content(error)),
        ..Default::default()
    });
}

/// Bind above navigation so changes continue on every route, including while Appearance is closed.
pub fn bind<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    host: Option<t3_client::types::EnvironmentId>,
) {
    let service = client.settings();
    let settings = client.composer_preferences();
    let store = client.store().clone();
    let contrast = signal(settings.snapshot().appearance_contrast as u32);
    let settings_ready = signal(false);
    let environment = signal(Rc::<[ThemeDefinition]>::default());
    let observed = service.clone();
    let mut changes = observed.changes();
    let catalog = client.clone();
    let client_catalog = move || catalog.environments();
    watch(async move {
        let mut previous = None;
        loop {
            let prefs = settings.snapshot();
            if contrast.get_untracked() != prefs.appearance_contrast as u32 {
                contrast.set(prefs.appearance_contrast as u32);
            }
            settings_ready.set(!matches!(
                settings.hydration(),
                SettingsHydration::Pending | SettingsHydration::Retrying
            ));
            let source = observed.primary_environment().or_else(|| {
                host.as_ref()
                    .filter(|id| client_catalog().iter().any(|entry| &entry.id == *id))
                    .cloned()
            });
            let config =
                source.and_then(|id| store.snapshot().environments.get(&id)?.config.clone());
            let themes = config
                .as_ref()
                .and_then(|config| config.environment_themes.as_ref()?.as_ref());
            if previous.as_ref() != themes {
                let palettes = themes
                    .into_iter()
                    .flatten()
                    .filter_map(|theme| {
                        serde_json::from_value(serde_json::to_value(theme).ok()?).ok()
                    })
                    .collect::<Vec<_>>();
                environment.set(Rc::from(published_theme_definitions(&palettes)));
                previous = themes.cloned();
            }
            if changes.next().await.is_none() {
                break;
            }
        }
    });
    let device = signal(Preferences::default());
    let custom = signal(Rc::<[ThemeDefinition]>::default());
    let loaded = signal(false);
    let choice = computed(move || {
        device.with(|p| p.choice(custom.get(), environment.get(), contrast.get()))
    });
    let (send, receive) = mpsc::unbounded::<Request>();
    let files = client.plan_services();
    let owner = crate::runtime::tasks::TaskOwner::here();
    let export = files.can_download().then(|| {
        Box::new(move |theme: &ThemeDefinition| {
            match t3_theme::palette::serialize_theme_file(theme) {
                Ok(json) => {
                    let download = files.download_text(
                        format!("{}.json", theme.id),
                        json,
                        "application/json".into(),
                    );
                    owner.spawn(async move {
                        if let Err(e) = download.await {
                            save_error(e.to_string());
                        }
                    });
                }
                Err(e) => save_error(e.to_string()),
            }
        }) as Export
    });
    provide_context(ThemeState {
        preview: signal(None),
        export: stored(export),
        choice,
        ready: computed(move || loaded.get() && settings_ready.get()),
        requests: stored(send),
    });
    let external = service.theme_preference_changes();
    watch(async move {
        let (mut prefs, _) = Preferences::load(&service).await;
        let update_library = move |prefs: &Preferences| {
            custom.set(Rc::from(ThemeLibrary::new(prefs.clone()).custom_themes()));
        };
        update_library(&prefs);
        device.set(prefs.clone());
        loaded.set(true);
        let mut requests = futures::stream::select(receive.map(Some), external.map(|()| None));
        while let Some(request) = requests.next().await {
            let Some((edit, done)) = request else {
                let (refreshed, _) = Preferences::load(&service).await;
                if refreshed != prefs {
                    if refreshed.library != prefs.library {
                        update_library(&refreshed);
                    }
                    prefs = refreshed;
                    device.set(prefs.clone());
                }
                continue;
            };
            let result = async {
                for _ in 0..3 {
                    let (latest, guard) = Preferences::load(&service).await;
                    let catalogue = if latest.library == prefs.library {
                        custom.get_untracked()
                    } else {
                        Rc::from(ThemeLibrary::new(latest.clone()).custom_themes())
                    };
                    let choice = latest.choice(
                        catalogue,
                        environment.get_untracked(),
                        contrast.get_untracked(),
                    );
                    let mut next = latest.clone();
                    next.edit(edit.clone(), &choice)?;
                    let writes = latest.changes(&next)?;
                    if let Some((_, error)) = guard
                        .errors
                        .iter()
                        .find(|(key, _)| writes.iter().any(|(written, _)| key == written))
                    {
                        return Err(error.clone());
                    }
                    if !writes.is_empty()
                        && !service
                            .write_theme_preferences(writes, guard.conditions)
                            .await
                            .map_err(|e| e.to_string())?
                    {
                        continue;
                    }
                    if next.library != prefs.library {
                        update_library(&next);
                    }
                    prefs = next;
                    device.set(prefs.clone());
                    return Ok(());
                }
                Err("Theme settings changed in another window. Try again.".into())
            }
            .await;
            done(result);
        }
    });
}

/// Do not construct any visible route until its initial device palette and contrast are installed.
pub fn with_saved_theme(build: impl FnOnce() -> AnyElement + 'static) -> AnyElement {
    let Some(state) = use_context::<ThemeState>() else {
        install(&ThemeChoice::default());
        return build();
    };
    let painted = signal(false);
    effect(move || {
        if state.ready.get() {
            let mut choice = state.choice.get();
            if let Some(preview) = state.preview.get() {
                choice.mode = if preview.appearance == Half::Light {
                    AppearanceMode::Light
                } else {
                    AppearanceMode::Dark
                };
                choice.theme.clone_from(&preview.id);
                choice.halves = None;
                choice.custom = Rc::from([preview]);
            }
            untracked(|| {
                install(&choice);
                if !painted.get_untracked() {
                    painted.set(true);
                }
            });
        }
    });
    let build = stored(Some(build));
    dynamic(move || {
        if !painted.get() {
            return Vec::new();
        }
        build
            .with(Option::take)
            .map(|build| {
                untracked(|| crate::screens::settings::appearance::with_form_host(state, build))
            })
            .unwrap_or_default()
    })
}
