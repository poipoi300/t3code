use crate::{
    UsageProviderKind as P, UsageRecord, UsageSpeed, UsageTokenTotals,
    codex::CodexScanState,
    empty_totals,
    safety::{
        self, BudgetReader, MAX_DEPTH, MAX_ENTRIES, MAX_FILE_BYTES, MAX_LINE_BYTES, MAX_RECORDS,
        MAX_RETAINED_BYTES, MAX_STATE_BYTES, ScanBudget,
    },
    text, timestamp_ms, tokens, total_tokens,
};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor},
};
use serde_json::{Value, json};
use std::{
    fmt,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub const GUARD_LENGTH: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsePosition {
    pub resume_offset: u64,
    pub guard_length: usize,
    pub guard_hash: u32,
    pub state: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult {
    pub records: Vec<UsageRecord>,
    pub tail_records: Vec<UsageRecord>,
    pub malformed_records: u64,
    pub position: ParsePosition,
    pub resumed: bool,
}

#[derive(Debug, Clone)]
pub enum ScanState {
    Stateless,
    Codex(Box<CodexScanState>),
    Pi { session_id: String },
}

impl ScanState {
    pub fn initial(provider: &P) -> Self {
        match provider {
            P::Codex => Self::Codex(Box::default()),
            P::Pi => Self::Pi {
                session_id: String::new(),
            },
            _ => Self::Stateless,
        }
    }
    pub fn decode(provider: &P, value: &Value) -> Option<Self> {
        match provider {
            P::Codex => serde_json::from_value::<CodexScanState>(value.clone())
                .ok()
                .filter(|s| {
                    s.fork_copy_anchor_ms.is_finite()
                        && s.latest_cumulative_at_ms.is_none_or(f64::is_finite)
                        && value.get("lastUsageSignature").is_some()
                        && value.get("lastCumulativeUsage").is_some()
                        && s.last_cumulative_usage
                            .as_ref()
                            .is_none_or(|u| u.is_valid())
                        && s.unreported_requests
                            .as_ref()
                            .is_none_or(|requests| requests.iter().all(|r| r.usage.is_valid()))
                })
                .map(|s| Self::Codex(Box::new(s))),
            P::Pi => value["sessionId"].as_str().map(|id| Self::Pi {
                session_id: id.into(),
            }),
            _ => value.is_null().then_some(Self::Stateless),
        }
    }
    pub fn encode(&self) -> Value {
        match self {
            Self::Stateless => Value::Null,
            Self::Pi { session_id } => json!({"sessionId":session_id}),
            Self::Codex(s) => serde_json::to_value(s).unwrap_or(Value::Null),
        }
    }
    fn malformed(&self) -> u64 {
        if let Self::Codex(s) = self {
            s.malformed_records
        } else {
            0
        }
    }
}

fn base(provider: P, at: f64, model: &str, session: &str) -> UsageRecord {
    UsageRecord {
        provider,
        timestamp_ms: at,
        model: model.into(),
        rate_model: None,
        session_id: session.into(),
        totals: empty_totals(),
        cache_creation1h_tokens: None,
        reported_cost_usd: None,
        speed: UsageSpeed::Standard,
        unresolved_service_tier_turn_id: None,
        dedupe_key: None,
    }
}

fn claude_totals(v: &Value) -> UsageTokenTotals {
    UsageTokenTotals {
        uncached_input_tokens: tokens(&v["input_tokens"]),
        cached_input_tokens: tokens(&v["cache_read_input_tokens"]),
        cache_creation_tokens: tokens(&v["cache_creation_input_tokens"]),
        output_tokens: tokens(&v["output_tokens"]),
        reasoning_tokens: 0,
    }
}

pub fn parse_record(provider: &P, v: &Value, state: &mut ScanState) -> Vec<UsageRecord> {
    match provider {
        P::Codex => {
            if let ScanState::Codex(s) = state {
                s.parse(v)
            } else {
                vec![]
            }
        }
        P::Claude => {
            let u = &v["message"]["usage"];
            let model = text(&v["message"]["model"]);
            if v["type"] != "assistant"
                || !u.is_object()
                || model.is_empty()
                || model == "<synthetic>"
            {
                return vec![];
            }
            let Some(at) = timestamp_ms(text(&v["timestamp"])) else {
                return vec![];
            };
            let mut r = base(P::Claude, at, model, text(&v["sessionId"]));
            r.totals = claude_totals(u);
            r.cache_creation1h_tokens =
                Some(tokens(&u["cache_creation"]["ephemeral_1h_input_tokens"])).filter(|n| *n > 0);
            r.reported_cost_usd = v["costUSD"].as_f64().filter(|n| n.is_finite());
            if u["speed"] == "fast" {
                r.speed = UsageSpeed::Fast;
            }
            if v["message"]["id"].is_string() || v["requestId"].is_string() {
                r.dedupe_key = Some(format!(
                    "{}:{}",
                    text(&v["message"]["id"]),
                    text(&v["requestId"])
                ));
            }
            let mut out = vec![r.clone()];
            for (i, a) in u["iterations"].as_array().into_iter().flatten().enumerate() {
                let model = text(&a["model"]);
                if a["type"] != "advisor_message" || model.is_empty() || model == "<synthetic>" {
                    continue;
                }
                let mut advisor = base(P::Claude, at, model, &r.session_id);
                advisor.totals = claude_totals(a);
                advisor.speed = r.speed;
                advisor.cache_creation1h_tokens =
                    Some(tokens(&a["cache_creation"]["ephemeral_1h_input_tokens"]))
                        .filter(|n| *n > 0);
                advisor.dedupe_key = r.dedupe_key.as_ref().map(|k| format!("{k}:advisor:{i}"));
                out.push(advisor);
            }
            out
        }
        P::Grok => parse_grok(v),
        P::Pi => {
            let ScanState::Pi { session_id } = state else {
                return vec![];
            };
            let kind = text(&v["type"]);
            if kind == "session" && !text(&v["id"]).is_empty() {
                *session_id = text(&v["id"]).into();
                return vec![];
            }
            if !["message", "usage", "compaction", "branch_summary"].contains(&kind)
                || text(&v["id"]).is_empty()
            {
                return vec![];
            }
            let source = if kind == "message" { &v["message"] } else { v };
            if kind == "message" && !["assistant", "toolResult"].contains(&text(&source["role"])) {
                return vec![];
            }
            let has_model = kind == "usage" || source["role"] == "assistant";
            if has_model
                && (text(&source["model"]).is_empty() || text(&source["provider"]).is_empty())
            {
                return vec![];
            }
            if source
                .get("responseModel")
                .is_some_and(|s| text(s).is_empty())
            {
                return vec![];
            }
            let Some(at) = timestamp_ms(text(&v["timestamp"])) else {
                return vec![];
            };
            let u = &source["usage"];
            if !u.is_object() {
                return vec![];
            }
            let rate_model = has_model
                .then(|| text(source.get("responseModel").unwrap_or(&source["model"])).to_string());
            let model = rate_model
                .as_ref()
                .map(|m| format!("{}/{m}", text(&source["provider"])))
                .unwrap_or("Tools/summaries".into());
            let mut r = base(P::Pi, at, &model, session_id);
            r.rate_model = rate_model;
            r.totals = UsageTokenTotals {
                uncached_input_tokens: tokens(&u["input"]),
                cached_input_tokens: tokens(&u["cacheRead"]),
                cache_creation_tokens: tokens(&u["cacheWrite"]),
                output_tokens: tokens(&u["output"]),
                reasoning_tokens: tokens(&u["reasoning"]).min(tokens(&u["output"])),
            };
            r.reported_cost_usd = u["cost"]["total"]
                .as_f64()
                .filter(|n| n.is_finite() && *n > 0.0);
            r.dedupe_key = Some(json!(["pi", v["id"], v["timestamp"], model]).to_string());
            if total_tokens(&r.totals) == 0 && r.reported_cost_usd.is_none() {
                vec![]
            } else {
                vec![r]
            }
        }
        P::Muse => {
            let e = &v["payload"]["event"];
            let u = &e["usage"];
            let model = e["model"]
                .as_str()
                .or_else(|| e["model"]["model_id"].as_str())
                .unwrap_or("");
            if v["schema_version"] != 1
                || v["record_type"] != "event"
                || v["payload_type"] != "runtime.session"
                || ![Some(1), Some(3)].contains(&v["payload_schema_version"].as_u64())
                || text(&v["id"]).is_empty()
                || v["stream"]["kind"] != "session"
                || text(&v["stream"]["id"]).is_empty()
                || !["model_completed", "automated_review_completed"].contains(&text(&e["kind"]))
                || model.is_empty()
                || !u.is_object()
            {
                return vec![];
            }
            let Some(at) = v["recorded_at"].as_f64().filter(|n| {
                n.is_finite() && *n >= 0.0 && *n <= 9_007_199_254_740_991.0 && n.fract() == 0.0
            }) else {
                return vec![];
            };
            let mut r = base(
                P::Muse,
                (at / 1000.0).trunc(),
                model,
                text(&v["stream"]["id"]),
            );
            let input = tokens(&u["input_tokens"]);
            let cached = tokens(
                u.get("cache_read_tokens")
                    .filter(|v| !v.is_null())
                    .or_else(|| u.get("cached_input_tokens").filter(|v| !v.is_null()))
                    .unwrap_or(&u["cached_tokens"]),
            )
            .min(input);
            let write = tokens(&u["cache_write_tokens"]).min(input - cached);
            r.totals = UsageTokenTotals {
                uncached_input_tokens: input - cached - write,
                cached_input_tokens: cached,
                cache_creation_tokens: write,
                output_tokens: tokens(&u["output_tokens"]),
                reasoning_tokens: tokens(&u["reasoning_tokens"]).min(tokens(&u["output_tokens"])),
            };
            r.dedupe_key = Some(
                json!([
                    "muse",
                    v["payload"].get("source_run_record_id").unwrap_or(&v["id"])
                ])
                .to_string(),
            );
            if total_tokens(&r.totals) == 0 {
                vec![]
            } else {
                vec![r]
            }
        }
        _ => vec![],
    }
}

fn grok_totals(v: &Value) -> UsageTokenTotals {
    let cached = tokens(&v["cachedReadTokens"]);
    let write = tokens(&v["cacheCreationTokens"]);
    let output = tokens(&v["outputTokens"]);
    UsageTokenTotals {
        uncached_input_tokens: tokens(&v["inputTokens"])
            .saturating_sub(cached.saturating_add(write)),
        cached_input_tokens: cached,
        cache_creation_tokens: write,
        output_tokens: output,
        reasoning_tokens: tokens(&v["reasoningTokens"]).min(output),
    }
}
fn ticks(v: &Value) -> Option<f64> {
    v["costUsdTicks"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n / 10_000_000_000.0)
}
fn parse_grok(v: &Value) -> Vec<UsageRecord> {
    let p = &v["params"];
    let update = &p["update"];
    let usage = &update["usage"];
    if update["sessionUpdate"] != "turn_completed" || !usage.is_object() {
        return vec![];
    }
    let at = p["_meta"]["agentTimestampMs"]
        .as_f64()
        .filter(|n| n.is_finite())
        .or_else(|| {
            v["timestamp"]
                .as_f64()
                .filter(|n| n.is_finite())
                .map(|n| if n > 1e12 { n } else { n * 1000.0 })
        });
    let Some(at) = at else {
        return vec![];
    };
    let mut entries: Vec<_> = usage["modelUsage"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter(|(m, u)| !m.is_empty() && u.is_object())
        .map(|(m, u)| (m.as_str(), u))
        .collect();
    if entries.is_empty() {
        entries.push(("grok", usage));
    }
    let used: f64 = entries
        .iter()
        .filter(|(_, u)| total_tokens(&grok_totals(u)) > 0)
        .filter_map(|(_, u)| ticks(u))
        .sum();
    let denominator: u64 = entries
        .iter()
        .filter(|(_, u)| !u["costUsdTicks"].is_number())
        .map(|(_, u)| total_tokens(&grok_totals(u)))
        .fold(0u64, u64::saturating_add);
    let remaining = ticks(usage).map(|n| (n - used).max(0.0));
    entries
        .into_iter()
        .filter_map(|(model, u)| {
            let totals = grok_totals(u);
            let total = total_tokens(&totals);
            if total == 0 {
                return None;
            }
            let mut r = base(P::Grok, at, model, text(&p["sessionId"]));
            r.totals = totals;
            r.reported_cost_usd = ticks(u).or_else(|| {
                remaining
                    .filter(|_| denominator > 0)
                    .map(|n| n * total as f64 / denominator as f64)
            });
            if update["prompt_id"].is_string() {
                r.dedupe_key = Some(format!(
                    "{}:{}:{model}",
                    r.session_id,
                    text(&update["prompt_id"])
                ));
            }
            Some(r)
        })
        .collect()
}

// IgnoredAny avoids retaining unselected prompt and tool payloads.
struct Projection<'a> {
    fields: &'a Value,
}
impl<'de> DeserializeSeed<'de> for Projection<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        if self.fields == &Value::Bool(true) {
            let value = Value::deserialize(d)?;
            fn bounded(v: &Value) -> bool {
                match v {
                    Value::String(s) => s.len() <= 4096,
                    Value::Array(a) => a.iter().all(bounded),
                    Value::Object(o) => o.iter().all(|(k, v)| k.len() <= 4096 && bounded(v)),
                    _ => true,
                }
            }
            return if bounded(&value) {
                Ok(value)
            } else {
                Err(de::Error::custom("Usage field exceeds safety budget"))
            };
        }
        struct Object<'a>(&'a Value);
        impl<'de> Visitor<'de> for Object<'_> {
            type Value = Value;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Value, M::Error> {
                let mut out = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if let Some(fields) = self.0.get(&key) {
                        out.insert(key, map.next_value_seed(Projection { fields })?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Value::Object(out))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
                while a.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Value::Null)
            }
        }
        d.deserialize_any(Object(self.fields))
    }
}

