//! Port of packages/shared/src/changedFileKind.ts, written without regular expressions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedFileKind {
    Generated,
    Mocks,
    Tests,
    Docs,
    Code,
}

const GENERATED_FILES: [&str; 20] = [
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "deno.lock",
    "flake.lock",
    "gemfile.lock",
    "go.sum",
    "gradle.lockfile",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "packages.lock.json",
    "pipfile.lock",
    "pnpm-lock.yaml",
    "podfile.lock",
    "poetry.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
];
const GENERATED_DIRS: [&str; 3] = ["__generated__", "_generated", "generated"];
const MOCK_DIRS: [&str; 10] = [
    "__fixtures__",
    "__mocks__",
    "__snapshots__",
    "fakes",
    "fixture",
    "fixtures",
    "mock",
    "mocks",
    "stubs",
    "testdata",
];
const TEST_DIRS: [&str; 9] = [
    "__tests__",
    "__test__",
    "cypress",
    "e2e",
    "evals",
    "spec",
    "specs",
    "test",
    "tests",
];
const MOCK_WORDS: [&str; 8] = [
    "mocks", "mock", "fixtures", "fixture", "stubs", "stub", "fakes", "fake",
];

/// `\.(words)\.[^.]+$`: the segment before a dot-free, non-empty extension is one of `words`.
fn infix_before_extension(name: &str, words: &[&str]) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if extension.is_empty() {
        return false;
    }
    stem.rsplit_once('.')
        .is_some_and(|(_, infix)| words.contains(&infix))
}

fn generated_name(name: &str) -> bool {
    name.contains(".generated.")
        || name.contains(".gen.")
        || [
            ".g.dart",
            ".pb.go",
            "_pb2.py",
            "_pb2_grpc.py",
            ".pb.cc",
            ".pb.h",
            ".min.js",
            ".min.css",
            ".js.map",
            ".css.map",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

fn mock_name(name: &str) -> bool {
    name.ends_with(".snap")
        || infix_before_extension(name, &MOCK_WORDS)
        || MOCK_WORDS.iter().any(|word| {
            name.strip_prefix(word).is_some_and(|rest| {
                rest.chars()
                    .next()
                    .is_none_or(|c| matches!(c, '-' | '_' | '.') || c.is_ascii_uppercase())
            })
        })
}

fn test_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let underscore_suffix = lower.rsplit_once('.').is_some_and(|(stem, extension)| {
        !extension.is_empty() && (stem.ends_with("_test") || stem.ends_with("_spec"))
    });
    let setup = {
        let parts: Vec<_> = lower.split('.').collect();
        parts.len() == 3
            && matches!(parts[0], "vitest" | "jest")
            && parts[1] == "setup"
            && !parts[2].is_empty()
    };
    infix_before_extension(&lower, &["test", "spec", "e2e", "eval"])
        || underscore_suffix
        || (lower.starts_with("test_") && lower.ends_with(".py") && lower.len() >= 9)
        || lower == "conftest.py"
        || setup
}

// Case-sensitive: `FooTest.java` is a test class, `Latest.java` is not.
fn class_test_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        ["Test", "Tests", "Spec"]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
            && matches!(
                extension,
                "java" | "kt" | "kts" | "scala" | "groovy" | "cs" | "php" | "swift" | "m"
            )
    })
}

fn docs_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some((stem, extension)) = lower.rsplit_once('.') else {
        return false;
    };
    let docs = matches!(
        extension,
        "md" | "mdx" | "markdown" | "rst" | "adoc" | "txt"
    );
    let not_docs = extension == "txt"
        && (stem.starts_with("requirements")
            || stem.starts_with("constraints")
            || matches!(stem, "cmakelists" | "robots" | "llms"));
    docs && !not_docs
}

pub fn classify_changed_file(path: &str) -> ChangedFileKind {
    let path = path.replace('\\', "/");
    let mut segments: Vec<&str> = path.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    let dirs: Vec<String> = segments.iter().map(|s| s.to_lowercase()).collect();
    let in_dir = |set: &[&str]| dirs.iter().any(|dir| set.contains(&dir.as_str()));
    if GENERATED_FILES.contains(&name.to_lowercase().as_str())
        || generated_name(name)
        || in_dir(&GENERATED_DIRS)
    {
        return ChangedFileKind::Generated;
    }
    if mock_name(name) || in_dir(&MOCK_DIRS) {
        return ChangedFileKind::Mocks;
    }
    if test_name(name) || class_test_name(name) || in_dir(&TEST_DIRS) {
        return ChangedFileKind::Tests;
    }
    if docs_name(name) {
        return ChangedFileKind::Docs;
    }
    ChangedFileKind::Code
}

pub fn is_test_changed_file(path: &str) -> bool {
    matches!(
        classify_changed_file(path),
        ChangedFileKind::Tests | ChangedFileKind::Mocks
    )
}

#[cfg(test)]
mod tests {
    use super::{ChangedFileKind::*, *};

    #[test]
    fn classifies_the_source_cases() {
        for (path, kind) in [
            ("src/contest.py", Code),
            ("test/fixtures/pr.json", Mocks),
            ("pnpm-lock.yaml", Generated),
            ("convex/_generated/api.d.ts", Generated),
            ("src/agent.eval.ts", Tests),
            ("evals/title-generation.ts", Tests),
            ("README.md", Docs),
            ("tests/README.md", Tests),
        ] {
            assert_eq!(classify_changed_file(path), kind, "{path}");
        }
    }

    #[test]
    fn hand_written_patterns_follow_the_source_regular_expressions() {
        for (path, kind) in [
            ("src/FooTest.java", Tests),
            ("src/Latest.java", Code),
            ("src/footest.java", Code),
            ("src/Button.test.tsx", Tests),
            ("src/BUTTON.SPEC.TS", Tests),
            ("pkg/server_test.go", Tests),
            ("test_api.py", Tests),
            ("test_.py", Code),
            ("conftest.py", Tests),
            ("vitest.setup.ts", Tests),
            ("vitest.setup.config.ts", Code),
            ("src/__snapshots__/a.snap", Mocks),
            ("src/user.fixture.json", Mocks),
            ("mockServer.ts", Mocks),
            ("mock-data.ts", Mocks),
            ("mockup.ts", Code),
            ("api.pb.go", Generated),
            ("bundle.min.js", Generated),
            ("Cargo.lock", Generated),
            ("requirements-dev.txt", Code),
            ("notes.txt", Docs),
            ("src\\tests\\a.rs", Tests),
        ] {
            assert_eq!(classify_changed_file(path), kind, "{path}");
        }
        assert!(is_test_changed_file("src/a.spec.ts"));
        assert!(is_test_changed_file("__mocks__/fs.ts"));
        assert!(!is_test_changed_file("README.md"));
    }
}
