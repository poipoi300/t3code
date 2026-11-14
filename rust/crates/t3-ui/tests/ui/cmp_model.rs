//! CMP-MODEL: headless tests of Model, provider and option pickers. Start the app with `crate::support`.
//! Controlled picker interactions; model policy itself is exercised in t3-client.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::view_logic::shell::models::ModelOption;
use t3_contracts::model::ProviderOptionDescriptor;
use t3_contracts::server::ServerProvider;
use t3_ui::i18n::t;
use t3_ui::screens::composer::model_picker::{
    CatalogState, ModelKey, PickerData, PickerProps, PickerProvider, TraitsView,
    model_picker_create_handle, provider_model_picker,
};
use t3_ui::theme;

use crate::support::{DESKTOP, PHONE};

fn key(instance: &str, model: &str) -> ModelKey {
    ModelKey {
        instance_id: t3_contracts::provider_instance::ProviderInstanceId(instance.into()),
        model: model.into(),
    }
}

fn data() -> PickerData {
    let providers = ["codex", "claudeAgent"]
        .into_iter()
        .map(|id| {
            let snapshot: ServerProvider = serde_json::from_value(serde_json::json!({
            "instanceId": id, "driver": id, "enabled": true, "installed": true,
            "version": null, "status": "ready", "auth": {"status": "authenticated"},
            "checkedAt": "2026-01-01T00:00:00.000Z", "models": [], "slashCommands": [], "skills": []
        })).expect("provider");
            PickerProvider {
                snapshot,
                display_name: if id == "codex" { "Codex" } else { "Claude" }.into(),
                enabled: true,
                available: true,
                ready: true,
                continuation_group: None,
                accent_color: None,
                acp_agent_id: None,
                acp_icon_url: None,
                options: ["Original model", "Second model", "Legacy model"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| ModelOption {
                        slug: ["original", "second", "legacy"][i].into(),
                        name: name.into(),
                        is_legacy: i == 2,
                        ..Default::default()
                    })
                    .collect(),
            }
        })
        .collect();
    PickerData {
        providers,
        selection: key("codex", "original"),
        selected_models: None,
        favorites: BTreeSet::new(),
        locked_provider: None,
        locked_continuation_group: None,
        disabled_models: BTreeMap::new(),
    }
}

fn traits(model: ModelKey) -> TraitsView {
    let descriptors: Vec<ProviderOptionDescriptor> = serde_json::from_value(serde_json::json!([
        {"id":"thought_level", "label":"Thinking", "type":"select", "role":"reasoning", "currentValue":"medium", "options":[{"id":"low","label":"Low"},{"id":"medium","label":"Medium"},{"id":"high","label":"High"}]},
        {"id":"mode", "label":"Session Mode", "type":"select", "currentValue":"code", "options":[{"id":"code","label":"Code"},{"id":"plan","label":"Plan"}]},
        {"id":"auto_approve", "label":"Auto-approve tools", "type":"boolean", "currentValue":false},
        {"id":"serviceTier", "label":"Speed", "type":"select", "currentValue":"standard", "options":[{"id":"standard","label":"Standard"},{"id":"fast","label":"Fast"}]}
    ])).expect("descriptors");
    TraitsView {
        model: Some(model),
        reasoning: Some(descriptors[0].clone()),
        others: descriptors[1..3].to_vec(),
        speed: Some(descriptors[3].clone()),
        ..Default::default()
    }
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    docked: bool,
    disabled_favorite: bool,
}

#[component]
fn Harness(harness: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let mut initial = data();
    if harness.disabled_favorite {
        initial.favorites.insert(key("claudeAgent", "original"));
        initial.providers[1].enabled = false;
        initial.providers[1].ready = false;
    }
    let data = signal(Rc::new(initial));
    let catalog = signal(CatalogState::Ready);
    let options = signal(traits(key("codex", "original")));
    let picks = signal(0usize);
    let option_picks = signal(0usize);
    let refreshes = signal(0usize);
    let handle = model_picker_create_handle();
    let mut props = PickerProps::new(data.into(), move |model| {
        picks.update(|n| *n += 1);
        data.update(|d| Rc::make_mut(d).selection = model.clone());
        options.set(traits(model));
    });
    props.catalog = catalog.into();
    props.traits = options.into();
    props.docked = harness.docked;
    props.handle = Some(handle);
    props.on_refresh = Some(Rc::new(move || refreshes.update(|n| *n += 1)));
    props.trigger_label = Some("Choose model".into()).into();
    props.on_toggle_favorite = Some(Rc::new(move |model| {
        data.update(|d| {
            let d = Rc::make_mut(d);
            if !d.favorites.remove(&model) {
                d.favorites.insert(model);
            }
        })
    }));
    props.on_option_pick = Some(Rc::new(move |_, selection| {
        option_picks.update(|n| *n += 1);
        options.update(|traits| {
            for descriptor in traits
                .reasoning
                .iter_mut()
                .chain(traits.others.iter_mut())
                .chain(traits.speed.iter_mut())
            {
                match descriptor {
                    ProviderOptionDescriptor::Select(d) if d.id == selection.id => {
                        if let t3_contracts::model::ProviderOptionSelectionValue::String(value) =
                            &selection.value
                        {
                            d.current_value = Some(Some(value.clone()));
                        }
                    }
                    ProviderOptionDescriptor::Boolean(d) if d.id == selection.id => {
                        if let t3_contracts::model::ProviderOptionSelectionValue::Boolean(value) =
                            &selection.value
                        {
                            d.current_value = Some(Some(*value));
                        }
                    }
                    _ => {}
                }
            }
        });
    }));
    view()
        .w_full()
        .h_full()
        .child(provider_model_picker(props))
        .child(text(move || format!("Picks: {}", picks.get())))
        .child(text(move || format!("Refreshes: {}", refreshes.get())))
        .child(text(move || {
            format!("Option picks: {}", option_picks.get())
        }))
        .child(text(move || {
            format!(
                "Selected: {}:{}",
                data.get().selection.instance_id.0,
                data.get().selection.model
            )
        }))
        .child(
            nsh_base::button()
                .accessibility_label(t("Open model picker"))
                .on_press(move || handle.set_open(true))
                .child(text(t("Open model picker"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Disconnect"))
                .on_press(move || catalog.set(CatalogState::Offline))
                .child(text(t("Disconnect"))),
        )
        .child(
            nsh_base::button()
                .accessibility_label(t("Lock provider"))
                .on_press(move || {
                    data.update(|d| Rc::make_mut(d).locked_provider = Some("codex".into()))
                })
                .child(text(t("Lock provider"))),
        )
}

fn start(window: (f32, f32), docked: bool, disabled_favorite: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            docked,
            disabled_favorite,
        }))
    };
    config.environment.os = if window == PHONE {
        Os::Android
    } else {
        Os::Web
    };
    config.environment.idiom = if window == PHONE {
        Idiom::Phone
    } else {
        Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("picker starts");
    app.wait_for("role:button[Choose model]")
        .expect("picker trigger");
    app
}

