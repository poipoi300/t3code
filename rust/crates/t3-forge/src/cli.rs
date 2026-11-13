use crate::{FailureReason,ForgeError,Result};
use async_trait::async_trait;
use std::{path::PathBuf,time::Duration};

pub struct CliRequest {
    pub program: &'static str,
    pub provider: &'static str,
    pub operation: String,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub environment: t3_process::Environment,
    pub max_bytes: usize,
    pub timeout: Duration,
}
pub struct CliResponse {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
    pub truncated: bool,
}
#[async_trait]
pub trait ForgeCli: Send+Sync {
    async fn run(&self,request:CliRequest)->Result<CliResponse>;
}
pub struct ProcessForgeCli;
#[async_trait]
impl ForgeCli for ProcessForgeCli {
    async fn run(&self,request:CliRequest)->Result<CliResponse> {
        let mut options = t3_process::ProcessOptions::new(request.program).args(request.args);
        options.cwd=Some(request.cwd);
        options.env=request.environment;
        options.stdin=request.stdin;
        options.max_output_bytes=request.max_bytes;
        options.timeout=Some(request.timeout);
        options.check_exit=false;
        let output=t3_process::run(options).await.map_err(|error|ForgeError::provider(request.provider,&request.operation,if matches!(error,t3_process::ProcessError::NotFound{..}){FailureReason::MissingTool}else{FailureReason::Failed},"The source control CLI could not complete the operation."))?;
        Ok(CliResponse {stdout:output.stdout,stderr:output.stderr,code:output.code.unwrap_or(-1),truncated:output.stdout_truncated||output.stderr_truncated})
    }
}
