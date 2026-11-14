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

#[test]
fn synced_plugins_follow_the_account_generation_namespace_and_local_setting_rules() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let config = home.join("config");
    let workspace = home.join("workspace");
    let organization = "11111111-2222-4333-8444-555555555555";
    let account = "66666666-7777-4888-9999-aaaaaaaaaaaa";
    let bucket_name = format!("{organization}_{account}");
    let synced = config.join("plugins/synced");
    let bucket = synced.join(&bucket_name);
    for (directory, prefix, name) in [
        ("pstack", "pstack", "review"),
        ("pstack-copy", "PSTACK", "duplicate"),
        ("design~g2", "design", "critique"),
    ] {
        let plugin = bucket.join(directory);
        skill(&plugin.join("skills"), name, "# Synced");
        write(
            &plugin.join(".claude-plugin/plugin.json"),
            &json!({"name":prefix}).to_string(),
        );
    }
    write(&bucket.join("manifest.json"),&json!({"plugins":[{"pluginId":"one","name":"pstack"},{"pluginId":"duplicate","name":"pstack-copy"},{"pluginId":"two","name":"design","generation":2},{"name":"unlisted"}]}).to_string());
    write(&synced.join(format!(".bucket-{bucket_name}")), "");
    write(
        &config.join(".claude.json"),
        &json!({"oauthAccount":{"organizationUuid":organization,"accountUuid":account}})
            .to_string(),
    );
    let names = |cwd: Option<&Path>| {
        discover_at(home, cwd)
            .iter()
            .map(|skill| skill["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(None), ["design:critique", "pstack:review"]);
    let installed = config.join("plugins/installed");
    skill(&installed.join("skills"), "installed", "# Installed");
    write(
        &installed.join(".claude-plugin/plugin.json"),
        r#"{"name":"pstack"}"#,
    );
    write(
        &config.join("plugins/installed_plugins.json"),
        &json!({"plugins":{"pstack@shop":[{"scope":"user","installPath":installed}]}}).to_string(),
    );
    write(
        &config.join("settings.json"),
        r#"{"enabledPlugins":{"pstack@shop":true}}"#,
    );
    assert_eq!(names(None), ["design:critique", "pstack:installed"]);
    write(
        &config.join("settings.json"),
        r#"{"enabledPlugins":{"pstack@synced":false}}"#,
    );
    assert_eq!(names(None), ["design:critique"]);
    write(
        &workspace.join(".claude/settings.json"),
        r#"{"syncClaudeAiPlugins":false}"#,
    );
    assert_eq!(names(Some(&workspace)), ["design:critique"]);
    write(
        &workspace.join(".claude/settings.local.json"),
        r#"{"syncClaudeAiPlugins":false}"#,
    );
    assert!(names(Some(&workspace)).is_empty());
    write(&config.join(".claude.json"),&json!({"oauthAccount":{"organizationUuid":organization,"accountUuid":"77777777-8888-4999-aaaa-bbbbbbbbbbbb"}}).to_string());
    assert!(names(None).is_empty());
}
fn link_directory(target: &Path, link: &Path) {
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}
#[test]
fn project_plugins_share_only_verified_worktrees_and_manifest_paths_cannot_escape_through_links() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(temp.path()).unwrap();
    let canonical = canonical.to_string_lossy();
    let home = Path::new(canonical.strip_prefix("\\\\?\\").unwrap_or(&canonical));
    let config = home.join("config");
    let workspace = home.join("workspace");
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    let nested = workspace.join("packages/app");
    std::fs::create_dir_all(&nested).unwrap();
    let plugin = config.join("plugins/installed");
    skill(&plugin, "custom", "---\nname: custom.review\n---\n");
    let outside = home.join("outside");
    skill(&outside, "escaped", "# Outside");
    link_directory(&outside, &plugin.join("escape"));
    write(
        &plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"tools","skills":["./custom","./escape","../../../outside"]}"#,
    );
    write(&config.join("plugins/installed_plugins.json"),&json!({"plugins":{"tools@shop":[{"scope":"local","projectPath":workspace,"installPath":plugin}]}}).to_string());
    write(
        &config.join("settings.json"),
        r#"{"enabledPlugins":{"tools@shop":true}}"#,
    );
    let found = discover_at(home, Some(&nested));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["name"], "tools:custom-review");
    assert_eq!(found[0]["scope"], "project");
    let worktree = home.join("worktree");
    let gitdir = workspace.join(".git/worktrees/fixture");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", gitdir.display()),
    );
    write(&gitdir.join("commondir"), "../..\n");
    write(
        &gitdir.join("gitdir"),
        &worktree.join(".git").to_string_lossy(),
    );
    assert_eq!(discover_at(home, Some(&worktree)), found);
    let alias = home.join("alias");
    link_directory(&worktree, &alias);
    assert_eq!(discover_at(home, Some(&alias)), found);
    write(
        &gitdir.join("gitdir"),
        &home.join("forged/.git").to_string_lossy(),
    );
    assert!(discover_at(home, Some(&worktree)).is_empty());
}
