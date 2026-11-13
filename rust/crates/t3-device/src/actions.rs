use crate::{
    Error, Result,
    error::validate_selector,
    host::Ready,
    io::{Command, Commands},
};
use serde_json::{Value, json};
use t3_contracts::device::*;

pub fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

async fn checked(run: &dyn Commands, command: Command, operation: &str) -> Result<String> {
    let result = run.run(command).await?;
    if result.code != 0 {
        return Err(Error::command(operation, result.code));
    }
    Ok(result.stdout)
}

async fn adb(run: &dyn Commands, serial: &str, args: Vec<String>, op: &str) -> Result<String> {
    checked(
        run,
        Command::new("adb", [vec!["-s".into(), serial.into()], args].concat()),
        op,
    )
    .await
}
async fn shell(run: &dyn Commands, serial: &str, args: Vec<String>, op: &str) -> Result<String> {
    adb(
        run,
        serial,
        [
            vec!["shell".into()],
            args.iter().map(|s| shell_word(s)).collect(),
        ]
        .concat(),
        op,
    )
    .await
}
fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

pub async fn run_action(
    ready: &Ready,
    platform: DevicePlatform,
    input: &DeviceActionInput,
) -> Result<()> {
    let value = serde_json::to_value(input).map_err(|_| crate::error::invalid("action"))?;
    let text = |key| value.get(key).and_then(Value::as_str).unwrap_or("");
    let op = text("type");
    if op == "setLocation" {
        for(key,limit)in [("latitude",90.0),("longitude",180.0)] {
            if !value[key].as_f64().is_some_and(|n|n.is_finite()&&(-limit..=limit).contains(&n)) {
                return Err(crate::error::invalid("location"));
            }
        }
    }
    let id = text("deviceId");
    validate_selector(id, "device id")?;
    let app = text("appId");
    if !app.is_empty() {
        validate_selector(app, "app id")?;
    }
    let selected = text("value");
    let toggle = value.get("value").and_then(Value::as_bool).unwrap_or(false);
    let setting = text("setting");
    let unsupported = || Error::unsupported(op, platform.clone(), false);
    let run = ready.commands.as_ref();
    if platform == DevicePlatform::Ios {
        let mut args = match op {
            "setAppearance" => words(&["ui", id, "appearance", selected]),
            "setTextSize" => words(&[
                "ui",
                id,
                "content_size",
                match selected {
                    "small" => "small",
                    "large" => "extra-extra-large",
                    "extra-large" => "accessibility-large",
                    _ => "large",
                },
            ]),
            "setToggle" if setting == "increaseContrast" => words(&[
                "ui",
                id,
                "increase_contrast",
                if toggle { "enabled" } else { "disabled" },
            ]),
            "setToggle" | "setLiquidGlass" | "setColorFilter" => {
                let option = match (op, setting) {
                    ("setLiquidGlass", _) => "liquid-glass",
                    ("setColorFilter", _) => "color-filter",
                    (_, "reduceMotion") => "reduce-motion",
                    (_, "reduceTransparency") => "reduce-transparency",
                    (_, "showBorders") => "show-borders",
                    (_, "voiceOver") => "voiceover",
                    _ => return Err(unsupported()),
                };
                let helper = ready
                    .ax_helper
                    .as_deref()
                    .ok_or_else(|| Error::unsupported("accessibility", platform.clone(), true))?;
                words(&[
                    "spawn",
                    id,
                    helper,
                    "set",
                    option,
                    if op == "setToggle" {
                        if toggle { "on" } else { "off" }
                    } else {
                        selected
                    },
                ])
            }
            "setLocation" => vec![
                "location".into(),
                id.into(),
                "set".into(),
                format!("{},{}", value["latitude"], value["longitude"]),
            ],
            "clearLocation" => words(&["location", id, "clear"]),
            "setPermission" => {
                let permission = text("permission");
                let decision = text("decision");
                if permission == "notifications" {
                    let cli = ready
                        .sim_cli
                        .as_deref()
                        .ok_or_else(|| Error::unsupported("permission", platform.clone(), true))?;
                    checked(
                        run,
                        Command::new(
                            &ready.node_path,
                            [cli, "permissions", decision, permission, app, "-d", id],
                        ),
                        "permission",
                    )
                    .await?;
                    return Ok(());
                }
                if ![
                    "camera",
                    "microphone",
                    "photos",
                    "contacts",
                    "calendar",
                    "reminders",
                    "motion",
                    "media-library",
                    "faceid",
                    "location",
                ]
                .contains(&permission)
                {
                    return Err(unsupported());
                }
                words(&["privacy", id, decision, permission, app])
            }
            "openUrl" => words(&["openurl", id, text("url")]),
            "launchApp" => words(&["launch", id, app]),
            "terminateApp" => words(&["terminate", id, app]),
            "sendPush" => {
                let payload = match &value["payload"] {
                    Value::String(s) => json!({"aps":{"alert":s}}),
                    v => v.clone(),
                };
                let mut command = Command::new("xcrun", ["simctl", "push", id, app, "-"]);
                command.stdin =
                    Some(serde_json::to_vec(&payload).map_err(|_| crate::error::invalid("push"))?);
                checked(run, command, "push").await?;
                return Ok(());
            }
            _ => return Err(unsupported()),
        };
        args.insert(0, "simctl".into());
        checked(run, Command::new("xcrun", args), op).await?;
        return Ok(());
    }
    let args = match op {
        "setAppearance" => words(&[
            "cmd",
            "uimode",
            "night",
            if selected == "dark" { "yes" } else { "no" },
        ]),
        "setTextSize" => words(&[
            "settings",
            "put",
            "system",
            "font_scale",
            match selected {
                "small" => "0.85",
                "large" => "1.15",
                "extra-large" => "1.3",
                _ => "1.0",
            },
        ]),
        "setToggle" if setting == "networkEnabled" => {
            for mode in ["wifi", "data"] {
                shell(
                    run,
                    id,
                    words(&["svc", mode, if toggle { "enable" } else { "disable" }]),
                    "network",
                )
                .await?;
            }
            return Ok(());
        }
        "setToggle" if setting == "reduceMotion" => {
            for key in [
                "animator_duration_scale",
                "transition_animation_scale",
                "window_animation_scale",
            ] {
                shell(
                    run,
                    id,
                    words(&[
                        "settings",
                        "put",
                        "global",
                        key,
                        if toggle { "0" } else { "1" },
                    ]),
                    "reduce motion",
                )
                .await?;
            }
            return Ok(());
        }
        "setOrientation" => {
            let size = shell(run, id, words(&["wm", "size"]), "orientation")
                .await
                .ok();
            let landscape = size
                .as_deref()
                .and_then(|s| {
                    s.lines()
                        .filter_map(|s| s.split_once("size: ").map(|(_, s)| s))
                        .next_back()
                })
                .and_then(|s| s.split_once('x'))
                .and_then(|(w, h)| {
                    Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
                })
                .is_some_and(|(w, h)| w > h);
            let base = match selected {
                "landscape_left" => 1,
                "portrait_upside_down" => 2,
                "landscape_right" => 3,
                _ => 0,
            };
            let rotation = if landscape { (base + 3) % 4 } else { base };
            if id.starts_with("emulator-") {
                shell(
                    run,
                    id,
                    words(&["settings", "put", "system", "accelerometer_rotation", "1"]),
                    op,
                )
                .await?;
                shell(
                    run,
                    id,
                    words(&["cmd", "window", "user-rotation", "free"]),
                    op,
                )
                .await?;
                adb(
                    run,
                    id,
                    words(&[
                        "emu",
                        "sensor",
                        "set",
                        "acceleration",
                        ["0:9.81:0", "9.81:0:0", "0:-9.81:0", "-9.81:0:0"][rotation],
                    ]),
                    op,
                )
                .await?;
                return Ok(());
            }
            words(&[
                "cmd",
                "window",
                "user-rotation",
                "lock",
                &rotation.to_string(),
            ])
        }
        "setLocation" => {
            adb(
                run,
                id,
                vec![
                    "emu".into(),
                    "geo".into(),
                    "fix".into(),
                    value["longitude"].to_string(),
                    value["latitude"].to_string(),
                ],
                "location",
            )
            .await?;
            return Ok(());
        }
        "clearLocation" => return Ok(()),
        "setPermission" => {
            let permissions: &[&str] = match text("permission") {
                "camera" => &["CAMERA"],
                "microphone" => &["RECORD_AUDIO"],
                "photos" => &["READ_MEDIA_IMAGES", "READ_EXTERNAL_STORAGE"],
                "contacts" => &["READ_CONTACTS", "WRITE_CONTACTS"],
                "calendar" => &["READ_CALENDAR", "WRITE_CALENDAR"],
                "location" => &["ACCESS_FINE_LOCATION", "ACCESS_COARSE_LOCATION"],
                "notifications" => &["POST_NOTIFICATIONS"],
                "motion" => &["ACTIVITY_RECOGNITION"],
                _ => return Err(unsupported()),
            };
            let mut failure = None;
            let mut success = false;
            for permission in permissions {
                match shell(
                    run,
                    id,
                    words(&[
                        "pm",
                        if text("decision") == "grant" {
                            "grant"
                        } else {
                            "revoke"
                        },
                        app,
                        &format!("android.permission.{permission}"),
                    ]),
                    "permission",
                )
                .await
                {
                    Ok(_) => success = true,
                    Err(e) => {
                        if failure.is_none() {
                            failure = Some(e)
                        }
                    }
                }
            }
            if !success && let Some(e) = failure {
                return Err(e);
            }
            return Ok(());
        }
        "openUrl" => words(&[
            "am",
            "start",
            "-a",
            "android.intent.action.VIEW",
            "-d",
            text("url"),
        ]),
        "launchApp" => words(&[
            "monkey",
            "-p",
            app,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ]),
        "terminateApp" => words(&["am", "force-stop", app]),
        _ => return Err(unsupported()),
    };
    shell(run, id, args, op).await?;
    Ok(())
}

