#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::server::ServerProviderState;
use t3_provider::{
    discovery::{Driver, VersionProbe},
    environment::Environment,
    native_provider::ExecutableProbe,
    opencode_service::*,
    sensitive::*,
};
use tokio::sync::{Mutex, mpsc, oneshot};
#[derive(Debug)]
enum Event {
    Version,
    Discover(PathBuf, Option<String>),
    Resolve,
    Stop,
    Ensure(ServiceStart),
}
struct Backend {
    events: mpsc::UnboundedSender<Event>,
    version: String,
    url: String,
    current: bool,
    shell: bool,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
#[async_trait]
impl ExecutableProbe for Backend {
    async fn version(
        &self,
        driver: Driver,
        binary: &str,
        _: &Environment,
        _: &Path,
    ) -> ServiceResult<VersionProbe> {
        assert_eq!(driver, Driver::OpenCode);
        assert_eq!(binary, "opencode");
        self.events.send(Event::Version).unwrap();
        Ok(VersionProbe {
            installed: true,
            version: Some(self.version.clone()),
            status: ServerProviderState::Ready,
            message: None,
            resolved_executable: None,
        })
    }
}
#[async_trait]
impl OpenCodeServiceBackend for Backend {
    async fn discover(
        &self,
        file: &Path,
        version: Option<&str>,
    ) -> ServiceResult<Option<RegisteredService>> {
        self.events
            .send(Event::Discover(file.into(), version.map(str::to_owned)))
            .unwrap();
        Ok(
            (version.is_none() || self.current).then(|| RegisteredService {
                url: self.url.clone(),
            }),
        )
    }
    async fn wrap_resolved_command(
        &self,
        _: &str,
        _: &Environment,
    ) -> ServiceResult<Option<ServiceCommand>> {
        self.events.send(Event::Resolve).unwrap();
        Ok(Some(ServiceCommand {
            executable: "owned-scope-wrapper".into(),
            args: vec!["opencode".into(), "serve".into(), "--service".into()],
            shell: self.shell,
        }))
    }
    async fn stop_handoff(&self, _: &Path) -> ServiceResult<()> {
        self.events.send(Event::Stop).unwrap();
        if let Some(release) = self.release.lock().await.take() {
            release.await.unwrap();
        }
        Ok(())
    }
    async fn ensure(&self, _: &Path, start: ServiceStart) -> ServiceResult<()> {
        assert!(!format!("{start:?}").contains("never-environment-secret"));
        self.events.send(Event::Ensure(start)).unwrap();
        Ok(())
    }
}
fn environment(root: &Path) -> Environment {
    Environment::from([
        ("HOME".into(), root.to_string_lossy().into_owned()),
        (
            "XDG_STATE_HOME".into(),
            root.join("state").to_string_lossy().into_owned(),
        ),
        (
            "OPENCODE_CONFIG_DIR".into(),
            root.join("config").to_string_lossy().into_owned(),
        ),
        ("PATH".into(), "inherited".into()),
        ("ELECTRON_RUN_AS_NODE".into(), "1".into()),
    ])
}
#[test]
fn registered_service_matching_keeps_loopback_address_families_separate() {
    for (registered, configured, expected) in [
        ("http://0.0.0.0:4096", "http://127.0.0.1:4096/path", true),
        ("http://[::]:4096", "http://[::1]:4096", true),
        ("http://0.0.0.0:4096", "http://localhost:4096", false),
        ("http://[::]:4096", "http://127.0.0.1:4096", false),
        ("http://127.0.0.1:4096", "https://127.0.0.1:4096", false),
        ("http://127.0.0.1:4096", "http://127.0.0.1:4097", false),
        ("invalid", "invalid", false),
    ] {
        assert_eq!(registered_address(registered, configured), expected);
    }
}
#[tokio::test]
async fn replacement_validates_registration_and_version_and_preserves_state_and_private_environment()
 {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config/service.json"),serde_json::to_vec(&json!({"env":{"PATH":"configured","XDG_STATE_HOME":"must-not-move","ELECTRON_RUN_AS_NODE":"2","KEY":"never-environment-secret"}})).unwrap()).unwrap();
    for (version, url, current, shell, replace) in [
        ("2.1.0", "http://0.0.0.0:4096", false, false, true),
        ("2.1.0", "http://other.invalid:4096", false, false, false),
        ("2.1.0", "http://0.0.0.0:4096", true, false, false),
        ("2.1.0", "http://0.0.0.0:4096", false, true, false),
        ("1.14.19", "http://0.0.0.0:4096", false, false, false),
    ] {
        let (events, mut received) = mpsc::unbounded_channel();
        let backend = Arc::new(Backend {
            events,
            version: version.into(),
            url: url.into(),
            current,
            shell,
            release: Mutex::new(None),
        });
        let replacement = OpenCodeServiceReplacement::new(
            backend.clone(),
            backend,
            root.path().into(),
            root.path().into(),
        );
        let environment = environment(root.path());
        replacement
            .replace("opencode", "http://127.0.0.1:4096", &environment)
            .await
            .unwrap();
        let mut starts = vec![];
        let mut stopped = false;
        while let Ok(event) = received.try_recv() {
            match event {
                Event::Ensure(start) => {
                    assert!(stopped);
                    starts.push(start)
                }
                Event::Stop => stopped = true,
                Event::Discover(file, expected) => {
                    assert_eq!(file, root.path().join("state/opencode/service.json"));
                    if let Some(expected) = expected {
                        assert_eq!(expected, version);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(stopped, replace);
        assert_eq!(starts.len(), usize::from(replace));
        if let Some(start) = starts.pop() {
            assert_eq!(start.command.args, ["opencode", "serve", "--service"]);
            let environment = start.environment.expose();
            assert_eq!(environment["PATH"], "configured");
            assert_eq!(
                environment["XDG_STATE_HOME"],
                root.path().join("state").to_string_lossy()
            );
            assert!(!environment.contains_key("ELECTRON_RUN_AS_NODE"));
            assert_eq!(environment["KEY"], "never-environment-secret");
        }
    }
}
#[tokio::test]
async fn rpc_cancellation_after_the_handoff_stop_cannot_abandon_the_replacement_start() {
    let root = tempfile::tempdir().unwrap();
    let (events, mut received) = mpsc::unbounded_channel();
    let (release, blocked) = oneshot::channel();
    let backend = Arc::new(Backend {
        events,
        version: "2.1.0".into(),
        url: "http://127.0.0.1:4096".into(),
        current: false,
        shell: false,
        release: Mutex::new(Some(blocked)),
    });
    let replacement = Arc::new(OpenCodeServiceReplacement::new(
        backend.clone(),
        backend,
        root.path().into(),
        root.path().into(),
    ));
    let request = tokio::spawn({
        let replacement = replacement.clone();
        let environment = environment(root.path());
        async move {
            replacement
                .replace("opencode", "http://127.0.0.1:4096", &environment)
                .await
        }
    });
    loop {
        if matches!(received.recv().await.unwrap(), Event::Stop) {
            break;
        }
    }
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    assert!(matches!(received.recv().await.unwrap(), Event::Ensure(_)));
}
