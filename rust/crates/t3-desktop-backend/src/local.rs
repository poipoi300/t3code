//! The configuration of the local server the desktop starts: `t3 serve --bootstrap-fd 0` with a
//! desktop envelope that carries fresh credentials for every run.

use std::{ffi::OsString, path::PathBuf};

use crate::{ConfigSource, StartConfig};

/// The one local server of this desktop.
#[derive(Debug, Clone)]
pub struct LocalServer {
    pub program: PathBuf,
    /// Where the server listens (`backendExposure.port`).
    pub port: u16,
    /// The address it binds.
    pub host: String,
    /// `T3CODE_HOME` for the server, when it is not the default.
    pub t3_home: Option<String>,
}

/// 32 random bytes as hex: the rotating bootstrap secret and the legacy token.
fn fresh_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "no secure random numbers".to_owned())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl ConfigSource for LocalServer {
    fn resolve(&self) -> Result<StartConfig, String> {
        let mut envelope = serde_json::json!({
            "mode": "desktop",
            "noBrowser": true,
            "port": self.port,
            "host": self.host,
            "desktopBootstrapToken": fresh_secret()?,
            "desktopBootstrapSecret": fresh_secret()?,
            "tailscaleServeEnabled": false,
            "tailscaleServePort": 443,
        });
        if let (Some(home), Some(map)) = (&self.t3_home, envelope.as_object_mut()) {
            map.insert("t3Home".into(), home.clone().into());
        }
        // Dial loopback when the server binds every interface.
        let ready_host = match self.host.as_str() {
            "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_owned(),
            host => host.trim_matches(['[', ']']).to_owned(),
        };
        Ok(StartConfig::with_line(
            OsString::from(self.program.as_os_str()),
            vec!["serve".into(), "--bootstrap-fd".into(), "0".into()],
            envelope.to_string(),
            ready_host,
            self.port,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_contracts::desktop_bootstrap::DesktopBackendBootstrap;

    fn server() -> LocalServer {
        LocalServer {
            program: "/opt/t3/t3".into(),
            port: 4100,
            host: "0.0.0.0".into(),
            t3_home: Some("/home/me/.t3".into()),
        }
    }

    #[test]
    fn the_envelope_is_one_decodable_desktop_line_with_fresh_credentials() {
        let (first, second) = (server().resolve().unwrap(), server().resolve().unwrap());
        for config in [&first, &second] {
            let line = config.bootstrap_line.as_str();
            assert!(!line.contains('\n'));
            let envelope: DesktopBackendBootstrap = serde_json::from_str(line).unwrap();
            assert_eq!(envelope.port, 4100);
            assert!(envelope.no_browser);
            assert_eq!(envelope.desktop_bootstrap_token.len(), 64);
            assert_eq!(envelope.t3_home, Some(Some("/home/me/.t3".into())));
        }
        assert_ne!(first.bootstrap_line, second.bootstrap_line);
        assert_eq!(
            first.args,
            ["serve", "--bootstrap-fd", "0"].map(OsString::from)
        );
    }

    #[test]
    fn a_wildcard_bind_is_probed_on_loopback() {
        let config = server().resolve().unwrap();
        assert_eq!(
            (config.ready_host.as_str(), config.ready_port),
            ("127.0.0.1", 4100)
        );
        let mut ipv6 = server();
        ipv6.host = "[::1]".into();
        assert_eq!(ipv6.resolve().unwrap().ready_host, "::1");
    }
}
