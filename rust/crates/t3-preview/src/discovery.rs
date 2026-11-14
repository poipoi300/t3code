use crate::{
    Error, Result, lock, now,
    urls::{configured_urls, is_loopback_host},
};
use futures_util::{FutureExt, StreamExt, stream};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use t3_contracts::preview::*;
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const AGENT_THREAD_ENV: &str = "T3CODE_THREAD_ID";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub pid: Option<u64>,
    pub process_name: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u64,
    pub ppid: u64,
    pub pgid: u64,
}
pub fn parse_process_table(raw: &str) -> BTreeMap<u64, ProcessRow> {
    raw.lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 {
                return None;
            }
            let row = ProcessRow {
                pid: fields[0].parse().ok()?,
                ppid: fields[1].parse().ok()?,
                pgid: fields[2].parse().ok()?,
            };
            Some((row.pid, row))
        })
        .collect()
}
pub fn stoppable_process_group(
    table: &BTreeMap<u64, ProcessRow>,
    pid: u64,
    server_pid: u64,
) -> Option<u64> {
    let group = table.get(&pid)?.pgid;
    if group <= 1 {
        return None;
    }
    let mut guarded = BTreeSet::from([server_pid]);
    let mut parent = table.get(&server_pid).map(|r| r.ppid);
    while let Some(pid) = parent.filter(|pid| *pid > 1 && !guarded.contains(pid)) {
        guarded.insert(pid);
        parent = table.get(&pid).map(|r| r.ppid);
    }
    if table
        .values()
        .any(|row| row.pgid == group && (guarded.contains(&row.pid) || row.ppid == server_pid))
    {
        return None;
    }
    Some(group)
}
fn local_endpoint(name: &str) -> Option<u16> {
    let endpoint = name.split_whitespace().next()?;
    let (host, port) = endpoint.rsplit_once(':')?;
    if !is_loopback_host(host) && !matches!(host, "*" | "[::]" | "::") {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    (port > 0).then_some(port)
}
pub fn parse_lsof(raw: &str) -> Vec<Listener> {
    let mut pid = None;
    let mut name = None;
    let mut listeners = Vec::new();
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u64>().ok().filter(|p| *p > 0);
            name = None;
        } else if let Some(value) = line.strip_prefix('c') {
            name = (!value.trim().is_empty()).then(|| value.trim().into());
        } else if let Some(value) = line.strip_prefix('n')
            && let Some(port) = local_endpoint(value)
        {
            listeners.push(Listener {
                port,
                pid,
                process_name: name.clone(),
            });
        }
    }
    listeners
}
pub fn parse_windows_listeners(raw: &str) -> Vec<Listener> {
    raw.lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4
                || !fields[0].eq_ignore_ascii_case("TCP")
                || !fields[2].ends_with(":0")
            {
                return None;
            }
            Some(Listener {
                port: local_endpoint(fields[1])?,
                pid: fields.last()?.parse::<u64>().ok().filter(|pid| *pid > 0),
                process_name: None,
            })
        })
        .collect()
}
pub fn parse_proc_tcp(raw: &str) -> Vec<(u16, String)> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10 || fields[3] != "0A" {
                return None;
            }
            let (address, port) = fields[1].split_once(':')?;
            let address = address.to_ascii_uppercase();
            let local = matches!(
                address.as_str(),
                "00000000"
                    | "0100007F"
                    | "00000000000000000000000000000000"
                    | "00000000000000000000000001000000"
            ) || address.starts_with("0000000000000000FFFF0000")
                && matches!(&address[24..], "00000000" | "0100007F");
            if !local {
                return None;
            }
            let port = u16::from_str_radix(port, 16).ok()?;
            if port == 0 {
                return None;
            }
            Some((port, fields[9].into()))
        })
        .collect()
}
pub fn parse_proc_environment(raw: &str) -> Option<String> {
    raw.split('\0')
        .find_map(|item| item.strip_prefix("T3CODE_THREAD_ID="))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn probe_is_web(status: u16, content_type: Option<&str>, location: Option<&str>) -> bool {
    if matches!(status, 301 | 302 | 303 | 307 | 308)
        && location.is_some_and(|l| !l.trim().is_empty())
    {
        return true;
    }
    (200..300).contains(&status)
        && !matches!(status, 204 | 205)
        && content_type.is_some_and(|t| {
            matches!(
                t.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "text/html" | "application/xhtml+xml"
            )
        })
}

struct Probe {
    pid: Option<u64>,
    web: bool,
    expires: Instant,
}
#[derive(Default)]
struct ScanCache {
    probes: BTreeMap<String, Probe>,
    socket_owners: BTreeMap<String, (u64, Option<String>)>,
    socket_misses: BTreeMap<String, u8>,
    threads: BTreeMap<u64, Option<String>>,
    names: BTreeMap<u64, (Option<String>, Instant)>,
    lsof_missing: bool,
}
struct Subscription {
    urls: Vec<String>,
    last: Vec<DiscoveredLocalServer>,
    tx: mpsc::UnboundedSender<DiscoveredLocalServerList>,
}
struct State {
    terminals: BTreeMap<(String, String), BTreeSet<u64>>,
    subscriptions: BTreeMap<u64, Subscription>,
    next: u64,
    retained: usize,
    agents: usize,
    agent_pending: bool,
}
struct Inner {
    client: reqwest::Client,
    state: Mutex<State>,
    cache: tokio::sync::Mutex<ScanCache>,
    enabled: bool,
    proc_root: PathBuf,
    stop: CancellationToken,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
#[derive(Clone)]
pub struct PortDiscovery {
    inner: Arc<Inner>,
}
pub struct DiscoverySubscription {
    owner: Weak<Inner>,
    id: u64,
    pub events: mpsc::UnboundedReceiver<DiscoveredLocalServerList>,
}
pub struct DiscoveryRetention {
    owner: Weak<Inner>,
}
pub struct AgentTracking {
    owner: Weak<Inner>,
}
impl Drop for DiscoverySubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.owner.upgrade() {
            lock(&inner.state).subscriptions.remove(&self.id);
        }
    }
}
impl Drop for DiscoveryRetention {
    fn drop(&mut self) {
        if let Some(inner) = self.owner.upgrade() {
            let mut state = lock(&inner.state);
            state.retained = state.retained.saturating_sub(1);
            if state.retained == 0 {
                state.agent_pending = true;
            }
        }
    }
}
impl Drop for AgentTracking {
    fn drop(&mut self) {
        if let Some(inner) = self.owner.upgrade() {
            let mut state = lock(&inner.state);
            state.agents = state.agents.saturating_sub(1);
            state.agent_pending = true;
        }
    }
}

