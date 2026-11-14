use serde_json::json;
use std::collections::BTreeSet;
use t3_client::{
    services::views::ComposerPathSearchTarget,
    thread_views::composer::{new_task, path_search},
    types::EnvironmentId,
    workspace::projects::EnvironmentProject,
};

fn project(id: &str, environment: &str, repository: Option<&str>) -> EnvironmentProject {
    EnvironmentProject {
        environment: EnvironmentId(environment.into()),
        is_scratch: false,
        project: serde_json::from_value(json!({
            "id": id, "title": id, "workspaceRoot": format!("/repo/{id}"),
            "repositoryIdentity": repository.map(|key| json!({"canonicalKey":key,"locator":{"source":"git-remote","remoteName":"origin","remoteUrl":format!("https://{key}.git")}})),
            "defaultModelSelection":null, "scripts":[],
            "createdAt":"2026-01-01T00:00:00Z", "updatedAt":"2026-01-01T00:00:00Z"
        })).expect("project"),
    }
}

#[test]
fn bare_at_is_active_and_bounded_and_waits_for_the_filesystem_grant() {
    for query in [
        "",
        "src",
        "  src  ",
        &"a".repeat(256),
        &format!("{}😀", "a".repeat(255)),
    ] {
        let target = ComposerPathSearchTarget {
            environment: Some(EnvironmentId("env".into())),
            cwd: Some("/repo".into()),
            query: Some(query.into()),
        };
        let request = path_search::request(&target, true).expect("active search");
        assert_eq!(request.input.limit, 20);
        assert!(request.input.query.0.encode_utf16().count() <= 256);
        assert_eq!(
            path_search::normalize_query(target.query.as_deref()),
            Some(query.trim().into())
        );
        assert!(path_search::request(&target, false).is_none());
        assert!(path_search::pending(&target, true));
    }
    let inactive = ComposerPathSearchTarget {
        environment: Some(EnvironmentId("env".into())),
        cwd: Some("/repo".into()),
        query: None,
    };
    assert_eq!(path_search::normalize_query(None), None);
    assert!(path_search::request(&inactive, true).is_none());
    assert!(!path_search::pending(&inactive, true));
}

#[test]
fn filtered_project_skips_archived_copies_and_prefers_the_remembered_copy() {
    let mut mac = project("t3code", "mac", None);
    mac.project.archived_at = Some(Some(mac.project.created_at.clone()));
    let server = project("t3code", "server", None);
    let connected = BTreeSet::from([mac.environment.clone(), server.environment.clone()]);
    assert_eq!(
        new_task::filtered_project(
            &[mac.clone(), server.clone()],
            &mac,
            None,
            &connected,
            Some(&mac.key())
        ),
        Some(server.key())
    );
    mac.project.archived_at = None;
    assert_eq!(
        new_task::filtered_project(
            &[mac.clone(), server.clone()],
            &mac,
            None,
            &connected,
            Some(&server.key())
        ),
        Some(server.key())
    );
    assert_eq!(
        new_task::filtered_project(&[mac.clone(), server.clone()], &mac, None, &connected, None),
        Some(mac.key())
    );
    let projects = [mac.clone(), server.clone()];
    assert_eq!(
        new_task::destination(
            &projects,
            None,
            Some(&mac.key()),
            Some(&server.key()),
            Some(&server.key())
        ),
        Some(mac.key())
    );
    assert_eq!(
        new_task::destination(&projects, None, None, Some(&mac.key()), Some(&server.key())),
        Some(mac.key())
    );
    assert_eq!(
        new_task::destination(
            &projects,
            Some(false),
            Some(&mac.key()),
            Some(&mac.key()),
            Some(&server.key())
        ),
        None
    );
    mac.project.archived_at = Some(Some(mac.project.created_at.clone()));
    assert_eq!(
        new_task::destination(&[mac.clone()], None, None, None, Some(&mac.key())),
        None
    );
    assert_eq!(
        new_task::destination(&[mac.clone()], None, Some(&mac.key()), None, None),
        Some(mac.key())
    );
}

