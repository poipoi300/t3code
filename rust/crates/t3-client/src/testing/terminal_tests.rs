//! The connectable fake serves terminal RPCs through the same session as the UI.
#![allow(clippy::unwrap_used)]

use std::rc::Rc;

use futures::{executor::LocalPool, task::LocalSpawnExt};
use serde_json::json;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;

use super::environment::FakeEnvironment;
use crate::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    types::{EnvironmentId, ThreadId, ThreadKey},
};

#[test]
fn a_connectable_environment_can_open_write_and_close_a_terminal() {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let spawn: Spawner = Rc::new(move |future| spawner.spawn_local(future).unwrap());
    let fake = FakeEnvironment::new("env", spawn.clone());
    fake.edit_config(|config| config["environment"]["capabilities"]["terminal"] = json!(true));
    fake.set_permissions(&["orchestration:read", "terminal:read", "terminal:operate"]);
    let client = ClientRuntime::new(
        fake.platform(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        spawn,
    );
    pool.run_until(client.pair(fake.pairing())).unwrap();
    pool.run_until_stalled();
    let terminals = client.terminals();
    let key = ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread".into()),
    };
    let input = serde_json::from_value(json!({
        "threadId": "thread", "terminalId": "demo", "cwd": "/workspace/project"
    }))
    .unwrap();
    let opened = pool.run_until(terminals.open(&key, input)).unwrap();
    assert_eq!(opened.terminal_id, "demo");
    assert_eq!(opened.cwd, "/workspace/project");

    // The fixture lets the test drive the stream on the same socket as commands.
    let frames = fake.terminal_stream(super::environment::TerminalStream::Attach);
    let attach = serde_json::from_value(json!({
        "threadId": "thread", "terminalId": "demo", "cwd": "/workspace/project"
    }))
    .unwrap();
    let lease = terminals.session(&key.environment, attach);
    pool.run_until_stalled();
    frames
        .unbounded_send(json!({"type": "snapshot", "snapshot": opened}))
        .unwrap();
    frames
        .unbounded_send(json!({
            "type": "output", "threadId": "thread", "terminalId": "demo", "data": "hello\r\n"
        }))
        .unwrap();
    pool.run_until_stalled();
    assert_eq!(fake.calls("terminal.attach").len(), 1);
    assert_eq!(
        lease.with_output(crate::services::terminal::terminal_output_replay),
        "hello\r\n"
    );

    pool.run_until(terminals.write(&key, "demo", "echo hello\n".into()))
        .unwrap();
    pool.run_until(terminals.close(&key, "demo", false))
        .unwrap();
    assert_eq!(fake.calls("terminal.write")[0]["data"], "echo hello\n");
    assert_eq!(fake.calls("terminal.close")[0]["terminalId"], "demo");
}
