use crate::{session::CredentialRequest, *};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};
use t3_contracts::base_schemas::{ProviderSessionId, ThreadId};

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn resolve(&self, token: &str) -> Option<InvocationScope>;
}
#[async_trait]
impl CredentialResolver for SessionRegistry {
    async fn resolve(&self, token: &str) -> Option<InvocationScope> {
        SessionRegistry::resolve(self, token)
    }
}

type Key = (ThreadId, String);
#[derive(Default)]
struct State {
    configs: HashMap<ThreadId, ProviderSessionConfig>,
    reservations: HashMap<Key, usize>,
    owners: HashSet<(ProviderSessionId, Key)>,
    gates: HashMap<ThreadId, Weak<tokio::sync::Mutex<()>>>,
}
struct Inner {
    registry: Arc<SessionRegistry>,
    resolver: Arc<dyn CredentialResolver>,
    state: Mutex<State>,
}

/// MCP credential lifecycle only; native provider processes remain owned by orchestration.
#[derive(Clone)]
pub struct ProviderCredentials(Arc<Inner>);
impl ProviderCredentials {
    pub fn new(registry: Arc<SessionRegistry>) -> Self {
        Self::with_resolver(registry.clone(), registry)
    }
    pub fn with_resolver(
        registry: Arc<SessionRegistry>,
        resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self(Arc::new(Inner {
            registry,
            resolver,
            state: Mutex::new(State::default()),
        }))
    }
    fn gate(&self, thread: &ThreadId) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut state = self.0.state.lock().map_err(|_| McpError::Internal)?;
        state.gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = state.gates.get(thread).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        state.gates.insert(thread.clone(), Arc::downgrade(&gate));
        Ok(gate)
    }
    pub fn read(&self, thread: &ThreadId) -> Result<Option<ProviderSessionConfig>> {
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| McpError::Internal)?
            .configs
            .get(thread)
            .cloned())
    }
    fn reserve(
        &self,
        thread: ThreadId,
        config: ProviderSessionConfig,
        issued: bool,
    ) -> Result<PreparedCredential> {
        let id = config
            .scope
            .thread
            .as_ref()
            .ok_or(McpError::Internal)?
            .provider_session_id
            .clone();
        *self
            .0
            .state
            .lock()
            .map_err(|_| McpError::Internal)?
            .reservations
            .entry((thread.clone(), id.clone()))
            .or_default() += 1;
        Ok(PreparedCredential {
            config,
            issued,
            key: (thread, id),
            inner: self.0.clone(),
            active: true,
            cleanup: false,
        })
    }
    pub async fn prepare(&self, request: CredentialRequest) -> Result<PreparedCredential> {
        let gate = self.gate(&request.thread_id)?;
        let _guard = gate.lock().await;
        let expected_preview = request
            .capabilities
            .as_ref()
            .map_or(request.browser_tools_available, |c| {
                c.contains(&Capability::Preview)
            });
        let expected_device = request
            .capabilities
            .as_ref()
            .is_some_and(|c| c.contains(&Capability::Device));
        if let Some(config) = self.read(&request.thread_id)? {
            let mut reserved = self.reserve(request.thread_id.clone(), config, false)?;
            reserved.cleanup = true;
            let token = reserved
                .config
                .authorization_header
                .strip_prefix("Bearer ")
                .ok_or(McpError::Internal)?;
            let resolved = self.0.resolver.resolve(token).await;
            if resolved.is_some_and(|scope| {
                scope.thread.as_ref().is_some_and(|caller| {
                    caller.thread_id == request.thread_id
                        && caller.provider_instance_id == request.provider_instance_id
                        && caller.subagent == request.subagent
                }) && scope.capabilities.contains(&Capability::Preview) == expected_preview
                    && scope.capabilities.contains(&Capability::Device) == expected_device
            }) {
                reserved.cleanup = true;
                return Ok(reserved);
            }
            drop(reserved);
        }
        let mut state = self.0.state.lock().map_err(|_| McpError::Internal)?;
        self.0.registry.revoke_thread(&request.thread_id)?;
        state.configs.remove(&request.thread_id);
        let thread = request.thread_id.clone();
        let config = self.0.registry.issue(request)?;
        state.configs.insert(thread.clone(), config.clone());
        drop(state);
        let mut prepared = self.reserve(thread, config, true)?;
        prepared.cleanup = true;
        Ok(prepared)
    }
    pub fn touch(&self, thread: &ThreadId) -> Result<()> {
        self.0.registry.touch(thread)
    }
    pub async fn disable(&self, thread: &ThreadId) -> Result<()> {
        let gate = self.gate(thread)?;
        let _guard = gate.lock().await;
        let mut state = self.0.state.lock().map_err(|_| McpError::Internal)?;
        self.0.registry.revoke_thread(thread)?;
        state.configs.remove(thread);
        Ok(())
    }
    pub fn release_runtime(&self, runtime: &ProviderSessionId) -> Result<()> {
        let mut state = self.0.state.lock().map_err(|_| McpError::Internal)?;
        let released = state
            .owners
            .iter()
            .filter(|(owner, _)| owner == runtime)
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>();
        state.owners.retain(|(owner, _)| owner != runtime);
        for key in released {
            clear_unused(&self.0, &mut state, &key)?;
        }
        Ok(())
    }
    pub fn detach(
        &self,
        runtime: &ProviderSessionId,
        thread: &ThreadId,
        id: &str,
        revoke: bool,
    ) -> Result<()> {
        if !revoke {
            return Ok(());
        }
        let key = (thread.clone(), id.into());
        let mut state = self.0.state.lock().map_err(|_| McpError::Internal)?;
        state.owners.remove(&(runtime.clone(), key.clone()));
        clear_unused(&self.0, &mut state, &key)
    }
}
fn clear_unused(inner: &Inner, state: &mut State, key: &Key) -> Result<()> {
    if state.reservations.get(key).is_some_and(|count| *count > 0)
        || state.owners.iter().any(|(_, owned)| owned == key)
    {
        return Ok(());
    }
    inner.registry.revoke_provider_session(&key.1)?;
    if state
        .configs
        .get(&key.0)
        .and_then(|c| c.scope.thread.as_ref())
        .is_some_and(|c| c.provider_session_id == key.1)
    {
        state.configs.remove(&key.0);
    }
    Ok(())
}

