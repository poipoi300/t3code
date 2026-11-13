use crate::{Error, UsageProviderKind as P, UsageRecord, UsageSpeed};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

pub type ProbeFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct ProviderInstance {
    pub provider: P,
    pub home_path: Option<PathBuf>,
    pub managed: bool,
    pub environment: BTreeMap<String, String>,
    pub pi_session_dir: Option<PathBuf>,
    pub muse_binary: PathBuf,
    pub antigravity_profile: Option<PathBuf>,
}
impl ProviderInstance {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            home_path: None,
            managed: false,
            environment: BTreeMap::new(),
            pi_session_dir: None,
            muse_binary: "muse".into(),
            antigravity_profile: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum SourceKind {
    Transcripts { file_name: Option<String> },
    OpenCode,
    Antigravity,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub provider: P,
    pub directory: PathBuf,
    pub kind: SourceKind,
    pub refreshing: bool,
}
impl Source {
    pub fn transcripts(provider: P, directory: impl Into<PathBuf>) -> Self {
        Self {
            provider,
            directory: directory.into(),
            kind: SourceKind::Transcripts { file_name: None },
            refreshing: false,
        }
    }
}
pub fn expand_home(path: &Path, home: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        home.into()
    } else if text.starts_with("~/") || text.starts_with("~\\") {
        home.join(&text[2..])
    } else {
        path.into()
    }
}
pub(crate) fn canonical(path: PathBuf) -> PathBuf {
    let path = std::path::absolute(&path).unwrap_or(path);
    dunce::canonicalize(&path).unwrap_or(path)
}
pub fn resolve_sources(
    home: &Path,
    instance: &ProviderInstance,
    muse_home: Option<&Path>,
) -> Vec<Source> {
    let env = |name: &str| {
        instance
            .environment
            .get(name)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    };
    let configured = instance
        .home_path
        .as_ref()
        .filter(|p| !p.as_os_str().is_empty());
    let root = |configured: Option<&Path>, var: &str, default: PathBuf| {
        canonical(expand_home(
            configured
                .map(Path::to_path_buf)
                .or_else(|| env(var).map(PathBuf::from))
                .as_deref()
                .unwrap_or(&default),
            home,
        ))
    };
    let p = instance.provider.clone();
    let transcript = |directory: PathBuf, name: Option<&str>| Source {
        provider: p.clone(),
        directory,
        kind: SourceKind::Transcripts {
            file_name: name.map(str::to_string),
        },
        refreshing: false,
    };
    match p {
        P::Claude => vec![transcript(
            root(
                configured.map(PathBuf::as_path),
                "CLAUDE_CONFIG_DIR",
                home.join(".claude"),
            )
            .join("projects"),
            None,
        )],
        P::Codex => {
            let path = if instance.managed {
                configured.cloned().unwrap_or_else(|| home.join(".codex"))
            } else {
                root(
                    configured.map(PathBuf::as_path),
                    "CODEX_HOME",
                    home.join(".codex"),
                )
            };
            vec![transcript(
                canonical(expand_home(&path, home)).join("sessions"),
                None,
            )]
        }
        P::Grok => vec![transcript(
            root(None, "GROK_HOME", home.join(".grok")).join("sessions"),
            Some("updates.jsonl"),
        )],
        P::Pi => {
            let launch_home = instance
                .environment
                .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or(home.into());
            let mut out = vec![];
            if let Some(path) = &instance.pi_session_dir {
                let path = expand_home(path, &launch_home);
                if path.is_absolute() {
                    out.push(transcript(canonical(path), None));
                }
            }
            let path = if let Some(session) = env("PI_CODING_AGENT_SESSION_DIR") {
                expand_home(Path::new(session), &launch_home)
            } else {
                env("PI_CODING_AGENT_DIR")
                    .map(|s| expand_home(Path::new(s), &launch_home))
                    .unwrap_or(launch_home.join(".pi/agent"))
                    .join("sessions")
            };
            out.push(transcript(canonical(path), None));
            out
        }
        P::Muse => muse_home
            .map(|h| {
                vec![transcript(
                    canonical(h.into()).join("sessions"),
                    Some("session.jsonl"),
                )]
            })
            .unwrap_or_else(|| {
                let mut source = transcript(instance.muse_binary.clone(), Some("session.jsonl"));
                source.refreshing = true;
                vec![source]
            }),
        P::Opencode => {
            let defaults = vec![
                env("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .unwrap_or(home.join(".local/share"))
                    .join("opencode"),
            ];
            let roots = env("OPENCODE_DATA_DIR")
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or(defaults);
            roots
                .into_iter()
                .map(|r| Source {
                    provider: p.clone(),
                    directory: canonical(expand_home(&r, home)),
                    kind: SourceKind::OpenCode,
                    refreshing: false,
                })
                .collect()
        }
        P::Antigravity => {
            let defaults = [
                "antigravity",
                "antigravity-cli",
                "antigravity-ide",
                "antigravity-backup",
            ]
            .map(|n| home.join(".gemini").join(n))
            .into_iter()
            .chain([home.join(".config/antigravity")])
            .collect();
            let mut roots = env("ANTIGRAVITY_DATA_DIR")
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or(defaults);
            if let Some(profile) = &instance.antigravity_profile {
                roots.push(profile.join("antigravity-acp"));
            }
            roots
                .into_iter()
                .map(|r| {
                    let root = canonical(expand_home(&r, home));
                    let nested = root.join("conversations");
                    Source {
                        provider: p.clone(),
                        directory: if nested.exists() {
                            canonical(nested)
                        } else {
                            root
                        },
                        kind: SourceKind::Antigravity,
                        refreshing: false,
                    }
                })
                .collect()
        }
        P::Cursor => vec![],
    }
}

#[derive(Debug, Clone)]
pub struct ExternalSource {
    pub provider: P,
    pub directory: String,
    pub host_id: Option<String>,
    pub volume_id: String,
    pub files: Option<Vec<(String, Vec<UsageRecord>)>>,
    pub status: Option<String>,
    pub message: Option<String>,
    pub refreshing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TierRequest {
    pub session_id: String,
    pub native_turn_id: String,
    pub model: String,
}

/// Runtime-owned calls: Muse SDK discovery, Cursor's account API and Codex's persisted turn evidence.
#[derive(Debug, Clone, PartialEq)]
pub enum MuseHome {
    Pending,
    Unavailable,
    Resolved(PathBuf),
}

pub trait ProviderUsageProbe: Send + Sync {
    fn muse_home<'a>(
        &'a self,
        _instance: &'a ProviderInstance,
        _await_refresh: bool,
    ) -> ProbeFuture<'a, MuseHome> {
        Box::pin(async { Ok(MuseHome::Unavailable) })
    }
    fn scan<'a>(
        &'a self,
        _instances: &'a [ProviderInstance],
        _since: f64,
        _until: f64,
        _await_refresh: bool,
    ) -> ProbeFuture<'a, Vec<ExternalSource>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn codex_tiers<'a>(&'a self, _requests: &'a [TierRequest]) -> ProbeFuture<'a, HashMapTiers> {
        Box::pin(async { Ok(HashMapTiers::new()) })
    }
}
pub type HashMapTiers = std::collections::HashMap<TierRequest, UsageSpeed>;
#[derive(Debug, Default)]
pub struct NoProviderProbe;
impl ProviderUsageProbe for NoProviderProbe {}

