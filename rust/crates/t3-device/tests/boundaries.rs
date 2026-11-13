#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use http::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{device::*, settings_secret_value::SettingsSecretValue};
use t3_device::{
    Error, Result,
    actions::{read_detail, run_action},
    cloud::{
        LiteralSecrets, Secrets,
        expo::{ExpoApi, ExpoHttp, resolve_credential},
        simlock,
    },
    host::{DeviceOperations, OpenContext},
    hub::HubOperations,
    io::{CommandOutput, Http, HttpRequest, HttpResponse},
    local::{Machine, android_sdk},
    proxy::{Scope, policy},
};

#[tokio::test]
async fn simlock_identity_never_echoes_credentials_from_a_role_error() {
    use t3_device::service::Accounts;
    let home = tempfile::tempdir().unwrap();
    let accounts = t3_device::cloud::AccountService {
        settings: settings(false, false),
        secrets: Arc::new(LiteralSecrets),
        http: FakeHttp::new([(200, json!({"role":"worker-private-token","name":"test"}))]),
        environment: BTreeMap::new(),
        expo_home: home.path().into(),
    };
    let input = serde_json::from_value(
        json!({"gatewayUrl":"https://pool.test","agentToken":"private-token"}),
    )
    .unwrap();
    let error = accounts.simlock_identity(input).await.unwrap_err();
    assert!(!format!("{error:?}").contains("private-token"));
    assert!(error.to_string().contains("--role agent"));
}

#[tokio::test]
async fn location_refinements_are_checked_before_any_command() {
    for (latitude, longitude) in [
        (json!(90.1), json!(0)),
        (json!(0), json!(180.1)),
        (json!("NaN"), json!(0)),
        (json!(0), json!("Infinity")),
    ] {
        let commands = FakeCommands::encoder();
        let host = FakeHost::new(
            "local",
            FakeOps::new("local", DevicePlatform::Ios, true),
            commands.clone(),
            None,
        );
        let input=serde_json::from_value(json!({"type":"setLocation","deviceId":"phone","latitude":latitude,"longitude":longitude})).unwrap();
        assert!(
            run_action(&host.ready, DevicePlatform::Ios, &input)
                .await
                .is_err()
        );
        assert!(commands.calls.lock().unwrap().is_empty());
    }
}

fn hub(http: Arc<dyn Http>, commands: Arc<FakeCommands>) -> HubOperations {
    HubOperations {
        host_id: "local".into(),
        origin: "http://hub.test".into(),
        http,
        commands,
        platforms: vec![DevicePlatform::Ios, DevicePlatform::Android],
    }
}
fn raw_device(id: &str, platform: &str) -> Value {
    json!({"id":id,"name":"Pixel_API_35","platform":platform,"version":"Android 15","booted":true,"physical":false})
}

