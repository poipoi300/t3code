//! Lucide saved-name compatibility; glyph geometry is transcribed from the installed package.

use t3_ui::ui::icon::Icon;

#[test]
fn saved_names_follow_the_web_export_resolver_including_deprecated_aliases_and_digit_names() {
    for (name, expected) in [
        ("folder-code", Some(Icon::FolderCode)),
        ("FolderCode", Some(Icon::FolderCode)),
        ("code-2", Icon::from_name("code-xml")),
        ("globe-2", Icon::from_name("earth")),
        ("alarm-check", Icon::from_name("alarm-clock-check")),
        ("axis-3-d", Icon::from_name("axis-3d")),
        ("leaf-icon", Some(Icon::Leaf)),
        ("lucide-leaf", Some(Icon::Leaf)),
        ("not-an-icon", None),
        ("icons", None),
        ("create-lucide-icon", None),
        (" folder-code", None),
        ("folder_code", None),
        ("", None),
    ] {
        assert_eq!(Icon::from_name(name), expected, "{name}");
    }
}