/// Reads the first V2 running selection for each attempt; ambiguous native turns remain unresolved.
pub fn resolve_codex_history(path: &Path, requests: &[TierRequest]) -> Result<HashMapTiers, Error> {
    let mut resolved = HashMapTiers::new();
    if requests.is_empty() {
        return Ok(resolved);
    }
    let c = crate::snapshot::connect(path)?;
    let ids = serde_json::to_string(
        &requests
            .iter()
            .map(|r| &r.native_turn_id)
            .collect::<Vec<_>>(),
    )?;
    let mut query=c.prepare("SELECT json_extract(attempt.payload_json,'$.nativeThreadId'), \
        json_extract(turn.payload_json,'$.nativeTurnRef.nativeId'), \
        (SELECT json_extract(event.payload_json,'$.modelSelection') FROM orchestration_events AS event \
         WHERE event.aggregate_kind='thread' AND event.application_event_version=2 \
         AND event.stream_id=attempt.thread_id AND event.event_type='run.updated' \
         AND json_extract(event.payload_json,'$.id')=attempt.run_id \
         AND json_extract(event.payload_json,'$.activeAttemptId')=attempt.attempt_id \
         AND json_extract(event.payload_json,'$.status')='running' ORDER BY event.sequence LIMIT 1) \
        FROM orchestration_v2_projection_provider_turns AS turn \
        LEFT JOIN orchestration_v2_projection_run_attempts AS attempt \
        ON attempt.attempt_id=turn.run_attempt_id AND attempt.thread_id=turn.thread_id \
        AND attempt.provider_thread_id=turn.provider_thread_id \
        WHERE json_extract(turn.payload_json,'$.nativeTurnRef.driver')='codex' \
        AND json_extract(turn.payload_json,'$.nativeTurnRef.nativeId') IN (SELECT value FROM json_each(?1))")?;
    let mut rows = query.query([ids])?;
    let mut by_turn = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let session: Option<String> = row.get(0)?;
        let turn: String = row.get(1)?;
        let selection: Option<String> = row.get(2)?;
        by_turn
            .entry(turn)
            .and_modify(|held| *held = None)
            .or_insert(Some((session, selection)));
    }
    for request in requests {
        let Some(Some((Some(session), Some(selection)))) = by_turn.get(&request.native_turn_id)
        else {
            continue;
        };
        if session != &request.session_id {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(selection) else {
            continue;
        };
        if v["model"] != request.model {
            continue;
        }
        let option = v["options"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["id"] == "serviceTier");
        let tier = option.map(|o| o["value"].as_str());
        if tier == Some(None) {
            continue;
        }
        let tier = tier.flatten().unwrap_or("default");
        if ["default", "standard", "fast", "priority", "ultrafast"].contains(&tier) {
            resolved.insert(request.clone(), UsageSpeed::from_tier(tier));
        }
    }
    Ok(resolved)
}
