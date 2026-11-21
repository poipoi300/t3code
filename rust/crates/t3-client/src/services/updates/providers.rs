//! Candidate selection and settlement for ProviderUpdatesAction.tsx.
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::RpcEndpoint,
    types::{EnvironmentId, Generation},
};
use std::collections::BTreeMap;
use t3_contracts::{
    auth::AuthEnvironmentScope,
    provider_instance::{ProviderDriverKind, ProviderInstanceId, default_instance_id_for_driver},
    rpc::WsServerUpdateProviderRpc,
    server::*,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub environment: EnvironmentId,
    pub machine: String,
    pub generation: Generation,
    pub driver: ProviderDriverKind,
    pub instance: ProviderInstanceId,
    pub latest: String,
    pub checked_at: String,
}
fn eligible(p: &ServerProvider) -> bool {
    p.enabled
        && !matches!(
            p.compatibility_advisory
                .as_ref()
                .and_then(|a| a.latest_version_status.as_ref()),
            Some(
                ServerProviderCompatibilityStatus::Broken
                    | ServerProviderCompatibilityStatus::Unsupported
            )
        )
        && p.version_advisory.as_ref().is_some_and(|a| {
            a.status == ServerProviderVersionAdvisoryStatus::BehindLatest
                && a.latest_version.is_some()
        })
}
pub fn candidates<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let snapshot = client.store().snapshot();
    for env in client.environments() {
        if env.state.phase != crate::connection::Phase::Connected
            || !client.first_run().can_manage(&env.id)
        {
            continue;
        }
        let Some(config) = snapshot
            .environments
            .get(&env.id)
            .and_then(|s| s.config.as_deref())
        else {
            continue;
        };
        let mut drivers = BTreeMap::<_, Vec<_>>::new();
        for p in config.providers.0.iter().filter(|p| eligible(p)) {
            drivers.entry(&p.driver.0).or_default().push(p);
        }
        for group in drivers.into_values() {
            let Some(p) = group.iter().copied().max_by_key(|p| {
                (
                    p.instance_id == default_instance_id_for_driver(&p.driver),
                    &p.checked_at.0,
                )
            }) else {
                continue;
            };
            let Some(advisory) = &p.version_advisory else {
                continue;
            };
            if advisory.update_command.is_none()
                || p.update_state.as_ref().is_some_and(|state| {
                    matches!(
                        state.status,
                        ServerProviderUpdateStatus::Queued | ServerProviderUpdateStatus::Running
                    )
                })
                || group.iter().any(|p| {
                    p.version_advisory.as_ref().is_none_or(|a| {
                        !a.can_update || a.update_command != advisory.update_command
                    })
                })
            {
                continue;
            }
            if let Some(latest) = &advisory.latest_version {
                out.push(Candidate {
                    environment: env.id.clone(),
                    machine: env.label.clone(),
                    generation: env.state.generation,
                    driver: p.driver.clone(),
                    instance: p.instance_id.clone(),
                    latest: latest.0.clone(),
                    checked_at: p.checked_at.0.clone(),
                });
            }
        }
    }
    out
}
pub async fn update<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
    candidate: &Candidate,
) -> Result<Option<String>, ClientError> {
    if !candidates(client).contains(candidate) {
        return Err(ClientError::Cancelled);
    }
    let endpoint = client
        .client(&candidate.environment)
        .and_then(|c| c.session())
        .ok_or(ClientError::Disconnected)?;
    if endpoint.grants_scope(AuthEnvironmentScope::ProvidersManage) != Some(true) {
        return Err(ClientError::PermissionDenied {
            method: "server.updateProvider".into(),
        });
    }
    let reply = endpoint
        .call::<WsServerUpdateProviderRpc>(ServerProviderUpdateInput {
            provider: candidate.driver.clone(),
            instance_id: Some(candidate.instance.clone()),
            target_version: None,
        })
        .await?;
    let provider = reply
        .providers
        .0
        .iter()
        .find(|p| p.instance_id == candidate.instance);
    let state = provider.and_then(|p| p.update_state.as_ref());
    let message = state.and_then(|s| s.message.as_ref()).map(|s| s.0.clone());
    if !state.is_some_and(|s| s.status == ServerProviderUpdateStatus::Succeeded) {
        return Err(ClientError::Rejected {
            message: message.unwrap_or_else(|| "Provider update did not finish.".into()),
            detail: None,
        });
    }
    Ok(provider
        .filter(|p| {
            p.installed_version
                .as_ref()
                .and_then(Option::as_ref)
                .zip(p.version.as_ref())
                .is_some_and(|(installed, running)| installed != running)
        })
        .and(message))
}

