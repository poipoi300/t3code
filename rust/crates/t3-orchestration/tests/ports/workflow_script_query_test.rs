use super::workflow_agent_transcript_test::{MemoryFiles, root};
use t3_contracts::orchestration_v2::OrchestrationGetWorkflowScriptErrorReason as Reason;
use t3_orchestration::ports::runs_ingestion::workflow_script_query::read_workflow_script;

#[tokio::test]
async fn scripts_are_contained_regular_javascript_and_bounded() {
    let backend = MemoryFiles::default();
    let path = root().join("script.js");
    backend.put(path.clone(), b"export default 1".to_vec(), 1, true);
    let result = read_workflow_script(&backend, &root(), &path)
        .await
        .unwrap();
    assert_eq!(result.contents, "export default 1");
    assert!(!result.truncated);
    backend.put(path.clone(), vec![b'x'; 256 * 1024 + 1], 2, true);
    let result = read_workflow_script(&backend, &root(), &path)
        .await
        .unwrap();
    assert!(result.truncated);
    assert_eq!(result.contents.len(), 256 * 1024);
    backend.put(path.clone(), vec![], 3, false);
    assert_eq!(
        read_workflow_script(&backend, &root(), &path)
            .await
            .err()
            .unwrap()
            .reason,
        Reason::NotRegularFile
    );
    backend.aliases.lock().unwrap().insert(
        path.clone(),
        root().with_extension("outside").join("escape.js"),
    );
    assert_eq!(
        read_workflow_script(&backend, &root(), &path)
            .await
            .err()
            .unwrap()
            .reason,
        Reason::OutsideRoot
    );
    backend
        .aliases
        .lock()
        .unwrap()
        .insert(path.clone(), root().join("escape.txt"));
    assert_eq!(
        read_workflow_script(&backend, &root(), &path)
            .await
            .err()
            .unwrap()
            .reason,
        Reason::NotJs
    );
}
