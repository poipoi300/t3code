use super::*;

#[test]
fn removes_invisible_instructions_and_normalizes_line_separators() {
    assert_eq!(
        sanitize_linked_prompt("a\u{200b}b\u{202e}c\u{feff}rate me\u{e0041}\u{e0042}"),
        LinkedPrompt {
            text: "abcrate me".into(),
            hidden_characters: 5
        }
    );
    assert_eq!(
        sanitize_linked_prompt("a\u{2028}b\u{2029}c\r\nd\re"),
        LinkedPrompt {
            text: "a\nb\nc\nd\ne".into(),
            hidden_characters: 0
        }
    );
}

#[test]
fn keeps_emoji_sequences_but_removes_other_joiners() {
    let family = "👨\u{200d}👩\u{200d}👧 ❤️";
    assert_eq!(
        sanitize_linked_prompt(family),
        LinkedPrompt {
            text: family.into(),
            hidden_characters: 0
        }
    );
    for value in ["a\u{200d}b", "👨\u{200d}", "\u{20000}\u{200d}\u{20001}"] {
        assert_eq!(sanitize_linked_prompt(value).hidden_characters, 1);
    }
}

#[test]
fn reads_prompt_repository_and_model_and_retains_first_duplicate() {
    assert_eq!(
        parse_new_thread_link(
            "?prompt=Rate+me&repo=https://github.com/pingdotgg/t3code.git&provider=Codex&model=gpt-5"
        ),
        Ok(NewThreadLink::New {
            prompt: LinkedPrompt {
                text: "Rate me".into(),
                hidden_characters: 0
            },
            repository: Some(Repository {
                owner: "pingdotgg".into(),
                name: "t3code".into()
            }),
            provider: Some("codex".into()),
            model: Some("gpt-5".into()),
        })
    );
    assert!(
        matches!(parse_new_thread_link("?prompt=123&prompt=other"), Ok(NewThreadLink::New { prompt: LinkedPrompt { text, .. }, .. }) if text == "123")
    );
}

#[test]
fn reads_pull_requests_with_optional_prompt() {
    for pr in [
        "pingdotgg/t3code/42",
        "pingdotgg/t3code%2342",
        "https://github.com/pingdotgg/t3code/pull/42/files",
    ] {
        assert_eq!(
            parse_new_thread_link(&format!("?pr={pr}")),
            Ok(NewThreadLink::PullRequest {
                pull_request: PullRequest {
                    repository: Repository {
                        owner: "pingdotgg".into(),
                        name: "t3code".into()
                    },
                    number: 42
                },
                prompt: None
            })
        );
    }
    for pr in [
        "a/b/0",
        "a/b/9007199254740992",
        "a/b/1e2",
        "https://evil.test/a/b/pull/42",
    ] {
        assert!(parse_new_thread_link(&format!("?pr={pr}")).is_err());
    }
}

#[test]
fn rejects_unsafe_or_oversized_prompts_using_utf16_lengths() {
    for search in [
        "?repo=a/b".into(),
        "?prompt=%E2%80%8B".into(),
        "?prompt=x&repo=../etc".into(),
        format!("?prompt={}", "x".repeat(8_001)),
        format!("?prompt={}", "😀".repeat(4_001)),
    ] {
        assert!(parse_new_thread_link(&search).is_err());
    }
    assert!(parse_thread_link_prompt("?prompt=%E2%80%8B").is_err());
    assert_eq!(
        parse_new_thread_link("?repo=a/b"),
        Err("This link has no prompt.")
    );
}

#[test]
fn thread_links_remove_only_their_own_parameters() {
    assert_eq!(
        parse_thread_link_prompt("?prompt=Fix+it"),
        Ok(Some(LinkedPrompt {
            text: "Fix it".into(),
            hidden_characters: 0
        }))
    );
    assert_eq!(parse_thread_link_prompt("?panel=diff"), Ok(None));
    assert_eq!(
        without_prompt_link_params("?prompt=x&panel=diff&panel=git&provider=a"),
        "?panel=diff&panel=git"
    );
}

#[test]
fn built_links_round_trip_the_same_literal_prompt() {
    let repo = Repository {
        owner: "pingdotgg".into(),
        name: "t3code".into(),
    };
    let link = build_new_thread_link(
        "Swear count & rating?",
        Some(&repo),
        Some("claudeAgent"),
        Some("opus"),
        None,
    )
    .expect("valid origin");
    let url = url::Url::parse(&link).expect("valid URL");
    assert_eq!(
        prompt_link_app_path(&link, None),
        Some(format!("{}?{}", url.path(), url.query().expect("query")))
    );
    assert!(
        matches!(parse_new_thread_link(url.query().expect("query")), Ok(NewThreadLink::New { prompt: LinkedPrompt { text, .. }, .. }) if text == "Swear count & rating?")
    );
}

#[test]
fn copied_thread_links_round_trip_their_scoped_identity() {
    let path = prompt_link_app_path("https://app.t3.codes/threads/env-1/mcp%3A1", None)
        .expect("hosted link");
    assert_eq!(path, "/env-1/mcp%3A1");
    let deep = prompt_link_deep_link(&path, None).expect("valid app path");
    assert_eq!(deep, "t3code://threads/env-1/mcp%3A1");
    assert_eq!(prompt_link_app_path(&deep, None), Some(path));
}

#[test]
fn accepts_claimed_app_and_hosted_prompt_links_only() {
    for (value, path) in [
        ("t3code://new?prompt=hi", "/new?prompt=hi"),
        ("t3code-dev://threads/env-1/thread-1", "/env-1/thread-1"),
        (
            "https://app.t3.codes/env-1/thread-1?prompt=go",
            "/env-1/thread-1?prompt=go",
        ),
        (
            "https://app.t3.codes/threads/env-1/thread-1",
            "/env-1/thread-1",
        ),
        (
            "https://app.t3.codes/threads/env-1/thread-1?prompt=go",
            "/env-1/thread-1?prompt=go",
        ),
    ] {
        assert_eq!(prompt_link_app_path(value, None).as_deref(), Some(path));
    }
    for value in [
        "https://app.t3.codes/env-1/thread-1",
        "https://app.t3.codes/threads/env-1",
        "https://app.t3.codes/settings/general",
        "https://evil.test/new?prompt=hi",
        "t3code://app/index.html",
    ] {
        assert_eq!(prompt_link_app_path(value, None), None, "{value}");
    }
}