impl PortDiscovery {
    pub fn new() -> Result<Self> {
        Self::with_options(
            std::env::var("T3CODE_PORT_DISCOVERY").as_deref() != Ok("0"),
            PathBuf::from("/proc"),
        )
    }
    pub fn with_options(enabled: bool, proc_root: PathBuf) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()
            .map_err(|e| Error::Io(e.to_string()))?;
        let inner = Arc::new(Inner {
            client,
            state: Mutex::new(State {
                terminals: BTreeMap::new(),
                subscriptions: BTreeMap::new(),
                next: 0,
                retained: 0,
                agents: 0,
                agent_pending: true,
            }),
            cache: tokio::sync::Mutex::new(ScanCache::default()),
            enabled,
            proc_root,
            stop: CancellationToken::new(),
        });
        let weak = Arc::downgrade(&inner);
        let stop = inner.stop.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            tick.tick().await;
            loop {
                tokio::select! {_=stop.cancelled()=>break,_=tick.tick()=>{let Some(inner)=weak.upgrade()else{break;};let retained={let state=lock(&inner.state);state.retained>0||!state.subscriptions.is_empty()};if retained {let _=Self {inner}.poll().await;}}}
            }
        });
        Ok(Self { inner })
    }
    pub fn register_terminal_processes(&self, thread: &str, terminal: &str, pids: &[u64]) {
        let mut state = lock(&self.inner.state);
        let key = (thread.into(), terminal.into());
        let pids: BTreeSet<_> = pids.iter().copied().filter(|pid| *pid > 0).collect();
        if pids.is_empty() {
            state.terminals.remove(&key);
        } else {
            state.terminals.insert(key, pids);
        }
    }
    pub fn unregister_terminal(&self, thread: &str, terminal: &str) {
        lock(&self.inner.state)
            .terminals
            .remove(&(thread.into(), terminal.into()));
    }
    pub fn track_agent(&self) -> AgentTracking {
        lock(&self.inner.state).agents += 1;
        AgentTracking {
            owner: Arc::downgrade(&self.inner),
        }
    }
    pub async fn retain(&self) -> Result<DiscoveryRetention> {
        let was_idle = {
            let mut state = lock(&self.inner.state);
            let idle = state.retained == 0;
            state.retained += 1;
            idle
        };
        let retention = DiscoveryRetention {
            owner: Arc::downgrade(&self.inner),
        };
        if was_idle {
            let mut cache = self.inner.cache.lock().await;
            cache.socket_owners.clear();
            cache.socket_misses.clear();
            cache.threads.clear();
            drop(cache);
            self.poll().await?;
        }
        Ok(retention)
    }
    pub async fn subscribe(&self, urls: Vec<String>) -> Result<DiscoverySubscription> {
        let urls = configured_urls(&urls);
        let initial = self.scan(&urls).await?;
        let (tx, events) = mpsc::unbounded_channel();
        let _ = tx.send(list_event(initial.clone()));
        let id = {
            let mut state = lock(&self.inner.state);
            let id = state.next;
            state.next += 1;
            state.subscriptions.insert(
                id,
                Subscription {
                    urls,
                    last: initial,
                    tx,
                },
            );
            id
        };
        Ok(DiscoverySubscription {
            owner: Arc::downgrade(&self.inner),
            id,
            events,
        })
    }
    pub async fn poll(&self) -> Result<()> {
        let subscriptions = {
            lock(&self.inner.state)
                .subscriptions
                .iter()
                .map(|(id, s)| (*id, s.urls.clone()))
                .collect::<Vec<_>>()
        };
        let urls = subscriptions
            .iter()
            .flat_map(|(_, urls)| urls.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let snapshot = self.snapshot(&urls).await?;
        let mut state = lock(&self.inner.state);
        for (id, urls) in subscriptions {
            let projected = project_snapshot(&snapshot, &urls);
            if let Some(subscription) = state.subscriptions.get_mut(&id)
                && subscription.last != projected
            {
                subscription.last = projected.clone();
                let _ = subscription.tx.send(list_event(projected));
            }
        }
        state
            .subscriptions
            .retain(|_, subscription| !subscription.tx.is_closed());
        Ok(())
    }
    pub async fn scan(&self, urls: &[String]) -> Result<Vec<DiscoveredLocalServer>> {
        let urls = configured_urls(urls);
        Ok(project_snapshot(&self.snapshot(&urls).await?, &urls))
    }
    async fn snapshot(&self, urls: &[String]) -> Result<Snapshot> {
        let mut cache = self.inner.cache.lock().await;
        let (terminal_owners, agents_possible) = {
            let state = lock(&self.inner.state);
            let mut owners = BTreeMap::new();
            for ((thread, terminal), pids) in &state.terminals {
                for pid in pids {
                    owners.insert(
                        *pid,
                        DiscoveredLocalServerTerminal {
                            thread_id: thread.as_str().into(),
                            terminal_id: terminal.as_str().into(),
                        },
                    );
                }
            }
            let agents = !cfg!(windows)
                && (state.agents > 0
                    || state.agent_pending
                    || cache.threads.values().any(Option::is_some));
            (owners, agents)
        };
        let (listeners, listeners_succeeded) = if self.inner.enabled
            && (!terminal_owners.is_empty() || !urls.is_empty() || agents_possible)
        {
            match self.listeners(&mut cache).await {
                Ok(listeners) => (listeners, true),
                Err(error) => {
                    tracing::debug!(%error,"preview listener discovery failed");
                    (vec![], false)
                }
            }
        } else {
            (vec![], false)
        };
        let pids = listeners
            .iter()
            .filter_map(|l| l.pid)
            .collect::<BTreeSet<_>>();
        if agents_possible && listeners_succeeded {
            let unread = pids
                .iter()
                .filter(|pid| {
                    !cache.threads.contains_key(pid) && !terminal_owners.contains_key(pid)
                })
                .copied()
                .collect::<Vec<_>>();
            let environments = self.agent_threads(&unread).await;
            if let Ok(environments) = environments {
                cache.threads.extend(environments);
                lock(&self.inner.state).agent_pending = false;
            }
            cache.threads.retain(|pid, _| pids.contains(pid));
        }
        let mut servers: BTreeMap<u16, DiscoveredLocalServer> = BTreeMap::new();
        for listener in listeners {
            let terminal = listener
                .pid
                .and_then(|pid| terminal_owners.get(&pid))
                .cloned();
            let agent = if terminal.is_none() {
                listener
                    .pid
                    .and_then(|pid| cache.threads.get(&pid))
                    .cloned()
                    .flatten()
                    .map(|thread| DiscoveredLocalServerAgent {
                        thread_id: thread.as_str().into(),
                    })
            } else {
                None
            };
            let owned = terminal.is_some() || agent.is_some();
            if let Some(existing) = servers.get_mut(&listener.port) {
                if !owned {
                    existing.terminal = None;
                    existing.agent = Some(None);
                }
                continue;
            }
            servers.insert(
                listener.port,
                DiscoveredLocalServer {
                    host: "localhost".into(),
                    port: listener.port,
                    url: format!("http://localhost:{}", listener.port).into(),
                    pid: listener.pid,
                    process_name: listener.process_name.map(Into::into),
                    terminal,
                    agent: Some(agent),
                },
            );
        }
        let mut resources = Vec::new();
        let mut seen = BTreeSet::new();
        for raw in urls {
            let Ok(url) = Url::parse(raw) else {
                continue;
            };
            let Some(port) = url.port_or_known_default() else {
                continue;
            };
            let mut key = url.clone();
            key.set_fragment(None);
            let key = key.to_string();
            if !seen.insert(key.clone()) {
                continue;
            }
            let server = servers
                .get(&port)
                .cloned()
                .unwrap_or(DiscoveredLocalServer {
                    host: url.host_str().unwrap_or("localhost").into(),
                    port,
                    url: raw.as_str().into(),
                    process_name: None,
                    pid: None,
                    terminal: None,
                    agent: None,
                });
            resources.push((key, raw.clone(), server));
        }
        let client = self.inner.client.clone();
        let now = Instant::now();
        let jobs = resources
            .into_iter()
            .map(|(key, url, server)| {
                let cached = cache
                    .probes
                    .get(&key)
                    .filter(|probe| probe.pid == server.pid && probe.expires > now)
                    .map(|p| p.web);
                let client = client.clone();
                async move {
                    let web = if let Some(web) = cached {
                        web
                    } else {
                        match client.get(&url).send().await {
                            Ok(response) => probe_is_web(
                                response.status().as_u16(),
                                response
                                    .headers()
                                    .get("content-type")
                                    .and_then(|v| v.to_str().ok()),
                                response
                                    .headers()
                                    .get("location")
                                    .and_then(|v| v.to_str().ok()),
                            ),
                            Err(_) => false,
                        }
                    };
                    (key, url, server, web, cached.is_none())
                }
                .boxed()
            })
            .collect::<Vec<_>>();
        let probes = stream::iter(jobs)
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
        let completed = Instant::now();
        cache.probes.retain(|_, probe| probe.expires > completed);
        let mut configured = BTreeMap::new();
        for (key, url, mut server, web, fresh) in probes {
            if fresh {
                cache.probes.insert(
                    key.clone(),
                    Probe {
                        pid: server.pid,
                        web,
                        expires: completed + Duration::from_secs(15),
                    },
                );
            }
            if web {
                server.url = url.into();
                configured.insert(key, server);
            }
        }
        let mut snapshot = Snapshot {
            discovered: servers.into_values().filter(is_owned).collect(),
            configured,
        };
        if cfg!(windows) {
            self.process_names(&mut snapshot, &mut cache).await;
        }
        Ok(snapshot)
    }
    async fn listeners(&self, cache: &mut ScanCache) -> Result<Vec<Listener>> {
        if cfg!(windows) {
            let (v4, v6) = tokio::join!(
                run("netstat.exe", &["-ano", "-p", "tcp"], 16 * 1024 * 1024),
                run("netstat.exe", &["-ano", "-p", "tcpv6"], 16 * 1024 * 1024)
            );
            return Ok(parse_windows_listeners(&(v4? + "\n" + &v6?)));
        }
        if cfg!(target_os = "linux")
            && let Ok(listeners) = self.proc_listeners(cache).await
        {
            return Ok(listeners);
        }
        if !cache.lsof_missing {
            match run(
                "lsof",
                &["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-F", "pcn"],
                1024 * 1024,
            )
            .await
            {
                Ok(output) => return Ok(parse_lsof(&output)),
                Err(Error::Io(error)) if error.contains("executable not found") => {
                    cache.lsof_missing = true
                }
                Err(error) => return Err(error),
            }
        }
        if cfg!(target_os = "linux") {
            return self.proc_listeners(cache).await;
        }
        Ok(vec![])
    }
    async fn proc_listeners(&self, cache: &mut ScanCache) -> Result<Vec<Listener>> {
        let mut tables = Vec::new();
        for name in ["net/tcp", "net/tcp6"] {
            match tokio::fs::read_to_string(self.inner.proc_root.join(name)).await {
                Ok(table) => {
                    let header = table.lines().next().unwrap_or("");
                    if !header.contains("local_address") || !header.contains(" st ") {
                        return Err(Error::Io("Unusable proc TCP table".into()));
                    }
                    tables.push(table);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if tables.is_empty() {
            return Err(Error::Io("No proc TCP tables".into()));
        }
        let sockets = tables
            .iter()
            .flat_map(|table| parse_proc_tcp(table))
            .collect::<Vec<_>>();
        let inodes = sockets
            .iter()
            .map(|(_, inode)| inode.clone())
            .collect::<BTreeSet<_>>();
        let unseen = inodes
            .iter()
            .filter(|inode| {
                !cache.socket_owners.contains_key(*inode)
                    && cache.socket_misses.get(*inode).copied().unwrap_or(0) < 3
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if !unseen.is_empty() {
            let root = self.inner.proc_root.clone();
            let wanted = unseen.clone();
            let allow_partial = cache.lsof_missing;
            let found = tokio::task::spawn_blocking(move || {
                find_socket_owners(&root, &wanted, allow_partial)
            })
            .await
            .map_err(|e| Error::Io(e.to_string()))??;
            for inode in unseen {
                if !found.contains_key(&inode) {
                    *cache.socket_misses.entry(inode).or_default() += 1;
                }
            }
            cache.socket_owners.extend(found);
        }
        cache
            .socket_owners
            .retain(|inode, _| inodes.contains(inode));
        cache
            .socket_misses
            .retain(|inode, _| inodes.contains(inode));
        Ok(sockets
            .into_iter()
            .map(|(port, inode)| {
                let owner = cache.socket_owners.get(&inode);
                Listener {
                    port,
                    pid: owner.map(|(pid, _)| *pid),
                    process_name: owner.and_then(|(_, name)| name.clone()),
                }
            })
            .collect())
    }
    async fn agent_threads(&self, pids: &[u64]) -> Result<BTreeMap<u64, Option<String>>> {
        if pids.is_empty() {
            return Ok(BTreeMap::new());
        }
        if cfg!(target_os = "linux") {
            let root = self.inner.proc_root.clone();
            let futures: Vec<futures_util::future::BoxFuture<'static, _>> = pids
                .iter()
                .copied()
                .map(|pid| {
                    let root = root.clone();
                    async move {
                        let raw = tokio::fs::read_to_string(root.join(format!("{pid}/environ")))
                            .await
                            .unwrap_or_default();
                        (pid, parse_proc_environment(&raw))
                    }
                    .boxed()
                })
                .collect();
            Ok(stream::iter(futures).buffer_unordered(16).collect().await)
        } else if cfg!(target_os = "macos") {
            let pids = pids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let raw = run(
                "ps",
                &["-E", "-ww", "-o", "pid=,command=", "-p", &pids],
                8 * 1024 * 1024,
            )
            .await?;
            let mut result = BTreeMap::new();
            for line in raw.lines() {
                let mut words = line.split_whitespace();
                if let Some(pid) = words.next().and_then(|v| v.parse::<u64>().ok()) {
                    let thread = words
                        .find_map(|word| word.strip_prefix("T3CODE_THREAD_ID="))
                        .map(str::to_owned);
                    result.insert(pid, thread);
                }
            }
            Ok(result)
        } else {
            Ok(BTreeMap::new())
        }
    }
    async fn process_names(&self, snapshot: &mut Snapshot, cache: &mut ScanCache) {
        let pids = snapshot
            .discovered
            .iter()
            .chain(snapshot.configured.values())
            .filter_map(|s| s.pid)
            .collect::<BTreeSet<_>>();
        let now = Instant::now();
        cache
            .names
            .retain(|pid, (_, expires)| pids.contains(pid) && *expires > now);
        let unread = pids
            .into_iter()
            .filter(|pid| !cache.names.contains_key(pid))
            .collect::<Vec<_>>();
        let jobs = unread
            .into_iter()
            .map(|pid| {
                async move {
                    let filter = format!("PID eq {pid}");
                    let output = run(
                        "tasklist.exe",
                        &["/FO", "CSV", "/NH", "/FI", &filter],
                        64 * 1024,
                    )
                    .await
                    .ok()?;
                    let name = output.lines().find_map(|line| {
                        let fields = line
                            .trim()
                            .trim_matches('"')
                            .split("\",\"")
                            .collect::<Vec<_>>();
                        if fields.len() < 2 || fields[1].parse::<u64>().ok() != Some(pid) {
                            return None;
                        }
                        Some(
                            fields[0]
                                .strip_suffix(".exe")
                                .unwrap_or(fields[0])
                                .to_owned(),
                        )
                    });
                    Some((pid, (name, now + Duration::from_secs(60))))
                }
                .boxed()
            })
            .collect::<Vec<_>>();
        let fetched = stream::iter(jobs)
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        cache.names.extend(fetched.into_iter().flatten());
        for server in snapshot
            .discovered
            .iter_mut()
            .chain(snapshot.configured.values_mut())
        {
            if server.process_name.is_none() {
                server.process_name = server
                    .pid
                    .and_then(|pid| cache.names.get(&pid))
                    .and_then(|(name, _)| name.clone())
                    .map(Into::into);
            }
        }
    }
    pub async fn stop_server(&self, input: PreviewStopLocalServerInput) -> Result<()> {
        self.stop_owned_server(input).await?;
        self.poll().await
    }
    async fn stop_owned_server(&self, input: PreviewStopLocalServerInput) -> Result<()> {
        if cfg!(windows) {
            return Err(Error::Stop(PreviewStopLocalServerReason::Unsupported));
        }
        {
            let mut cache = self.inner.cache.lock().await;
            cache.socket_owners.retain(|_, (pid, _)| *pid != input.pid);
            cache.threads.remove(&input.pid);
        }
        lock(&self.inner.state).agent_pending = true;
        let listed = self.scan(&[]).await?;
        authorize_stop(&listed, &input)?;
        let table = read_process_table().await?;
        if !table.contains_key(&input.pid) {
            return Ok(());
        }
        let group = stoppable_process_group(&table, input.pid, std::process::id() as u64)
            .ok_or(Error::Stop(PreviewStopLocalServerReason::SharedGroup))?;
        if !signal_group(group, false)? {
            return Ok(());
        }
        let members: Vec<_> = table
            .values()
            .filter(|r| r.pgid == group)
            .map(|r| r.pid)
            .collect();
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let current = match read_process_table().await {
                Ok(table) => table,
                Err(_) => return Ok(()),
            };
            if !members
                .iter()
                .any(|pid| current.get(pid).is_some_and(|r| r.pgid == group))
            {
                return Ok(());
            }
        }
        signal_group(group, true)?;
        Ok(())
    }
}
pub fn authorize_stop(
    listed: &[DiscoveredLocalServer],
    input: &PreviewStopLocalServerInput,
) -> Result<()> {
    if !listed
        .iter()
        .any(|s| s.pid == Some(input.pid) && s.port == input.port && is_owned(s))
    {
        return Err(Error::Stop(PreviewStopLocalServerReason::NotFound));
    }
    Ok(())
}
async fn read_process_table() -> Result<BTreeMap<u64, ProcessRow>> {
    let output = run("ps", &["-A", "-o", "pid=,ppid=,pgid="], 8 * 1024 * 1024)
        .await
        .map_err(|_| Error::Stop(PreviewStopLocalServerReason::SignalFailed))?;
    let table = parse_process_table(&output);
    if table.is_empty() {
        return Err(Error::Stop(PreviewStopLocalServerReason::SignalFailed));
    }
    Ok(table)
}
#[cfg(unix)]
fn signal_group(group: u64, kill: bool) -> Result<bool> {
    let pid = i32::try_from(group)
        .map_err(|_| Error::Stop(PreviewStopLocalServerReason::SignalFailed))?;
    match nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid),
        if kill {
            nix::sys::signal::Signal::SIGKILL
        } else {
            nix::sys::signal::Signal::SIGTERM
        },
    ) {
        Ok(()) => Ok(true),
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(_) => Err(Error::Stop(PreviewStopLocalServerReason::SignalFailed)),
    }
}
#[cfg(not(unix))]
fn signal_group(_: u64, _: bool) -> Result<bool> {
    Err(Error::Stop(PreviewStopLocalServerReason::Unsupported))
}
async fn run(program: &str, args: &[&str], limit: usize) -> Result<String> {
    let mut options = t3_process::ProcessOptions::new(program).args(args.iter().copied());
    options.timeout = Some(Duration::from_secs(5));
    options.max_output_bytes = limit;
    let output = t3_process::run(options).await?;
    Ok(output.stdout_text())
}
fn find_socket_owners(
    root: &Path,
    inodes: &BTreeSet<String>,
    allow_partial: bool,
) -> Result<BTreeMap<String, (u64, Option<String>)>> {
    let mut pids = std::fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u64>().ok())
        .collect::<Vec<_>>();
    pids.sort_unstable();
    let mut budget = 50_000usize;
    let mut owners = BTreeMap::new();
    for pid in pids {
        if owners.len() == inodes.len() || budget == 0 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(root.join(format!("{pid}/fd"))) else {
            continue;
        };
        let mut matched = Vec::new();
        for fd in entries.filter_map(std::result::Result::ok).take(budget) {
            budget -= 1;
            if let Ok(link) = std::fs::read_link(fd.path()) {
                let text = link.to_string_lossy();
                if let Some(inode) = text
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                    && inodes.contains(inode)
                    && !owners.contains_key(inode)
                {
                    matched.push(inode.to_owned());
                }
            }
        }
        let name = std::fs::read_to_string(root.join(format!("{pid}/comm")))
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        for inode in matched {
            owners.insert(inode, (pid, name.clone()));
        }
    }
    if budget == 0 && owners.len() < inodes.len() && !allow_partial {
        return Err(Error::Io("Proc FD budget exhausted".into()));
    }
    Ok(owners)
}
struct Snapshot {
    discovered: Vec<DiscoveredLocalServer>,
    configured: BTreeMap<String, DiscoveredLocalServer>,
}
fn project_snapshot(snapshot: &Snapshot, urls: &[String]) -> Vec<DiscoveredLocalServer> {
    let mut visible = BTreeMap::new();
    let mut preferred = BTreeMap::new();
    for raw in urls {
        let Ok(mut url) = Url::parse(raw) else {
            continue;
        };
        let Some(port) = url.port_or_known_default() else {
            continue;
        };
        preferred.entry(port).or_insert(raw);
        url.set_fragment(None);
        if let Some(server) = snapshot.configured.get(url.as_str())
            && !visible.contains_key(&port)
        {
            let mut server = server.clone();
            server.url = raw.as_str().into();
            visible.insert(port, server);
        }
    }
    for server in &snapshot.discovered {
        visible.entry(server.port).or_insert_with(|| {
            let mut server = server.clone();
            if let Some(url) = preferred.get(&server.port) {
                server.url = url.as_str().into();
            }
            server
        });
    }
    visible.into_values().collect()
}
fn is_owned(server: &DiscoveredLocalServer) -> bool {
    server.terminal.is_some() || server.agent.as_ref().and_then(Option::as_ref).is_some()
}
fn list_event(servers: Vec<DiscoveredLocalServer>) -> DiscoveredLocalServerList {
    DiscoveredLocalServerList {
        servers,
        scanned_at: now(),
        configured_url_probing: Some(Some(PreviewConfiguredUrlProbing)),
    }
}
