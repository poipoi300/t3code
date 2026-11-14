#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use t3_contracts::desktop_browser::*;
use t3_preview::{Result, desktop::*, driver::TabKey};
use tokio::sync::{Notify, mpsc};
struct Ipc {
    commands: mpsc::UnboundedSender<DesktopBrowserCommand>,
    inbound: Mutex<Vec<mpsc::UnboundedReceiver<String>>>,
    stopped: Mutex<Vec<String>>,
    blocked: AtomicBool,
    relaying: Notify,
}
#[async_trait]
impl DesktopIpc for Ipc {
    async fn stop_loading(&self, tab_id: &str) -> Result<()> {
        self.stopped.lock().unwrap().push(tab_id.into());
        Ok(())
    }
    async fn send(&self, command: DesktopBrowserCommand) -> Result<()> {
        let _ = self.commands.send(command);
        Ok(())
    }
    async fn relay(
        &self,
        _key: TabKey,
        incoming: mpsc::UnboundedReceiver<String>,
    ) -> Result<DesktopEndpoint> {
        self.inbound.lock().unwrap().push(incoming);
        self.relaying.notify_one();
        if self.blocked.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(DesktopEndpoint {
            url: "ws://127.0.0.1:1234/private".into(),
            lease: Box::new(()),
        })
    }
}
fn fixture() -> (
    DesktopBrowserChannel,
    Arc<Ipc>,
    mpsc::UnboundedReceiver<DesktopBrowserCommand>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let ipc = Arc::new(Ipc {
        commands: tx,
        inbound: Mutex::default(),
        stopped: Mutex::default(),
        blocked: AtomicBool::new(false),
        relaying: Notify::new(),
    });
    (DesktopBrowserChannel::new(ipc.clone()), ipc, rx)
}
fn attached(channel: &DesktopBrowserChannel) {
    channel.receive(DesktopBrowserEvent::Attached {
        thread_id: "t".into(),
        tab_id: "tab".into(),
    });
}
#[tokio::test]
async fn desktop_cdp_has_one_lease_and_forwards_only_the_matching_tab() {
    let (channel, ipc, mut commands) = fixture();
    let key = TabKey::new("t", "tab");
    assert!(channel.endpoint(&key).await.is_err());
    attached(&channel);
    let connection = channel.endpoint(&key).await.unwrap();
    assert!(channel.endpoint(&key).await.is_err());
    channel.receive(DesktopBrowserEvent::Cdp {
        thread_id: "other".into(),
        tab_id: "tab".into(),
        message: "wrong".into(),
    });
    channel.receive(DesktopBrowserEvent::Cdp {
        thread_id: "t".into(),
        tab_id: "tab".into(),
        message: "reply".into(),
    });
    let mut incoming = ipc.inbound.lock().unwrap().pop().unwrap();
    assert_eq!(incoming.recv().await.unwrap(), "reply");
    assert!(incoming.try_recv().is_err());
    drop(connection);
    assert!(matches!(
        commands.recv().await.unwrap(),
        DesktopBrowserCommand::Release { .. }
    ));
    assert!(incoming.recv().await.is_none());
    let _replacement = channel.endpoint(&key).await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn stalled_or_cancelled_relays_release_registration_and_allow_reconnection() {
    let (channel, ipc, mut commands) = fixture();
    attached(&channel);
    let key = TabKey::new("t", "tab");
    ipc.blocked.store(true, Ordering::SeqCst);
    let task = {
        let channel = channel.clone();
        let key = key.clone();
        tokio::spawn(async move { channel.endpoint(&key).await })
    };
    ipc.relaying.notified().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(task.await.unwrap().is_err());
    assert!(matches!(
        commands.recv().await.unwrap(),
        DesktopBrowserCommand::Release { .. }
    ));
    let task = {
        let channel = channel.clone();
        let key = key.clone();
        tokio::spawn(async move { channel.endpoint(&key).await })
    };
    ipc.relaying.notified().await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert!(matches!(
        commands.recv().await.unwrap(),
        DesktopBrowserCommand::Release { .. }
    ));
    ipc.blocked.store(false, Ordering::SeqCst);
    let _connection = channel.endpoint(&key).await.unwrap();
}
#[tokio::test]
async fn stale_desktop_lease_cannot_remove_a_replacement_after_detach() {
    let (channel, _ipc, mut commands) = fixture();
    attached(&channel);
    let key = TabKey::new("t", "tab");
    let old = channel.endpoint(&key).await.unwrap();
    channel.receive(DesktopBrowserEvent::Detached {
        thread_id: "t".into(),
        tab_id: "tab".into(),
    });
    attached(&channel);
    let replacement = channel.endpoint(&key).await.unwrap();
    drop(old);
    assert!(commands.try_recv().is_err());
    assert!(channel.endpoint(&key).await.is_err());
    drop(replacement);
    assert!(matches!(
        commands.recv().await.unwrap(),
        DesktopBrowserCommand::Release { .. }
    ));
}

#[tokio::test]
async fn desktop_stop_targets_the_guest_without_releasing_the_cdp_connection() {
    let (channel, ipc, mut commands) = fixture();
    let key = TabKey::new("t", "tab");
    assert!(channel.stop_loading(&key).await.is_err());
    attached(&channel);
    let _connection = channel.endpoint(&key).await.unwrap();
    channel.stop_loading(&key).await.unwrap();
    assert_eq!(*ipc.stopped.lock().unwrap(), vec!["tab"]);
    assert!(channel.is_attached(&key));
    assert!(commands.try_recv().is_err());
    channel.receive(DesktopBrowserEvent::Cdp {
        thread_id: "t".into(),
        tab_id: "tab".into(),
        message: "still connected".into(),
    });
    assert_eq!(
        ipc.inbound.lock().unwrap()[0].try_recv().unwrap(),
        "still connected"
    );
    assert_eq!(PREVIEW_STOP_LOADING_CHANNEL, "desktop:preview-stop-loading");
}
