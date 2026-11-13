use futures_util::FutureExt;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_host::keep_awake::{hold_with_grace, inhibitor_command};

struct Held(Arc<Mutex<Vec<&'static str>>>);
impl Drop for Held {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("released");
    }
}
#[tokio::test(start_paused = true)]
async fn false_releases_after_grace_true_reuses_the_hold_and_channel_end_releases_immediately() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = tokio::sync::watch::channel(true);
    let task = hold_with_grace(
        rx,
        || async {
            events.lock().unwrap().push("held");
            let _held = Held(events.clone());
            std::future::pending::<()>().await;
        },
        Duration::from_secs(60),
    );
    tokio::pin!(task);
    assert!(task.as_mut().now_or_never().is_none());
    assert_eq!(*events.lock().unwrap(), vec!["held"]);
    tx.send(false).unwrap();
    assert!(task.as_mut().now_or_never().is_none());
    tokio::time::advance(Duration::from_secs(59)).await;
    assert!(task.as_mut().now_or_never().is_none());
    assert_eq!(*events.lock().unwrap(), vec!["held"]);
    tx.send(true).unwrap();
    assert!(task.as_mut().now_or_never().is_none());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(task.as_mut().now_or_never().is_none());
    assert_eq!(*events.lock().unwrap(), vec!["held"]);
    tx.send(false).unwrap();
    assert!(task.as_mut().now_or_never().is_none());
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(task.as_mut().now_or_never().is_none());
    assert_eq!(*events.lock().unwrap(), vec!["held", "released"]);
    tx.send(true).unwrap();
    assert!(task.as_mut().now_or_never().is_none());
    assert_eq!(*events.lock().unwrap(), vec!["held", "released", "held"]);
    drop(tx);
    task.await;
    assert_eq!(
        *events.lock().unwrap(),
        vec!["held", "released", "held", "released"]
    );
}
#[test]
fn inhibitor_commands_match_platform_lifetime_and_no_display_sleep_flags() {
    let mac = inhibitor_command("darwin", 123).unwrap();
    assert_eq!(mac.program, "caffeinate");
    assert_eq!(mac.args, ["-i", "-w", "123"]);
    let linux = inhibitor_command("linux", 123).unwrap();
    assert_eq!(linux.program, "systemd-inhibit");
    assert_eq!(
        linux.args,
        [
            "--what=idle:sleep",
            "--who=T3 Code",
            "--why=Agents are working",
            "--mode=block",
            "tail",
            "--pid=123",
            "-f",
            "/dev/null"
        ]
    );
    let win = inhibitor_command("win32", 123).unwrap();
    assert_eq!(win.program, "powershell.exe");
    assert!(
        win.args
            .last()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("[uint32]2147483649); Wait-Process -Id 123")
    );
    assert!(inhibitor_command("other", 123).is_none());
}
