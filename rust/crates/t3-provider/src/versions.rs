use crate::{Result, environment::Environment, invalid};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, sync::LazyLock};
use t3_contracts::server::{
    ServerProviderCompatibilityAdvisory, ServerProviderCompatibilityStatus as Status,
};

static GENERIC_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bv?(\d+\.\d+\.\d+)\b").expect("version regex"));
static MUSE_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+\.\d+\.\d+-R\d+(?:\.\d+)?\b").expect("Muse version regex"));
static CODEX_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s)v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?:\s|$)")
        .expect("Codex version regex")
});
static RANGE_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-[0-9A-Za-z.-]+)?$")
        .expect("range version regex")
});
static COMPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\^|>=|>|<=|<|=)?v?(\d+(?:\.\d+){0,2})$").expect("comparator regex")
});

pub fn parse_generic_version(output: &str) -> Option<String> {
    GENERIC_VERSION.captures(output).map(|c| c[1].to_owned())
}
pub fn parse_muse_version(output: &str) -> Option<String> {
    MUSE_VERSION
        .find(output)
        .map(|m| m.as_str().to_owned())
        .or_else(|| parse_generic_version(output))
}
pub fn parse_codex_version(output: &str) -> Option<String> {
    CODEX_VERSION.captures(output).map(|c| c[1].to_owned())
}

