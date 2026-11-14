use crate::{environment::Platform, versions::compare_versions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const MANIFEST_BASE: &str =
    "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub name: String,
    pub bytes: Option<u64>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Format {
    Executable,
    TarGz,
    Zip,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    pub archive_bytes: Option<u64>,
    pub executable: Member,
    pub harness: Option<Member>,
    pub format: Format,
}
impl std::fmt::Debug for Release {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Release([REDACTED])")
    }
}

impl Release {
    pub fn id(&self) -> Option<&str> {
        self.sha512
            .as_deref()
            .and_then(|hash| hash.get(..64))
            .or(self.sha256.as_deref())
    }
}
pub fn target(platform: Platform, architecture: &str, musl: bool) -> Option<String> {
    let cpu = match architecture {
        "x64" | "x86_64" => "amd64",
        "arm64" | "aarch64" => "arm64",
        _ => return None,
    };
    let os = match platform {
        Platform::Windows => "windows",
        Platform::Macos => "darwin",
        Platform::Linux => "linux",
    };
    Some(format!(
        "{os}_{cpu}{}",
        if platform == Platform::Linux && musl {
            "_musl"
        } else {
            ""
        }
    ))
}
pub fn parse(value: &Value, platform: Platform, architecture: &str, musl: bool) -> Option<Release> {
    target(platform, architecture, musl)?;
    let version = value["version"].as_str()?;
    let url = value["url"].as_str()?;
    let hash = value["sha512"].as_str()?;
    if !regex::Regex::new(r"^\d+\.\d+\.\d+$")
        .ok()?
        .is_match(version)
        || compare_versions(version, "1.5.0").is_lt()
        || hash.len() != 128
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let cpu = if matches!(architecture, "x64" | "x86_64") {
        "x64"
    } else {
        "arm64"
    };
    let os = match platform {
        Platform::Windows => "windows",
        Platform::Macos => "mac",
        Platform::Linux => "linux",
    };
    let suffix = if platform == Platform::Linux && musl {
        "_musl"
    } else {
        ""
    };
    let folder = format!(
        "{}-{}{}",
        match platform {
            Platform::Windows => "windows",
            Platform::Macos => "darwin",
            Platform::Linux => "linux",
        },
        if cpu == "arm64" { "arm" } else { "x64" },
        if suffix.is_empty() { "" } else { "-musl" }
    );
    let extension = if platform == Platform::Windows {
        "exe"
    } else {
        "tar.gz"
    };
    let prefix =
        format!("https://storage.googleapis.com/antigravity-public/antigravity-cli/{version}-");
    let rest = url.strip_prefix(&prefix)?;
    let (build, path) = rest.split_once('/')?;
    if build.is_empty()
        || !build.bytes().all(|byte| byte.is_ascii_digit())
        || path != format!("{folder}/cli_{os}_{cpu}{suffix}.{extension}")
    {
        return None;
    }
    Some(Release {
        version: version.into(),
        url: url.into(),
        sha256: None,
        sha512: Some(hash.to_ascii_lowercase()),
        archive_bytes: None,
        executable: Member {
            name: if platform == Platform::Windows {
                "agy.exe"
            } else {
                "antigravity"
            }
            .into(),
            bytes: None,
        },
        harness: None,
        format: if platform == Platform::Windows {
            Format::Executable
        } else {
            Format::TarGz
        },
    })
}
