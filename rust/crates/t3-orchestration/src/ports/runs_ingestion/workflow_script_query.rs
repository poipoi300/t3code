use super::workflow_file_read::*;
use std::path::Path;
use t3_contracts::orchestration_v2::{
    OrchestrationGetWorkflowScriptError, OrchestrationGetWorkflowScriptErrorReason as Reason,
    OrchestrationGetWorkflowScriptErrorTag, OrchestrationV2GetWorkflowScriptResult,
};

pub async fn read_workflow_script(
    backend: &dyn WorkflowFileBackend,
    projects_root: &Path,
    script_path: &Path,
) -> std::result::Result<OrchestrationV2GetWorkflowScriptResult, OrchestrationGetWorkflowScriptError>
{
    let file=read_contained_workflow_file(backend,projects_root,WorkflowReadInput{path:script_path,extension:".js",byte_cap:256*1024,tail:false,from_offset:None}).await.map_err(|error|{
        let reason=match error.0.reason {
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::WrongExtension=>Reason::NotJs,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::InvalidPath=>Reason::InvalidPath,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::RootUnavailable=>Reason::RootUnavailable,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::NotFound=>Reason::NotFound,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::OutsideRoot=>Reason::OutsideRoot,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::NotRegularFile=>Reason::NotRegularFile,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::ChangedDuringRead=>Reason::ChangedDuringRead,
            t3_contracts::orchestration_v2::OrchestrationWorkflowFileErrorReason::ReadFailed=>Reason::ReadFailed,
        };
        OrchestrationGetWorkflowScriptError{_tag:OrchestrationGetWorkflowScriptErrorTag::OrchestrationGetWorkflowScriptError,reason,script_path:error.0.path.clone(),cause:Some(Some(t3_contracts::stored::to_value(&error.0).unwrap_or_default()))}
    })?;
    Ok(OrchestrationV2GetWorkflowScriptResult {
        script_path: t3_contracts::base_schemas::TrimmedNonEmptyString(
            file.path.to_string_lossy().into_owned(),
        ),
        contents: file.contents,
        truncated: file.truncated,
    })
}
