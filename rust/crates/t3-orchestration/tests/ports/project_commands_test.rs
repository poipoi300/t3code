use serde_json::{Value, json};
use t3_contracts::{application_event::ApplicationProjectEvent, base_schemas::*};
use t3_orchestration::{
    error::Result,
    ports::thread_project::project_commands::*,
    stores::{event_store::EventStore, projects},
    testing::{TEST_TIME, TestDatabase},
};
// Real ProjectCommands.ts executed against every original planner test assertion.
const ORACLE: &str = r###"[{"name":"keeps legacy scripts editable and removable while rejecting new invalid ones","input":{"command":{"type":"project.meta.update","commandId":"cmd-repair-script","projectId":"project-scripts","scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp install","icon":"configure","runOnWorktreeCreate":false},{"id":"lint","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}]},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-repair-script","causationEventId":null,"correlationId":"cmd-repair-script","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp install","icon":"configure","runOnWorktreeCreate":false},{"id":"lint","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}],"updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"keeps legacy scripts editable and removable while rejecting new invalid ones","input":{"command":{"type":"project.meta.update","commandId":"cmd-repair-script","projectId":"project-scripts","scripts":[]},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-repair-script","causationEventId":null,"correlationId":"cmd-repair-script","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","scripts":[],"updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"keeps legacy scripts editable and removable while rejecting new invalid ones","input":{"command":{"type":"project.meta.update","commandId":"cmd-repair-script","projectId":"project-scripts","scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false},{"id":"another.invalid.id","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}]},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[{"id":"install-javascript-dependencies","name":"Install dependencies","command":"vp i","icon":"configure","runOnWorktreeCreate":false}],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.meta.update","detail":"Script IDs must be 1-24 lowercase letters, digits or hyphens, starting with a letter or digit (got 18 characters).","_tag":"ProjectCommandInvariantError","message":"Project command invariant failed (project.meta.update): Script IDs must be 1-24 lowercase letters, digits or hyphens, starting with a letter or digit (got 18 characters)."}}},{"name":"keeps the first pin and archive time, and clears both on undo","input":{"command":{"type":"project.meta.update","commandId":"cmd-organize","projectId":"project-scripts","pinned":true,"archived":true},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-organize","causationEventId":null,"correlationId":"cmd-organize","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","pinnedAt":"2026-01-01T00:00:00.000Z","archivedAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"keeps the first pin and archive time, and clears both on undo","input":{"command":{"type":"project.meta.update","commandId":"cmd-organize","projectId":"project-scripts","pinned":true,"archived":true},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":"2025-12-01T00:00:00.000Z","archivedAt":"2025-12-01T00:00:00.000Z","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-organize","causationEventId":null,"correlationId":"cmd-organize","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","pinnedAt":"2025-12-01T00:00:00.000Z","archivedAt":"2025-12-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"keeps the first pin and archive time, and clears both on undo","input":{"command":{"type":"project.meta.update","commandId":"cmd-organize","projectId":"project-scripts","pinned":false,"archived":false},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":"2025-12-01T00:00:00.000Z","archivedAt":"2025-12-01T00:00:00.000Z","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-organize","causationEventId":null,"correlationId":"cmd-organize","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","pinnedAt":null,"archivedAt":null,"updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"keeps the first pin and archive time, and clears both on undo","input":{"command":{"type":"project.meta.update","commandId":"cmd-organize","projectId":"project-scripts"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":"2025-12-01T00:00:00.000Z","archivedAt":"2025-12-01T00:00:00.000Z","createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"success","value":{"eventId":"event:planned","aggregateKind":"project","aggregateId":"project-scripts","occurredAt":"2026-01-01T00:00:00.000Z","commandId":"cmd-organize","causationEventId":null,"correlationId":"cmd-organize","metadata":{},"type":"project.meta-updated","payload":{"projectId":"project-scripts","updatedAt":"2026-01-01T00:00:00.000Z"}}}},{"name":"rejects a workspace root held by another active project","input":{"command":{"type":"project.create","commandId":"cmd-duplicate-root","projectId":"project-duplicate-root","title":"Duplicate","workspaceRoot":"/tmp/project"},"state":{"workspaceOwner":{"projectId":"project-existing","title":"Scripts","workspaceRoot":"/tmp/project","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"workspaceRoot":"/tmp/project","conflictingProjectId":"project-existing","_tag":"ProjectWorkspaceConflictError","message":"Active project 'project-existing' already exists for workspace root '/tmp/project'."}}},{"name":"rejects a workspace root held by another active project","input":{"command":{"type":"project.meta.update","commandId":"cmd-move-root","projectId":"project-scripts","workspaceRoot":"/tmp/project"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null},"workspaceOwner":{"projectId":"project-existing","title":"Scripts","workspaceRoot":"/tmp/project","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"workspaceRoot":"/tmp/project","conflictingProjectId":"project-existing","_tag":"ProjectWorkspaceConflictError","message":"Active project 'project-existing' already exists for workspace root '/tmp/project'."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.create","commandId":"cmd-create-twice","projectId":"project-scripts","title":"Twice","workspaceRoot":"/tmp/twice"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":null}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.create","detail":"Project 'project-scripts' already exists and cannot be created twice.","_tag":"ProjectCommandInvariantError","message":"Project command invariant failed (project.create): Project 'project-scripts' already exists and cannot be created twice."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.create","commandId":"cmd-create-twice","projectId":"project-scripts","title":"Twice","workspaceRoot":"/tmp/twice"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":"2026-01-01T00:00:00.000Z"}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.create","detail":"Project 'project-scripts' already exists and cannot be created twice.","_tag":"ProjectCommandInvariantError","message":"Project command invariant failed (project.create): Project 'project-scripts' already exists and cannot be created twice."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.meta.update","commandId":"cmd-missing","projectId":"project-scripts"},"state":{},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.meta.update","projectId":"project-scripts","_tag":"ProjectCommandMissingProjectError","message":"Project 'project-scripts' does not exist for command 'project.meta.update'."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.meta.update","commandId":"cmd-missing","projectId":"project-scripts"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":"2026-01-01T00:00:00.000Z"}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.meta.update","projectId":"project-scripts","_tag":"ProjectCommandMissingProjectError","message":"Project 'project-scripts' does not exist for command 'project.meta.update'."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.delete","commandId":"cmd-missing","projectId":"project-scripts"},"state":{},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.delete","projectId":"project-scripts","_tag":"ProjectCommandMissingProjectError","message":"Project 'project-scripts' does not exist for command 'project.delete'."}}},{"name":"requires the project to exist, and to be absent on create","input":{"command":{"type":"project.delete","commandId":"cmd-missing","projectId":"project-scripts"},"state":{"project":{"projectId":"project-scripts","title":"Scripts","workspaceRoot":"/tmp/scripts","defaultModelSelection":null,"defaultThreadEnvMode":null,"hasCustomTitle":false,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","deletedAt":"2026-01-01T00:00:00.000Z"}},"eventId":"event:planned","now":"2026-01-01T00:00:00.000Z"},"result":{"tag":"failure","failure":{"commandType":"project.delete","projectId":"project-scripts","_tag":"ProjectCommandMissingProjectError","message":"Project 'project-scripts' does not exist for command 'project.delete'."}}}]"###;
async fn case(name: &str) -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let name = name.to_owned();
    fixture
        .stores
        .transaction(move |tx| {
            let rows: Vec<Value> = serde_json::from_str(ORACLE)?;
            for row in rows.into_iter().filter(|row| row["name"] == name) {
                let input = &row["input"];
                let project = input["state"]
                    .get("project")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?;
                let owner = input["state"]
                    .get("workspaceOwner")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?;
                if let Some(project) = &project {
                    projects::upsert(tx, project)?;
                }
                let result = plan_project_command(
                    &serde_json::from_value(input["command"].clone())?,
                    ProjectCommandState {
                        project: project.as_ref(),
                        workspace_owner: owner.as_ref(),
                    },
                    serde_json::from_value(input["eventId"].clone())?,
                    &serde_json::from_value(input["now"].clone())?,
                    |text| text.chars().count(),
                )?;
                if row["result"]["tag"] == "success" {
                    let event = result.expect("success");
                    let mut expected = row["result"]["value"].clone();
                    expected["sequence"] = json!(0);
                    assert_eq!(serde_json::to_value(&event)?, expected);
                    projects::apply(tx, &event)?;
                } else {
                    let rejection = result.expect_err("rejection");
                    let encoded = encode_project_command_rejection(&rejection)?;
                    assert_eq!(
                        decode_project_command_rejection(&encoded),
                        Some(rejection.clone())
                    );
                    assert_eq!(rejection.to_string(), row["result"]["failure"]["message"]);
                    let mut expected = row["result"]["failure"].clone();
                    expected
                        .as_object_mut()
                        .expect("object")
                        .shift_remove("message");
                    assert_eq!(serde_json::to_value(rejection)?, expected);
                }
            }
            Ok(())
        })
        .await
}

