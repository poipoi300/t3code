use crate::{
    connection::{ConnectionMode, ConnectionState, FailureKind, Phase},
    environment::EnvironmentRegistration,
    types::EnvironmentId,
};
use std::{collections::BTreeMap, rc::Rc};
use t3_contracts::{environment::EnvironmentMachineKind, server::ServerConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    Available,
    Offline,
    Connecting,
    Reconnecting,
    Connected,
    Error,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPresentation {
    pub phase: ConnectionPhase,
    pub error: Option<String>,
    pub trace_id: Option<String>,
}

pub fn present_connection(
    state: Option<&ConnectionState>,
    enabled: bool,
    unsupported: Option<&str>,
) -> ConnectionPresentation {
    let (phase, failure) = if let Some(reason) = unsupported {
        return ConnectionPresentation {
            phase: ConnectionPhase::Unsupported,
            error: Some(reason.into()),
            trace_id: None,
        };
    } else if let Some(state) = state {
        let phase = match state.phase {
            Phase::Available => ConnectionPhase::Available,
            Phase::Offline => ConnectionPhase::Offline,
            Phase::Connecting if state.attempt <= 1 && state.last_failure.is_none() => {
                ConnectionPhase::Connecting
            }
            Phase::Connecting | Phase::Backoff => ConnectionPhase::Reconnecting,
            Phase::Connected => ConnectionPhase::Connected,
            Phase::Blocked
                if state
                    .last_failure
                    .as_ref()
                    .is_some_and(|failure| failure.kind == FailureKind::Unsupported) =>
            {
                ConnectionPhase::Unsupported
            }
            Phase::Blocked => ConnectionPhase::Error,
        };
        let failure = matches!(
            state.phase,
            Phase::Connecting | Phase::Backoff | Phase::Blocked
        )
        .then_some(state.last_failure.as_ref())
        .flatten();
        (phase, failure)
    } else {
        (
            if enabled {
                ConnectionPhase::Connecting
            } else {
                ConnectionPhase::Available
            },
            None,
        )
    };
    ConnectionPresentation {
        phase,
        error: failure.map(|failure| failure.detail.clone()),
        trace_id: failure.and_then(|failure| failure.trace_id.clone()),
    }
}

pub struct PresentationInput<'a> {
    pub entry: Rc<EnvironmentRegistration>,
    pub unsupported_reason: Option<&'a str>,
    pub state: Option<&'a ConnectionState>,
    pub config: Option<Rc<ServerConfig>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentPresentation {
    pub entry: Rc<EnvironmentRegistration>,
    pub connection: Rc<ConnectionPresentation>,
    /// Provider pushes are excluded from this view's invalidation identity.
    pub config_without_providers: Option<Rc<ServerConfig>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSummary {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub display_url: String,
    pub relay_managed: bool,
    pub enabled: bool,
    pub connection: ConnectionPresentation,
}

fn reuse<T: PartialEq>(previous: Option<&Rc<T>>, next: T) -> Rc<T> {
    match previous {
        Some(previous) if **previous == next => previous.clone(),
        _ => Rc::new(next),
    }
}

#[derive(Default)]
pub struct EnvironmentPresentations {
    pub ordered_ids: Vec<EnvironmentId>,
    pub presentations: BTreeMap<EnvironmentId, Rc<EnvironmentPresentation>>,
    pub summaries: BTreeMap<EnvironmentId, Rc<EnvironmentSummary>>,
    pub configs: BTreeMap<EnvironmentId, Rc<ServerConfig>>,
    pub providers: BTreeMap<EnvironmentId, Rc<t3_contracts::server::ServerProviders>>,
    pub enabled_with_providers: BTreeMap<EnvironmentId, Rc<ServerConfig>>,
}

impl EnvironmentPresentations {
    pub fn update<'a>(&mut self, inputs: impl IntoIterator<Item = PresentationInput<'a>>) {
        let mut next = Self::default();
        for input in inputs {
            let id = input.entry.id.clone();
            let prior = self.presentations.get(&id);
            let connection = reuse(
                prior.map(|p| &p.connection),
                present_connection(input.state, input.entry.enabled, input.unsupported_reason),
            );
            let config = input.config.as_ref().map(|config| {
                let mut quiet = (**config).clone();
                quiet.providers.0.clear();
                reuse(
                    prior.and_then(|p| p.config_without_providers.as_ref()),
                    quiet,
                )
            });
            let presentation = reuse(
                prior,
                EnvironmentPresentation {
                    entry: input.entry.clone(),
                    connection: connection.clone(),
                    config_without_providers: config.clone(),
                },
            );
            let display_url = input
                .entry
                .routes
                .first()
                .map_or_else(String::new, |route| route.http_base.clone());
            next.summaries.insert(
                id.clone(),
                reuse(
                    self.summaries.get(&id),
                    EnvironmentSummary {
                        environment_id: id.clone(),
                        label: input.entry.label.clone(),
                        display_url,
                        relay_managed: input
                            .entry
                            .routes
                            .iter()
                            .any(|r| r.mode == ConnectionMode::Relay),
                        enabled: input.entry.enabled,
                        connection: (*connection).clone(),
                    },
                ),
            );
            if let Some(full) = input.config {
                next.providers.insert(
                    id.clone(),
                    reuse(self.providers.get(&id), full.providers.clone()),
                );
                if input.entry.enabled && input.unsupported_reason.is_none() {
                    if let Some(config) = config {
                        next.configs.insert(id.clone(), config);
                    }
                    next.enabled_with_providers.insert(id.clone(), full);
                }
            }
            next.presentations.insert(id.clone(), presentation);
            next.ordered_ids.push(id);
        }
        *self = next;
    }

    pub fn connected_ids(&self) -> Vec<&EnvironmentId> {
        self.ordered_ids
            .iter()
            .filter(|id| {
                self.summaries
                    .get(*id)
                    .is_some_and(|s| s.connection.phase == ConnectionPhase::Connected)
            })
            .collect()
    }

    pub fn identities(&self) -> Vec<(&EnvironmentId, &str)> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.summaries.get(id).map(|s| (id, s.label.as_str())))
            .collect()
    }

    pub fn pull_requests_supported(&self) -> bool {
        self.presentations
            .values()
            .any(|p| p.config_without_providers.is_some())
    }

    pub fn machine_kind(&self, id: &EnvironmentId) -> EnvironmentMachineKind {
        self.presentations
            .get(id)
            .and_then(|p| p.config_without_providers.as_ref())
            .map_or(EnvironmentMachineKind::Server, |c| {
                c.settings
                    .environment_icon
                    .clone()
                    .or_else(|| c.environment.platform.machine.clone())
                    .unwrap_or(EnvironmentMachineKind::Server)
            })
    }
}
