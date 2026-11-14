#![cfg(all(
    feature = "local-services",
    feature = "workspace",
    feature = "terminal"
))]
#![allow(clippy::unwrap_used)]
mod e_common;
use e_common::*;
use futures::{StreamExt, poll};
use serde_json::json;
use std::{sync::Arc, task::Poll};
use t3_contracts::terminal::*;
use t3_server::{
    adapters::terminal::{TerminalOutputWindow, output_stream},
    registry::Failure,
};

fn ok<T>(result: t3_server::adapters::terminal::TerminalResult<T>) -> T {
    match result {
        Ok(value) => value,
        Err(Failure::Typed(error)) => panic!("typed terminal error: {error:?}"),
        Err(Failure::Defect(error)) => panic!(
            "terminal defect: {}",
            serde_json::to_string(&error).unwrap()
        ),
    }
}
fn output(data: String) -> TerminalAttachStreamEvent {
    decode(
        json!({"type":"output","threadId":"thread-e","terminalId":"term-1","data":data,"sequence":1}),
    )
}

#[tokio::test]
async fn terminal_output_window_obeys_both_byte_and_chunk_caps_and_real_acks() {
    let window = TerminalOutputWindow::default();
    for _ in 0..7 {
        window.sent(&[output("x".into())]);
    }
    assert!(!window.is_full());
    window.sent(&[output("x".into())]);
    assert!(window.is_full());
    let mut capacity = Box::pin(window.capacity());
    assert!(matches!(poll!(&mut capacity), Poll::Pending));
    window.ack();
    assert!(matches!(poll!(&mut capacity), Poll::Ready(())));
    let window = TerminalOutputWindow::default();
    window.sent(&[output("é".repeat(17_000))]);
    assert!(!window.is_full());
    window.sent(&[output("é".repeat(17_000))]);
    assert!(
        window.is_full(),
        "bytes must be UTF-8 bytes, not character count"
    );
    window.ack();
    assert!(!window.is_full());
}