fn open(app: &mut Headless) {
    app.press("role:button[Choose model]").expect("open");
    app.wait_for("role:button[Second model, Codex]")
        .expect("models");
}

#[test]
fn picking_updates_the_controlled_model_and_a_docked_picker_stays_open_at_both_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true, false);
        open(&mut app);
        app.press("role:button[Second model, Codex]").expect("pick");
        app.wait_for("Selected: codex:second")
            .expect("selected identity");
        assert!(app.screen().shows("Picks: 1"));
        app.wait_for("role:button[Original model, Codex]")
            .expect("docked list remains");
        app.press("role:button[Show more]").expect("expand legacy");
        app.wait_for("role:button[Legacy model, Codex]")
            .expect("legacy revealed");
        app.press("role:button[Show less]")
            .expect("collapse legacy");
        assert!(
            !app.screen()
                .matches("role:button[Legacy model, Codex]")
                .expect("selector")
        );
    }
}

#[test]
fn disabled_favorites_do_not_hide_the_active_provider_and_an_undocked_pick_closes() {
    let mut app = start(DESKTOP, false, true);
    open(&mut app);
    app.press("role:button[Second model, Codex]").expect("pick");
    app.wait_for("Selected: codex:second").expect("selection");
    app.wait_until(
        "standalone popup closes",
        std::time::Duration::from_secs(5),
        |screen| {
            !screen
                .matches("role:button[Original model, Codex]")
                .expect("selector")
        },
    )
    .expect("popup removed");
}

#[test]
fn provider_lock_and_disconnect_prevent_choices_already_shown_in_the_popup() {
    let mut app = start(DESKTOP, true, false);
    open(&mut app);
    app.press("role:button[Disconnect]").expect("disconnect");
    app.wait_for("Not connected. Model selection is unavailable.")
        .expect("offline explanation");
    assert!(
        !app.screen()
            .enabled("role:button[Second model, Codex]")
            .expect("row")
    );
    assert!(app.press("role:button[Second model, Codex]").is_err());
    assert!(app.screen().shows("Picks: 0"));
}

#[test]
fn arbitrary_agent_options_change_without_becoming_reasoning_levels() {
    let mut app = start(DESKTOP, true, false);
    open(&mut app);
    app.press("role:radio[High]").expect("reasoning");
    app.wait_for("Option picks: 1").expect("option callback");
    app.press("role:switch[Auto-approve tools]")
        .expect("boolean option");
    app.wait_for("Option picks: 2")
        .expect("second option callback");
    app.press("role:combobox[Session Mode]")
        .expect("mode submenu");
    app.wait_for("role:option[Plan]").expect("choice");
    app.press("role:option[Plan]").expect("plan");
    app.wait_for("Option picks: 3").expect("select callback");
    app.press("role:radio[Fast]").expect("speed segment");
    app.wait_for("Option picks: 4").expect("speed callback");
    app.wait_for("role:button[Original model, Codex]")
        .expect("docked list remains");
}

#[test]
fn external_open_refreshes_once_and_favorites_can_be_added_and_removed_without_picking_a_model() {
    let mut app = start(DESKTOP, true, false);
    app.press("role:button[Open model picker]")
        .expect("palette seam");
    app.wait_for("role:button[Second model, Codex]")
        .expect("models");
    app.wait_for("Refreshes: 1").expect("opening refresh");
    app.press("role:button[Add to favorites] >> nth=1")
        .expect("star second model");
    app.press("role:button[Favorites]").expect("favorites tab");
    app.wait_for("role:button[Second model, Codex]")
        .expect("favorite shown");
    assert!(
        !app.screen()
            .matches("role:button[Original model, Codex]")
            .expect("selector")
    );
    app.press("role:button[Remove from favorites]")
        .expect("unstar");
    app.wait_for("No favorites yet").expect("empty favorites");
    assert!(app.screen().shows("Picks: 0"));
    assert!(app.screen().shows("Refreshes: 1"));
}