#[test]
fn repository_identity_controls_pr_workspace_preservation_and_weaker_matches() {
    let selected = project("t3code", "mac", Some("github.com/t3tools/t3code"));
    let mut same = project("clone", "server", Some("github.com/t3tools/t3code"));
    let fork = project("t3code", "server", Some("github.com/fork/t3code"));
    let unknown = project("t3code", "server", None);
    assert!(new_task::same_repository(Some(&same), Some(&selected)));
    assert!(!new_task::same_repository(Some(&fork), Some(&selected)));
    assert!(!new_task::same_repository(Some(&unknown), Some(&selected)));
    assert_eq!(
        new_task::environment_project_match(&[fork.clone(), same.clone()], Some(&selected)),
        Some(&same)
    );
    assert_eq!(
        new_task::environment_project_match(&[fork.clone(), unknown.clone()], Some(&selected)),
        Some(&unknown)
    );
    assert_eq!(
        new_task::environment_project_match(&[fork.clone()], Some(&selected)),
        Some(&fork)
    );
    same.project.repository_identity = None;
    same.project.title = selected.project.title.clone();
    assert_eq!(
        new_task::environment_project_match(&[same.clone()], Some(&selected)),
        Some(&same)
    );
}

#[test]
fn moving_a_pr_draft_preserves_its_workspace_but_clears_the_old_machine_path() {
    use t3_client::thread_views::composer::mobile_content::{retarget, retarget_with_options};
    let draft = json!({"text":"moving house","attachments":[],"project":{"environmentId":"mac","projectId":"p","createdAt":"original"},"workspaceSelection":{"mode":"worktree","branch":"feature/pr-head","worktreePath":"/old-machine/worktrees/pr-42","startFromOrigin":false,"pullRequestReference":"https://github.com/acme/repo/pull/42"}});
    let moved = retarget_with_options(&draft, "server", "p", "later", true);
    assert_eq!(moved["text"], draft["text"]);
    assert_eq!(moved["project"]["createdAt"], "original");
    let mut workspace = draft["workspaceSelection"].clone();
    workspace["worktreePath"] = serde_json::Value::Null;
    assert_eq!(moved["workspaceSelection"], workspace);
    assert!(
        retarget(&moved, "mac", "p", "later")
            .get("workspaceSelection")
            .is_none()
    );
}

#[test]
fn closing_markdown_source_clears_stored_marks_before_a_plain_line() {
    use t3_client::thread_views::{composer::markdown_source, document::Selection};
    for source in ["`code`", "**bold**"] {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":source,"marks":[{"type":"markdownSource"}]}]}]});
        let marks = [json!({"type":"markdownSource"})];
        let collapsed = markdown_source::collapse_shown(
            &document,
            Selection::caret(1 + source.len()),
            Some(&marks),
        );
        assert_eq!(collapsed.stored_marks, Some(vec![]));
        assert!(markdown_source::source_regions(&collapsed.rewrite.document).is_empty());
        assert!(
            !collapsed.rewrite.document["content"][0]["content"][0]["marks"]
                .as_array()
                .expect("marks")
                .is_empty()
        );
        let inside = markdown_source::collapse_shown(&document, Selection::caret(2), Some(&marks));
        assert_eq!(inside.stored_marks, Some(marks.to_vec()));
    }
}

#[test]
fn new_and_existing_thread_dispatches_use_the_destination_clock() {
    use t3_client::view_logic::{chat_view::*, clock::ServerClocks};
    let env = EnvironmentId("destination".into());
    let active = EnvironmentId("active".into());
    let mut clocks = ServerClocks::default();
    clocks.record(&env, Some(10_800_000.0), 7_200_000.0, 7_200_000.0);
    clocks.record(&active, Some(3_600_000.0), 7_200_000.0, 7_200_000.0);
    for (environment, fallback, expected) in [
        (Some(&env), None, 10_801_000),
        (None, Some(&active), 3_601_000),
        (Some(&env), Some(&active), 10_801_000),
        (None, None, 7_201_000),
    ] {
        let snapshot = create_local_dispatch_snapshot_with_clock(
            LocalDispatchClock {
                clocks: &clocks,
                now_epoch_ms: 7_201_000,
                environment,
                active_thread_environment: fallback,
            },
            None,
            None,
            true,
            None,
            SubmissionIntent::Foreground,
        );
        assert_eq!(
            snapshot.started_at,
            t3_client::time::iso_from_epoch_ms(expected)
        );
    }
}

