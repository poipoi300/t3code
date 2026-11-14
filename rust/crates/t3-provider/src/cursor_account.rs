use crate::{
    sensitive::{SecretString, Sensitive, ServiceError, ServiceResult},
    transport::{HttpRequest, HttpResponse, HttpTransport, ProviderClock},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
    time::Duration,
};
pub use t3_usage::cache::{
    CURSOR_ACCOUNT_CACHE_FILE_NAME, CursorAccountCache, decode_cursor_account_caches,
    write_cursor_account_caches,
};
use t3_usage::{UsageProviderKind, UsageRecord, UsageSpeed, UsageTokenTotals};
use tokio::sync::{Mutex, Semaphore};
const ENDPOINT: &str = "https://api2.cursor.sh";
pub struct AccountUsage {
    pub account_key: Option<String>,
    pub records: Vec<UsageRecord>,
    pub missing: bool,
    pub error: Option<&'static str>,
}
struct Session {
    token: SecretString,
    account: String,
    teams: Vec<u64>,
    user: u64,
    expires: i64,
}
struct Cached {
    key: String,
    session: Arc<Session>,
}
pub struct CursorAccountReader {
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    sessions: Mutex<VecDeque<Cached>>,
    gates: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn safe_id(value: &Value) -> Option<u64> {
    value
        .as_f64()
        .filter(|id| *id > 0. && id.fract() == 0. && *id <= 9_007_199_254_740_991.)
        .map(|id| id as u64)
}
fn valid_body(value: &Value) -> bool {
    value.is_object()
        && ["error", "message", "code"]
            .iter()
            .all(|field| value.get(field).is_none())
}
fn integer(value: f64) -> Value {
    if value.fract() == 0. && value.abs() <= 9_007_199_254_740_991. {
        json!(value as i64)
    } else {
        json!(value)
    }
}
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let entries = object.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", json!(key), canonical(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Number(number) => number
            .as_f64()
            .map(integer)
            .unwrap_or(Value::Null)
            .to_string(),
        _ => value.to_string(),
    }
}
pub fn rate_model(model: &str) -> String {
    static SUFFIX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?:-thinking)?(?:-(?:none|minimal|low|medium|high|xhigh|max))?(?:-fast)?$",
        )
        .expect("Cursor suffix")
    });
    let base = SUFFIX.replace(model.strip_prefix("cursor-").unwrap_or(model), "");
    if base.starts_with("grok-") {
        format!("xai/{base}")
    } else {
        base.into_owned()
    }
}
impl CursorAccountReader {
    pub fn new(http: Arc<dyn HttpTransport>, clock: Arc<dyn ProviderClock>) -> Arc<Self> {
        Arc::new(Self {
            http,
            clock,
            sessions: Mutex::new(VecDeque::new()),
            gates: Mutex::new(BTreeMap::new()),
        })
    }
    async fn request(
        &self,
        method: &str,
        token: &str,
        body: Value,
        membership: bool,
    ) -> ServiceResult<HttpResponse> {
        let url = if method == "exchange_user_api_key" {
            format!("{ENDPOINT}/auth/{method}")
        } else {
            format!("{ENDPOINT}/aiserver.v1.DashboardService/{method}")
        };
        let mut headers = BTreeMap::from([
            (
                "authorization".into(),
                Sensitive::new(format!("Bearer {token}")),
            ),
            (
                "content-type".into(),
                Sensitive::new("application/json".into()),
            ),
        ]);
        if membership {
            headers.insert(
                "connect-protocol-version".into(),
                Sensitive::new("1".into()),
            );
        }
        tokio::time::timeout(
            Duration::from_secs(10),
            self.http.send(HttpRequest {
                method: "POST",
                url,
                headers,
                body: Some(Sensitive::new(body.to_string())),
            }),
        )
        .await
        .map_err(|_| ServiceError::Network)?
    }
    async fn cached(&self, key: &str) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.lock().await;
        let index = sessions
            .iter()
            .position(|entry| entry.key == key && entry.session.expires > self.clock.now_ms())?;
        let entry = sessions.remove(index)?;
        let session = entry.session.clone();
        sessions.push_back(entry);
        Some(session)
    }
    async fn session(&self, key: &SecretString) -> ServiceResult<Arc<Session>> {
        let digest = hash(key.expose());
        if let Some(session) = self.cached(&digest).await {
            return Ok(session);
        }
        let gate = {
            let mut gates = self.gates.lock().await;
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(&digest).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(digest.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let _permit = gate.lock().await;
        if let Some(session) = self.cached(&digest).await {
            return Ok(session);
        }
        let exchanged = self
            .request("exchange_user_api_key", key.expose(), json!({}), false)
            .await?;
        if !(200..300).contains(&exchanged.status) {
            return Err(ServiceError::Identity);
        }
        let token = exchanged.body.expose()["accessToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or(ServiceError::Identity)?;
        let payload = URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1).unwrap_or_default())
            .map_err(|_| ServiceError::Identity)?;
        let payload: Value =
            serde_json::from_slice(&payload).map_err(|_| ServiceError::Identity)?;
        let subject = payload["sub"]
            .as_str()
            .filter(|sub| !sub.is_empty())
            .ok_or(ServiceError::Identity)?;
        let membership = async |method| {
            let response = self.request(method, token, json!({}), true).await?;
            if !(200..300).contains(&response.status) || !valid_body(response.body.expose()) {
                Err(ServiceError::Response)
            } else {
                Ok(response.body.into_inner())
            }
        };
        let (teams, me) = tokio::try_join!(membership("GetTeams"), membership("GetMe"))?;
        let mut team_ids = vec![];
        if let Some(teams) = teams.get("teams") {
            for team in teams.as_array().ok_or(ServiceError::Response)? {
                let id = safe_id(&team["id"]).ok_or(ServiceError::Response)?;
                if !team_ids.contains(&id) {
                    team_ids.push(id);
                }
            }
        }
        let user = safe_id(&me["userId"]).ok_or(ServiceError::Response)?;
        let now = self.clock.now_ms();
        let ttl = payload["exp"]
            .as_f64()
            .map(|expiry| (expiry * 1000. - now as f64 - 30_000.).clamp(0., 300_000.) as i64)
            .unwrap_or(300_000);
        let session = Arc::new(Session {
            token: Sensitive::new(token.into()),
            account: hash(subject),
            teams: team_ids,
            user,
            expires: now.saturating_add(ttl),
        });
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|entry| entry.key != digest);
        sessions.push_back(Cached {
            key: digest,
            session: session.clone(),
        });
        while sessions.len() > 16 {
            sessions.pop_front();
        }
        Ok(session)
    }
    pub async fn read(
        self: &Arc<Self>,
        key: &SecretString,
        since: f64,
        until: f64,
    ) -> AccountUsage {
        let mut account_key = None;
        let work = async {
            if !since.is_finite() || !until.is_finite() || since < 0. || since > until {
                return Err(Failure::Invalid);
            }
            let session = self.session(key).await.map_err(|_| Failure::Invalid)?;
            account_key = Some(session.account.clone());
            let slots = Arc::new(Semaphore::new(6));
            let scopes = Arc::new(Semaphore::new(2));
            let mut jobs = tokio::task::JoinSet::new();
            for (index, team) in std::iter::once(None)
                .chain(session.teams.iter().copied().map(Some))
                .enumerate()
            {
                let reader = self.clone();
                let session = session.clone();
                let slots = slots.clone();
                let scopes = scopes.clone();
                jobs.spawn(async move {
                    let _scope = scopes.acquire_owned().await.map_err(|_| Failure::Invalid)?;
                    reader
                        .scope(session, slots, team, since, until)
                        .await
                        .map(|records| (index, records))
                });
            }
            let mut results = BTreeMap::new();
            while let Some(result) = jobs.join_next().await {
                let (index, records) = result.map_err(|_| Failure::Invalid)??;
                results.insert(index, records);
            }
            Ok(results.into_values().flatten().collect::<Vec<_>>())
        };
        let result = tokio::time::timeout(Duration::from_secs(60), work).await;
        let (records, error) = match result {
            Ok(Ok(records)) => (records, None),
            Ok(Err(Failure::Unauthorized)) => {
                self.sessions
                    .lock()
                    .await
                    .retain(|entry| entry.key != hash(key.expose()));
                (
                    vec![],
                    Some("Sign in to Cursor again to read account usage."),
                )
            }
            _ => (vec![], Some("Cursor account usage could not be read.")),
        };
        AccountUsage {
            account_key,
            records,
            missing: false,
            error,
        }
    }
    async fn page(
        &self,
        session: &Session,
        slots: Arc<Semaphore>,
        team: Option<u64>,
        page: usize,
        total: Option<usize>,
        window: (f64, f64),
    ) -> Result<Page, Failure> {
        let (since, until) = window;
        if page
            > total.map_or(1000, |total| {
                total.div_ceil(1000).saturating_mul(2).saturating_add(1)
            })
        {
            return Err(Failure::Invalid);
        }
        let _slot = slots.acquire_owned().await.map_err(|_| Failure::Invalid)?;
        let mut body = json!({"page":page,"pageSize":1000,"startDate":integer(since).to_string(),"endDate":integer(until).to_string()});
        if let Some(team) = team {
            body["teamId"] = json!(team);
            body["userId"] = json!(session.user);
        }
        let response = self
            .request("GetFilteredUsageEvents", session.token.expose(), body, true)
            .await
            .map_err(|_| Failure::Invalid)?;
        if matches!(response.status, 401 | 403) {
            return Err(Failure::Unauthorized);
        }
        let body = response.body.expose();
        if !(200..300).contains(&response.status) || !valid_body(body) {
            return Err(Failure::Invalid);
        }
        let empty = body.as_object().is_some_and(|body| body.is_empty());
        let count = if empty {
            Some(0)
        } else if let Some(value) = body.get("totalUsageEventsCount") {
            let value = value
                .as_f64()
                .filter(|count| {
                    *count >= 0. && count.fract() == 0. && *count <= 9_007_199_254_740_991.
                })
                .ok_or(Failure::Invalid)?;
            Some(value as usize)
        } else {
            None
        };
        let events = if empty
            || body
                .as_object()
                .is_some_and(|body| body.len() == 1 && body.contains_key("totalUsageEventsCount"))
        {
            vec![]
        } else {
            body["usageEventsDisplay"]
                .as_array()
                .cloned()
                .ok_or(Failure::Invalid)?
        };
        if events.len() > 1000 || total.zip(count).is_some_and(|(before, now)| before != now) {
            return Err(Failure::Invalid);
        }
        Ok(Page {
            events,
            total: count,
        })
    }
    async fn scope(
        self: Arc<Self>,
        session: Arc<Session>,
        slots: Arc<Semaphore>,
        team: Option<u64>,
        since: f64,
        until: f64,
    ) -> Result<Vec<UsageRecord>, Failure> {
        let mut total = None;
        let mut previous = vec![];
        let mut normalized = vec![];
        let mut pages = vec![];
        let mut ahead = VecDeque::new();
        for page in 1.. {
            let count = total
                .unwrap_or(0_usize)
                .div_ceil(1000)
                .saturating_sub(page - 1)
                .min(6);
            while ahead.len() < count {
                let reader = self.clone();
                let session = session.clone();
                let slots = slots.clone();
                let number = page + ahead.len();
                ahead.push_back(PageTask(tokio::spawn(async move {
                    reader
                        .page(&session, slots, team, number, total, (since, until))
                        .await
                })));
            }
            let next = if let Some(mut task) = ahead.pop_front() {
                (&mut task.0).await.map_err(|_| Failure::Invalid)??
            } else {
                self.page(&session, slots.clone(), team, page, total, (since, until))
                    .await?
            };
            if total
                .zip(next.total)
                .is_some_and(|(before, now)| before != now)
            {
                return Err(Failure::Invalid);
            }
            if next.total.is_some() {
                total = next.total;
            }
            let start = normalized.len();
            let overlap = overlap(&previous, &next.events);
            previous = next.events.clone();
            for event in &next.events {
                normalized.push(normalize(event, &session.account, team, since, until)?);
            }
            pages.push((start, normalized.len(), overlap));
            if next.events.len() < 1000 {
                break;
            }
            tokio::task::consume_budget().await;
        }
        if total.is_some_and(|total| normalized.len() < total) {
            return Err(Failure::Invalid);
        }
        let mut remove = total.map_or(0, |total| normalized.len() - total);
        let mut records = vec![];
        let mut occurrences = BTreeMap::new();
        for (start, end, overlap) in pages {
            let count = remove.min(overlap);
            remove -= count;
            for (offset, record) in normalized[start..end].iter_mut().enumerate() {
                if offset < count {
                    continue;
                }
                let Some(mut record) = record.take() else {
                    continue;
                };
                let key = record.dedupe_key.take().ok_or(Failure::Invalid)?;
                let occurrence = occurrences.entry(key.clone()).or_insert(0_u64);
                record.dedupe_key = Some(format!(
                    "cursor-account:{}:{key}:{occurrence}",
                    session.account
                ));
                *occurrence += 1;
                records.push(record);
            }
        }
        if remove != 0 {
            return Err(Failure::Invalid);
        }
        Ok(records)
    }
}
#[derive(Clone, Copy)]
enum Failure {
    Invalid,
    Unauthorized,
}
struct Page {
    events: Vec<Value>,
    total: Option<usize>,
}
struct PageTask(tokio::task::JoinHandle<Result<Page, Failure>>);
impl Drop for PageTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn overlap(previous: &[Value], current: &[Value]) -> usize {
    let previous = previous.iter().map(canonical).collect::<Vec<_>>();
    let current = current.iter().map(canonical).collect::<Vec<_>>();
    let sequence = current
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(""))
        .chain(previous.iter().map(String::as_str))
        .collect::<Vec<_>>();
    let mut lengths = vec![0; sequence.len()];
    for index in 1..sequence.len() {
        let mut length = lengths[index - 1];
        while length > 0 && sequence[index] != sequence[length] {
            length = lengths[length - 1];
        }
        if sequence[index] == sequence[length] {
            length += 1;
        }
        lengths[index] = length;
    }
    lengths.last().copied().unwrap_or(0)
}
fn normalize(
    event: &Value,
    _account: &str,
    team: Option<u64>,
    since: f64,
    until: f64,
) -> Result<Option<UsageRecord>, Failure> {
    let Some(usage) = event.get("tokenUsage").filter(|usage| !usage.is_null()) else {
        return Ok(None);
    };
    for field in [
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
        "totalCents",
    ] {
        if usage.get(field).is_some_and(|value| {
            !value
                .as_f64()
                .is_some_and(|value| value.is_finite() && value >= 0.)
        }) {
            return Err(Failure::Invalid);
        }
    }
    let raw_timestamp = &event["timestamp"];
    let timestamp = if raw_timestamp.is_string()
        && raw_timestamp
            .as_str()
            .is_some_and(|value| t3_contracts::wire::trim_js(value).is_empty())
    {
        None
    } else {
        crate::cursor_usage::number(raw_timestamp)
    }
    .ok_or(Failure::Invalid)?;
    let model = event["model"]
        .as_str()
        .filter(|model| !model.is_empty())
        .ok_or(Failure::Invalid)?;
    if timestamp < since || timestamp > until {
        return Ok(None);
    }
    let tokens = |field: &str| usage[field].as_f64().unwrap_or(0.).trunc() as u64;
    let totals = UsageTokenTotals {
        uncached_input_tokens: tokens("inputTokens"),
        cached_input_tokens: tokens("cacheReadTokens"),
        cache_creation_tokens: tokens("cacheWriteTokens"),
        output_tokens: tokens("outputTokens"),
        reasoning_tokens: 0,
    };
    let reported_cost = usage["totalCents"].as_f64().map(|cents| cents / 100.);
    let session = event["conversationId"].as_str().unwrap_or_default();
    let mut identity = vec![];
    if let Some(team) = team {
        identity.push(json!(team));
    }
    identity.extend([integer(timestamp),json!(model),json!(session),json!({"uncachedInputTokens":totals.uncached_input_tokens,"cachedInputTokens":totals.cached_input_tokens,"cacheCreationTokens":totals.cache_creation_tokens,"outputTokens":totals.output_tokens,"reasoningTokens":0}),reported_cost.map(integer).unwrap_or(Value::Null)]);
    let key = hash(&Value::Array(identity).to_string());
    Ok(Some(UsageRecord {
        provider: UsageProviderKind::Cursor,
        timestamp_ms: timestamp,
        model: model.into(),
        rate_model: Some(rate_model(model)),
        session_id: session.into(),
        totals,
        cache_creation1h_tokens: None,
        reported_cost_usd: reported_cost,
        speed: UsageSpeed::Standard,
        unresolved_service_tier_turn_id: None,
        dedupe_key: Some(key),
    }))
}