pub fn check_targets<P: Platform + Clone + 'static>(
    client: &ClientRuntime<P>,
) -> Vec<(EnvironmentId, String)> {
    client
        .environments()
        .into_iter()
        .filter(|env| {
            env.state.phase == crate::connection::Phase::Connected
                && client.first_run().can_manage(&env.id)
        })
        .map(|env| (env.id, env.label))
        .collect()
}
pub async fn check<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Vec<String> {
    let targets = check_targets(client);
    let results = futures::future::join_all(targets.iter().map(|(id, label)| async move {
        let endpoint = client.client(id).and_then(|c| c.session())?;
        let input = serde_json::from_value(serde_json::json!({"refreshModels":true})).ok()?;
        endpoint
            .call::<t3_contracts::rpc::WsServerRefreshProvidersRpc>(input)
            .await
            .err()
            .map(|e| format!("{label}: {e}"))
    }))
    .await;
    results.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientStore, composition::Spawner, testing::environment::FakeEnvironment};
    use futures::{executor::LocalPool, task::LocalSpawnExt};
    use serde_json::{Value, json};
    use std::rc::Rc;
    #[test]
    fn candidates_enforce_grant_compatibility_command_and_representative() {
        let mut pool = LocalPool::new();
        let local = pool.spawner();
        let spawn: Spawner = Rc::new(move |f| local.spawn_local(f).expect("local spawner"));
        let primary = FakeEnvironment::new("manage", spawn.clone());
        let read_only = FakeEnvironment::at_origin("reader", "https://other.test", spawn.clone());
        primary.set_permissions(&["orchestration:read", "providers:manage"]);
        read_only.set_permissions(&["orchestration:read"]);
        let base = json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"version":"1.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-06-20T00:00:00Z","models":[],"skills":[],"slashCommands":[],"versionAdvisory":{"status":"behind_latest","currentVersion":"1","latestVersion":"2","updateCommand":"update","canUpdate":true,"checkedAt":null,"message":null}});
        for server in [&primary, &read_only] {
            server.edit_config(|c| c["providers"] = json!([base]));
        }
        let client = ClientRuntime::new(
            primary.platform().with_environment(&read_only),
            ClientStore::default(),
            t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
            spawn,
        );
        pool.run_until(client.pair(primary.pairing()))
            .expect("paired manageable");
        pool.run_until(client.pair(read_only.pairing()))
            .expect("paired reader");
        pool.run_until_stalled();
        let set = |providers: Value| {
            let snapshot = client.store().snapshot();
            let id = EnvironmentId("manage".into());
            let env = &snapshot.environments[&id];
            let mut config = env.config.as_deref().expect("config").clone();
            config.providers = serde_json::from_value(providers).expect("valid providers");
            assert!(client.store().set_config(&id, env.session, config));
        };
        assert_eq!(
            candidates(&client)
                .iter()
                .map(|c| c.environment.0.as_str())
                .collect::<Vec<_>>(),
            ["manage"]
        );
        let mut other = base.clone();
        other["instanceId"] = json!("extra");
        let mut incompatible = base.clone();
        incompatible["compatibilityAdvisory"] = json!({"status":"supported","latestVersionStatus":"broken","message":null,"recommendedVersion":null,"recommendedRange":null});
        let mut disabled = base.clone();
        disabled["enabled"] = json!(false);
        let mut conflict = other.clone();
        conflict["versionAdvisory"]["updateCommand"] = json!("other update");
        let mut manual = other.clone();
        manual["versionAdvisory"]["canUpdate"] = json!(false);
        for list in [
            json!([incompatible]),
            json!([disabled]),
            json!([base, conflict]),
            json!([base, manual]),
        ] {
            set(list);
            assert!(candidates(&client).is_empty());
        }
        other["updateState"] = json!({"status":"running","startedAt":null,"finishedAt":null,"message":null,"output":null});
        set(json!([other, base]));
        assert_eq!(
            candidates(&client)
                .iter()
                .map(|c| c.instance.0.as_str())
                .collect::<Vec<_>>(),
            ["codex"]
        );
    }
}
