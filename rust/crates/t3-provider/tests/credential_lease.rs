#![allow(clippy::unwrap_used)]
use std::time::Duration;
use t3_provider::{
    credential_lease::CredentialLease,
    sensitive::{ServiceError, ServiceResult},
};

#[tokio::test(start_paused = true)]
async fn leases_use_the_node_lock_directory_and_serialize_until_release() {
    let directory = tempfile::tempdir().unwrap();
    let first = CredentialLease::acquire(directory.path(), "profile")
        .await
        .unwrap();
    assert!(directory.path().join("profile.bin.lock").is_dir());
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            CredentialLease::acquire(directory.path(), "profile")
        )
        .await
        .is_err()
    );
    first.check().unwrap();
    drop(first);
    let second = CredentialLease::acquire(directory.path(), "profile")
        .await
        .unwrap();
    second.check().unwrap();
    drop(second);
    assert!(!directory.path().join("profile.bin.lock").exists());
}

#[tokio::test]
async fn a_replaced_lock_is_detected_and_never_removed_by_the_old_owner() {
    let directory = tempfile::tempdir().unwrap();
    let first = CredentialLease::acquire(directory.path(), "profile")
        .await
        .unwrap();
    let path = directory.path().join("profile.bin.lock");
    std::fs::rename(&path, directory.path().join("old-lock")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(first.check().unwrap_err(), ServiceError::Storage);
    drop(first);
    assert!(path.is_dir());
}

#[tokio::test]
async fn a_symlink_to_the_original_directory_does_not_retain_lease_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let lease = CredentialLease::acquire(directory.path(), "profile")
        .await
        .unwrap();
    let path = directory.path().join("profile.bin.lock");
    let moved = directory.path().join("moved-lock");
    std::fs::rename(&path, &moved).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&moved, &path).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&moved, &path).unwrap();
    assert_eq!(lease.check().unwrap_err(), ServiceError::Storage);
    drop(lease);
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[tokio::test(start_paused = true)]
async fn compromise_interrupts_an_in_flight_operation_before_it_can_publish() {
    let directory = tempfile::tempdir().unwrap();
    let lease = CredentialLease::acquire(directory.path(), "profile")
        .await
        .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (dropped, cancelled) = tokio::sync::oneshot::channel();
    struct OnDrop(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let task = tokio::spawn(async move {
        lease
            .run(async {
                let _operation = OnDrop(Some(dropped));
                started.send(()).unwrap();
                std::future::pending::<ServiceResult<()>>().await
            })
            .await
    });
    ready.await.unwrap();
    let path = directory.path().join("profile.bin.lock");
    std::fs::rename(&path, directory.path().join("old-lock")).unwrap();
    std::fs::create_dir(&path).unwrap();
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(task.await.unwrap().unwrap_err(), ServiceError::Storage);
    cancelled.await.unwrap();
    assert!(path.is_dir());
}