pub(crate) fn selected_fields(provider: &P) -> Value {
    match provider {
        P::Claude => {
            json!({"type":true,"timestamp":true,"requestId":true,"sessionId":true,"costUSD":true,"message":{"id":true,"model":true,"usage":true}})
        }
        P::Codex => {
            json!({"type":true,"timestamp":true,"payload":{"type":true,"id":true,"session_id":true,"model_provider":true,"turn_id":true,"response_id":true,"model":true,"thread_settings":{"service_tier":true},"forked_from_id":true,"source":{"subagent":{"thread_spawn":{"parent_thread_id":true}}},"info":{"last_token_usage":true,"total_token_usage":true},"usage":true}})
        }
        P::Grok => {
            json!({"timestamp":true,"params":{"sessionId":true,"_meta":{"agentTimestampMs":true},"update":{"sessionUpdate":true,"prompt_id":true,"usage":true}}})
        }
        P::Pi => {
            json!({"type":true,"id":true,"timestamp":true,"provider":true,"model":true,"usage":true,"message":{"role":true,"provider":true,"model":true,"responseModel":true,"usage":true}})
        }
        P::Muse => {
            json!({"schema_version":true,"record_type":true,"payload_type":true,"payload_schema_version":true,"id":true,"recorded_at":true,"stream":{"kind":true,"id":true},"payload":{"source_run_record_id":true,"event":{"kind":true,"model":true,"usage":true}}})
        }
        _ => Value::Bool(true),
    }
}

