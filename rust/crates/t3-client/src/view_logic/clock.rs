use std::collections::BTreeMap;
use t3_contracts::base_schemas::EnvironmentId;

#[derive(Default, Clone, Debug)]
pub struct ServerClocks(BTreeMap<EnvironmentId, f64>);

impl ServerClocks {
    pub fn format_working_timer_now(
        &self,
        environment: &EnvironmentId,
        start: &str,
        client_now: f64,
    ) -> String {
        format_working_timer_at(start, self.to_server_clock(environment, client_now))
    }
    pub fn record(
        &mut self,
        environment: &EnvironmentId,
        server: Option<f64>,
        requested: f64,
        received: f64,
    ) {
        let Some(server) = server.filter(|v| v.is_finite()) else {
            self.0.remove(environment);
            return;
        };
        let offset = if server > received {
            server - received
        } else if server < requested {
            server - requested
        } else {
            0.0
        };
        self.0.insert(environment.clone(), offset);
    }
    pub fn to_server_clock(&self, environment: &EnvironmentId, client: f64) -> f64 {
        client + self.0.get(environment).copied().unwrap_or(0.0)
    }
}

pub fn format_working_timer_at(start: &str, server_now: f64) -> String {
    let Some(start) = crate::time::epoch_ms(start) else {
        return "0s".into();
    };
    let elapsed = ((server_now - start as f64) / 1_000.0).floor().max(0.0);
    if elapsed < 60.0 {
        format!("{elapsed}s")
    } else {
        super::session::timestamps::format_duration(elapsed * 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn working_timer_uses_host_clock_and_clamps_future_or_invalid_starts() {
        let env = EnvironmentId("host".into());
        let mut clocks = ServerClocks::default();
        clocks.record(&env, Some(3_600_000.0), 0.0, 0.0);
        assert_eq!(
            clocks.format_working_timer_now(&env, "1970-01-01T01:00:00Z", 5_900.0),
            "5s"
        );
        assert_eq!(
            clocks.format_working_timer_now(&env, "1970-01-01T01:00:00Z", 65_900.0),
            "1m 5s"
        );
        assert_eq!(
            clocks.format_working_timer_now(&env, "1970-01-01T01:00:00Z", -100.0),
            "0s"
        );
        assert_eq!(clocks.format_working_timer_now(&env, "invalid", 0.0), "0s");
    }

    #[test]
    fn upstream_server_clock_windows_and_reconnect_clear() {
        let mut clocks = ServerClocks::default();
        let env = EnvironmentId("host".into());
        for (server, requested, received, now, expected) in [
            (3_601_000.0, 900.0, 1_050.0, 2_050.0, 3_602_000.0),
            (1_000.0, 3_600_900.0, 3_601_050.0, 3_601_900.0, 2_000.0),
            (5_000.0, 1_000.0, 9_000.0, 12_345.0, 12_345.0),
        ] {
            clocks.record(&env, Some(server), requested, received);
            assert_eq!(clocks.to_server_clock(&env, now), expected);
            assert_eq!(
                clocks.to_server_clock(&EnvironmentId("other".into()), now),
                now
            );
        }
        for absent in [None, Some(f64::NAN), Some(f64::INFINITY)] {
            clocks.record(&env, Some(3_601_000.0), 900.0, 1_050.0);
            clocks.record(&env, absent, 2_000.0, 2_100.0);
            assert_eq!(clocks.to_server_clock(&env, 3_000.0), 3_000.0);
        }
    }
}
