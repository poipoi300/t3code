#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use t3_provider::{environment::Platform, opencode_ledger::*, sensitive::ServiceResult};
use tokio::sync::Mutex;
struct Processes {
    rows: Mutex<BTreeMap<u32, ObservedProcess>>,
    signals: Mutex<Vec<(u32, GroupSignal)>>,
}
#[async_trait]
impl LedgerProcesses for Processes {
    async fn observe(&self, pid: u32) -> ServiceResult<Option<ObservedProcess>> {
        Ok(self.rows.lock().await.get(&pid).cloned())
    }
    async fn group(&self, pgid: u32) -> ServiceResult<Vec<ObservedProcess>> {
        Ok(self
            .rows
            .lock()
            .await
            .values()
            .filter(|row| row.pgid == pgid)
            .cloned()
            .collect())
    }
    async fn exists(&self, _: u32) -> ServiceResult<bool> {
        Ok(false)
    }
    async fn signal(&self, pgid: u32, signal: GroupSignal) -> ServiceResult<()> {
        self.signals.lock().await.push((pgid, signal));
        Ok(())
    }
}
fn process(pid: u32, pgid: u32, command: &str) -> ObservedProcess {
    ObservedProcess {
        pid,
        pgid,
        command: command.into(),
        start_time: format!("start-{pid}"),
        zombie: false,
    }
}
#[tokio::test]
async fn recorded_serve_args_allow_wrapper_exec_but_never_recycled_processes_or_live_owners() {
    let dir = tempfile::tempdir().unwrap();
    let processes = Arc::new(Processes {
        rows: Mutex::new(BTreeMap::from([
            (1, process(1, 1, "host")),
            (42, process(42, 42, "wrapper serve --port=1234")),
        ])),
        signals: Mutex::new(vec![]),
    });
    let ledger = OpenCodeLedger::open(dir.path().into(), Platform::Linux, 1, processes.clone())
        .await
        .unwrap();
    ledger
        .track(42, 1234, vec!["serve".into(), "--port=1234".into()])
        .await
        .unwrap()
        .unwrap();
    ledger.reap_orphans().await.unwrap();
    assert!(processes.signals.lock().await.is_empty());
    processes.rows.lock().await.remove(&1);
    processes.rows.lock().await.get_mut(&42).unwrap().command =
        "native-runtime serve --port=1234".into();
    ledger.reap_orphans().await.unwrap();
    assert_eq!(
        *processes.signals.lock().await,
        vec![(42, GroupSignal::Terminate)]
    );
    assert!(!dir.path().join("opencode-servers/42.json").exists());
    processes.rows.lock().await.insert(1, process(1, 1, "host"));
    processes
        .rows
        .lock()
        .await
        .insert(42, process(42, 42, "native serve --port=1234"));
    ledger
        .track(42, 1234, vec!["serve".into(), "--port=1234".into()])
        .await
        .unwrap();
    let path = dir.path().join("opencode-servers/42.json");
    let mut entry: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
    entry.as_object_mut().unwrap().remove("args");
    tokio::fs::write(&path, serde_json::to_vec(&entry).unwrap())
        .await
        .unwrap();
    processes.rows.lock().await.remove(&1);
    ledger.reap_orphans().await.unwrap();
    assert_eq!(processes.signals.lock().await.len(), 2);
    // Old entries without args retain exact command matching.
    for change in [
        "legacy-command",
        "recycled",
        "wrong-group",
        "zombie",
        "wrong-args",
    ] {
        processes.rows.lock().await.insert(1, process(1, 1, "host"));
        processes
            .rows
            .lock()
            .await
            .insert(42, process(42, 42, "wrapper serve --port=1234"));
        ledger
            .track(42, 1234, vec!["serve".into(), "--port=1234".into()])
            .await
            .unwrap();
        let path = dir.path().join("opencode-servers/42.json");
        if change == "legacy-command" {
            let mut entry: serde_json::Value =
                serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
            entry.as_object_mut().unwrap().remove("args");
            tokio::fs::write(&path, serde_json::to_vec(&entry).unwrap())
                .await
                .unwrap();
        }
        let mut rows = processes.rows.lock().await;
        rows.remove(&1);
        let row = rows.get_mut(&42).unwrap();
        match change {
            "legacy-command" => row.command = "native serve --port=1234".into(),
            "recycled" => row.start_time = "different".into(),
            "wrong-group" => row.pgid = 43,
            "zombie" => row.zombie = true,
            _ => row.command = "unrelated serve --port=4321".into(),
        }
        drop(rows);
        ledger.reap_orphans().await.unwrap();
        assert_eq!(processes.signals.lock().await.len(), 2, "{change}");
    }
}
#[tokio::test]
async fn leaderless_groups_record_matching_members_and_windows_does_not_observe_or_record() {
    let dir = tempfile::tempdir().unwrap();
    let processes = Arc::new(Processes {
        rows: Mutex::new(BTreeMap::from([
            (1, process(1, 1, "host")),
            (43, process(43, 42, "other")),
            (44, process(44, 42, "native serve --port=1234")),
        ])),
        signals: Mutex::new(vec![]),
    });
    let ledger = OpenCodeLedger::open(dir.path().into(), Platform::Linux, 1, processes.clone())
        .await
        .unwrap();
    let lease = ledger
        .track(42, 1234, vec!["serve".into(), "--port=1234".into()])
        .await
        .unwrap()
        .unwrap();
    let entry: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(dir.path().join("opencode-servers/42.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["pid"], 44);
    assert!(!format!("{:?}", processes.rows.lock().await[&44]).contains("--port"));
    lease.forget().await.unwrap();
    let windows = OpenCodeLedger::open(dir.path().into(), Platform::Windows, 1, processes)
        .await
        .unwrap();
    assert!(windows.track(42, 1234, vec![]).await.unwrap().is_none());
}
