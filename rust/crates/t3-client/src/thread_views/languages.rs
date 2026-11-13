pub const LANGUAGES: &[(&str, &str)] = &[
    ("", "Plain text"),
    ("asm", "Assembly"),
    ("bash", "Bash"),
    ("c", "C"),
    ("cpp", "C++"),
    ("csharp", "C#"),
    ("css", "CSS"),
    ("dart", "Dart"),
    ("diff", "Diff"),
    ("dockerfile", "Dockerfile"),
    ("elixir", "Elixir"),
    ("go", "Go"),
    ("graphql", "GraphQL"),
    ("haskell", "Haskell"),
    ("html", "HTML"),
    ("java", "Java"),
    ("javascript", "JavaScript"),
    ("json", "JSON"),
    ("jsx", "JSX"),
    ("kotlin", "Kotlin"),
    ("lua", "Lua"),
    ("markdown", "Markdown"),
    ("php", "PHP"),
    ("powershell", "PowerShell"),
    ("python", "Python"),
    ("r", "R"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("scala", "Scala"),
    ("scss", "SCSS"),
    ("sql", "SQL"),
    ("svelte", "Svelte"),
    ("swift", "Swift"),
    ("toml", "TOML"),
    ("tsx", "TSX"),
    ("typescript", "TypeScript"),
    ("vue", "Vue"),
    ("xml", "XML"),
    ("yaml", "YAML"),
    ("zig", "Zig"),
];
fn space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}
pub fn language_of_info_string(info: &str) -> &str {
    info.trim_matches(space)
        .split(space)
        .next()
        .unwrap_or_default()
}
pub fn with_info_string_language(info: &str, language: &str) -> String {
    let leading_end = info
        .char_indices()
        .take_while(|(_, c)| space(*c))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let leading = &info[..leading_end];
    let rest = &info[leading_end + language_of_info_string(info).len()..];
    if !language.is_empty() {
        format!("{leading}{language}{rest}")
    } else if !rest.trim_matches(space).is_empty() {
        format!("{leading}text{rest}")
    } else {
        String::new()
    }
}
pub fn language_entry(language: &str) -> Option<(&'static str, &'static str)> {
    let lower = language.to_lowercase();
    let id = match lower.as_str() {
        "assembly" | "s" => "asm",
        "cs" => "csharp",
        "c++" => "cpp",
        "js" => "javascript",
        "md" => "markdown",
        "plaintext" | "text" | "txt" => "",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "sh" | "shell" => "bash",
        "ts" => "typescript",
        "yml" => "yaml",
        _ => &lower,
    };
    LANGUAGES
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .copied()
}
pub fn language_label(language: &str) -> &str {
    language_entry(language).map_or(language, |(_, label)| label)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn plaintext_metadata() {
        for language in ["Text", "TEXT", "plaintext", "txt"] {
            assert_eq!(
                language_entry(language_of_info_string(&format!(
                    " {language} title=notes.txt"
                ))),
                Some(("", "Plain text"))
            );
        }
    }
    #[test]
    fn aliases_unknown_and_metadata_replacement() {
        assert_eq!(language_entry("RS"), Some(("rust", "Rust")));
        assert_eq!(language_label("custom"), "custom");
        assert_eq!(
            with_info_string_language("  ts title=x", "py"),
            "  py title=x"
        );
        assert_eq!(with_info_string_language("ts title=x", ""), "text title=x");
        assert_eq!(with_info_string_language(" ts  ", ""), "");
        assert_eq!(with_info_string_language("  ", "rust"), "  rust");
    }
}