pub(crate) fn project_json<R: Read>(reader: R, fields: &Value) -> Result<Value, serde_json::Error> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(serde_json::Error::io)?;
    if bytes.len() > MAX_LINE_BYTES || !safety::json_depth_ok(&bytes) {
        return Err(serde_json::Error::io(safety::exceeded()));
    }
    let decoded = String::from_utf8_lossy(&bytes);
    let mut d = serde_json::Deserializer::from_str(&decoded);
    let v = Projection { fields }.deserialize(&mut d)?;
    d.end()?;
    Ok(v)
}

fn read_line(input: &mut impl BufRead, bytes: &mut Vec<u8>) -> io::Result<(u64, bool, bool)> {
    bytes.clear();
    let (mut length, mut oversized) = (0u64, false);
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            return Ok((length, false, oversized));
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let n = newline.map_or(chunk.len(), |i| i + 1);
        length += n as u64;
        if length <= MAX_LINE_BYTES as u64 && !oversized {
            bytes.extend_from_slice(&chunk[..n]);
        } else {
            oversized = true;
            bytes.clear();
        }
        input.consume(n);
        if newline.is_some() {
            return Ok((length, true, oversized));
        }
    }
}

fn carries_usage(provider: &P, bytes: &[u8]) -> bool {
    let markers: &[&[u8]] = match provider {
        P::Claude => &[b"usage"],
        P::Codex => &[
            b"session_meta",
            b"turn_context",
            b"token_count",
            b"token_usage_record",
            b"thread_settings_applied",
        ],
        P::Pi => &[b"session", b"usage"],
        P::Grok | P::Muse => &[b"usage"],
        _ => return true,
    };
    markers
        .iter()
        .any(|marker| bytes.windows(marker.len()).any(|w| w == *marker))
}

