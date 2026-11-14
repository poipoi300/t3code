#![allow(clippy::unwrap_used)]
use std::path::Path;
use t3_provider::{cursor_skills::*, environment::Environment};
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
#[test]
fn cursor_skills_use_project_then_user_then_system_precedence_and_native_surface_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let project = directory.path().join("project");
    write(
        &project.join(".cursor/skills/same/SKILL.md"),
        "---\nname: Project Display\ndescription: Project wins\ndisable-model-invocation: YES\nuser-invocable: off\n---\n",
    );
    write(
        &home.join(".cursor/skills/same/SKILL.md"),
        "---\ndescription: User loses\n---\n",
    );
    write(
        &home.join(".cursor/skills-cursor/same/SKILL.md"),
        "# System loses",
    );
    write(
        &project.join(".agents/skills/nested/group/SKILL.md"),
        "# Nested skill",
    );
    write(
        &home.join(".claude/skills/hidden/SKILL.md"),
        "---\nmetadata:\n  surfaces: [desktop, web]\n---\n",
    );
    write(
        &home.join(".codex/skills/visible/SKILL.md"),
        "---\nmetadata:\n  surfaces: 'desktop, CLI'\n---\n",
    );
    write(
        &home.join(".cursor/skills/broken/SKILL.md"),
        "---\n[ invalid YAML\n---\n",
    );
    write(
        &home.join(".cursor/skills/node_modules/ignored/SKILL.md"),
        "# ignored",
    );
    let inspection = inspect(Some(&project), &home, &Environment::new());
    assert_eq!(inspection.failure, None);
    let values: Vec<_> = inspection
        .skills
        .into_iter()
        .map(|skill| serde_json::to_value(skill).unwrap())
        .collect();
    assert_eq!(
        values
            .iter()
            .map(|skill| skill["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["group", "same", "visible"]
    );
    let same = values.iter().find(|skill| skill["name"] == "same").unwrap();
    assert_eq!(same["scope"], "project");
    assert_eq!(same["description"], "Project wins");
    assert_eq!(same["displayName"], "Project Display");
    assert_eq!(same["userInvocationOnly"], true);
    assert_eq!(same["userInvocable"], false);
}
#[test]
fn symlink_cycles_and_external_trees_are_bounded_but_explicit_linked_skill_files_remain_visible() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let root = home.join(".cursor/skills");
    let external = directory.path().join("external");
    write(&external.join("SKILL.md"), "# Linked skill");
    write(
        &external.join("nested/SKILL.md"),
        "# Never traverse outside root",
    );
    std::fs::create_dir_all(&root).unwrap();
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(&external, root.join("linked")).unwrap();
        std::os::windows::fs::symlink_dir(&root, root.join("loop")).unwrap();
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&external, root.join("linked")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
    }
    let inspection = inspect(None, &home, &Environment::new());
    assert_eq!(inspection.failure, None);
    assert_eq!(inspection.skills.len(), 1);
    assert_eq!(inspection.skills[0].name.0, "linked");
}
