#![allow(clippy::unwrap_used)]
mod common;
use serde_json::json;
use std::{path::Path, sync::Arc};
use t3_provider::{
    catalog::Manifest,
    environment::{Environment, Platform},
    fx_instance::FxInstance,
    fx_skills::*,
    instances::InstanceSpec,
    native_provider::NativeProviderContext,
    service::ProviderInstanceService,
    transport::SystemClock,
};
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn skill(root: &Path, folder: &str, name: &str) {
    write(
        &root.join(folder).join("SKILL.md"),
        format!("---\nname: {name}\n---\n").as_bytes(),
    );
}
fn link(target: &Path, path: &Path) {
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, path).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, path).unwrap();
}
#[test]
fn byte_frontmatter_preserves_boms_and_quoted_values_and_rejects_invalid_names_continuations_and_utf8()
 {
    for (bytes, expected) in [
        (b"No frontmatter".as_slice(), Some("folder")),
        (
            "\u{feff}---\nname: bom-name\n---\n".as_bytes(),
            Some("folder"),
        ),
        (b"---\nname: 'with spaces'\n---\n", Some("with spaces")),
        (b"---\nname: review\x0b\n---\n", None),
        (b"---\ndescription: No name\n---\n", None),
        (b"---\nname: one\nname: two\n---\n", None),
        (
            b"---\nname: one\ndescription: text\n continued\n---\n",
            None,
        ),
        (b"---\nname: bad\xff\n---\n", None),
        (b"---\nname: unterminated", None),
    ] {
        assert_eq!(
            metadata(bytes, "folder").map(|(name, _)| name).as_deref(),
            expected
        );
    }
    let literal = metadata(
        b"---\nname: review\ndescription: |\n  first\n  second\n---\n",
        "folder",
    )
    .unwrap();
    assert_eq!(literal.1.as_deref(), Some("first\nsecond"));
    let folded = metadata(
        b"---\nname: review\ndescription: >-\n  first\n  second\n\n  third\n---\n",
        "folder",
    )
    .unwrap();
    assert_eq!(folded.1.as_deref(), Some("first second\n\nthird"));
    assert!(
        metadata(
            b"---\nname: review\ndescription: >\n  first\n second\n---\n",
            "folder"
        )
        .is_none()
    );
    assert!(
        metadata(
            format!("---\nname: {}\n---\n", "a".repeat(257)).as_bytes(),
            "folder"
        )
        .is_none()
    );
}
#[tokio::test]
async fn roots_follow_fx_precedence_and_link_rules_and_workspace_refresh_does_not_reprobe_the_cli()
{
    let fixture = common::Fixture::new(
        json!({"version":"fx 0.0.13","status":{"auth":"oauth","model":"model"},"models":{"ids":["model"]}}),
    );
    let home = fixture.directory.path();
    let workspace = home.join("app/nested");
    std::fs::create_dir_all(&workspace).unwrap();
    skill(&home.join(".fx/skills"), "managed", "managed");
    skill(&workspace.join(".fx/skills"), "review", "review");
    skill(&home.join("app/skills"), "shared", "shared");
    skill(&home.join(".claude/skills"), "case", "Review");
    skill(&home.join(".agents/skills"), "duplicate", "review");
    skill(&home.join("library"), "linked", "linked");
    link(
        &home.join("library/linked"),
        &home.join(".fx/skills/managed-link"),
    );
    std::fs::create_dir_all(workspace.join(".agents/skills")).unwrap();
    link(
        &home.join("library/linked"),
        &workspace.join(".agents/skills/link"),
    );
    let environment = Environment::from([("HOME".into(), home.to_string_lossy().into_owned())]);
    let discovered = discover(&workspace, &environment);
    assert_eq!(
        discovered
            .iter()
            .map(|skill| skill.name.0.as_str())
            .collect::<Vec<_>>(),
        ["managed", "review", "shared", "linked", "Review"]
    );
    assert!(discover(&workspace, &Environment::new()).is_empty());
    let source = FxInstance::new(
        Arc::new(InstanceSpec {
            instance_id: "fx-work".into(),
            driver: "fx".into(),
            display_name: Some("Work".into()),
            accent_color: None,
            enabled: true,
            config: json!({"binaryPath":fixture.binary()}),
            variables: serde_json::from_value(
                json!([{"name":"HOME","value":home,"sensitive":false}]),
            )
            .unwrap(),
        }),
        NativeProviderContext {
            environment: fixture.environment.clone(),
            home: home.into(),
            cwd: workspace.clone(),
            platform: Platform::current(),
            managed_mcp: false,
            server_version: "0.1.0".into(),
        },
        Arc::new(SystemClock),
        Arc::new(Manifest::bundled().unwrap()).into(),
    )
    .unwrap();
    assert!(fixture.calls().is_empty());
    source.snapshot(None, false).await.unwrap();
    let calls = fixture.calls();
    let snapshot = source
        .snapshot(Some(workspace.to_str().unwrap()), false)
        .await
        .unwrap();
    assert_eq!(fixture.calls(), calls);
    assert_eq!(
        snapshot.workspace_snapshots.unwrap()[0].skills.len(),
        discovered.len()
    );
    assert!(source.maintenance(false).await.unwrap().update.is_none());
}
