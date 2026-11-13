use t3_contracts::{application_event::ApplicationProjectEvent, base_schemas::ProjectId};
use t3_orchestration::{
    error::Result,
    stores::projects,
    testing::{TEST_TIME, TestDatabase, project_event},
};
fn meta(id: &str, payload: serde_json::Value) -> Result<ApplicationProjectEvent> {
    Ok(serde_json::from_value(
        serde_json::json!({"sequence":1,"eventId":id,"aggregateKind":"project","aggregateId":"project","occurredAt":TEST_TIME,"commandId":null,"causationEventId":null,"correlationId":null,"metadata":{},"type":"project.meta-updated","payload":payload}),
    )?)
}
#[tokio::test]
async fn model_without_options_stays_omitted_in_sql_json() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let mut value = serde_json::to_value(project_event("created", "project")?)?;
            value["payload"]["defaultModelSelection"] =
                serde_json::json!({"instanceId":"codex","model":"gpt-5.4"});
            projects::apply(tx, &serde_json::from_value(value)?)?;
            let raw: String = tx.query_row(
                "SELECT default_model_selection_json FROM projection_projects",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(raw, r#"{"instanceId":"codex","model":"gpt-5.4"}"#);
            let project = projects::get(tx, &ProjectId("project".into()), false)?.expect("project");
            assert!(project.default_model_selection.is_some());
            assert!(projects::to_shell(&project).is_ok());
            Ok(())
        })
        .await
}
#[tokio::test]
async fn pin_archive_custom_title_soft_delete_and_exact_workspace_matching() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        projects::apply(tx,&project_event("created","project")?)?;
        let id=ProjectId("project".into());assert!(!projects::get(tx,&id,false)?.expect("project").has_custom_title);
        projects::apply(tx,&meta("organized",serde_json::json!({"projectId":"project","pinnedAt":TEST_TIME,"archivedAt":TEST_TIME,"updatedAt":TEST_TIME}))?)?;
        projects::apply(tx,&meta("rename",serde_json::json!({"projectId":"project","title":"Custom","updatedAt":TEST_TIME}))?)?;
        let row=projects::get(tx,&id,false)?.expect("project");assert!(row.has_custom_title);assert!(row.pinned_at.is_some());assert!(row.archived_at.is_some());assert_eq!(row.title,"Custom");
        projects::apply(tx,&project_event("reimport","project")?)?;assert_eq!(projects::get(tx,&id,false)?.expect("project").title,"Custom");
        assert!(projects::find_active_by_workspace_root(tx,"/test/project")?.is_some());assert!(projects::find_active_by_workspace_root(tx,"/test/project/")?.is_none());
        assert!(projects::list(tx,Some(&[]),false)?.is_empty());
        let mut deleted=serde_json::to_value(project_event("delete","project")?)?;deleted["type"]="project.deleted".into();deleted["payload"]=serde_json::json!({"projectId":"project","deletedAt":TEST_TIME});
        projects::apply(tx,&serde_json::from_value(deleted)?)?;assert!(projects::get(tx,&id,false)?.is_none());assert!(projects::get(tx,&id,true)?.is_some());
        Ok(())
    }).await
}

/// Project rows, shells and reads produced by the real TypeScript `ProjectStoreV2` for the same
/// events (`tests/fixtures/store-oracle.json`): every column including the icon, scripts and model
/// selection JSON, fields set then cleared with null, a re-create and a soft delete.
#[tokio::test]
async fn project_rows_and_shells_match_the_typescript_project_store() -> Result<()> {
    use crate::persistence_orchestration_event_store_test::{
        assert_rows, assert_same_json, snapshot, store_oracle,
    };
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let oracle = &store_oracle()["projects"];
            for event in oracle["events"].as_array().expect("events") {
                projects::apply(tx, &serde_json::from_value(event.clone())?)?;
            }
            assert_rows(
                &snapshot(tx, "projection_projects")?,
                &oracle["rows"],
                "project rows",
            );
            let shells = serde_json::to_value(projects::list_shells(tx, None)?)?;
            assert_same_json(&shells, &oracle["shells"], "project shells");
            let deleted = projects::get(tx, &ProjectId("project:c".into()), true)?;
            assert_same_json(
                &serde_json::to_value(deleted)?,
                &oracle["deleted"],
                "deleted project",
            );
            assert!(projects::get(tx, &ProjectId("project:c".into()), false)?.is_none());
            let by_root = projects::find_active_by_workspace_root(tx, "C:/work/beta2")?;
            assert_same_json(
                &serde_json::to_value(by_root)?,
                &oracle["byRoot"],
                "workspace lookup",
            );
            Ok(())
        })
        .await
}
