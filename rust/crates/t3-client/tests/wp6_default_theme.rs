#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use t3_client::{services::default_theme::*, testing::FakePlatform, types::EnvironmentId};
use t3_contracts::settings::{DefaultAppearancePreference as Appearance, ServerSettings};

#[test]
fn theme_mode_uses_independent_appearance_timestamps() {
    let mut settings = ServerSettings {
        default_theme_set_at: "2026-11-13T10:00:00.000Z".into(),
        ..Default::default()
    };
    for (appearance, time, expected) in [
        (Appearance::Unset, "", true),
        (Appearance::Dark, "2026-11-13T09:00:00.000Z", true),
        (Appearance::Dark, "2026-11-13T10:00:00.000Z", false),
        (Appearance::Light, "2026-11-13T11:00:00.000Z", false),
    ] {
        settings.default_appearance = appearance;
        settings.default_appearance_set_at = time.into();
        assert_eq!(theme_sets_own_mode(&settings), expected);
    }
}

#[test]
fn generations_apply_once_per_environment_and_failed_writes_remain_retryable() {
    futures::executor::block_on(async {
        let storage = FakePlatform::default();
        let environment = EnvironmentId("one".into());
        let mut settings = ServerSettings::default();
        settings.default_theme = serde_json::from_str("\"nightfall\"").unwrap();
        settings.default_theme_set_at = "2026-11-13T10:00:00.000Z".into();
        settings.default_appearance = Appearance::Dark;
        settings.default_appearance_set_at = settings.default_theme_set_at.clone();
        let themes = RefCell::new(Vec::new());
        let appearances = RefCell::new(Vec::new());
        let resolve = |_: &str| {
            Some(ResolvedTheme {
                single_appearance: Some(Appearance::Light),
            })
        };
        for _ in 0..2 {
            adopt_defaults(
                &storage,
                Some(&environment),
                &settings,
                resolve,
                |theme| {
                    themes.borrow_mut().push(theme.to_owned());
                    true
                },
                |mode| {
                    appearances.borrow_mut().push(mode);
                    true
                },
            )
            .await;
        }
        assert_eq!(*themes.borrow(), ["nightfall"]);
        assert_eq!(*appearances.borrow(), [Appearance::Dark]);
        let other = EnvironmentId("two".into());
        adopt_defaults(
            &storage,
            Some(&other),
            &settings,
            resolve,
            |_| true,
            |_| false,
        )
        .await;
        let called = RefCell::new(Vec::new());
        adopt_defaults(
            &storage,
            Some(&other),
            &settings,
            resolve,
            |_| false,
            |mode| {
                called.borrow_mut().push(mode);
                true
            },
        )
        .await;
        assert_eq!(*called.borrow(), [Appearance::Dark]);
        settings.default_appearance_set_at = "2026-11-14T00:00:00.000Z".into();
        settings.default_theme_set_at = settings.default_appearance_set_at.clone();
        adopt_defaults(
            &storage,
            Some(&environment),
            &settings,
            resolve,
            |theme| {
                themes.borrow_mut().push(theme.to_owned());
                true
            },
            |mode| {
                appearances.borrow_mut().push(mode);
                true
            },
        )
        .await;
        assert_eq!(themes.borrow().len(), 2);
        assert_eq!(appearances.borrow().len(), 2);
    });
}

#[test]
fn unresolved_theme_and_failed_single_appearance_wait_before_recording_adoption() {
    futures::executor::block_on(async {
        let storage = FakePlatform::default();
        let environment = EnvironmentId("late".into());
        let settings = ServerSettings {
            default_theme: serde_json::from_str("\"nightfall\"").unwrap(),
            ..ServerSettings::default()
        };
        let called = RefCell::new(Vec::new());
        adopt_defaults(
            &storage,
            Some(&environment),
            &settings,
            |_| None,
            |_| panic!("unresolved"),
            |_| panic!("unset"),
        )
        .await;
        for accept in [false, true, true] {
            adopt_defaults(
                &storage,
                Some(&environment),
                &settings,
                |_| {
                    Some(ResolvedTheme {
                        single_appearance: Some(Appearance::Light),
                    })
                },
                |_| {
                    called.borrow_mut().push("theme");
                    true
                },
                |_| {
                    called.borrow_mut().push("mode");
                    accept
                },
            )
            .await;
        }
        assert_eq!(*called.borrow(), ["theme", "mode", "theme", "mode"]);
        assert!(default_theme_to_apply(None, "nightfall", "", None, true).is_none());
        assert!(default_theme_to_apply(Some(&environment), "", "", None, true).is_none());
    });
}