#[test]
fn answer_annotation_chips_reach_the_provider_with_their_full_comment() {
    use t3_client::view_logic::session::questions::{
        AnswerContextRecord, project_pending_user_input_answer,
    };
    let record = serde_json::from_value(json!({"version":1,"kind":"preview-annotation","contextId":"preview-annotation_annotation-1","label":"Note","annotationId":"annotation-1","pageUrl":"http://localhost:3000/","pageTitle":"Home","comment":"FIRST NOTE: this features section looks too bare; add three feature cards","targetSummary":"","styleChanges":[]})).expect("annotation record");
    let answer = json!(
        "See my notes: [Note](t3-context://v1/preview-annotation/preview-annotation_annotation-1)"
    );
    let projected =
        project_pending_user_input_answer(&answer, &[AnswerContextRecord::Preview(&record)]);
    assert!(projected.as_str().expect("text").contains(
        "comment: FIRST NOTE: this features section looks too bare; add three feature cards"
    ));
    let draft = json!({"reviewComments":[{"id":"file-comment-1","sectionId":"branch","sectionTitle":"Changes","filePath":"notes.md","startIndex":1,"endIndex":1,"rangeLabel":"L2","text":"Rename this to line two updated","diff":"@@ -0,0 +2,1 @@\n+line two changed","fenceLanguage":"diff"}],"previewAnnotations":[{"id":"annotation-1","pageUrl":"http://localhost:3000/","pageTitle":"Home","comment":"FIRST NOTE: this features section looks too bare; add three feature cards","elements":[],"regions":[],"strokes":[],"styleChanges":[],"screenshot":null,"createdAt":"2026-01-01T00:00:00.000Z"}]});
    let projected = t3_client::thread_views::composer::answer_context::project_answer(&json!("See my notes: [Comment](t3-context://v1/review-comment/review-comment_file-comment-1) [Note](t3-context://v1/preview-annotation/preview-annotation_annotation-1)"),None,Some(&draft)).expect("project");
    let text = projected.as_str().expect("text");
    assert!(text.contains("Rename this to line two updated"));
    assert!(text.contains("+line two changed"));
    assert!(text.contains("comment: FIRST NOTE:"));
}

#[test]
fn upstream_underscore_and_paste_cases_match_the_typescript_parser() {
    use t3_client::thread_views::rich_text::{Mark, parse_inline_markdown};
    let mut cases = vec![
        "Read https://example.com/_handle_/status/1 and open src/__test__.tsx".to_owned(),
        "https://example.com/~_user_/a".into(),
        "_build_/file.ts".into(),
        "edit __init__.py, then".into(),
        "see __init__.py:42".into(),
        "open src/_a.ts_ and example.com/_handle_.".into(),
        format!("/{}a", "_".repeat(50_000)),
    ];
    cases.extend(
        [
            "_see src/a_b.ts_ now",
            "_read/write access_ now",
            "__read/write access__ now",
            "_see src/_a.ts_(optional)",
            "__see src/__a.ts__(optional)",
            "_see src/_a.ts_.",
            "_notes.md_ here",
            "_e.g._ this",
            "_and_/or",
            "_yes_/_no_",
            "__yes__/__no__.",
            "_a_b_/_no_",
            "and/_or_",
            "_build_/file.ts and _x_, __init__.py and __y__",
            "__test__ now",
        ]
        .map(str::to_owned),
    );
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/web/src/composer-rich-text.ts");
    let output = std::process::Command::new("node").args(["--experimental-strip-types","--input-type=module","-e",r#"import {pathToFileURL} from 'node:url'; import {readFileSync} from 'node:fs'; const {parseInlineMarkdown} = await import(pathToFileURL(process.argv[1]).href); console.log(JSON.stringify(JSON.parse(readFileSync(0,'utf8')).map(parseInlineMarkdown)));"#]).arg(source).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("node");
    let mut child = output;
    use std::io::Write;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(&cases).expect("cases").as_bytes())
        .expect("write cases");
    let output = child.wait_with_output().expect("oracle");
    assert!(output.status.success());
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).expect("oracle JSON");
    let actual = cases.iter().map(|text| parse_inline_markdown(text).into_iter().map(|span| json!({"text":span.text,"marks":span.marks.iter().map(|mark|match mark {Mark::Bold=>"bold",Mark::Italic=>"italic",Mark::Strike=>"strike",Mark::Code=>"code"}).collect::<Vec<_>>()})).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(json!(actual), expected);
}