pub fn probe_timeout_ms(provider: &str, default: u64, env: &Environment) -> u64 {
    let value = env
        .get(&format!(
            "T3CODE_{}_PROBE_TIMEOUT_MS",
            provider.to_uppercase()
        ))
        .and_then(|v| v.trim().parse::<f64>().ok());
    match value {
        Some(value)
            if value.fract() == 0.0 && value > default as f64 && value <= i32::MAX as f64 =>
        {
            value as u64
        }
        _ => default,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub numbers: [u64; 3],
    pub prerelease: Vec<String>,
}

pub fn parse_version(value: &str) -> Option<Version> {
    let value = value
        .trim()
        .split('+')
        .next()?
        .strip_prefix('v')
        .unwrap_or(value.trim().split('+').next()?);
    let (main, prerelease) = value.split_once('-').unwrap_or((value, ""));
    let mut segments: Vec<_> = main
        .split('.')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    while !segments.is_empty() && segments.len() < 3 {
        segments.push("0");
    }
    if segments.len() != 3
        || segments
            .iter()
            .any(|s| !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(Version {
        numbers: [
            segments[0].parse().ok()?,
            segments[1].parse().ok()?,
            segments[2].parse().ok()?,
        ],
        prerelease: prerelease
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let (Some(left_v), Some(right_v)) = (parse_version(left), parse_version(right)) else {
        return left.cmp(right);
    };
    let numeric = left_v.numbers.cmp(&right_v.numbers);
    if numeric != Ordering::Equal {
        return numeric;
    }
    if left_v.prerelease.is_empty() != right_v.prerelease.is_empty() {
        return left_v
            .prerelease
            .is_empty()
            .cmp(&right_v.prerelease.is_empty());
    }
    for (a, b) in left_v.prerelease.iter().zip(&right_v.prerelease) {
        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => a.cmp(b),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left_v.prerelease.len().cmp(&right_v.prerelease.len())
}

pub fn compare_muse_versions(left: &str, right: &str) -> Ordering {
    fn revision(value: &str) -> Option<String> {
        let (base, revision) = value.rsplit_once("-R")?;
        Some(format!(
            "{base}-release.{revision}{}",
            if revision.contains('.') { "" } else { ".0" }
        ))
    }
    match (revision(left), revision(right)) {
        (Some(a), Some(b)) => compare_versions(&a, &b),
        (Some(_), None) | (None, Some(_)) => compare_versions(
            left.split("-R").next().unwrap_or(left),
            right.split("-R").next().unwrap_or(right),
        ),
        _ => compare_versions(left, right),
    }
}

pub fn satisfies_range(value: &str, range: &str) -> bool {
    let Some(version) = RANGE_VERSION.captures(value.trim()) else {
        return false;
    };
    let numbers = |captures: &regex::Captures<'_>| -> Option<[u64; 3]> {
        Some([
            captures.get(1)?.as_str().parse().ok()?,
            captures.get(2).map_or("0", |m| m.as_str()).parse().ok()?,
            captures.get(3).map_or("0", |m| m.as_str()).parse().ok()?,
        ])
    };
    let Some(version) = numbers(&version) else {
        return false;
    };
    range.split("||").any(|group| {
        let group = group.trim();
        if group.is_empty() {
            return false;
        }
        let mut tokens = group.split_whitespace();
        let mut all = true;
        while let Some(token) = tokens.next() {
            let token = if matches!(token, "^" | ">=" | ">" | "<=" | "<" | "=") {
                format!("{token}{}", tokens.next().unwrap_or(""))
            } else {
                token.to_owned()
            };
            let Some(c) = COMPARATOR.captures(&token) else {
                return false;
            };
            let Some(target_c) = RANGE_VERSION.captures(&c[2]) else {
                return false;
            };
            let Some(target) = numbers(&target_c) else {
                return false;
            };
            let order = version.cmp(&target);
            all &= match c.get(1).map_or("=", |m| m.as_str()) {
                ">=" => order != Ordering::Less,
                ">" => order == Ordering::Greater,
                "<=" => order != Ordering::Greater,
                "<" => order == Ordering::Less,
                "=" => order == Ordering::Equal,
                "^" => {
                    order != Ordering::Less
                        && if target[0] > 0 || target_c.get(2).is_none() {
                            version[0] == target[0]
                        } else if target[1] > 0 || target_c.get(3).is_none() {
                            version[0] == 0 && version[1] == target[1]
                        } else {
                            version == target
                        }
                }
                _ => false,
            };
        }
        all
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityPolicy {
    pub driver: String,
    pub t3_code_range: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_version: Option<String>,
    pub ranges: Vec<CompatibilityRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityRange {
    pub range: String,
    pub status: Status,
}

pub fn validate_compatibility_policy(policy: &CompatibilityPolicy) -> Result<()> {
    for range in std::iter::once(&policy.t3_code_range)
        .chain(policy.recommended_range.iter())
        .chain(policy.ranges.iter().map(|r| &r.range))
    {
        if range.trim().is_empty()
            || !range.split("||").all(|g| {
                !g.trim().is_empty() && g.split_whitespace().all(|t| COMPARATOR.is_match(t))
            })
        {
            return Err(invalid("compatibility range", range));
        }
    }
    if let Some(version) = &policy.recommended_version
        && (!stable_version(version)
            || policy
                .recommended_range
                .as_ref()
                .is_some_and(|r| !satisfies_range(version, r))
            || policy
                .ranges
                .iter()
                .find(|r| satisfies_range(version, &r.range))
                .map(|r| r.status)
                != Some(Status::Supported))
    {
        return Err(invalid("recommended version", version));
    }
    Ok(())
}

fn stable_version(value: &str) -> bool {
    value.split('.').count() == 3
        && value
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

pub fn resolve_compatibility(
    policies: &[CompatibilityPolicy],
    driver: &str,
    version: Option<&str>,
    t3_version: &str,
) -> Option<ServerProviderCompatibilityAdvisory> {
    let policy = policies
        .iter()
        .find(|p| p.driver == driver && satisfies_range(t3_version, &p.t3_code_range))?;
    let mut version = version
        .unwrap_or("")
        .strip_prefix('v')
        .unwrap_or(version.unwrap_or(""))
        .to_owned();
    if driver == "antigravity" {
        version = version
            .strip_prefix("agy_acp_server_")
            .unwrap_or(&version)
            .to_owned();
    }
    if driver == "cursor"
        && let Some((base, hash)) = version.split_once('-')
        && base.split('.').map(str::len).eq([4, 2, 2])
        && !hash.is_empty()
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        version = base.into();
    }
    if driver == "muse"
        && let Some((base, revision)) = version.split_once("-R")
        && !revision.is_empty()
        && revision.split('.').count() <= 2
        && revision
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        version = base.into();
    }
    let status = if stable_version(&version) {
        policy
            .ranges
            .iter()
            .find(|r| satisfies_range(&version, &r.range))
            .map_or(Status::Unknown, |r| r.status)
    } else {
        Status::Unknown
    };
    let message = match status {
        Status::Broken => {
            Some("This provider version is known to be incompatible with this T3 Code release.")
        }
        Status::Unsupported => {
            Some("This provider version is outside the supported range for this T3 Code release.")
        }
        Status::Graceful => {
            Some("This provider version has limited compatibility with this T3 Code release.")
        }
        _ => None,
    }
    .map(|s| {
        match policy
            .recommended_version
            .as_ref()
            .or(policy.recommended_range.as_ref())
        {
            Some(r) => format!("{s} Use {r}."),
            None => s.to_owned(),
        }
    });
    Some(ServerProviderCompatibilityAdvisory {
        status,
        message: message.map(t3_contracts::base_schemas::TrimmedNonEmptyString),
        recommended_version: policy
            .recommended_version
            .clone()
            .map(t3_contracts::base_schemas::TrimmedNonEmptyString),
        recommended_range: policy
            .recommended_range
            .clone()
            .map(t3_contracts::base_schemas::TrimmedNonEmptyString),
        latest_version_status: None,
    })
}