#[tokio::test]
async fn project_case_1() -> Result<()> {
    case("keeps legacy scripts editable and removable while rejecting new invalid ones").await
}

#[tokio::test]
async fn project_case_2() -> Result<()> {
    case("keeps the first pin and archive time, and clears both on undo").await
}

#[tokio::test]
async fn project_case_3() -> Result<()> {
    case("rejects a workspace root held by another active project").await
}

#[tokio::test]
async fn project_case_4() -> Result<()> {
    case("requires the project to exist, and to be absent on create").await
}

#[tokio::test]
async fn nullable_settings_keep_their_sql_and_event_encoding() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        let command:ProjectCommand=serde_json::from_value(json!({"type":"project.create","commandId":"create","projectId":"p","title":"Project","workspaceRoot":"/p"}))?;
        let now=serde_json::from_value(json!(TEST_TIME))?;
        let event=plan_project_command(&command,ProjectCommandState::default(),EventId("e1".into()),&now,|text|text.chars().count())?.expect("create");
        projects::apply(tx,&event)?;EventStore::new().append_project(tx,&event)?;
        let project=projects::get(tx,&ProjectId("p".into()),false)?.expect("row");
        let update=serde_json::from_value(json!({"type":"project.meta.update","commandId":"update","projectId":"p","defaultModelSelection":null,"defaultThreadEnvMode":"worktree","autoPull":true,"faviconPath":"brand/icon.svg","projectIcon":{"kind":"emoji","emoji":"🦊"}}))?;
        let event=plan_project_command(&update,ProjectCommandState {project:Some(&project),workspace_owner:None},EventId("e2".into()),&now,|text|text.chars().count())?.expect("update");
        projects::apply(tx,&event)?;EventStore::new().append_project(tx,&event)?;
        let actual: (String,i64,String,String,Option<String>)=tx.query_row("SELECT default_thread_env_mode,auto_pull,favicon_path,project_icon_json,default_model_selection_json FROM projection_projects",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        assert_eq!(actual,("worktree".into(),1,"brand/icon.svg".into(),r#"{"kind":"emoji","emoji":"🦊"}"#.into(),None));
        let encoded=serde_json::to_value(event)?;assert!(encoded["payload"].get("defaultModelSelection").is_some());assert_eq!(encoded["payload"]["defaultModelSelection"],Value::Null);
        let persisted:ApplicationProjectEvent=serde_json::from_value(encoded)?;assert!(serde_json::to_value(persisted)?["payload"].get("pinnedAt").is_none());
        Ok(())
    }).await
}