pub async fn read_detail(ready: &Ready, device: &DeviceSummary) -> Result<DeviceDetail> {
    let run = ready.commands.as_ref();
    let id = device.id.as_ref();
    let mut settings = serde_json::Map::new();
    let mut foreground = Value::Null;
    if device.platform == DevicePlatform::Ios {
        let ui = |key: &'static str| async move {
            checked(run, Command::new("xcrun", ["simctl", "ui", id, key]), key)
                .await
                .ok()
                .map(|s| s.trim().to_lowercase())
        };
        let (appearance, size, contrast) = tokio::join!(
            ui("appearance"),
            ui("content_size"),
            ui("increase_contrast")
        );
        if let Some(v) = appearance.filter(|v| ["light", "dark"].contains(&v.as_str())) {
            settings.insert("appearance".into(), json!(v));
        }
        if let Some(v) = size.filter(|s| !s.is_empty()) {
            let size = if v.starts_with("accessibility") {
                "extra-large"
            } else if ["extra-small", "small", "medium"].contains(&v.as_str()) {
                "small"
            } else if v.contains("extra") {
                "large"
            } else {
                "default"
            };
            settings.insert("textSize".into(), json!(size));
        }
        if let Some(v) = contrast.filter(|v| !v.is_empty()) {
            settings.insert("increaseContrast".into(), json!(v == "enabled"));
        }
        if let Some(helper) = &ready.ax_helper
            && let Ok(out) = checked(
                run,
                Command::new("xcrun", ["simctl", "spawn", id, helper, "status"]),
                "accessibility",
            )
            .await
            && let Ok(ax) = serde_json::from_str::<std::collections::BTreeMap<String, String>>(&out)
        {
            for (key, name) in [
                ("reduce-motion", "reduceMotion"),
                ("reduce-transparency", "reduceTransparency"),
                ("show-borders", "showBorders"),
                ("voiceover", "voiceOver"),
            ] {
                if let Some(v) = ax.get(key).filter(|v| ["on", "off"].contains(&v.as_str())) {
                    settings.insert(name.into(), json!(v == "on"));
                }
            }
            for (key, name, values) in [
                ("liquid-glass", "liquidGlass", &["clear", "tinted"][..]),
                (
                    "color-filter",
                    "colorFilter",
                    &["none", "grayscale", "red-green", "green-red", "blue-yellow"][..],
                ),
            ] {
                if let Some(v) = ax.get(key).filter(|v| values.contains(&v.as_str())) {
                    settings.insert(name.into(), json!(v));
                }
            }
        }
    } else {
        let read = |args: Vec<String>| async move {
            shell(run, id, args, "read settings")
                .await
                .ok()
                .map(|s| s.trim().to_owned())
        };
        let (night, scale, animator, wifi, focus) = tokio::join!(
            read(words(&["cmd", "uimode", "night"])),
            read(words(&["settings", "get", "system", "font_scale"])),
            read(words(&[
                "settings",
                "get",
                "global",
                "animator_duration_scale"
            ])),
            read(words(&["settings", "get", "global", "wifi_on"])),
            read(words(&["dumpsys", "window"]))
        );
        if let Some(night) = night {
            if night.contains("yes") {
                settings.insert("appearance".into(), json!("dark"));
            } else if night.contains("no") {
                settings.insert("appearance".into(), json!("light"));
            }
        }
        if let Some(scale) = scale
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|f| f.is_finite())
        {
            settings.insert(
                "textSize".into(),
                json!(if scale <= 0.9 {
                    "small"
                } else if scale >= 1.25 {
                    "extra-large"
                } else if scale >= 1.1 {
                    "large"
                } else {
                    "default"
                }),
            );
        }
        if let Some(v) = animator.filter(|v| v != "null") {
            settings.insert(
                "reduceMotion".into(),
                json!(v.parse::<f64>().ok() == Some(0.0)),
            );
        }
        if let Some(v) = wifi.filter(|v| ["0", "1"].contains(&v.as_str())) {
            settings.insert("networkEnabled".into(), json!(v == "1"));
        }
        if let Some(focus) = focus {
            let re = regex::Regex::new(r"m(?:CurrentFocus|FocusedApp)=\w+\{[^ ]+ u\d+ ([^/ ]+)/")
                .map_err(|_| crate::error::invalid("focus"))?;
            if let Some(c) = re.captures(&focus) {
                foreground = json!({"id":&c[1]});
            }
        }
    }
    serde_json::from_value(json!({"hostId":device.host_id,"deviceId":device.id,"settings":settings,"foregroundApp":foreground,"readAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true)})).map_err(|_|crate::error::invalid("device detail"))
}