#[tokio::test]
async fn hub_discovery_merges_avds_without_duplicate_running_name() {
    let http = FakeHttp::new([(
        200,
        json!({"simulators":[],"emulators":[raw_device("emulator-5554","android")]}),
    )]);
    let commands = FakeCommands::new(|command| {
        assert_eq!(command.program, "emulator");
        assert_eq!(command.args, ["-list-avds"]);
        Ok(CommandOutput {
            code: 0,
            stdout: "Pixel_API_35\r\nPixel_API_36\n\n".into(),
            stderr: String::new(),
        })
    });
    let listing = hub(http.clone(), commands).list().await.unwrap();
    assert_eq!(
        listing
            .devices
            .iter()
            .map(|d| d.id.as_ref())
            .collect::<Vec<_>>(),
        ["emulator-5554", "Pixel_API_36"]
    );
    assert!(!listing.devices[1].booted);
    assert!(!listing.android_discovery_incomplete);
    assert_eq!(
        http.requests.lock().await[0].timeout,
        Some(Duration::from_secs(40))
    );
}
#[tokio::test]
async fn failed_avd_listing_keeps_discovered_devices_and_marks_incomplete() {
    let http = FakeHttp::new([(
        200,
        json!({"simulators":[],"emulators":[raw_device("emulator-5554","android")]}),
    )]);
    let commands = FakeCommands::new(|_| {
        Ok(CommandOutput {
            code: 1,
            stdout: "should-not-be-listed".into(),
            stderr: "private tool output".into(),
        })
    });
    let listing = hub(http, commands).list().await.unwrap();
    assert_eq!(listing.devices.len(), 1);
    assert!(listing.android_discovery_incomplete);
    assert!(listing.detail.unwrap().contains("exit code 1"));
}
#[tokio::test]
async fn simulator_discovery_warning_marks_incomplete_and_hides_only_deleted_plists() {
    let message = "[apple-utils] Failed to run `xcrun simctl list devices --json`:";
    let http = FakeHttp::new([(
        200,
        json!({"simulators":[],"emulators":[],"errors":[{"message":"[apple-utils] Failed to read CoreSimulator plist at /Users/test/Library/Developer/CoreSimulator/Devices/deleted/device.plist:","error":"Error: ENOENT: no such file or directory"},{"message":message,"error":"Error: Command failed\nUnable to connect to CoreSimulatorService\n    at privateStack\nstderr: credentials"}]}),
    )]);
    let listing = hub(http, FakeCommands::new(|_| Ok(CommandOutput::default())))
        .list()
        .await
        .unwrap();
    assert_eq!(listing.incomplete_platforms, [DevicePlatform::Ios]);
    let detail = listing.detail.unwrap();
    assert!(detail.contains("Unable to connect to CoreSimulatorService"));
    assert!(!detail.contains("ENOENT"));
    assert!(!detail.contains("privateStack"));
    assert!(!detail.contains("credentials"));
}
#[tokio::test]
async fn ios_shutdown_does_not_accept_missing_or_still_booted_device() {
    for state in [None, Some(true), Some(false)] {
        let devices = state
            .map(|booted| {
                let mut d = raw_device("phone", "ios");
                d["booted"] = json!(booted);
                vec![d]
            })
            .unwrap_or_default();
        let http = FakeHttp::new([
            (200, json!({"ok":false})),
            (200, json!({"simulators":devices,"emulators":[]})),
        ]);
        let result = hub(
            http.clone(),
            FakeCommands::new(|_| Ok(CommandOutput::default())),
        )
        .shutdown("phone", DevicePlatform::Ios)
        .await;
        assert_eq!(result.is_ok(), state == Some(false));
        assert!(
            http.requests.lock().await[0]
                .url
                .path()
                .ends_with("/vendor/serve-sim/api/shutdown")
        );
    }
}
#[tokio::test]
async fn ios_screenshot_fallback_is_limited_to_hub_rejection_on_old_runtimes() {
    for (version, status, expected, fallback) in [
        ("iOS 26.3", 400, true, true),
        ("iOS 27.0", 500, false, false),
        ("iOS 26.3", 200, true, false),
        ("unknown", 500, false, false),
    ] {
        let http = FakeHttp::new([(status, json!(null))]);
        let commands = FakeCommands::new(|command| {
            assert_eq!(command.program, "sh");
            assert_eq!(command.max_bytes, 64 * 1024 * 1024);
            assert_eq!(command.timeout, Duration::from_secs(20));
            assert_eq!(command.args.last().unwrap(), "phone");
            Ok(CommandOutput {
                stdout: "iVBORw==".into(),
                ..Default::default()
            })
        });
        let mut d = device("local", "phone", DevicePlatform::Ios, true);
        d.version = version.into();
        let result = hub(http, commands.clone()).screenshot(&d).await;
        assert_eq!(result.is_ok(), expected);
        assert_eq!(!commands.calls.lock().unwrap().is_empty(), fallback);
        if fallback {
            assert_eq!(result.unwrap(), [137, 80, 78, 71]);
        }
    }
}
#[tokio::test]
async fn hub_boot_classifies_diagnostics_and_uses_serial() {
    for (text, reason) in [
        ("insufficient disk space", DeviceBootErrorReason::DiskSpace),
        ("device timed out", DeviceBootErrorReason::Timeout),
        (
            "emulator launch failed",
            DeviceBootErrorReason::LaunchFailed,
        ),
    ] {
        let http = FakeHttp::new([(
            200,
            json!({"ok":false,"error":text,"errors":[{"message":"nested cause","error":"x".repeat(2400)}]}),
        )]);
        let error = hub(http, FakeCommands::encoder())
            .boot(
                &device("local", "phone", DevicePlatform::Android, false),
                &OpenContext::default(),
            )
            .await
            .unwrap_err();
        let Error::Device { wire, .. } = error else {
            panic!("wrong error")
        };
        let DeviceError::DeviceBootError(error) = *wire else {
            panic!("wrong wire error")
        };
        assert_eq!(error.reason, reason);
        let diagnostics = error.diagnostics.flatten().unwrap();
        assert_eq!(diagnostics.chars().count(), 2000);
        assert!(diagnostics.contains("nested cause"));
        assert!(diagnostics.ends_with('…'));
    }
    let http = FakeHttp::new([(200, json!({"ok":true,"serial":"emulator-5554"}))]);
    let id = hub(http, FakeCommands::encoder())
        .boot(
            &device("local", "avd", DevicePlatform::Android, false),
            &OpenContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(id, "emulator-5554");
}
struct HangingHttp;
#[async_trait]
impl Http for HangingHttp {
    async fn execute(&self, _: HttpRequest) -> Result<HttpResponse> {
        std::future::pending().await
    }
}
#[tokio::test(start_paused = true)]
async fn unresponsive_discovery_is_bounded_and_preserves_only_available_platforms() {
    let mut hub = hub(Arc::new(HangingHttp), FakeCommands::encoder());
    hub.platforms = vec![DevicePlatform::Ios];
    let listing = hub.list().await.unwrap();
    assert_eq!(listing.incomplete_platforms, [DevicePlatform::Ios]);
    assert_eq!(
        listing.detail.as_deref(),
        Some("The device hub did not answer within 40 seconds.")
    );
}
#[tokio::test(start_paused = true)]
async fn screenshot_and_close_cannot_hang() {
    let http: Arc<dyn Http> = Arc::new(HangingHttp);
    assert!(
        hub(http.clone(), FakeCommands::encoder())
            .screenshot(&device("local", "phone", DevicePlatform::Ios, true))
            .await
            .is_err()
    );
    let endpoint = t3_device::host::AgentEndpoint {
        base_url: "http://agent.test".into(),
        token: t3_device::io::Secret::new("secret"),
        entry_path: "/entry".into(),
        agent_device_id: None,
        session: None,
        state_dir: None,
        install_directory: None,
    };
    assert!(
        t3_device::agent::release_session(http.as_ref(), &endpoint, "session")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn agent_close_accepts_only_ok_or_confirmed_missing_session() {
    for (status, body, expected) in [
        (200, json!({"result":{"ok":true}}), true),
        (
            404,
            json!({"error":{"data":{"code":"SESSION_NOT_FOUND"}}}),
            true,
        ),
        (200, json!({"error":{"code":-32000}}), false),
        (503, json!({"result":{"ok":true}}), false),
        (404, json!({"error":{"data":{"code":"OTHER"}}}), false),
    ] {
        let http = FakeHttp::new([(status, body)]);
        let endpoint = t3_device::host::AgentEndpoint {
            base_url: "http://agent.test".into(),
            token: t3_device::io::Secret::new("secret"),
            entry_path: "/entry".into(),
            agent_device_id: None,
            session: None,
            state_dir: None,
            install_directory: None,
        };
        assert_eq!(
            t3_device::agent::release_session(http.as_ref(), &endpoint, "session")
                .await
                .is_ok(),
            expected
        );
        let requests = http.requests.lock().await;
        let request = &requests[0];
        assert_eq!(request.headers["authorization"].expose(), "Bearer secret");
        let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["params"]["session"], "session");
        assert_eq!(body["params"]["command"], "close");
        assert!(!format!("{endpoint:?}").contains("secret"));
    }
}
#[tokio::test]
async fn ios_and_android_detail_parse_recorded_outputs_and_fail_to_unknown() {
    let commands = FakeCommands::new(|c| {
        let args = c.args.join(" ");
        let out = if args.ends_with("ui phone appearance") {
            "dark\n"
        } else if args.ends_with("ui phone content_size") {
            "extra-small\n"
        } else if args.ends_with("ui phone increase_contrast") {
            "enabled\n"
        } else if args.contains("serve-sim-ax-settings status") {
            r#"{"reduce-motion":"on","reduce-transparency":"off","show-borders":"off","voiceover":"off","liquid-glass":"tinted","color-filter":"grayscale"}"#
        } else if args.ends_with("cmd uimode night") {
            "Night mode: yes\n"
        } else if args.ends_with("font_scale") {
            "0.85\n"
        } else if args.ends_with("animator_duration_scale") {
            "0\n"
        } else if args.ends_with("wifi_on") {
            "1\n"
        } else if args.ends_with("dumpsys window") {
            " mFocusedApp=ActivityRecord{155579877 u0 com.example.app/.MainActivity t15}\n mCurrentFocus=Window{1a2b u0 com.example.app/com.example.app.MainActivity}"
        } else {
            return Ok(CommandOutput {
                code: 1,
                ..Default::default()
            });
        };
        Ok(CommandOutput {
            stdout: out.into(),
            ..Default::default()
        })
    });
    let host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        commands,
        None,
    );
    let detail = read_detail(
        &host.ready,
        &device("local", "phone", DevicePlatform::Ios, true),
    )
    .await
    .unwrap();
    let v = serde_json::to_value(detail.settings).unwrap();
    assert_eq!(
        v,
        json!({"appearance":"dark","textSize":"small","increaseContrast":true,"reduceMotion":true,"reduceTransparency":false,"showBorders":false,"voiceOver":false,"liquidGlass":"tinted","colorFilter":"grayscale"})
    );
    let detail = read_detail(
        &host.ready,
        &device("local", "emulator-5554", DevicePlatform::Android, true),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(detail.settings).unwrap(),
        json!({"appearance":"dark","textSize":"small","reduceMotion":true,"networkEnabled":true})
    );
    assert_eq!(
        serde_json::to_value(detail.foreground_app).unwrap(),
        json!({"id":"com.example.app"})
    );
    let host = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::new(|_| {
            Ok(CommandOutput {
                code: 1,
                ..Default::default()
            })
        }),
        None,
    );
    assert_eq!(
        serde_json::to_value(
            read_detail(
                &host.ready,
                &device("local", "phone", DevicePlatform::Ios, true)
            )
            .await
            .unwrap()
            .settings
        )
        .unwrap(),
        json!({})
    );
}
#[tokio::test]
async fn android_rotation_uses_natural_size_last_override_and_accelerometer() {
    for (id, size, orientation, expected) in [
        (
            "emulator-5554",
            "Physical size: 2560x1600",
            "portrait",
            "-9.81:0:0",
        ),
        (
            "emulator-5554",
            "Physical size: 2560x1600",
            "landscape_left",
            "0:9.81:0",
        ),
        ("R5CT1234", "Physical size: 2560x1600", "portrait", "3"),
        (
            "R5CT1234",
            "Physical size: 1080x1920\nOverride size: 1920x1080",
            "portrait",
            "3",
        ),
    ] {
        let commands = FakeCommands::new(move |c| {
            Ok(CommandOutput {
                stdout: if c.args.ends_with(&["wm".into(), "size".into()]) {
                    size.into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
        });
        let host = FakeHost::new(
            "local",
            FakeOps::new("local", DevicePlatform::Android, true),
            commands.clone(),
            None,
        );
        let input = serde_json::from_value(
            json!({"type":"setOrientation","deviceId":id,"value":orientation}),
        )
        .unwrap();
        run_action(&host.ready, DevicePlatform::Android, &input)
            .await
            .unwrap();
        let calls = commands.calls.lock().unwrap();
        assert_eq!(calls.last().unwrap().args.last().unwrap(), expected);
        if id.starts_with("emulator-") {
            assert!(calls.last().unwrap().args.contains(&"acceleration".into()));
        } else {
            assert!(calls.last().unwrap().args.contains(&"lock".into()));
        }
    }
}
#[tokio::test]
async fn android_permissions_fail_only_when_none_applied() {
    for success in [
        None,
        Some("android.permission.READ_EXTERNAL_STORAGE"),
        Some("android.permission.READ_MEDIA_IMAGES"),
    ] {
        let commands = FakeCommands::new(move |c| {
            Ok(CommandOutput {
                code: if c.args.last().map(String::as_str) == success {
                    0
                } else {
                    1
                },
                ..Default::default()
            })
        });
        let host = FakeHost::new(
            "local",
            FakeOps::new("local", DevicePlatform::Android, true),
            commands.clone(),
            None,
        );
        let input=serde_json::from_value(json!({"type":"setPermission","deviceId":"emulator-5554","appId":"com.example.app","permission":"photos","decision":"grant"})).unwrap();
        assert_eq!(
            run_action(&host.ready, DevicePlatform::Android, &input)
                .await
                .is_ok(),
            success.is_some()
        );
        assert_eq!(commands.calls.lock().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn unsupported_actions_and_missing_helpers_execute_no_commands() {
    for (platform, input) in [
        (
            DevicePlatform::Ios,
            json!({"type":"shake","deviceId":"phone"}),
        ),
        (
            DevicePlatform::Ios,
            json!({"type":"setOrientation","deviceId":"phone","value":"portrait"}),
        ),
        (
            DevicePlatform::Android,
            json!({"type":"sendPush","deviceId":"phone","appId":"app","payload":"text"}),
        ),
        (
            DevicePlatform::Android,
            json!({"type":"setToggle","deviceId":"phone","setting":"voiceOver","value":true}),
        ),
    ] {
        let commands = FakeCommands::encoder();
        let host = FakeHost::new(
            "local",
            FakeOps::new("local", platform.clone(), true),
            commands.clone(),
            None,
        );
        assert!(
            run_action(
                &host.ready,
                platform,
                &serde_json::from_value(input).unwrap()
            )
            .await
            .is_err()
        );
        assert!(commands.calls.lock().unwrap().is_empty());
    }
}
#[test]
fn shell_word_preserves_hostile_android_values() {
    for raw in [
        "https://example.com/a?x=1&y=2",
        "https://example.com/it's?a=1&b=2",
        "a b",
        "$(touch /tmp/x)",
        "",
        "line\nnext",
    ] {
        let word = t3_device::actions::shell_word(raw);
        assert!(word.starts_with('\''));
        assert!(word.ends_with('\''));
        if raw.contains('\'') {
            assert!(word.contains("'\\''"));
        }
    }
    assert_eq!(
        t3_device::actions::shell_word("com.example.app"),
        "com.example.app"
    );
}

#[test]
fn proxy_admission_matrix_blocks_shell_and_unsupported_methods() {
    for (path, method, upgrade, expected) in [
        ("/api/devices", Method::GET, false, Ok(Scope::Read)),
        ("/api/devices", Method::POST, false, Err(405)),
        ("/api/devices/ws", Method::GET, true, Ok(Scope::Read)),
        (
            "/vendor/serve-sim/helper/ws",
            Method::GET,
            true,
            Ok(Scope::Operate),
        ),
        (
            "/vendor/serve-emu/ws",
            Method::GET,
            true,
            Ok(Scope::Operate),
        ),
        (
            "/vendor/serve-emu/api/fold",
            Method::GET,
            false,
            Ok(Scope::Read),
        ),
        (
            "/vendor/serve-emu/api/fold",
            Method::POST,
            false,
            Ok(Scope::Operate),
        ),
        (
            "/vendor/serve-emu/api/stream-mode",
            Method::POST,
            false,
            Ok(Scope::Operate),
        ),
        (
            "/vendor/serve-sim/helper/id/webrtc/offer",
            Method::POST,
            false,
            Ok(Scope::Read),
        ),
        (
            "/vendor/serve-emu/webrtc/offer",
            Method::POST,
            false,
            Ok(Scope::Read),
        ),
        (
            "/vendor/serve-emu/webrtc/offer",
            Method::GET,
            false,
            Err(405),
        ),
        ("/vendor/serve-sim/exec", Method::POST, false, Err(404)),
        (
            "/vendor/serve-sim/helper/duo/panel/1/webrtc/offer",
            Method::POST,
            false,
            Err(404),
        ),
        (
            "/vendor/serve-sim/helper/duo/panel/3/stream.avcc",
            Method::GET,
            false,
            Ok(Scope::Read),
        ),
        (
            "/vendor/serve-sim/helper/duo/panel/2/stream.avcc",
            Method::GET,
            false,
            Err(404),
        ),
    ] {
        assert_eq!(policy(path, &method, upgrade), expected, "{path}");
    }
}
#[test]
fn proxy_never_forwards_client_credentials_or_upstream_cookies() {
    let headers: http::HeaderMap = [
        ("authorization", "Bearer environment-token"),
        ("cookie", "private"),
        ("origin", "https://t3.test"),
        ("x-forwarded-proto", "https"),
        ("dpop", "proof"),
        ("content-type", "application/json"),
    ]
    .into_iter()
    .map(|(k, v)| {
        (
            http::HeaderName::from_static(k),
            http::HeaderValue::from_static(v),
        )
    })
    .collect();
    let out = t3_device::proxy::forward_headers(&headers, "http://hub.test");
    assert_eq!(out["origin"].expose(), "http://hub.test");
    assert_eq!(out["content-type"].expose(), "application/json");
    for key in ["authorization", "cookie", "dpop", "x-forwarded-proto"] {
        assert!(!out.contains_key(key));
    }
    let out = t3_device::proxy::response_headers(BTreeMap::from([
        ("set-cookie".into(), "private".into()),
        ("content-encoding".into(), "gzip".into()),
        ("content-type".into(), "video/h264".into()),
    ]));
    assert!(!out.contains_key("set-cookie"));
    assert!(!out.contains_key("content-encoding"));
    assert_eq!(out["cache-control"], "no-store, no-transform");
}

#[tokio::test]
async fn credentials_follow_saved_env_login_precedence_and_reject_unsafe_headers() {
    let home = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(home.path().join(".expo"))
        .await
        .unwrap();
    tokio::fs::write(
        home.path().join(".expo/state.json"),
        r#"{"auth":{"sessionSecret":"session-secret"}}"#,
    )
    .await
    .unwrap();
    for (saved, env, source, header, token) in [
        (
            " saved ",
            Some("env-token"),
            DeviceEasCredentialSource::Settings,
            "authorization",
            "Bearer saved",
        ),
        (
            "",
            Some("env-token"),
            DeviceEasCredentialSource::Env,
            "authorization",
            "Bearer env-token",
        ),
        (
            "",
            None,
            DeviceEasCredentialSource::EasLogin,
            "expo-session",
            "session-secret",
        ),
    ] {
        let c = resolve_credential(saved, env, home.path()).await.unwrap();
        assert_eq!(c.source, source);
        assert_eq!(c.header, header);
        assert_eq!(c.value.expose(), token);
        assert!(!format!("{c:?}").contains(token));
    }
    for token in ["abc def", "abc\ndef", "••••••", "abc\0def"] {
        let error = resolve_credential(token, Some("valid-env"), home.path())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains(token));
        assert!(error.to_string().contains("not a usable Expo credential"));
    }
}
#[tokio::test]
async fn expo_rejection_classification_does_not_free_uncertain_creates() {
    let home = tempfile::tempdir().unwrap();
    for (status, body, rejected) in [
        (
            200,
            json!({"errors":[{"message":"Feature not enabled"}]}),
            true,
        ),
        (401, json!({}), true),
        (
            200,
            json!({"data":null,"errors":[{"message":"not allowed","extensions":{"errorType":"USER","code":"INTERNAL_SERVER_ERROR"}}]}),
            true,
        ),
        (502, json!({"errors":[{"message":"Internal"}]}), false),
        (
            200,
            json!({"errors":[{"message":"Internal","extensions":{"errorCode":"INTERNAL_SERVER_ERROR"}}]}),
            false,
        ),
        (
            200,
            json!({"data":{},"errors":[{"message":"conflict"}]}),
            false,
        ),
        (
            200,
            json!({"errors":[{"message":"transient","extensions":{"isTransient":true,"errorType":"USER"}}]}),
            false,
        ),
    ] {
        let http = FakeHttp::new([(status, body)]);
        let credential = resolve_credential("private-token", None, home.path())
            .await
            .unwrap();
        let api = ExpoHttp::new(http.clone(), credential);
        let error = api.stop("session").await.unwrap_err();
        assert_eq!(error.rejected, rejected, "{status}");
        assert!(!format!("{error:?}").contains("private-token"));
        let requests = http.requests.lock().await;
        assert_eq!(requests[0].url.as_str(), "https://api.expo.dev/graphql");
        assert_eq!(requests[0].timeout, Some(Duration::from_secs(30)));
        assert_eq!(
            requests[0].headers["authorization"].expose(),
            "Bearer private-token"
        );
    }
}
#[tokio::test]
async fn expo_errors_scrub_credential_and_bound_gateway_text() {
    let home = tempfile::tempdir().unwrap();
    let http = FakeHttp::new([(
        200,
        json!({"errors":[{"message":format!("Bearer private-token private-token {}","x".repeat(500))}]}),
    )]);
    let api = ExpoHttp::new(
        http,
        resolve_credential("private-token", None, home.path())
            .await
            .unwrap(),
    );
    let error = api.stop("session").await.unwrap_err();
    assert!(error.detail.contains("[redacted]"));
    assert!(!error.detail.contains("private-token"));
    assert!(error.detail.len() <= 300);
}

#[tokio::test]
async fn expo_active_sessions_accept_nullable_names_without_losing_owned_sessions() {
    let home = tempfile::tempdir().unwrap();
    let response = json!({"data":{"app":{"byId":{"deviceRunSessionsPaginated":{"edges":[{"node":{"id":"unrelated","name":null,"status":"IN_PROGRESS"}},{"node":{"id":"owned","name":"T3 Code – Cloud – abc123","status":"NEW"}}]}}}}});
    let api = ExpoHttp::new(
        FakeHttp::new([(200, response)]),
        resolve_credential("private-token", None, home.path())
            .await
            .unwrap(),
    );
    let sessions = api.active("app").await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[1].id, "owned");
}

#[tokio::test]
async fn expo_refuses_malformed_session_shapes_and_unknown_statuses() {
    let home = tempfile::tempdir().unwrap();
    for session in [
        json!({"id":"s","status":"UNKNOWN","remoteConfig":null}),
        json!({"id":"s","status":"NEW"}),
        json!({"id":"s","status":"NEW","remoteConfig":{}}),
        json!({"id":"s","status":"IN_PROGRESS","remoteConfig":{"__typename":"AgentDeviceRunSessionRemoteConfig","agentDeviceRemoteSessionUrl":"https://agent.test","agentDeviceRemoteSessionToken":"secret","previewApiUrl":42,"webPreviewToken":null}}),
    ] {
        let api = ExpoHttp::new(FakeHttp::new([(200, json!({"data":{"deviceRunSessions":{"byId":session}}}))]), resolve_credential("private-token", None, home.path()).await.unwrap());
        let error = api.session("s").await.err().expect("schema refusal");
        assert!(!error.rejected);
        assert!(!format!("{error:?}").contains("secret"));
    }
}
#[tokio::test]
async fn simlock_tokens_are_origin_bound_and_unreadable_references_fail_closed() {
    let env = BTreeMap::from([
        (
            "AGENT_DEVICE_DAEMON_BASE_URL".into(),
            "https://pool.test:4700/v1/agent-device".into(),
        ),
        (
            "AGENT_DEVICE_DAEMON_AUTH_TOKEN".into(),
            "machine-secret".into(),
        ),
    ]);
    assert!(
        simlock::resolve_token(&LiteralSecrets, "https://elsewhere.test", None, &env)
            .await
            .unwrap()
            .is_none()
    );
    let resolved = simlock::resolve_token(&LiteralSecrets, "https://pool.test:4700/", None, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.0.expose(), "machine-secret");
    assert_eq!(resolved.1, DeviceSimlockTokenSource::Env);
    let saved:SimlockDeviceHostConfig=serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://pool.test:4700","agentToken":{"provider":"homelab","reference":"simlock#token"}})).unwrap();
    assert!(
        simlock::resolve_token(
            &LiteralSecrets,
            "https://pool.test:4700",
            Some(&saved),
            &env
        )
        .await
        .is_err()
    );
    assert!(
        LiteralSecrets
            .resolve(&SettingsSecretValue::Literal("saved".into()))
            .await
            .is_ok()
    );
}
#[test]
fn url_and_command_boundaries_refuse_option_injection_and_credential_urls() {
    for url in [
        "file:///etc/passwd",
        "ftp://pool.test",
        "http://user:password@pool.test",
        "https://pool.test/path",
        "https://pool.test/?token=secret",
        "https://pool.test/#fragment",
        "https://pool.test/\n",
    ] {
        assert!(simlock::gateway(url).is_err(), "{url}");
    }
    for id in ["-e", "", "id\0value", "id\rvalue", "id\nvalue"] {
        assert!(t3_device::error::validate_selector(id, "id").is_err());
    }
    assert!(t3_device::error::validate_selector("simlock:ios:iPhone 17 Pro", "id").is_ok());
}

struct FakeMachine {
    files: BTreeSet<PathBuf>,
    env: BTreeMap<String, String>,
    platform: &'static str,
}
use std::path::{Path, PathBuf};
#[async_trait]
impl Machine for FakeMachine {
    fn platform(&self) -> &str {
        self.platform
    }
    fn environment(&self) -> BTreeMap<String, String> {
        self.env.clone()
    }
    async fn exists(&self, p: &Path) -> bool {
        self.files.contains(p)
    }
    async fn real_path(&self, p: &Path) -> Option<PathBuf> {
        Some(p.into())
    }
}
#[tokio::test]
async fn sdk_discovery_uses_standard_windows_sdk_and_requires_modern_avdmanager() {
    let root = PathBuf::from("C:/Users/test/AppData/Local/Android/Sdk");
    let machine = FakeMachine {
        files: BTreeSet::from([
            root.join("platform-tools/adb.exe"),
            root.join("emulator/emulator.exe"),
            root.join("tools/bin/avdmanager.bat"),
        ]),
        env: BTreeMap::from([("USERPROFILE".into(), "C:/Users/test".into())]),
        platform: "win32",
    };
    let sdk = android_sdk(&machine).await;
    assert_eq!(sdk.root, Some(root));
    assert!(sdk.adb && sdk.emulator && sdk.legacy_avdmanager);
    assert!(!sdk.avdmanager);
    let reason = t3_device::local::platform_reason(
        &machine,
        FakeCommands::encoder().as_ref(),
        DevicePlatform::Android,
    )
    .await
    .unwrap();
    assert!(reason.contains("older, unsupported version"));
}

#[tokio::test]
async fn agent_config_files_are_stable_per_host_device_and_unchanged_peers() {
    let temp = tempfile::tempdir().unwrap();
    let mut endpoint = t3_device::host::AgentEndpoint {
        base_url: "http://127.0.0.1:1".into(),
        token: t3_device::io::Secret::new("one"),
        entry_path: "/entry".into(),
        agent_device_id: None,
        session: None,
        state_dir: None,
        install_directory: None,
    };
    let a = t3_device::agent::config_path(temp.path(), "a", None);
    let b = t3_device::agent::config_path(temp.path(), "b", None);
    t3_device::agent::write_config(&a, &endpoint).await.unwrap();
    t3_device::agent::write_config(&b, &endpoint).await.unwrap();
    let peer = tokio::fs::read(&b).await.unwrap();
    endpoint.token = t3_device::io::Secret::new("two");
    t3_device::agent::write_config(&a, &endpoint).await.unwrap();
    assert_eq!(tokio::fs::read(&b).await.unwrap(), peer);
    let updated: Value = serde_json::from_slice(&tokio::fs::read(&a).await.unwrap()).unwrap();
    assert_eq!(updated["daemonAuthToken"], "two");
    assert_ne!(
        a,
        t3_device::agent::config_path(temp.path(), "a", Some("d"))
    );
    assert_ne!(
        t3_device::agent::session("t", "a", "same"),
        t3_device::agent::session("t", "b", "same")
    );
}
