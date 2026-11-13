use super::super::{Registry, register};
use t3_contracts::keybindings::*;

pub fn register_module(registry: &mut Registry) {
    register::<KeybindingCommand>(registry, "KeybindingCommand");
    register::<KeybindingRule>(registry, "KeybindingRule");
    register::<KeybindingShortcut>(registry, "KeybindingShortcut");
    register::<KeybindingValue>(registry, "KeybindingValue");
    register::<KeybindingWhen>(registry, "KeybindingWhen");
    register::<KeybindingWhenNode>(registry, "KeybindingWhenNode");
    register::<KeybindingsConfig>(registry, "KeybindingsConfig");
    register::<KeybindingsConfigError>(registry, "KeybindingsConfigError");
    register::<PreviewForwardedShortcut>(registry, "PreviewForwardedShortcut");
    register::<ResolvedKeybindingRule>(registry, "ResolvedKeybindingRule");
    register::<ResolvedKeybindingsConfig>(registry, "ResolvedKeybindingsConfig");
    register::<ScriptRunCommandPattern>(registry, "SCRIPT_RUN_COMMAND_PATTERN");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("keybindings", register_module)
}

#[test]
fn resolved_rules_drop_invalid_members_and_preserve_recursive_conditions()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    let rule = json!({
        "command": "preview.toggle",
        "shortcut": { "key": " ", "metaKey": false, "ctrlKey": false, "shiftKey": false, "altKey": false, "modKey": true },
        "whenAst": { "type": "and", "left": { "type": "identifier", "name": "previewFocus" }, "right": { "type": "not", "node": { "type": "identifier", "name": "inputFocus" } } }
    });
    let decoded: ResolvedKeybindingsConfig = serde_json::from_value(json!([
        rule, { "command": "future.command" }, null, { "command": "preview.toggle", "shortcut": false }
    ]))?;
    assert_eq!(serde_json::to_value(decoded)?, json!([rule]));
    assert!(serde_json::from_value::<KeybindingCommand>(json!("script.-bad.run")).is_err());
    assert!(serde_json::from_value::<KeybindingCommand>(json!("script.valid-id.run")).is_ok());
    Ok(())
}
