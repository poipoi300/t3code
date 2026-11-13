use crate::{UsageProviderKind, UsageRecord, UsageSpeed, UsageTokenTotals, text, timestamp_ms};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl TokenUsage {
    pub(crate) fn is_valid(&self) -> bool {
        Self::from_values(self.values()).as_ref() == Some(self)
    }
    fn values(&self) -> [i64; 6] {
        [
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_input_tokens,
            self.output_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ]
    }
    fn from_values(v: [i64; 6]) -> Option<Self> {
        if v.iter().any(|n| *n < 0 || *n > 9_007_199_254_740_991)
            || v[1] + v[2] > v[0]
            || v[4] > v[3]
            || v[5] < v[0] + v[3]
        {
            return None;
        }
        Some(Self {
            input_tokens: v[0],
            cached_input_tokens: v[1],
            cache_write_input_tokens: v[2],
            output_tokens: v[3],
            reasoning_output_tokens: v[4],
            total_tokens: v[5],
        })
    }
    fn read(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let count = |key: &str, fallback| -> Option<i64> {
            match obj.get(key) {
                None => Some(fallback),
                Some(n) => n
                    .as_f64()
                    .filter(|n| {
                        n.is_finite()
                            && n.fract() == 0.0
                            && *n >= 0.0
                            && *n <= 9_007_199_254_740_991.0
                    })
                    .map(|n| n as i64),
            }
        };
        let input = count("input_tokens", -1)?;
        let output = count("output_tokens", -1)?;
        Self::from_values([
            input,
            count("cached_input_tokens", 0)?,
            count("cache_write_input_tokens", 0)?,
            output,
            count("reasoning_output_tokens", 0)?,
            count("total_tokens", input + output)?,
        ])
    }
    fn plus(previous: Option<&Self>, last: &Self) -> Option<Self> {
        let p = previous.map(Self::values).unwrap_or([0; 6]);
        let l = last.values();
        Self::from_values(std::array::from_fn(|i| p[i] + l[i]))
    }
    fn minus(&self, other: &Self) -> Option<Self> {
        let a = self.values();
        let b = other.values();
        Self::from_values(std::array::from_fn(|i| a[i] - b[i]))
    }
    fn signature(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub timestamp_ms: f64,
    pub model: String,
    pub turn_id: String,
    pub speed: UsageSpeed,
    pub saw_thread_settings: bool,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexScanState {
    pub model: String,
    pub speed: UsageSpeed,
    pub session_id: String,
    pub last_usage_signature: Option<String>,
    pub last_cumulative_usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_start_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_cumulative_at_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreported_requests: Option<Vec<PendingRequest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_ledger_active: Option<bool>,
    pub malformed_records: u64,
    pub saw_session_meta: bool,
    pub model_provider: String,
    pub turn_id: String,
    pub saw_thread_settings: bool,
    pub suppressing_fork_copies: bool,
    pub fork_copy_anchor_ms: f64,
}

impl Default for CodexScanState {
    fn default() -> Self {
        Self {
            model: String::new(),
            speed: UsageSpeed::Standard,
            session_id: String::new(),
            last_usage_signature: None,
            last_cumulative_usage: None,
            sequence_start_signature: None,
            latest_cumulative_at_ms: None,
            unreported_requests: None,
            request_ledger_active: None,
            malformed_records: 0,
            saw_session_meta: false,
            model_provider: String::new(),
            turn_id: String::new(),
            saw_thread_settings: false,
            suppressing_fork_copies: false,
            fork_copy_anchor_ms: 0.0,
        }
    }
}

impl CodexScanState {
    fn fork_copy(&mut self, at: f64) -> bool {
        if !self.suppressing_fork_copies {
            return false;
        }
        if at - self.fork_copy_anchor_ms < 1000.0 {
            self.fork_copy_anchor_ms = at;
            return true;
        }
        self.suppressing_fork_copies = false;
        self.last_usage_signature = None;
        false
    }
    fn rebase(&mut self, usage: TokenUsage, at: f64) {
        if usage.input_tokens + usage.output_tokens == 0 {
            self.sequence_start_signature = None;
        }
        self.last_cumulative_usage = Some(usage);
        self.latest_cumulative_at_ms = Some(self.latest_cumulative_at_ms.unwrap_or(at).max(at));
        self.unreported_requests = None;
    }
    fn attribution(&self, usage: TokenUsage, at: f64) -> PendingRequest {
        PendingRequest {
            timestamp_ms: at,
            model: self.model.clone(),
            turn_id: self.turn_id.clone(),
            speed: self.speed,
            saw_thread_settings: self.saw_thread_settings,
            usage,
        }
    }
    fn record(&self, request: &PendingRequest) -> UsageRecord {
        let u = &request.usage;
        UsageRecord {
            provider: UsageProviderKind::Codex,
            timestamp_ms: request.timestamp_ms,
            model: request.model.clone(),
            rate_model: None,
            session_id: self.session_id.clone(),
            totals: UsageTokenTotals {
                uncached_input_tokens: (u.input_tokens
                    - u.cached_input_tokens
                    - u.cache_write_input_tokens) as u64,
                cached_input_tokens: u.cached_input_tokens as u64,
                cache_creation_tokens: u.cache_write_input_tokens as u64,
                output_tokens: u.output_tokens as u64,
                reasoning_tokens: u.reasoning_output_tokens as u64,
            },
            cache_creation1h_tokens: None,
            reported_cost_usd: None,
            speed: request.speed,
            unresolved_service_tier_turn_id: (!request.saw_thread_settings
                && self.model_provider == "openai"
                && !self.session_id.is_empty()
                && !request.turn_id.is_empty())
            .then(|| request.turn_id.clone()),
            dedupe_key: None,
        }
    }

    pub fn parse(&mut self, v: &Value) -> Vec<UsageRecord> {
        let p = &v["payload"];
        if !p.is_object() {
            return vec![];
        }
        match text(&v["type"]) {
            "session_meta" => {
                if self.saw_session_meta {
                    return vec![];
                }
                self.saw_session_meta = true;
                if let Some(id) = p
                    .get("id")
                    .filter(|v| !v.is_null())
                    .or_else(|| p.get("session_id"))
                    .and_then(Value::as_str)
                {
                    self.session_id = id.into();
                }
                if let Some(provider) = p["model_provider"].as_str() {
                    self.model_provider = provider.into();
                }
                if let Some(at) = timestamp_ms(text(&v["timestamp"]))
                    && (p["forked_from_id"].is_string()
                        || p["source"]["subagent"]["thread_spawn"]["parent_thread_id"].is_string())
                {
                    self.suppressing_fork_copies = true;
                    self.fork_copy_anchor_ms = at;
                }
                return vec![];
            }
            "turn_context" => {
                self.request_ledger_active = Some(false);
                if let Some(model) = p["model"].as_str() {
                    self.model = model.into();
                }
                self.turn_id = if !text(&p["model"]).is_empty() {
                    text(&p["turn_id"]).into()
                } else {
                    String::new()
                };
                return vec![];
            }
            "token_usage_record" => {
                let (Some(at), Some(usage)) = (
                    timestamp_ms(text(&v["timestamp"])),
                    TokenUsage::read(&p["usage"]),
                ) else {
                    return vec![];
                };
                if self.model.is_empty() || self.fork_copy(at) {
                    return vec![];
                }
                if !text(&p["response_id"]).is_empty() {
                    let pending = self.unreported_requests.take().unwrap_or_default();
                    self.request_ledger_active = Some(true);
                    for req in &pending {
                        self.last_cumulative_usage =
                            TokenUsage::plus(self.last_cumulative_usage.as_ref(), &req.usage);
                    }
                    self.last_cumulative_usage =
                        TokenUsage::plus(self.last_cumulative_usage.as_ref(), &usage);
                    let mut out: Vec<_> = pending.iter().map(|r| self.record(r)).collect();
                    let mut r = self.record(&self.attribution(usage, at));
                    r.dedupe_key =
                        Some(json!(["codex", self.session_id, p["response_id"]]).to_string());
                    out.push(r);
                    return out;
                }
                self.request_ledger_active = Some(false);
                let request = self.attribution(usage, at);
                self.unreported_requests
                    .get_or_insert_default()
                    .push(request);
                return vec![];
            }
            _ => {}
        }
        if p["type"] == "thread_settings_applied" {
            self.saw_thread_settings = true;
            if p["thread_settings"].is_object() {
                self.speed = UsageSpeed::from_tier(text(&p["thread_settings"]["service_tier"]));
            }
            return vec![];
        }
        if p["type"] != "token_count" || !p["info"].is_object() {
            return vec![];
        }
        let Some(at) = timestamp_ms(text(&v["timestamp"])) else {
            return vec![];
        };
        if self.model.is_empty() {
            return vec![];
        }
        let info = &p["info"];
        let last = TokenUsage::read(&info["last_token_usage"]);
        let has_cumulative = !info["total_token_usage"].is_null();
        let cumulative = TokenUsage::read(&info["total_token_usage"]);
        if self.fork_copy(at) {
            let signature = last.as_ref().map(TokenUsage::signature);
            if let Some(c) = cumulative {
                self.rebase(c, at);
            } else if !has_cumulative
                && signature != self.last_usage_signature
                && let Some(l) = last
            {
                self.last_cumulative_usage =
                    TokenUsage::plus(self.last_cumulative_usage.as_ref(), &l);
            }
            self.last_usage_signature = signature;
            return vec![];
        }
        if self.request_ledger_active == Some(true) {
            if let Some(c) = cumulative {
                self.rebase(c, at);
            }
            return vec![];
        }
        let previous = self.last_cumulative_usage.clone();
        let latest = self.latest_cumulative_at_ms;
        let pending = self.unreported_requests.clone();
        if let Some(c) = &cumulative {
            self.rebase(c.clone(), at);
        }
        let Some(last) = last.filter(|_| !has_cumulative || cumulative.is_some()) else {
            self.malformed_records += 1;
            return vec![];
        };
        if last.input_tokens + last.output_tokens == 0 {
            self.last_usage_signature = None;
            return vec![];
        }
        if cumulative.is_some() && cumulative == previous {
            return vec![];
        }
        if last.total_tokens != last.input_tokens + last.output_tokens {
            self.malformed_records += 1;
            return vec![];
        }
        let signature = last.signature();
        let mut unreported = vec![];
        if let Some(c) = &cumulative {
            let restarted = c == &last;
            let sequence_start = format!("[{},{}]", json!(at as i64), signature);
            if restarted && self.sequence_start_signature.as_ref() == Some(&sequence_start) {
                self.last_cumulative_usage = previous;
                return vec![];
            }
            let rewound = previous.as_ref().is_some_and(|p| {
                latest.is_none_or(|t| at > t)
                    && c.values().iter().zip(p.values()).any(|(a, b)| *a < b)
            });
            let reconciled = if previous.is_none() || rewound {
                c.minus(&last).is_some()
            } else {
                previous.as_ref().and_then(|p| c.minus(p)).as_ref() == Some(&last)
            };
            if !restarted && !reconciled {
                let bridged = (|| {
                    let prev = previous.as_ref()?;
                    let pending = pending.as_ref().filter(|r| !r.is_empty())?;
                    let sum = pending.iter().try_fold(None, |sum, req| {
                        TokenUsage::plus(sum.as_ref(), &req.usage).map(Some)
                    })?;
                    if c.minus(prev) != sum {
                        return None;
                    }
                    let mut requests = pending.clone();
                    let final_req = requests.last_mut()?;
                    final_req.usage = final_req.usage.minus(&last)?;
                    requests.retain(|r| r.usage.input_tokens + r.usage.output_tokens > 0);
                    Some(requests)
                })();
                let Some(requests) = bridged else {
                    self.malformed_records += 1;
                    return vec![];
                };
                unreported = requests;
            }
            if restarted {
                self.sequence_start_signature = Some(sequence_start);
            }
        } else {
            if self.last_usage_signature.as_ref() == Some(&signature) {
                return vec![];
            }
            let Some(advanced) = TokenUsage::plus(previous.as_ref(), &last) else {
                self.malformed_records += 1;
                return vec![];
            };
            self.last_cumulative_usage = Some(advanced);
        }
        self.last_usage_signature = Some(signature);
        let mut out: Vec<_> = unreported.iter().map(|r| self.record(r)).collect();
        out.push(self.record(&self.attribution(last, at)));
        out
    }
}
