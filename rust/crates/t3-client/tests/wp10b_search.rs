#![allow(clippy::unwrap_used)]
use serde_json::json;
use t3_client::view_logic::{TextRange, shell::search::*};
const NOW: f64 = 2_000_000_000_000.0;
const DAY: f64 = 86_400_000.0;
fn thread(title: &str) -> ThreadSubject {
    ThreadSubject {
        title: title.into(),
        activity_at: Some(NOW - 30.0 * DAY),
        ..Default::default()
    }
}
fn rank<'a>(query: &str, threads: &'a [ThreadSubject]) -> Vec<&'a str> {
    let query = fold_query(query);
    rank_items(threads, |t, typos| {
        score_thread(&query, t, NOW, typos, true)
    })
    .into_iter()
    .map(|(t, _)| t.title.as_str())
    .collect()
}
#[test]
fn whole_words_prefixes_and_midword_hits_keep_quality_before_recency() {
    let mut recent = thread("Fix oauth redirect");
    recent.activity_at = Some(NOW);
    let threads = [
        recent,
        thread("Authentication cleanup"),
        thread("Share auth across worktrees"),
    ];
    assert_eq!(
        rank("auth", &threads),
        [
            "Share auth across worktrees",
            "Authentication cleanup",
            "Fix oauth redirect"
        ]
    );
    let mut frequent = thread("Share authentication cleanup");
    frequent.opens =
        Some(serde_json::from_value(json!({"openedAt":NOW,"score":12,"picks":[]})).unwrap());
    assert_eq!(
        rank(
            "share auth",
            &[frequent, thread("Auth across worktrees to share")]
        )[0],
        "Auth across worktrees to share"
    );
}
#[test]
fn source_highlights_respect_utf16_camel_case_paths_digit_and_cjk_boundaries() {
    for (title, query, start, end) in [
        ("useThreadSearch hook", "search", 9, 15),
        ("fix/sidebar-perf", "perf", 12, 16),
        ("Upgrade to pnpm12", "12", 15, 17),
        ("Fix JSONSchema parsing", "sch", 8, 11),
        ("Fix HTTPServer crash", "ser", 8, 11),
        ("Rename index.ts", ".ts", 12, 15),
        ("🚀 Fix sidebar", "sidebar", 7, 14),
    ] {
        assert_eq!(
            score_thread(query, &thread(title), NOW, false, true)
                .unwrap()
                .ranges,
            [TextRange { start, end }],
            "{title}"
        );
    }
    assert!(score_thread("ci", &thread("Decision log"), NOW, false, true).is_none());
    for (title, query) in [
        ("实现全文搜索", "搜索"),
        ("新しい検索機能", "検索"),
        ("🚀 Launch checklist", "🚀"),
    ] {
        assert!(score_thread(query, &thread(title), NOW, false, true).is_some());
    }
}
#[test]
fn typo_fallback_handles_transpositions_but_never_invents_numeric_references() {
    for (title, query, found) in [
        ("Hosting alternatives to Railway", "railwya", true),
        ("Trim MCP model lists", "trim mpc", true),
        ("t3code perf audit", "t3coed", true),
        ("Fix oauth2 redirect", "oath2", true),
        ("Nightly build fix", "ing", false),
        ("Config cleanup", "cron", false),
        ("Server restart", "srv", true),
        ("Fix sidebar lag", "sidb", true),
        ("GitHub API usage", "githbu", true),
        ("Fix macOS window", "macso", true),
        ("Fix sidebar lag", "fix the sidebar", true),
        ("Fix sidebar lag", "the", false),
    ] {
        assert_eq!(!rank(query, &[thread(title)]).is_empty(), found, "{query}");
    }
    let mut s = thread("Stats");
    s.pull_requests = vec![SearchPullRequest {
        number: 22189,
        title: None,
        references: vec![],
    }];
    assert!(rank("22889", &[s]).is_empty());
    assert_eq!(
        rank(
            "cmd enter",
            &[thread("Fix Cmd+Enter"), thread("Open links external")]
        ),
        ["Fix Cmd+Enter"]
    );
}
#[test]
fn joined_words_folding_and_picked_query_limits_preserve_source_semantics() {
    for (title, query, found) in [
        ("VS Code extension settings", "vscode", true),
        ("Fix UI bug in sidebar", "fixuibug", true),
        ("Show skill names in thread overview", "inth", false),
        ("Move Client State to Server", "tos", false),
        ("T3 Code vs Gemini", "t3co", true),
        ("Fix sidebar lag", "\"sidebar lag\"", true),
        ("Café menu", "cafe\u{301}", true),
        ("Migrate to Go modules", "gomod", true),
        ("Don’t merge yet", "don't", true),
    ] {
        assert_eq!(!rank(query, &[thread(title)]).is_empty(), found, "{query}");
    }
    assert_eq!(
        picked_query(&format!("{}🚀", "a".repeat(31))),
        "a".repeat(31)
    );
    assert_eq!(picked_query("  Upl "), "upl");
    assert_eq!(
        rank(
            "fixsid",
            &[thread("Fix sidebar lag"), thread("Unrelated work")]
        ),
        ["Fix sidebar lag"]
    );
    assert!(rank("fsl", &[thread("Fix sidebar lag")]).is_empty());
    for q in ["#", "´´´"] {
        assert!(rank(q, &[thread("Fix sidebar lag")]).is_empty());
    }
    assert_eq!(
        rank("# fix", &[thread("Fix sidebar lag")]),
        ["Fix sidebar lag"]
    );
}
#[test]
fn branch_pr_and_environment_hints_require_coherent_fields_and_preserve_references() {
    let mut s = thread("Trim model lists");
    s.id = Some("thread-5fe6f59c-4976".into());
    s.branch = Some("t3/cleanup-payloads".into());
    s.project_title = Some("t3code".into());
    s.environment_label = Some("BeeBox".into());
    s.pull_requests = vec![SearchPullRequest {
        number: 22889,
        title: Some("Smaller MCP payloads".into()),
        references: vec![
            "pingdotgg/t3code#22889".into(),
            "https://github.com/pingdotgg/t3code/pull/22889".into(),
        ],
    }];
    for q in ["#22889", "pr 22889", "pr22889", "pull request #22889"] {
        let matched = score_thread(&fold_query(q), &s, NOW, false, true).unwrap();
        assert_eq!(matched.hint.unwrap().label, "PR");
    }
    for q in [
        "t3code#22889",
        "github.com/pingdotgg/t3code/pull/22889",
        "https://github.com/pingdotgg/t3code/pull/22889/files",
        "5fe6f59c",
        "beebox",
        "payloads t3code",
    ] {
        assert!(
            score_thread(&fold_query(q), &s, NOW, false, true).is_some(),
            "{q}"
        );
    }
    assert_eq!(
        score_thread("cleanup", &s, NOW, false, true)
            .unwrap()
            .hint
            .unwrap()
            .text,
        "cleanup-payloads"
    );
    assert!(
        score_thread("trim", &s, NOW, false, true)
            .unwrap()
            .hint
            .is_none()
    );
    let mut scattered = thread("Weekly notes");
    scattered.pull_requests = vec![
        SearchPullRequest {
            number: 1,
            title: Some("fix model picker".into()),
            references: vec![],
        },
        SearchPullRequest {
            number: 2,
            title: Some("CI speedups".into()),
            references: vec![],
        },
    ];
    assert!(score_thread("fix ci", &scattered, NOW, false, true).is_none());
}
#[test]
fn exact_picks_outrank_use_but_stale_picks_and_midword_accidents_do_not() {
    let mut selected = thread("Release checklist");
    selected.opens = Some(
        serde_json::from_value(
            json!({"openedAt":NOW-5.0*DAY,"score":1,"picks":[{"query":"rel","at":NOW-5.0*DAY}]}),
        )
        .unwrap(),
    );
    let mut recent = thread("Relay pairing");
    recent.activity_at = Some(NOW);
    assert_eq!(
        rank("rel", &[selected.clone(), recent.clone()]),
        ["Release checklist", "Relay pairing"]
    );
    assert_eq!(
        rank("re", &[selected.clone(), recent.clone()])[0],
        "Relay pairing"
    );
    selected.opens = Some(
        serde_json::from_value(
            json!({"openedAt":NOW,"score":1,"picks":[{"query":"rel","at":NOW-90.0*DAY}]}),
        )
        .unwrap(),
    );
    assert_eq!(rank("rel", &[selected, recent])[0], "Relay pairing");
}