pub fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c9dc5, |hash, b| {
        (hash ^ u32::from(*b)).wrapping_mul(0x01000193)
    })
}
fn guard(file: &mut File, offset: u64, length: usize) -> io::Result<u32> {
    if length == 0 || length > GUARD_LENGTH || length as u64 > offset {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    file.seek(SeekFrom::Start(offset - length as u64))?;
    let mut buf = [0; GUARD_LENGTH];
    file.read_exact(&mut buf[..length])?;
    Ok(fnv1a(&buf[..length]))
}

pub fn read_transcript(
    path: &Path,
    provider: &P,
    resume: Option<&ParsePosition>,
) -> Option<ParseResult> {
    read_transcript_bounded(path, provider, resume, &ScanBudget::default())
}
pub(crate) fn read_transcript_bounded(
    path: &Path,
    provider: &P,
    resume: Option<&ParsePosition>,
    budget: &ScanBudget,
) -> Option<ParseResult> {
    read_transcript_inner(path, provider, resume, budget).ok()
}
fn read_transcript_inner(
    path: &Path,
    provider: &P,
    resume: Option<&ParsePosition>,
    budget: &ScanBudget,
) -> io::Result<ParseResult> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(safety::exceeded());
    }
    let mut file = safety::open_regular(path)?;
    let size = file.metadata()?.len();
    if size > MAX_FILE_BYTES || !file.metadata()?.is_file() {
        return Err(safety::exceeded());
    }
    let compressed = path.to_string_lossy().ends_with(".jsonl.zst");
    let mut state = ScanState::initial(provider);
    let mut start = 0;
    let mut resumed = false;
    if !compressed
        && let Some(p) = resume.filter(|p| p.resume_offset > 0)
        && let Some(saved) = if p.state.is_null() && !matches!(provider, P::Codex | P::Pi) {
            Some(ScanState::Stateless)
        } else {
            ScanState::decode(provider, &p.state)
        }
        && guard(&mut file, p.resume_offset, p.guard_length).ok() == Some(p.guard_hash)
    {
        state = saved;
        start = p.resume_offset;
        resumed = true;
    }
    file.seek(SeekFrom::Start(start))?;
    let handle = file.try_clone()?.take(size.saturating_sub(start));
    let allowance = if compressed {
        size.saturating_mul(200)
            .clamp(8 * 1024 * 1024, MAX_FILE_BYTES)
    } else {
        MAX_FILE_BYTES
    };
    let input: Box<dyn Read> = if compressed {
        let mut decoder = zstd::stream::read::Decoder::new(handle)?;
        decoder.window_log_max(23)?;
        Box::new(decoder)
    } else {
        Box::new(handle)
    };
    let mut input = BufReader::with_capacity(
        256 * 1024,
        BudgetReader {
            input,
            remaining: allowance,
            budget,
        },
    );
    let fields = selected_fields(provider);
    let mut records = vec![];
    let mut tail_records = vec![];
    let mut offset = start;
    let mut committed_offset = start;
    let mut tail_state = None;
    let mut line = Vec::new();
    let mut retained_bytes = 0;
    loop {
        let (bytes, terminated, oversized) = read_line(&mut input, &mut line)?;
        if bytes == 0 {
            break;
        }
        let parsed = (!oversized && carries_usage(provider, &line))
            .then(|| project_json(line.as_slice(), &fields))
            .transpose()
            .ok()
            .flatten();
        offset += bytes;
        if terminated {
            if let Some(v) = parsed {
                let added = parse_record(provider, &v, &mut state);
                let bytes = serde_json::to_vec(&added)?.len();
                retained_bytes += bytes;
                budget.retain(bytes)?;
                records.extend(added);
            }
            committed_offset = offset;
        } else {
            let mut tentative = state.clone();
            if let Some(v) = parsed {
                tail_records.extend(parse_record(provider, &v, &mut tentative));
                budget.retain(serde_json::to_vec(&tail_records)?.len())?;
            }
            tail_state = Some(tentative);
            break;
        }
        if records.len() > MAX_RECORDS
            || retained_bytes > MAX_RETAINED_BYTES
            || matches!(&state, ScanState::Codex(s) if s.unreported_requests.as_ref().is_some_and(|r| r.len() > 256))
        {
            return Err(safety::exceeded());
        }
    }
    let encoded = state.encode();
    let state_bytes = serde_json::to_vec(&encoded)?.len();
    if state_bytes > MAX_STATE_BYTES {
        return Err(safety::exceeded());
    }
    budget.retain(state_bytes)?;
    let guard_length = if compressed {
        0
    } else {
        GUARD_LENGTH.min(committed_offset as usize)
    };
    let guard_hash = if guard_length == 0 {
        0
    } else {
        guard(&mut file, committed_offset, guard_length)?
    };
    Ok(ParseResult {
        records,
        tail_records,
        malformed_records: tail_state.as_ref().unwrap_or(&state).malformed(),
        resumed,
        position: ParsePosition {
            resume_offset: if compressed { 0 } else { committed_offset },
            guard_length,
            guard_hash,
            state: encoded,
        },
    })
}

