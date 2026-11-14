use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, Ordering},
};
use t3_contracts::{
    base_schemas::*, secret_reference::SecretReference, secret_request::*, settings_secret_value::*,
};
use t3_host::{Result, background::Clock, secret_requests::*, secrets::*};
use tokio::sync::Mutex;
struct Time(AtomicI64);
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Resolver {
    value: Mutex<Option<Secret>>,
    applied: Mutex<Option<Secret>>,
    fresh: AtomicBool,
    fail_read: AtomicBool,
    fail_label: AtomicBool,
    fail_refresh: AtomicBool,
}
#[async_trait]
impl SecretBackend for Resolver {
    async fn read(&self, refs: &[SecretReference], fresh: bool) -> Result<Vec<Option<Secret>>> {
        if self.fail_read.load(Ordering::SeqCst) {
            return Err(t3_host::Error::Service);
        }
        self.fresh.store(fresh, Ordering::SeqCst);
        Ok(vec![self.value.lock().await.clone(); refs.len()])
    }
    async fn label(&self, _: &SecretReference) -> Result<String> {
        if self.fail_label.load(Ordering::SeqCst) {
            return Err(t3_host::Error::Service);
        }
        Ok("Locked vault".into())
    }
    async fn apply_secret_and_refresh(&self, _: &SecretReference, value: &Secret) -> Result<()> {
        if self.fail_refresh.load(Ordering::SeqCst) {
            return Err(t3_host::Error::Service);
        }
        *self.applied.lock().await = Some(value.clone());
        Ok(())
    }
}
struct Cards {
    status: Mutex<String>,
    fail: AtomicBool,
    file: bool,
}
#[async_trait]
impl RequestCards for Cards {
    async fn read(
        &self,
        thread: &ThreadId,
        item: &TurnItemId,
    ) -> std::result::Result<RequestCard, RequestError> {
        Ok(RequestCard {
            project_id: ProjectId("project".into()),
            thread_id: thread.clone(),
            item_id: item.clone(),
            status: self.status.lock().await.clone(),
            run_active: true,
            file_delivery: self.file,
        })
    }
    async fn record(&self, _: &RequestCard, status: &str) -> std::result::Result<(), RequestError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(RequestError(SecretRequestFailureReason::RecordFailed));
        }
        *self.status.lock().await = status.into();
        Ok(())
    }
}
fn resolver() -> Arc<Resolver> {
    Arc::new(Resolver {
        value: Mutex::new(Some(Secret::new("sensitive-value"))),
        applied: Mutex::new(None),
        fresh: AtomicBool::new(false),
        fail_read: AtomicBool::new(false),
        fail_label: AtomicBool::new(false),
        fail_refresh: AtomicBool::new(false),
    })
}
fn answer(value: &str) -> SecretRequestAnswerInput {
    SecretRequestAnswerInput {
        thread_id: ThreadId("thread".into()),
        turn_item_id: TurnItemId("item".into()),
        answer: SecretRequestAnswer::Save {
            secret: value.into(),
        },
    }
}
#[tokio::test]
async fn fresh_check_refreshes_from_same_value_and_unresolved_references_fail_closed() {
    let backend = resolver();
    let check = SecretCheck::new(backend.clone());
    let reference: SecretReference =
        serde_json::from_value(serde_json::json!({"provider":"vault","reference":"op://v/i/f"}))
            .unwrap();
    assert_eq!(
        check.check(&reference).await.unwrap(),
        SecretSourceCheckResult::Resolved
    );
    assert!(backend.fresh.load(Ordering::SeqCst));
    assert_eq!(
        backend.applied.lock().await.as_ref().unwrap().expose(),
        "sensitive-value"
    );
    *backend.value.lock().await = Some(Secret::new(" \n"));
    let failure = check.check(&reference).await.unwrap();
    assert!(matches!(failure, SecretSourceCheckResult::Failed { .. }));
    let unresolved = check
        .require_settings_secret(&SettingsSecretValue::Reference(reference.clone()))
        .await
        .unwrap_err();
    assert_eq!(
        unresolved.to_string(),
        "Could not read the credential from the secret provider \"Locked vault\". Unlock or fix it in Settings, then try again."
    );
    assert!(format!("{unresolved:?}").contains("SettingsSecretUnresolvedError"));
    let t3_host::Error::SettingsSecretUnresolved(error) = unresolved else {
        panic!("typed unresolved secret error required")
    };
    assert_eq!(
        t3_contracts::stored::to_value(&error).unwrap(),
        serde_json::json!({"_tag":"SettingsSecretUnresolvedError","provider":"Locked vault"})
    );
    assert_eq!(
        check
            .resolve_settings_secrets(&[SettingsSecretValue::Reference(reference.clone())])
            .await
            .unwrap(),
        vec![SettingsSecretValue::Reference(reference)]
    );
}

