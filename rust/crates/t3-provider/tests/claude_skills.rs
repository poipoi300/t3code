#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::path::Path;
use t3_provider::{
    claude_skills::*,
    environment::{Environment, Platform},
};
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn skill(root: &Path, name: &str, text: &str) {
    write(&root.join(name).join("SKILL.md"), text);
}
fn discover_at(home: &Path, cwd: Option<&Path>) -> Vec<serde_json::Value> {
    discover(ClaudeSkillContext {
        config: &home.join("config"),
        custom_home: true,
        cwd,
        home,
        environment: &Environment::new(),
        platform: Platform::Windows,
        launch_args: "",
    })
    .into_iter()
    .map(|s| serde_json::to_value(s).unwrap())
    .collect()
}
#[test]
fn native_precedence_yaml_booleans_lenient_settings_and_invalid_override_maps_match_the_cli() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let config = home.join("config");
    let cwd = home.join("workspace");
    skill(
        &config.join("skills"),
        "same",
        "---\ndescription: User: wins # comment\ndisable-model-invocation: YES\nuser-invocable: off\n---\n",
    );
    skill(
        &cwd.join(".claude/skills"),
        "same",
        "---\ndescription: Project loses\n---\n",
    );
    skill(&cwd.join(".claude/skills"), "project", "# Project");
    skill(
        &config.join("skills"),
        "malformed",
        "---\n[ invalid YAML\n---\n",
    );
    write(
        &config.join("settings.json"),
        r#"{"skillOverrides":{"same":"off"}, // allowed comment
    "enabledPlugins":{},}"#,
    );
    let skills = discover_at(home, Some(&cwd));
    assert_eq!(skills.len(), 2);
    let same = skills.iter().find(|s| s["name"] == "same").unwrap();
    assert_eq!(same["description"], "User: wins");
    assert_eq!(same["enabled"], false);
    assert_eq!(same["userInvocationOnly"], true);
    assert_eq!(same["userInvocable"], false);
    assert_eq!(same["scope"], "user");
    write(
        &config.join("settings.json"),
        r#"{"skillOverrides":{"same":"off","invalid":true}}"#,
    );
    assert_eq!(
        discover_at(home, Some(&cwd))
            .iter()
            .find(|s| s["name"] == "same")
            .unwrap()["enabled"],
        true
    );
    assert!(parse_lenient_json(r#"{"url":"https://host/a//b",/* c */"array":[1,],}"#).is_some());
    assert!(parse_lenient_json("{unquoted:1}").is_none());
}
#[test]
fn enabled_plugins_use_manifest_names_single_skill_aliases_and_ignore_native_overrides() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let config = home.join("config");
    let plugin = config.join("plugins/cache/plugin");
    let disabled = config.join("plugins/disabled");
    skill(&plugin.join("skills"), "plain", "# Plain");
    skill(
        &plugin.join("engineering"),
        "review",
        "---\nname: review-alias\ndescription: Review changes.\ndisable-model-invocation: true\nuser-invocable: false\n---\n",
    );
    skill(&disabled.join("skills"), "hidden", "# Hidden");
    write(
        &plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"manifest-plugin","skills":["./engineering/review","../disabled/skills"]}"#,
    );
    write(&config.join("plugins/installed_plugins.json"),&json!({"plugins":{"plugin@shop":[{"scope":"user","installPath":plugin}],"disabled@shop":[{"scope":"user","installPath":disabled}]}}).to_string());
    write(
        &config.join("settings.json"),
        r#"{"enabledPlugins":{"plugin@shop":true,"disabled@shop":false},"skillOverrides":{"manifest-plugin:plain":"off"}}"#,
    );
    let skills = discover_at(home, None);
    assert_eq!(
        skills
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["manifest-plugin:plain", "manifest-plugin:review-alias"]
    );
    assert!(skills.iter().all(|s| s["enabled"] == true));
    assert_eq!(skills[1]["userInvocationOnly"], true);
}
#[test]
fn launch_directories_are_variadic_and_stop_at_flags_and_the_end_marker() {
    assert_eq!(
        launch_directories("--add-dir a b --model x --add-dir=c --add-dir d -- --add-dir e"),
        ["a", "b", "c", "d"]
    );
}
