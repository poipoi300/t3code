//! Port of apps/web/src/state/server.ts createShortcutKeybindingsAtom.

use super::keybindings::merge_with_defaults;
use crate::types::EnvironmentId;
use t3_contracts::keybindings::ResolvedKeybindingRule;

pub struct ShortcutSource<'a> {
    pub environment: &'a EnvironmentId,
    pub connected: bool,
    pub keybindings: Option<&'a [ResolvedKeybindingRule]>,
}

/// Sources retain catalogue order. A designated primary remains authoritative while loading.
pub fn effective_config(
    primary: Option<&EnvironmentId>,
    sources: &[ShortcutSource<'_>],
) -> Vec<ResolvedKeybindingRule> {
    let source = match primary {
        Some(primary) => sources.iter().find(|source| source.environment == primary),
        None => sources
            .iter()
            .find(|source| source.connected && source.keybindings.is_some()),
    };
    merge_with_defaults(source.and_then(|source| source.keybindings).unwrap_or(&[]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_logic::shell::keybindings::{
        KeyEvent, MatchContext, Modifiers, compile_config, resolve_shortcut_command,
    };

    fn rules(command: &str) -> Vec<ResolvedKeybindingRule> {
        compile_config(&[serde_json::from_value(serde_json::json!({
            "key": "mod+n", "command": command,
        }))
        .expect("TS keybinding rule")])
    }

    fn command(primary: Option<&EnvironmentId>, sources: &[ShortcutSource<'_>]) -> String {
        let rules = effective_config(primary, sources);
        resolve_shortcut_command(
            &KeyEvent {
                key: "n".into(),
                modifiers: Modifiers {
                    meta: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            &rules,
            "MacIntel",
            &MatchContext::default(),
        )
        .expect("Cmd+N")
        .0
        .clone()
    }

    #[test]
    fn follows_the_first_connected_config_through_load_edit_disconnect_and_reconnect() {
        let a = EnvironmentId("a".into());
        let b = EnvironmentId("b".into());
        let stale = rules("chat.newWithoutProject");
        let swapped = rules("chat.newLocal");
        let edited = rules("sidebar.toggle");
        assert_eq!(command(None, &[]), "chat.new");
        for (connected_a, rules_a, connected_b, expected) in [
            (false, Some(stale.as_slice()), true, "chat.newLocal"),
            (true, None, true, "chat.newLocal"),
            (true, Some(stale.as_slice()), true, "chat.newWithoutProject"),
            (true, Some(edited.as_slice()), true, "sidebar.toggle"),
            (false, Some(edited.as_slice()), true, "chat.newLocal"),
            (false, Some(edited.as_slice()), false, "chat.new"),
            (false, Some(stale.as_slice()), false, "chat.new"),
            (
                true,
                Some(stale.as_slice()),
                false,
                "chat.newWithoutProject",
            ),
        ] {
            assert_eq!(
                command(
                    None,
                    &[
                        ShortcutSource {
                            environment: &a,
                            connected: connected_a,
                            keybindings: rules_a
                        },
                        ShortcutSource {
                            environment: &b,
                            connected: connected_b,
                            keybindings: Some(&swapped)
                        },
                    ]
                ),
                expected
            );
        }
    }

    #[test]
    fn uses_primary_config_or_defaults_until_primary_is_removed() {
        let primary = EnvironmentId("primary".into());
        let b = EnvironmentId("b".into());
        let stale = rules("chat.newWithoutProject");
        let swapped = rules("chat.newLocal");
        let sources = |config| {
            [
                ShortcutSource {
                    environment: &primary,
                    connected: false,
                    keybindings: config,
                },
                ShortcutSource {
                    environment: &b,
                    connected: true,
                    keybindings: Some(swapped.as_slice()),
                },
            ]
        };
        assert_eq!(command(None, &sources(None)), "chat.newLocal");
        assert_eq!(command(Some(&primary), &sources(None)), "chat.new");
        assert_eq!(
            command(Some(&primary), &sources(Some(&stale))),
            "chat.newWithoutProject"
        );
        assert_eq!(command(Some(&primary), &sources(None)), "chat.new");
        assert_eq!(command(None, &sources(None)), "chat.newLocal");
        assert_eq!(
            command(Some(&EnvironmentId("missing".into())), &sources(None)),
            "chat.new"
        );
    }
}
