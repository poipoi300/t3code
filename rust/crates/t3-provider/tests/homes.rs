#![allow(clippy::unwrap_used)]
use t3_provider::{environment::codex_home_layout, homes::*, sensitive::ServiceError};
#[test]
fn overlay_shares_state_and_preserves_private_authentication_and_live_writer_locks() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared");
    let shadow = temp.path().join("shadow");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::create_dir_all(shadow.join("thread-writer-locks")).unwrap();
    std::fs::write(shared.join("auth.json"), "shared-token").unwrap();
    std::fs::write(shadow.join("auth.json"), "private-token").unwrap();
    std::fs::write(shared.join("config.toml"), "shared config").unwrap();
    std::fs::write(shadow.join("thread-writer-locks/active.lock"), "live").unwrap();
    let layout = codex_home_layout(
        &shared.to_string_lossy(),
        &shadow.to_string_lossy(),
        temp.path(),
        temp.path(),
    );
    assert_eq!(
        materialize_codex_shadow(&layout).unwrap(),
        [HomeWarning::LiveWriterLocks]
    );
    assert_eq!(
        std::fs::read_to_string(shadow.join("auth.json")).unwrap(),
        "private-token"
    );
    assert_eq!(
        std::fs::read_to_string(shadow.join("config.toml")).unwrap(),
        "shared config"
    );
    assert!(shadow.join("thread-writer-locks/active.lock").exists());
    std::fs::remove_file(shadow.join("thread-writer-locks/active.lock")).unwrap();
    materialize_codex_shadow(&layout).unwrap();
    assert!(
        std::fs::symlink_metadata(shadow.join("thread-writer-locks"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[test]
fn non_link_collisions_and_the_same_canonical_home_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared");
    let shadow = temp.path().join("shadow");
    std::fs::create_dir_all(shadow.join("sessions")).unwrap();
    let layout = codex_home_layout(
        &shared.to_string_lossy(),
        &shadow.to_string_lossy(),
        temp.path(),
        temp.path(),
    );
    assert_eq!(
        materialize_codex_shadow(&layout).unwrap_err(),
        ServiceError::Storage
    );
    let layout = codex_home_layout(
        &shared.to_string_lossy(),
        &shared.to_string_lossy(),
        temp.path(),
        temp.path(),
    );
    assert_eq!(
        materialize_codex_shadow(&layout).unwrap_err(),
        ServiceError::Identity
    );
}
