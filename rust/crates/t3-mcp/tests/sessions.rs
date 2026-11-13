#![allow(clippy::unwrap_used)]
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    provider_instance::ProviderInstanceId,
};
use t3_mcp::{
    session::{AliveMark, Clock, endpoint},
    *,
};
struct TestClock(Mutex<AliveMark>);
impl Clock for TestClock {
    fn now(&self) -> AliveMark {
        *self.0.lock().unwrap()
    }
}
fn request(thread: &str) -> CredentialRequest {
    CredentialRequest {
        thread_id: ThreadId(thread.into()),
        provider_instance_id: ProviderInstanceId("codex".into()),
        browser_tools_available: true,
        capabilities: None,
        subagent: false,
    }
}

#[test]
fn registry_liveness_rotation_capabilities_and_revocation_match_typescript() {
    let clock = Arc::new(TestClock(Mutex::new(AliveMark {
        wall_ms: 1000,
        monotonic_ms: 0,
    })));
    let registry =
        SessionRegistry::with_clock(EnvironmentId("env".into()), None, 100, clock.clone());
    let first = registry.issue(request("thread")).unwrap();
    let token = first.authorization_header.strip_prefix("Bearer ").unwrap();
    assert_eq!(token.len(), 43);
    assert!(!format!("{first:?}").contains(token));
    assert!(registry.resolve("").is_none());
    assert!(registry.resolve("unknown").is_none());
    assert!(
        registry
            .resolve(token)
            .unwrap()
            .capabilities
            .contains(&Capability::Preview)
    );
    let mut other = request("other");
    other.browser_tools_available = false;
    other.capabilities = Some(BTreeSet::from([Capability::Device]));
    let other = registry.issue(other).unwrap();
    assert!(!other.browser_tools_available);
    assert!(other.scope.capabilities.contains(&Capability::PullRequests));
    assert!(other.scope.capabilities.contains(&Capability::Device));
    *clock.0.lock().unwrap() = AliveMark {
        wall_ms: 900,
        monotonic_ms: 101,
    };
    assert!(registry.resolve(token).is_none());
    *clock.0.lock().unwrap() = AliveMark {
        wall_ms: 1000,
        monotonic_ms: 0,
    };
    assert!(registry.resolve(token).is_none());
    registry.touch(&ThreadId("other".into())).unwrap();
    assert!(registry.resolve(token).is_none());
    registry.touch(&ThreadId("thread".into())).unwrap();
    assert!(registry.resolve(token).is_some());
    let second = registry.issue(request("thread")).unwrap();
    registry
        .revoke_provider_session(&first.scope.thread.as_ref().unwrap().provider_session_id)
        .unwrap();
    registry.touch(&ThreadId("thread".into())).unwrap();
    assert!(registry.resolve(token).is_none());
    assert!(
        registry
            .resolve(second.authorization_header.strip_prefix("Bearer ").unwrap())
            .is_some()
    );
    registry.revoke_thread(&ThreadId("thread".into())).unwrap();
    registry.touch(&ThreadId("thread".into())).unwrap();
    assert!(
        registry
            .resolve(second.authorization_header.strip_prefix("Bearer ").unwrap())
            .is_none()
    );
    registry.revoke_all().unwrap();
    registry.touch(&ThreadId("other".into())).unwrap();
    assert!(
        registry
            .resolve(other.authorization_header.strip_prefix("Bearer ").unwrap())
            .is_none()
    );
}

#[test]
fn endpoints_use_bound_hosts_and_loopback_for_unspecified_addresses() {
    for (address, expected) in [
        ("0.0.0.0:3000", "http://127.0.0.1:3000/mcp"),
        ("[::]:3000", "http://127.0.0.1:3000/mcp"),
        ("[::1]:3000", "http://[::1]:3000/mcp"),
        ("192.168.1.4:80", "http://192.168.1.4:80/mcp"),
    ] {
        assert_eq!(endpoint(Some(address.parse().unwrap())), expected);
    }
    assert_eq!(endpoint(None), "http://127.0.0.1/mcp");
}