#[derive(Debug, Clone)]
pub struct TranscriptFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: f64,
}
#[derive(Debug, Default)]
pub struct FileList {
    pub files: Vec<TranscriptFile>,
    pub walked: Vec<PathBuf>,
    pub failed_paths: u64,
}
pub(crate) fn mtime_ms(metadata: &fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as f64 * 1000.0 + f64::from(d.subsec_nanos()) / 1e6,
            Err(e) => {
                let d = e.duration();
                if d.subsec_nanos() == 0 {
                    -(d.as_secs() as f64) * 1000.0
                } else {
                    -(d.as_secs() as f64 + 1.0) * 1000.0
                        + f64::from(1_000_000_000 - d.subsec_nanos()) / 1e6
                }
            }
        })
        .unwrap_or(0.0)
}

pub fn list_transcripts(root: &Path, since_ms: f64, file_name: Option<&str>) -> FileList {
    fn walk(
        root: &Path,
        since: f64,
        name: Option<&str>,
        out: &mut FileList,
        depth: usize,
        remaining: &mut usize,
    ) {
        if depth > MAX_DEPTH || *remaining == 0 {
            out.failed_paths += 1;
            return;
        }
        if !fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
            return;
        }
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => {
                out.failed_paths += 1;
                return;
            }
        };
        let mut entries: Vec<_> = entries
            .take(*remaining)
            .filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(_) => {
                    out.failed_paths += 1;
                    None
                }
            })
            .collect();
        // Node's readdir returns name order; explicitly retain it on every platform.
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if *remaining == 0 {
                out.failed_paths += 1;
                break;
            }
            *remaining -= 1;
            let p = e.path();
            let kind = match e.file_type() {
                Ok(t) => t,
                Err(_) => {
                    out.failed_paths += 1;
                    continue;
                }
            };
            if kind.is_dir() {
                walk(&p, since, name, out, depth + 1, remaining);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let n = e.file_name();
            let n = n.to_string_lossy();
            if !name
                .map(|name| n == name)
                .unwrap_or_else(|| n.ends_with(".jsonl") || n.ends_with(".jsonl.zst"))
            {
                continue;
            }
            out.walked.push(p.clone());
            match fs::metadata(&p) {
                Ok(m) => {
                    let at = mtime_ms(&m);
                    if at >= since {
                        out.files.push(TranscriptFile {
                            path: p,
                            size: m.len(),
                            mtime_ms: at,
                        });
                    }
                }
                Err(e) => {
                    if e.kind() != io::ErrorKind::NotFound {
                        out.failed_paths += 1;
                    }
                }
            }
        }
    }
    let mut out = FileList::default();
    let mut remaining = MAX_ENTRIES;
    walk(root, since_ms, file_name, &mut out, 0, &mut remaining);
    out
}
