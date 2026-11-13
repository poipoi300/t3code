#![cfg(feature = "acp-adapter")]
use serde_json::{Value, json};
use t3_orchestration::ports::acp_replay::adapters::open_code2_adapter_v2_testkit::operation_of;

#[tokio::test]
async fn opencode2_http_routes_equal_the_typescript_replay_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let mut requests = Vec::new();
    let paths = [
        "/api/experimental/job",
        "/api/fs/read/a.png",
        "/api/rpc/id/start",
        "/api/shell",
        "/api/shell/id",
        "/api/shell/id/output",
        "/api/event",
        "/api/model",
        "/api/agent",
        "/api/session",
        "/api/session/active",
        "/api/command",
        "/api/experimental/mcp/x",
        "/api/experimental/session/s/instructions/entries/k",
        "/api/skill",
        "/api/unknown",
    ];
    let rest = [
        "",
        "/prompt",
        "/command",
        "/compact",
        "/interrupt",
        "/model",
        "/agent",
        "/move",
        "/synthetic",
        "/fork",
        "/revert/stage",
        "/revert/commit",
        "/revert",
        "/inbox/a%2Fb%20%C3%A9",
        "/message",
        "/permission",
        "/form",
        "/form/f",
        "/form/f/reply",
        "/permission/r/reply",
    ];
    let mut paths: Vec<String> = paths.iter().map(|path| (*path).into()).collect();
    paths.extend(rest.iter().map(|rest| format!("/api/session/s{rest}")));
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        for path in &paths {
            requests.push(json!({"method":method,"path":path,"query":{"kind":"image","directory":"<workspace>","override":"query"},"body":{"optional":null,"enabled":false,"override":"body"}}));
        }
    }
    let mut options=t3_process::ProcessOptions::new("node").args(["--input-type=module","--eval",r#"
import {stripTypeScriptTypes} from 'node:module';
import {readFileSync} from 'node:fs';
const source=readFileSync('apps/server/src/orchestration-v2/Adapters/OpenCode2AdapterV2.testkit.ts','utf8');
const begin=source.indexOf('const operationOf =');
const end=source.indexOf('const replayHttpClient =',begin);
const code=stripTypeScriptTypes(source.slice(begin,end).replace('const operationOf =','export const operationOf ='));
const {operationOf}=await import('data:text/javascript,'+encodeURIComponent(code));
const rows=JSON.parse(readFileSync(0,'utf8'));
process.stdout.write(JSON.stringify(rows.map(({method,path,query,body})=>operationOf(method,path,query,body))));
"#]);
    options.cwd = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    options.stdin = Some(serde_json::to_vec(&requests)?);
    let output = t3_process::run(options).await?;
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(expected.len(), requests.len());
    for (request, expected) in requests.iter().zip(expected) {
        assert_eq!(
            operation_of(
                request["method"].as_str().unwrap(),
                request["path"].as_str().unwrap(),
                &request["query"],
                &request["body"]
            )?,
            expected,
            "{request}"
        );
    }
    Ok(())
}
