//! The configuration of the local server the desktop starts: `t3 serve --bootstrap-fd 0` with a
//! desktop envelope that carries fresh credentials for every run.

use std::{ffi::OsString, path::PathBuf};

use crate::{ConfigSource, StartConfig};

/// The one local server of this desktop.
#[derive(Debug, Clone)]
pub struct LocalServer {
    pub program: PathBuf,
    /// Where the server listens (`backendExposure.port`) when `scan_ports` is false; otherwise the
    /// first port to try.
    pub port: u16,
    /// Pick the first port from `port` that nothing listens on, on 127.0.0.1, 0.0.0.0 and `::`
    /// (`resolveDesktopBackendPort`); false when the port was configured.
    pub scan_ports: bool,
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

/// This machine has no address of that family (IPv6 off): `EADDRNOTAVAIL` or `EAFNOSUPPORT`
/// (`packages/shared/src/Net.ts`).
fn no_such_address(error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    const AF_NOT_SUPPORTED: i32 = 97;
    #[cfg(target_os = "macos")]
    const AF_NOT_SUPPORTED: i32 = 47;
    #[cfg(windows)]
    const AF_NOT_SUPPORTED: i32 = 10047;
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    const AF_NOT_SUPPORTED: i32 = -1;
    error.kind() == std::io::ErrorKind::AddrNotAvailable
        || error.raw_os_error() == Some(AF_NOT_SUPPORTED)
}

/// `DESKTOP_BACKEND_PORT_PROBE_HOSTS`: a port is free only if every one of these can bind it. A
/// host this machine has no address for (no IPv6) does not make a port busy.
fn can_listen(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    let free = |bound: std::io::Result<TcpListener>| match bound {
        Ok(_) => true,
        Err(error) => no_such_address(&error),
    };
    free(TcpListener::bind((Ipv4Addr::LOCALHOST, port)))
        && free(TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)))
        && free(TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)))
}

impl LocalServer {
    fn choose_port(&self) -> Result<u16, String> {
        if !self.scan_ports {
            return Ok(self.port);
        }
        (self.port..=u16::MAX)
            .find(|port| can_listen(*port))
            .ok_or_else(|| format!("no free port from {} up", self.port))
    }
}

impl ConfigSource for LocalServer {
    fn resolve(&self) -> Result<StartConfig, String> {
        let port = self.choose_port()?;
        let mut envelope = serde_json::json!({
            "mode": "desktop",
            "noBrowser": true,
            "port": port,
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
        let mut config = StartConfig::with_line(
            OsString::from(self.program.as_os_str()),
            vec!["serve".into(), "--bootstrap-fd".into(), "0".into()],
            envelope.to_string(),
            ready_host,
            port,
        );
        // The server exits when this process's end of its stdin closes (the parent was killed).
        config.env.insert(
            "T3CODE_DESKTOP_EXIT_ON_STDIN_CLOSE".into(),
            Some("1".into()),
        );
        Ok(match &self.t3_home {
            Some(home) => config.with_home(home),
            None => config,
        })
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
            scan_ports: false,
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
    fn a_scanned_port_skips_one_that_something_listens_on() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let mut scanning = server();
        scanning.port = port;
        scanning.scan_ports = true;
        scanning.host = "127.0.0.1".into();
        let config = scanning.resolve().unwrap();
        assert_ne!(config.ready_port, port);
        assert!(config.ready_port > port);
        // A configured port is used as given, even when it is busy.
        scanning.scan_ports = false;
        assert_eq!(scanning.resolve().unwrap().ready_port, port);
    }

    #[test]
    fn a_machine_without_ipv6_does_not_make_every_port_busy() {
        let unsupported = [
            #[cfg(target_os = "linux")]
            97,
            #[cfg(target_os = "macos")]
            47,
            #[cfg(windows)]
            10047,
        ];
        for code in unsupported {
            assert!(no_such_address(&std::io::Error::from_raw_os_error(code)));
        }
        assert!(no_such_address(
            &std::io::ErrorKind::AddrNotAvailable.into()
        ));
        assert!(!no_such_address(&std::io::ErrorKind::AddrInUse.into()));
        assert!(!no_such_address(
            &std::io::ErrorKind::PermissionDenied.into()
        ));
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
