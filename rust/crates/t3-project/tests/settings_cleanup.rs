#![allow(clippy::unwrap_used)]
mod support;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use support::Fixture;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn settings_cleanup_failure_warns_with_project_id_without_undoing_deletion() {
    let f = Fixture::new();
    let root = f.root("repo");
    f.service
        .create(f.create("cleanup-failure", "create-cleanup", &root))
        .await
        .unwrap();
    std::fs::create_dir(f.temp.path().join("settings.json")).unwrap();
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(capture.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let deleted = f
        .service
        .delete(f.delete("cleanup-failure", "delete-cleanup", false))
        .await
        .unwrap();
    assert!(deleted.deleted_at.is_some());
    assert!(root.is_dir());
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("WARN")
            && logs.contains("cleanup-failure")
            && logs.contains("Failed to clear settings overrides"),
        "missing project cleanup warning: {logs}"
    );
    f.shutdown().await;
}