#[tokio::test]
async fn secret_check_returns_failed_results_when_backend_operations_fail() {
    let backend = resolver();
    let check = SecretCheck::new(backend.clone());
    let reference: SecretReference =
        serde_json::from_value(serde_json::json!({"provider":"vault","reference":"op://v/i/f"}))
            .unwrap();
    backend.fail_read.store(true, Ordering::SeqCst);
    let result = check
        .check(&reference)
        .await
        .expect("source checks must return a result, not fail the RPC");
    assert!(matches!(result, SecretSourceCheckResult::Failed { .. }));
    backend.fail_label.store(true, Ordering::SeqCst);
    let result = check.check(&reference).await.unwrap();
    assert!(
        matches!(result,SecretSourceCheckResult::Failed{ref detail} if detail.contains("vault"))
    );
    backend.fail_read.store(false, Ordering::SeqCst);
    backend.fail_refresh.store(true, Ordering::SeqCst);
    let result = check.check(&reference).await.unwrap();
    assert!(matches!(result, SecretSourceCheckResult::Failed { .. }));
    assert!(!format!("{result:?}").contains("sensitive-value"));
}
#[test]
fn secret_values_are_redacted_in_every_formatter() {
    let secret = Secret::new("sensitive-value");
    for rendered in [
        format!("{secret}"),
        format!("{secret:?}"),
        format!("{secret:#?}"),
        format!("{:?}", answer("sensitive-value")),
        format!(
            "{:?}",
            SettingsSecretValue::Literal("sensitive-value".into())
        ),
    ] {
        assert!(!rendered.contains("sensitive-value"));
        assert!(rendered.contains("REDACTED"));
    }
    let error = RequestError(SecretRequestFailureReason::StoreFailed);
    assert_eq!(format!("{error}"), "secret request failed: StoreFailed");
    assert!(!format!("{error:?}").contains("sensitive-value"));
}
#[tokio::test]
async fn refs_are_project_bound_one_use_and_stable_across_service_restart() {
    let temp = tempfile::tempdir().unwrap();
    let time = Arc::new(Time(AtomicI64::new(100)));
    let cards = Arc::new(Cards {
        status: Mutex::new("pending".into()),
        fail: AtomicBool::new(false),
        file: false,
    });
    let backend = resolver();
    let requests = SecretRequests::new(
        temp.path().into(),
        time.clone(),
        cards.clone(),
        backend.clone(),
    )
    .unwrap();
    requests.answer(answer("sensitive-value")).await.unwrap();
    let reference = requests
        .saved_ref(&ThreadId("thread".into()), &TurnItemId("item".into()))
        .unwrap()
        .unwrap();
    assert!(
        requests
            .consume(&reference, &ProjectId("other".into()))
            .await
            .is_err()
    );
    let restarted = SecretRequests::new(temp.path().into(), time, cards, backend).unwrap();
    assert_eq!(
        restarted
            .ref_for(&ThreadId("thread".into()), "item")
            .unwrap(),
        reference
    );
    let project = ProjectId("project".into());
    let (a, b) = tokio::join!(
        restarted.consume(&reference, &project),
        restarted.consume(&reference, &project)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(a.or(b).unwrap().expose(), "sensitive-value");
    assert!(
        restarted
            .consume("secret-ref:../../escape", &ProjectId("project".into()))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn expiration_and_failed_recording_remove_values() {
    let temp = tempfile::tempdir().unwrap();
    let time = Arc::new(Time(AtomicI64::new(100)));
    let cards = Arc::new(Cards {
        status: Mutex::new("pending".into()),
        fail: AtomicBool::new(true),
        file: false,
    });
    let service =
        SecretRequests::new(temp.path().into(), time.clone(), cards.clone(), resolver()).unwrap();
    assert!(service.answer(answer("first")).await.is_err());
    assert!(
        service
            .saved_ref(&ThreadId("thread".into()), &TurnItemId("item".into()))
            .unwrap()
            .is_none()
    );
    cards.fail.store(false, Ordering::SeqCst);
    service.answer(answer("second")).await.unwrap();
    let reference = service.ref_for(&ThreadId("thread".into()), "item").unwrap();
    time.0.store(100 + 86_400_001, Ordering::SeqCst);
    assert!(matches!(
        service
            .consume(&reference, &ProjectId("project".into()))
            .await
            .unwrap_err()
            .0,
        SecretRequestFailureReason::RefExpired
    ));
    assert!(
        service
            .saved_ref(&ThreadId("thread".into()), &TurnItemId("item".into()))
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn pasted_files_are_private_idempotent_and_namespaced() {
    let temp = tempfile::tempdir().unwrap();
    let service = SecretRequests::new(
        temp.path().into(),
        Arc::new(Time(AtomicI64::new(100))),
        Arc::new(Cards {
            status: Mutex::new("pending".into()),
            fail: AtomicBool::new(false),
            file: true,
        }),
        resolver(),
    )
    .unwrap();
    let thread = ThreadId("thread".into());
    let path = service
        .save_pasted(&thread, "session-a", "request", &Secret::new("first"))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    assert_eq!(
        service
            .save_pasted(&thread, "session-a", "request", &Secret::new("second"))
            .await
            .unwrap(),
        path
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    assert_ne!(
        service
            .save_pasted(&thread, "session-b", "request", &Secret::new("third"))
            .await
            .unwrap(),
        path
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(-86_401, 0)).unwrap();
    let partial = temp
        .path()
        .join("files")
        .join(format!("{}.partial", "a".repeat(32)));
    let legacy = temp.path().join("files/.tmpABC123");
    let recent = temp
        .path()
        .join("files")
        .join(format!("{}.partial", "b".repeat(32)));
    for expired in [&partial, &legacy] {
        std::fs::write(expired, "crash left plaintext").unwrap();
        filetime::set_file_mtime(expired, filetime::FileTime::from_unix_time(-86_401, 0)).unwrap();
    }
    std::fs::write(&recent, "still in flight").unwrap();
    filetime::set_file_mtime(&recent, filetime::FileTime::from_unix_time(0, 0)).unwrap();
    let service = Arc::new(service);
    let _worker = service.start().await;
    assert!(!path.exists());
    assert!(!partial.exists() && !legacy.exists());
    assert!(recent.exists());
}

#[tokio::test]
async fn expiry_sweep_continues_after_an_undeletable_ref() {
    let dir = tempfile::tempdir().unwrap();
    let service = SecretRequests::new(
        dir.path().into(),
        Arc::new(Time(AtomicI64::new(86_400_001))),
        Arc::new(Cards {
            status: Mutex::new("pending".into()),
            fail: AtomicBool::new(false),
            file: false,
        }),
        resolver(),
    )
    .unwrap();
    let blocked = dir
        .path()
        .join(format!("secret-request-{}.bin", "0".repeat(32)));
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), "do not recurse").unwrap();
    let expired = dir
        .path()
        .join(format!("secret-request-{}.bin", "f".repeat(32)));
    std::fs::write(&expired, "malformed expired value").unwrap();
    assert_eq!(
        service
            .sweep_expired()
            .await
            .expect("one ref deletion failure must not stop other entries"),
        1
    );
    assert!(blocked.join("keep").exists());
    assert!(!expired.exists());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let files = dir.path().join("files");
        std::fs::create_dir(&files).unwrap();
        let locked = files.join("1".repeat(32));
        let next = files.join("e".repeat(32));
        for path in [&locked, &next] {
            std::fs::write(path, "expired plaintext").unwrap();
            filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(0, 0)).unwrap();
        }
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        assert_eq!(
            service
                .sweep_expired()
                .await
                .expect("one locked plaintext file must not stop the sweep"),
            1
        );
        assert!(locked.exists());
        assert!(!next.exists());
    }
}