pub struct PreparedCredential {
    pub config: ProviderSessionConfig,
    pub issued: bool,
    key: Key,
    inner: Arc<Inner>,
    active: bool,
    cleanup: bool,
}
fn unreserve(state: &mut State, key: &Key) {
    if let Some(count) = state.reservations.get_mut(key) {
        *count -= 1;
        if *count == 0 {
            state.reservations.remove(key);
        }
    }
}
impl PreparedCredential {
    /// Publish the runtime's reference before releasing the preparation reservation.
    pub fn publish(mut self, runtime: ProviderSessionId) -> Result<ProviderSessionConfig> {
        let mut state = self.inner.state.lock().map_err(|_| McpError::Internal)?;
        let current = state
            .configs
            .get(&self.key.0)
            .and_then(|c| c.scope.thread.as_ref())
            .is_some_and(|caller| caller.provider_session_id == self.key.1);
        if !current {
            return Err(McpError::failure(
                "parent_not_active",
                "The MCP credential was replaced before the provider attached.",
            ));
        }
        state.owners.insert((runtime, self.key.clone()));
        unreserve(&mut state, &self.key);
        self.active = false;
        Ok(self.config.clone())
    }
}
impl Drop for PreparedCredential {
    fn drop(&mut self) {
        if self.active
            && let Ok(mut state) = self.inner.state.lock()
        {
            unreserve(&mut state, &self.key);
            if self.cleanup {
                let _ = clear_unused(&self.inner, &mut state, &self.key);
            }
        }
    }
}
