use super::link_targets::*;
use super::prompt_links::{PullRequest, Repository};
use crate::{types::*, workspace::projects::EnvironmentProject};
use serde_json::{Value, json};

fn shell() -> Value {
    let project: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationProjectShell.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )
    .expect("project");
    let thread: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )
    .expect("thread");
    json!({"projects":[project],"threads":[thread]})
}
fn project(key: &str, origin: Option<&str>) -> EnvironmentProject {
    let mut value = shell()["projects"][0].clone();
    value["repositoryIdentity"] = json!({"canonicalKey":key,"locator":{"source":"git-remote","remoteName":"origin","remoteUrl":format!("https://{key}")}});
    if let Some(origin) = origin {
        value["repositoryIdentity"]["origin"] = json!({"canonicalKey":origin});
    }
    EnvironmentProject {
        environment: EnvironmentId("env".into()),
        project: serde_json::from_value(value).expect("project"),
        is_scratch: false,
    }
}
#[test]
fn repositories_keep_source_order_use_origin_and_match_gitcafe_mirrors() {
    let projects = vec![
        project("github.com/pingdotgg/t3code", Some("github.com/me/t3code")),
        project("github.com/PingDotGG/T3Code", None),
        project("git.cafe/pingdotgg/t3code", None),
        project("staging.git.cafe/pingdotgg/t3code", None),
        project("gitlab.com/pingdotgg/t3code", None),
    ];
    let matches = find_projects(
        &projects,
        &Repository {
            owner: "pingdotgg".into(),
            name: "t3code".into(),
        },
    );
    assert_eq!(matches.len(), 3);
    assert!(std::ptr::eq(matches[0], &projects[1]));
    assert!(std::ptr::eq(matches[2], &projects[3]));
    assert!(repository_matches(
        &projects[0],
        &Repository {
            owner: "me".into(),
            name: "t3code".into()
        }
    ));
}
#[test]
fn newest_pull_request_thread_excludes_side_chats_and_tombstones() {
    let base = shell()["threads"][0].clone();
    let thread = |id: &str, date: &str, side: bool| {
        let mut value = base.clone();
        value["id"] = json!(id);
        value["updatedAt"] = json!(date);
        value["lineage"]["sideChat"] = json!(side);
        value["pullRequests"] = json!([{"host":"github.com","repository":"acme/web","number":7,"url":"https://github.com/acme/web/pull/7","source":"manual","linkedAt":"2026-01-01T00:00:00.000Z","snapshot":null,"stack":null}]);
        serde_json::from_value(value).expect("thread")
    };
    let threads = [
        thread("old", "2026-01-01T00:00:00.000Z", false),
        thread("parent", "2026-01-02T00:00:00.000Z", false),
        thread("side", "2026-01-03T00:00:00.000Z", true),
    ];
    let env = EnvironmentId("env".into());
    assert_eq!(
        find_pull_request_thread(
            threads.iter().map(|t| (&env, t)),
            &PullRequest {
                repository: Repository {
                    owner: "acme".into(),
                    name: "web".into()
                },
                number: 7
            }
        )
        .expect("linked")
        .thread
        .0,
        "parent"
    );
}
#[test]
fn lookups_wait_for_every_preceding_answer_and_ignore_archived_threads() {
    let none = serde_json::from_value(json!({"threads":[]})).expect("empty");
    let linked = serde_json::from_value(
        json!({"threads":[{"id":"b","projectId":"p","title":"b","archivedAt":null}]}),
    )
    .expect("linked");
    let env = EnvironmentId("laptop".into());
    let server = EnvironmentId("server".into());
    assert!(matches!(
        pick_pull_request_thread([
            (env.clone(), Lookup::Pending),
            (server.clone(), Lookup::Ready(&linked))
        ]),
        LookupChoice::Pending
    ));
    assert!(matches!(
        pick_pull_request_thread([
            (env.clone(), Lookup::Ready(&linked)),
            (server.clone(), Lookup::Pending)
        ]),
        LookupChoice::Thread(_)
    ));
    assert!(
        matches!(pick_pull_request_thread([(env,Lookup::Ready(&none)),(server,Lookup::Ready(&linked))]),LookupChoice::Thread(key) if key.environment.0=="server"&&key.thread.0=="b")
    );
}
#[test]
fn requested_provider_and_model_aliases_resolve_to_a_usable_instance() {
    let config: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerProvider.jsonl")
            .lines()
            .next()
            .expect("fixture"),
    )
    .expect("config");
    let provider = |id: &str, driver: &str, status: &str| {
        let mut value = config.clone();
        value["instanceId"] = json!(id);
        value["driver"] = json!(driver);
        value["status"] = json!(status);
        value["enabled"] = json!(true);
        value["availability"] = Value::Null;
        value["models"] = json!([{"slug":"small","name":"Small","isCustom":false,"capabilities":null},{"slug":"big","name":"Big","aliases":["opus"],"isDefault":true,"isCustom":false,"capabilities":null}]);
        serde_json::from_value(value).expect("provider")
    };
    let providers = [
        provider("broken", "claudeAgent", "error"),
        provider("work", "claudeAgent", "ready"),
    ];
    let (pick, exact) = model_selection(&providers, "claude", Some("opus")).expect("selection");
    assert_eq!(
        (pick.instance_id.0, pick.model, exact),
        ("work".into(), "big".into(), true)
    );
    let providers = [provider("codex", "codex", "warning")];
    assert!(
        matches!(model_selection(&providers,"codex",Some("unknown")),Some((pick,false)) if pick.model=="big")
    );
    assert!(matches!(
        model_selection(&providers, "codex", None),
        Some((_, true))
    ));
    assert!(model_selection(&providers, "cursor", None).is_none());
}
