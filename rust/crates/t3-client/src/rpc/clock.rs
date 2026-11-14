//! A connection's first config is the only fresh host-clock observation.
use crate::{time::epoch_ms, types::EnvironmentId, view_logic::clock::ServerClocks};
use t3_contracts::server::ServerConfig;

#[derive(Debug)]
pub struct ConfigClock {
    environment: EnvironmentId,
    requested_at_ms: f64,
    sampled: bool,
    clocks: ServerClocks,
}
impl ConfigClock {
    pub fn new(environment: EnvironmentId, requested_at_ms: f64) -> Self {
        Self {
            environment,
            requested_at_ms,
            sampled: false,
            clocks: ServerClocks::default(),
        }
    }
    pub fn observe(&mut self, config: &ServerConfig, received_at_ms: f64) {
        if self.sampled {
            return;
        }
        self.clocks.record(
            &self.environment,
            config
                .server_time
                .as_ref()
                .and_then(|time| epoch_ms(&time.0))
                .map(|ms| ms as f64),
            self.requested_at_ms,
            received_at_ms,
        );
        self.sampled = true;
    }
    pub fn to_server_clock(&self, client_ms: f64) -> f64 {
        self.clocks.to_server_clock(&self.environment, client_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_first_config_before_readiness_ignores_repeats_and_drops_offset_on_reconnect() {
        let mut config: ServerConfig = serde_json::from_str(
            include_str!("../../../../fixtures/samples/ServerConfig.jsonl")
                .lines()
                .next()
                .expect("fixture"),
        )
        .expect("config");
        config.server_time =
            Some(serde_json::from_str("\"1970-01-01T01:00:00.000Z\"").expect("timestamp"));
        let environment = config.environment.environment_id.clone();
        let mut first = ConfigClock::new(environment.clone(), 0.0);
        first.observe(&config, 0.0);
        assert_eq!(first.to_server_clock(0.0), 3_600_000.0);
        first.observe(&config, 100_000.0);
        assert_eq!(first.to_server_clock(0.0), 3_600_000.0);
        config.server_time = None;
        let mut reconnected = ConfigClock::new(environment, 0.0);
        reconnected.observe(&config, 0.0);
        assert_eq!(reconnected.to_server_clock(0.0), 0.0);
    }
}