#[tokio::test]
async fn attached_viewers_keep_independent_grids_and_output_waits_for_ack() {
    let temp = tempfile::tempdir().unwrap();
    let adapter = terminal(temp.path());
    let root = temp.path().to_string_lossy();
    let base = json!({"threadId":"thread-e","terminalId":"term-1","cwd":root,"cols":120,"rows":40});
    let mut a = ok(adapter.attach(decode(base), "socket-a".into()).await);
    a.recv().await.unwrap().unwrap();
    let mut b = ok(adapter
        .attach(
            decode(
                json!({"threadId":"thread-e","terminalId":"term-1","cwd":root,"cols":60,"rows":20}),
            ),
            "socket-b".into(),
        )
        .await);
    b.recv().await.unwrap().unwrap();
    let session = || decode(json!({"threadId":"thread-e","terminalId":"term-1"}));
    let snapshot = adapter.manager.snapshot(session()).await.unwrap();
    assert_eq!(
        (snapshot.cols, snapshot.rows),
        (Some(Some(60)), Some(Some(20)))
    );
    ok(adapter
        .resize(
            decode(json!({"threadId":"thread-e","terminalId":"term-1","cols":50,"rows":15})),
            "socket-a".into(),
        )
        .await);
    ok(adapter
        .resize(
            decode(json!({"threadId":"thread-e","terminalId":"term-1","cols":100,"rows":50})),
            "socket-b".into(),
        )
        .await);
    let snapshot = adapter.manager.snapshot(session()).await.unwrap();
    assert_eq!(
        (snapshot.cols, snapshot.rows),
        (Some(Some(50)), Some(Some(15)))
    );
    let observe = ok(adapter.observe(session()).await);
    let window = Arc::new(TerminalOutputWindow::default());
    let mut stream = output_stream(observe, window.clone());
    let first = ok(stream.next().await.unwrap());
    assert!(matches!(
        first,
        TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(_)
    ));
    for _ in 0..7 {
        window.sent(&[output("pending".into())]);
    }
    assert!(window.is_full());
    let mut next = Box::pin(stream.next());
    assert!(
        matches!(poll!(&mut next), Poll::Pending),
        "producer cannot be polled past the window"
    );
    window.ack();
    ok(adapter.write(decode(json!({"threadId":"thread-e","terminalId":"term-1","data":if cfg!(windows) {"echo ack-^released\r\n"} else {"printf 'ack-%s\\n' released\n"}}))).await);
    let event = ok(next.await.unwrap());
    assert!(matches!(
        event,
        TerminalAttachStreamEvent::TerminalOutputEvent(_)
            | TerminalAttachStreamEvent::TerminalActivityEvent(_)
    ));
    drop(stream);
    drop(a);
    drop(b);
    adapter.manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_shutdown_is_silent_and_persists_final_output_for_resume() {
    let temp = tempfile::tempdir().unwrap();
    let adapter = terminal(temp.path());
    let input: TerminalOpenInput =
        decode(json!({"threadId":"thread-e","terminalId":"term-1","cwd":temp.path()}));
    ok(adapter.open(input).await);
    let mut metadata = adapter.manager.subscribe_metadata();
    metadata.recv().await.unwrap();
    let mut output = ok(adapter
        .observe(decode(json!({"threadId":"thread-e","terminalId":"term-1"})))
        .await);
    output.recv().await.unwrap().unwrap();
    ok(adapter.write(decode(json!({"threadId":"thread-e","terminalId":"term-1","data":if cfg!(windows) {"echo persisted-^marker\r\n"} else {"printf 'persisted-%s\\n' marker\n"}}))).await);
    let mut received = String::new();
    loop {
        if let TerminalAttachStreamEvent::TerminalOutputEvent(event) =
            output.recv().await.unwrap().unwrap()
        {
            received.push_str(&event.data);
            if received.contains("persisted-marker") {
                break;
            }
        }
    }
    adapter.manager.shutdown().await.unwrap();
    while let Some(event) = metadata.recv().await {
        assert!(!matches!(
            event,
            TerminalMetadataStreamEvent::TerminalMetadataRemoveEvent(_)
        ));
    }
    while let Some(event) = output.recv().await.unwrap() {
        assert!(!matches!(
            event,
            TerminalAttachStreamEvent::TerminalClosedEvent(_)
        ));
    }
    drop(output);
    drop(metadata);
    drop(adapter);
    let resumed = terminal(temp.path());
    let mut stream = ok(resumed.attach(decode(json!({"threadId":"thread-e","terminalId":"term-1","cwd":temp.path(),"resume":true})), "socket-resumed".into()).await);
    let event = stream.recv().await.unwrap().unwrap();
    let TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(event) = event else {
        panic!("resume snapshot")
    };
    assert!(
        event.snapshot.history.contains("persisted-marker"),
        "{}",
        event.snapshot.history
    );
    drop(stream);
    resumed.manager.shutdown().await.unwrap();
}

/// t3-git H-2: a restore's hold and a terminal start never interleave. A start during the hold
/// waits for it to drop; a hold taken while a start is in flight waits for the start and names
/// the terminal, so the restore refuses. Ordered by polling, not by time.
#[cfg(feature = "orchestration")]
#[tokio::test]
async fn a_restore_hold_and_a_terminal_start_never_interleave() {
    use t3_contracts::base_schemas::ThreadId;
    use t3_orchestration::api::ThreadWriters;
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("work");
    std::fs::create_dir_all(&folder).unwrap();
    let adapter = terminal(temp.path());
    let writers = t3_server::adapters::terminal::TerminalWriters(adapter.clone());
    let open = |thread: &str, terminal: &str| {
        decode::<TerminalOpenInput>(json!({"threadId":thread,"terminalId":terminal,"cwd":folder}))
    };

    let hold = writers
        .hold(ThreadId("thread-h".into()), folder.clone())
        .await
        .unwrap();
    assert!(hold.running.is_empty(), "{:?}", hold.running);
    let mut late = Box::pin(adapter.open(open("thread-h", "late")));
    assert!(matches!(poll!(&mut late), Poll::Pending));
    let mut metadata = adapter.manager.subscribe_metadata();
    let Some(TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(snapshot)) =
        metadata.recv().await
    else {
        panic!("metadata snapshot expected");
    };
    assert!(snapshot.terminals.is_empty(), "{:?}", snapshot.terminals);
    drop(hold);
    ok(late.await);

    let mut early = Box::pin(adapter.open(open("thread-i", "early")));
    let admitted = poll!(&mut early);
    let mut hold = Box::pin(writers.hold(ThreadId("thread-i".into()), folder.clone()));
    match admitted {
        Poll::Ready(result) => {
            ok(result);
        }
        Poll::Pending => {
            assert!(matches!(poll!(&mut hold), Poll::Pending));
            ok(early.await);
        }
    }
    let hold = hold.await.unwrap();
    // `early` for its thread and `late` for the folder.
    assert_eq!(hold.running.len(), 2, "{:?}", hold.running);
    assert!(hold.running.iter().any(|name| name.contains("early")), "{:?}", hold.running);
    drop(hold);
    adapter.manager.shutdown().await.unwrap();
}
