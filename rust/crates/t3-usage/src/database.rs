use crate::{
    Error, UsageProviderKind as P, UsageRecord, UsageSpeed, UsageTokenTotals,
    aggregation::merge_usage_copies,
    reader::{mtime_ms, project_json},
    text, tokens, total_tokens,
};
use crate::{
    safety::{
        self, MAX_DEPTH, MAX_ENTRIES, MAX_LINE_BYTES, MAX_RECORDS, MAX_RETAINED_BYTES, ScanBudget,
    },
    snapshot::connect_bounded,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default)]
pub struct DatabaseRead {
    pub files: Vec<(PathBuf, Vec<UsageRecord>)>,
    pub missing: bool,
    pub errors: Vec<PathBuf>,
}
fn tables(c: &Connection) -> Result<HashSet<String>, Error> {
    Ok(
        c.prepare("SELECT name FROM sqlite_master WHERE type='table' LIMIT 20000")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?,
    )
}

pub fn parse_opencode_message(
    v: &Value,
    id: &str,
    session: &str,
    at: Option<f64>,
) -> Option<UsageRecord> {
    if v.get("role").is_some_and(|role| role != "assistant") {
        return None;
    }
    let m = &v["model"];
    let model = m["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| m["modelID"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| v["modelID"].as_str())
        .filter(|s| !s.is_empty())?;
    let provider = m["providerID"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(text(&v["providerID"]));
    let at = v["time"]["created"]
        .as_f64()
        .or(at)
        .filter(|n| n.is_finite())?;
    let u = &v["tokens"];
    let reasoning = tokens(&u["reasoning"]);
    let totals = UsageTokenTotals {
        uncached_input_tokens: tokens(&u["input"]),
        cached_input_tokens: tokens(&u["cache"]["read"]),
        cache_creation_tokens: tokens(&u["cache"]["write"]),
        output_tokens: tokens(&u["output"]).saturating_add(reasoning),
        reasoning_tokens: reasoning,
    };
    if total_tokens(&totals) == 0 {
        return None;
    }
    let id = if id.is_empty() { text(&v["id"]) } else { id };
    let session = if session.is_empty() {
        text(&v["sessionID"])
    } else {
        session
    };
    let free = ["opencode", "opencode-go"].contains(&provider) && model.ends_with("-free");
    Some(UsageRecord {
        provider: P::Opencode,
        timestamp_ms: at,
        model: model.into(),
        rate_model: (provider == "opencode-go" && !model.contains('/'))
            .then(|| format!("opencode_go/{model}")),
        session_id: session.into(),
        totals,
        cache_creation1h_tokens: None,
        reported_cost_usd: v["cost"]
            .as_f64()
            .filter(|n| n.is_finite() && (*n > 0.0 || (free && *n == 0.0))),
        speed: UsageSpeed::Standard,
        unresolved_service_tier_turn_id: None,
        dedupe_key: (!id.is_empty()).then(|| format!("opencode:{id}")),
    })
}

pub fn read_opencode(root: &Path, since: f64) -> DatabaseRead {
    read_opencode_bounded(root, since, &ScanBudget::default())
}
pub(crate) fn read_opencode_bounded(root: &Path, since: f64, budget: &ScanBudget) -> DatabaseRead {
    let mut result = DatabaseRead {
        missing: true,
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let names = match fs::read_dir(root) {
        Ok(e) => e
            .take(MAX_ENTRIES)
            .filter_map(Result::ok)
            .collect::<Vec<_>>(),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                result.errors.push(root.into());
                result.missing = false;
            }
            vec![]
        }
    };
    let mut dbs: Vec<_> = names
        .into_iter()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let valid = name == "opencode.db"
                || name
                    .strip_prefix("opencode-")
                    .and_then(|n| n.strip_suffix(".db"))
                    .is_some_and(|n| {
                        !n.is_empty()
                            && n.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                    });
            valid && e.file_type().is_ok_and(|t| t.is_file())
        })
        .collect();
    dbs.sort_by_key(|e| (e.file_name() != "opencode.db", e.file_name()));
    for db in dbs {
        result.missing = false;
        let path = db.path();
        let mut records = vec![];
        let mut retained = 0;
        let read = (|| -> Result<(), Error> {
            let c = connect_bounded(&path, budget)?;
            c.execute_batch("BEGIN")?;
            let tables = tables(&c)?;
            if !tables.contains("message") && !tables.contains("session_message") {
                return Err(Error::ScanFailed("Missing OpenCode message tables".into()));
            }
            for table in ["message", "session_message"] {
                if !tables.contains(table) {
                    continue;
                }
                let columns: HashSet<String> = c
                    .prepare(&format!("PRAGMA table_info({table})"))?
                    .query_map([], |r| r.get(1))?
                    .collect::<Result<_, _>>()?;
                let timestamp = if columns.contains("time_created") {
                    "time_created"
                } else {
                    "NULL"
                };
                let predicate = if table == "session_message" {
                    " AND type='assistant'"
                } else {
                    ""
                };
                let query = format!(
                    "SELECT id,session_id,{timestamp},json_object('tokens',data -> '$.tokens','cost',data -> '$.cost', \
                    'model',data -> '$.model','modelID',data -> '$.modelID','providerID',data -> '$.providerID', \
                    'time',data -> '$.time','id',data -> '$.id','sessionID',data -> '$.sessionID') FROM {table} \
                    WHERE ({timestamp} IS NULL OR {timestamp} >= ?1){predicate} AND CASE WHEN typeof(data)='text' AND json_valid(data) \
                    THEN json_type(data,'$.role') IS NULL OR data ->> '$.role'='assistant' END ORDER BY rowid LIMIT 20000"
                );
                let mut statement = c.prepare(&query)?;
                let mut rows = statement.query([since])?;
                while let Some(row) = rows.next()? {
                    let raw: String = row.get(3)?;
                    budget.charge(raw.len() as u64)?;
                    let value = match project_json(raw.as_bytes(), &Value::Bool(true)) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let id = row.get::<_, String>(0).unwrap_or_default();
                    let session = row.get::<_, String>(1).unwrap_or_default();
                    if let Some(r) = parse_opencode_message(&value, &id, &session, row.get(2).ok())
                        && r.timestamp_ms >= since
                        && r.dedupe_key.as_ref().is_none_or(|k| seen.insert(k.clone()))
                    {
                        let bytes = serde_json::to_vec(&r)?.len();
                        budget.retain(bytes)?;
                        retained += bytes;
                        if records.len() >= MAX_RECORDS || retained > MAX_RETAINED_BYTES {
                            return Err(safety::exceeded().into());
                        }
                        records.push(r);
                    }
                }
            }
            Ok(())
        })();
        if read.is_err() {
            result.errors.push(path.clone());
        }
        result.files.push((path, records));
    }
    let fields = json!({"role":true,"tokens":true,"cost":true,"model":true,"modelID":true,"providerID":true,"time":true,"id":true,"sessionID":true});
    let mut dirs = vec![root.join("storage/message")];
    let mut remaining = MAX_ENTRIES;
    while let Some(dir) = dirs.pop() {
        if remaining == 0 || !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    result.errors.push(dir);
                    result.missing = false;
                }
                continue;
            }
        };
        let mut entries: Vec<_> = entries.take(remaining).filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if remaining == 0 {
                result.errors.push(dir.clone());
                break;
            }
            remaining -= 1;
            let path = e.path();
            let Ok(kind) = e.file_type() else {
                result.errors.push(path);
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if path
                    .strip_prefix(root)
                    .is_ok_and(|p| p.components().count() <= MAX_DEPTH)
                {
                    dirs.push(path);
                }
                continue;
            }
            if !kind.is_file() || path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            result.missing = false;
            let id = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
            if seen.contains(&format!("opencode:{id}"))
                || fs::metadata(&path).is_ok_and(|m| mtime_ms(&m) < since)
            {
                continue;
            }
            let read = safety::open_regular(&path)
                .and_then(|f| {
                    let len = f.metadata()?.len();
                    if len > MAX_LINE_BYTES as u64 {
                        return Err(safety::exceeded());
                    }
                    budget.charge(len)?;
                    Ok(BufReader::new(std::io::Read::take(f, len)))
                })
                .map_err(Error::from)
                .and_then(|f| project_json(f, &fields).map_err(Error::from));
            match read {
                Ok(v) => {
                    let records: Vec<_> = parse_opencode_message(&v, id, "", None)
                        .filter(|r| r.timestamp_ms >= since)
                        .filter(|r| r.dedupe_key.as_ref().is_none_or(|k| seen.insert(k.clone())))
                        .into_iter()
                        .collect();
                    if budget
                        .retain(
                            serde_json::to_vec(&records)
                                .map_or(MAX_RETAINED_BYTES + 1, |r| r.len()),
                        )
                        .is_err()
                    {
                        result.errors.push(path);
                        continue;
                    }
                    result.files.push((path, records));
                }
                Err(Error::Json(_)) => result.files.push((path, vec![])),
                Err(_) => result.errors.push(path),
            }
        }
    }
    result
}

#[derive(Debug, Clone)]
enum Field {
    Number(u64),
    Bytes(Vec<u8>),
}
type Fields = HashMap<u64, Vec<Field>>;
fn varint(bytes: &[u8], offset: &mut usize) -> Result<u64, Error> {
    let mut result = 0;
    for shift in (0..70).step_by(7) {
        let Some(byte) = bytes.get(*offset).copied() else {
            return Err(Error::ScanFailed("Truncated protobuf varint".into()));
        };
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(Error::ScanFailed("Invalid protobuf varint".into()));
        }
        result |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(result);
        }
    }
    Err(Error::ScanFailed("Invalid protobuf varint".into()))
}
fn fields(bytes: &[u8]) -> Result<Fields, Error> {
    let mut offset = 0;
    let mut count = 0;
    let mut out = Fields::new();
    while offset < bytes.len() {
        count += 1;
        if count > MAX_ENTRIES {
            return Err(safety::exceeded().into());
        }
        let tag = varint(bytes, &mut offset)?;
        if tag / 8 == 0 {
            return Err(Error::ScanFailed("Invalid protobuf field".into()));
        }
        let wire = tag % 8;
        let value = match wire {
            0 => Field::Number(varint(bytes, &mut offset)?),
            1 | 2 | 5 => {
                let len = if wire == 2 {
                    usize::try_from(varint(bytes, &mut offset)?)
                        .map_err(|_| Error::ScanFailed("Invalid protobuf length".into()))?
                } else if wire == 1 {
                    8
                } else {
                    4
                };
                if len > bytes.len() - offset {
                    return Err(Error::ScanFailed("Truncated protobuf field".into()));
                }
                let value = bytes[offset..offset + len].to_vec();
                offset += len;
                if wire != 2 {
                    continue;
                }
                Field::Bytes(value)
            }
            _ => return Err(Error::ScanFailed("Unsupported protobuf wire type".into())),
        };
        out.entry(tag / 8).or_default().push(value);
    }
    Ok(out)
}
fn number(f: &Fields, key: u64) -> u64 {
    match f.get(&key).and_then(|v| v.first()) {
        Some(Field::Number(n)) if *n <= 9_007_199_254_740_991 => *n,
        _ => 0,
    }
}
fn bytes(f: &Fields, key: u64) -> Option<&[u8]> {
    match f.get(&key).and_then(|v| v.first()) {
        Some(Field::Bytes(b)) => Some(b),
        _ => None,
    }
}
fn nested(f: &Fields, key: u64) -> Result<Fields, Error> {
    bytes(f, key)
        .map(fields)
        .unwrap_or_else(|| Ok(Fields::new()))
}
fn string(f: &Fields, key: u64) -> Result<String, Error> {
    bytes(f, key)
        .map(|b| {
            if b.len() > 4096 {
                return Err(safety::exceeded().into());
            }
            std::str::from_utf8(b)
                .map(|s| s.trim().to_string())
                .map_err(|e| Error::ScanFailed(e.to_string()))
        })
        .unwrap_or(Ok(String::new()))
}
fn proto_time(f: &Fields) -> Option<f64> {
    let s = number(f, 1);
    (s > 0).then(|| s as f64 * 1000.0 + (number(f, 2) / 1_000_000) as f64)
}
fn model_name(name: &str) -> String {
    let mut s = name.to_lowercase();
    if s.ends_with(')')
        && let Some((base, effort)) = s.rsplit_once('(')
    {
        s = format!("{}-{}", base.trim(), effort.trim_end_matches(')'));
    }
    s = s.split_whitespace().collect::<Vec<_>>().join("-");
    if s.starts_with("claude-") {
        let parts: Vec<_> = s.split('-').collect();
        if parts.len() >= 3
            && parts[1].chars().all(|c| c.is_ascii_digit() || c == '.')
            && ["sonnet", "opus", "haiku"].contains(&parts[2])
        {
            s = format!(
                "claude-{}-{}{}",
                parts[2],
                parts[1],
                if parts.len() > 3 {
                    format!("-{}", parts[3..].join("-"))
                } else {
                    String::new()
                }
            );
        }
        s = s.replace('.', "-");
    }
    s
}
fn concrete(s: &str) -> bool {
    [
        "gemini-",
        "claude-sonnet-",
        "claude-opus-",
        "claude-haiku-",
        "gpt-",
    ]
    .iter()
    .any(|p| {
        s.strip_prefix(p)
            .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()) || *p == "gpt-")
    }) && ![
        "gemini-3-flash-agent",
        "gemini-3-flash-agent-a",
        "gemini-3-flash-agent-b",
        "gemini-3-flash-a",
        "gemini-3-flash-b",
    ]
    .contains(&s)
}
#[derive(Debug, Clone)]
struct Metadata {
    model: String,
    model_id: u64,
    at: Option<f64>,
    usages: Vec<Fields>,
}
fn metadata(blob: &[u8], step: bool) -> Result<Metadata, Error> {
    let root = fields(blob)?;
    if !step && bytes(&root, 1).is_none() {
        return Err(Error::ScanFailed("Missing generation metadata".into()));
    }
    let data = if step { root } else { nested(&root, 1)? };
    let model = if step {
        nested(&data, 24)?
    } else {
        data.clone()
    };
    let mut usages = bytes(&data, if step { 9 } else { 4 })
        .map(fields)
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    let model_id = number(&model, if step { 1 } else { 3 });
    let model_id = if model_id == 0 {
        usages.first().map(|u| number(u, 1)).unwrap_or(0)
    } else {
        model_id
    };
    for retry in data.get(&if step { 28 } else { 17 }).into_iter().flatten() {
        let Field::Bytes(blob) = retry else {
            return Err(Error::ScanFailed("Invalid retry metadata".into()));
        };
        if let Some(usage) = bytes(&fields(blob)?, 2) {
            usages.push(fields(usage)?);
        }
    }
    let names = [
        model_name(&string(&model, if step { 12 } else { 19 })?),
        model_name(&string(&model, if step { 8 } else { 21 })?),
    ];
    let model = names
        .iter()
        .find(|s| concrete(s))
        .or_else(|| {
            names
                .iter()
                .find(|s| !s.is_empty() && !s.starts_with("model_") && !s.starts_with("model-"))
        })
        .cloned()
        .unwrap_or_default();
    let at = if step {
        proto_time(&nested(&data, 8)?).or(proto_time(&nested(&data, 1)?))
    } else {
        proto_time(&nested(&nested(&data, 9)?, 4)?)
    };
    Ok(Metadata {
        model,
        model_id,
        at,
        usages,
    })
}
#[derive(Debug, Clone)]
struct Candidate {
    record: UsageRecord,
    model_id: u64,
    model_quality: u8,
    time_quality: u8,
    keys: Vec<String>,
}
#[derive(Debug, Clone, Default)]
struct AntigravityData {
    candidates: Vec<Candidate>,
    models: HashMap<u64, Option<String>>,
}
impl AntigravityData {
    fn retained_size(&self) -> usize {
        self.candidates
            .iter()
            .map(|c| {
                c.record.model.len()
                    + c.record.session_id.len()
                    + c.record.dedupe_key.as_ref().map_or(0, String::len)
                    + c.keys.iter().map(String::len).sum::<usize>()
                    + 384
            })
            .sum::<usize>()
            + self
                .models
                .values()
                .map(|m| m.as_ref().map_or(0, String::len) + 64)
                .sum::<usize>()
    }
}
fn read_antigravity_db(path: &Path, c: &Connection) -> Result<AntigravityData, Error> {
    c.execute_batch("BEGIN")?;
    let tables = tables(c)?;
    if !tables.contains("gen_metadata") && !tables.contains("steps") {
        return Err(Error::ScanFailed(
            "Missing Antigravity metadata tables".into(),
        ));
    }
    let read = |table: &str, column: &str, step: bool| -> Result<Vec<(i64, Metadata)>, Error> {
        if !tables.contains(table) {
            return Ok(vec![]);
        }
        let mut query = c.prepare(&format!(
            "SELECT idx,{column} FROM {table} ORDER BY idx LIMIT 20000"
        ))?;
        let mut rows = query.query([])?;
        let mut out = vec![];
        let mut retained = 0;
        while let Some(row) = rows.next()? {
            if let Some(data) = row.get::<_, Option<Vec<u8>>>(1)? {
                retained += data.len();
                if retained > MAX_RETAINED_BYTES {
                    return Err(safety::exceeded().into());
                }
                out.push((row.get(0)?, metadata(&data, step)?));
            }
        }
        Ok(out)
    };
    let generations = read("gen_metadata", "data", false)?;
    let steps = read("steps", "metadata", true)?;
    let mut trajectory = None;
    if tables.contains("trajectory_metadata_blob") {
        let mut query = c.prepare("SELECT data FROM trajectory_metadata_blob LIMIT 20000")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let f = fields(&row.get::<_, Vec<u8>>(0)?)?;
            if trajectory.is_none() {
                trajectory = proto_time(&nested(&f, 2)?);
            }
        }
    }
    let fallback = fs::metadata(path).map(|m| mtime_ms(&m))?;
    let session = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut data = AntigravityData::default();
    let mut retained = 0;
    for (_, entry) in generations.iter().chain(&steps) {
        if entry.model_id > 0 && concrete(&entry.model) {
            remember(&mut data.models, entry.model_id, Some(entry.model.clone()));
        }
    }
    for (source, entries) in [("step", &steps), ("generation", &generations)] {
        for (index, (idx, entry)) in entries.iter().enumerate() {
            for (usage_index, u) in entry.usages.iter().enumerate() {
                let id = if number(u, 1) == 0 {
                    entry.model_id
                } else {
                    number(u, 1)
                };
                let generation = if source == "step" {
                    generations.iter().find(|(i, _)| i == idx).map(|(_, m)| m)
                } else {
                    None
                };
                let model = if (id == 0 || id == entry.model_id) && !entry.model.is_empty() {
                    entry.model.clone()
                } else {
                    generation
                        .filter(|g| id == 0 || id == g.model_id)
                        .map(|g| g.model.clone())
                        .unwrap_or_default()
                };
                let output = number(u, 3).max(number(u, 9).saturating_add(number(u, 10)));
                let totals = UsageTokenTotals {
                    uncached_input_tokens: number(u, 2),
                    cached_input_tokens: number(u, 5),
                    cache_creation_tokens: number(u, 4),
                    output_tokens: output,
                    reasoning_tokens: number(u, 9).min(output),
                };
                if total_tokens(&totals) == 0 {
                    continue;
                }
                let mut keys = vec![];
                for key in [11, 12, 7] {
                    let id = string(u, key)?;
                    if !id.is_empty() {
                        keys.push(format!("antigravity:{key}:{id}"));
                    }
                }
                let model_quality = if concrete(&model) {
                    4
                } else if model.is_empty() {
                    0
                } else {
                    1
                };
                if data.candidates.len() >= MAX_RECORDS {
                    return Err(safety::exceeded().into());
                }
                retained +=
                    model.len() + session.len() + keys.iter().map(String::len).sum::<usize>() + 256;
                if retained > MAX_RETAINED_BYTES {
                    return Err(safety::exceeded().into());
                }
                data.candidates.push(Candidate {
                    record: UsageRecord {
                        provider: P::Antigravity,
                        timestamp_ms: entry.at.or(trajectory).unwrap_or(fallback),
                        model,
                        rate_model: None,
                        session_id: session.into(),
                        totals,
                        cache_creation1h_tokens: None,
                        reported_cost_usd: None,
                        speed: UsageSpeed::Standard,
                        unresolved_service_tier_turn_id: None,
                        dedupe_key: Some(keys.first().cloned().unwrap_or_else(|| {
                            format!("antigravity:{session}:{source}:{index}:{usage_index}")
                        })),
                    },
                    model_id: id,
                    model_quality,
                    time_quality: if entry.at.is_some() {
                        2
                    } else if trajectory.is_some() {
                        1
                    } else {
                        0
                    },
                    keys,
                });
            }
        }
    }
    Ok(data)
}
fn remember(models: &mut HashMap<u64, Option<String>>, id: u64, model: Option<String>) {
    if let Some(old) = models.get_mut(&id) {
        if old != &model {
            *old = None;
        }
    } else {
        models.insert(id, model);
    }
}

#[derive(Debug, Default, Clone)]
pub struct AntigravityCache {
    entries: HashMap<PathBuf, (String, AntigravityData)>,
}
impl AntigravityCache {
    pub(crate) fn retained_size(&self) -> usize {
        self.entries
            .values()
            .map(|(_, data)| data.retained_size())
            .sum()
    }
}
pub fn read_antigravity(
    roots: &[PathBuf],
    since: f64,
    cache: &mut AntigravityCache,
) -> DatabaseRead {
    read_antigravity_bounded(roots, since, cache, &ScanBudget::default())
}
pub(crate) fn read_antigravity_bounded(
    roots: &[PathBuf],
    since: f64,
    cache: &mut AntigravityCache,
    budget: &ScanBudget,
) -> DatabaseRead {
    let mut out = DatabaseRead {
        missing: true,
        ..Default::default()
    };
    let mut visited = HashSet::new();
    let mut models = HashMap::new();
    let mut pending = vec![];
    let mut remaining = MAX_ENTRIES;
    for root in roots.iter().take(MAX_ENTRIES) {
        let mut paths = vec![root.clone()];
        while let Some(path) = paths.pop() {
            if remaining == 0 {
                out.errors.push(root.clone());
                break;
            }
            remaining -= 1;
            let metadata = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        out.errors.push(path);
                    }
                    continue;
                }
            };
            if metadata.is_dir() {
                if path
                    .strip_prefix(root)
                    .is_ok_and(|p| p.components().count() > MAX_DEPTH)
                {
                    out.errors.push(path);
                    continue;
                }
                let entries = match fs::read_dir(&path) {
                    Ok(e) => e,
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            out.errors.push(path);
                        }
                        continue;
                    }
                };
                let mut entries = entries
                    .take(remaining)
                    .filter_map(|entry| match entry {
                        Ok(entry) => Some(entry.path()),
                        Err(_) => {
                            out.errors.push(path.clone());
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                entries.sort();
                paths.extend(entries.into_iter().rev());
                continue;
            }
            if !metadata.is_file() || path.extension().is_none_or(|e| e != "db") {
                continue;
            }
            let canonical = fs::canonicalize(&path).unwrap_or(path.clone());
            if !visited.insert(canonical.clone()) {
                continue;
            }
            out.missing = false;
            let snapshot = match connect_bounded(&path, budget) {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    out.errors.push(path);
                    continue;
                }
            };
            let fingerprint = snapshot.fingerprint.clone();
            let read = if let Some((_, data)) = cache
                .entries
                .get(&canonical)
                .filter(|(f, _)| f == &fingerprint)
            {
                budget
                    .retain(data.retained_size())
                    .map(|()| data.clone())
                    .map_err(Error::from)
            } else {
                read_antigravity_db(&path, &snapshot).and_then(|data| {
                    budget.retain(data.retained_size())?;
                    Ok(data)
                })
            };
            match read {
                Ok(data) => {
                    for (id, model) in &data.models {
                        remember(&mut models, *id, model.clone());
                    }
                    cache.entries.insert(canonical, (fingerprint, data.clone()));
                    let index = out.files.len();
                    out.files.push((path, vec![]));
                    pending.push((index, data));
                }
                Err(_) => out.errors.push(path),
            }
        }
    }
    cache.entries.retain(|p, _| visited.contains(p));
    let mut groups: Vec<(Candidate, usize, usize)> = vec![];
    let mut identities: HashMap<String, usize> = HashMap::new();
    fn find(groups: &[(Candidate, usize, usize)], mut i: usize) -> usize {
        while groups[i].2 != i {
            i = groups[i].2;
        }
        i
    }
    for (file, data) in pending {
        for mut candidate in data.candidates {
            let learned = models.get(&candidate.model_id).and_then(|m| m.clone());
            let fallback = if models.get(&candidate.model_id) == Some(&None) {
                None
            } else {
                fallback_model(candidate.model_id).map(str::to_string)
            };
            if candidate.model_quality != 4 {
                if let Some(model) = learned {
                    candidate.record.model = model;
                    candidate.model_quality = 3;
                } else if let Some(model) = fallback {
                    candidate.record.model = model;
                    candidate.model_quality = 2;
                } else if candidate.record.model.is_empty() {
                    candidate.record.model = if candidate.model_id > 0 {
                        format!("antigravity-model-{}", candidate.model_id)
                    } else {
                        "antigravity-unknown".into()
                    };
                }
            }
            let index = groups.len();
            let keys = candidate.keys.clone();
            groups.push((candidate, file, index));
            for key in keys {
                if let Some(previous) = identities.get(&key) {
                    let a = find(&groups, *previous);
                    let b = find(&groups, index);
                    if a != b {
                        let (first, second) = if a < b { (a, b) } else { (b, a) };
                        let copy = groups[second].0.clone();
                        let held = &mut groups[first].0;
                        let best_model = (copy.model_quality > held.model_quality)
                            .then(|| copy.record.model.clone());
                        let best_time = (copy.time_quality > held.time_quality
                            || (copy.time_quality == held.time_quality
                                && copy.record.timestamp_ms < held.record.timestamp_ms))
                            .then_some(copy.record.timestamp_ms);
                        merge_usage_copies(&mut held.record, &copy.record);
                        if let Some(m) = best_model {
                            held.record.model = m;
                            held.model_quality = copy.model_quality;
                        }
                        if let Some(t) = best_time {
                            held.record.timestamp_ms = t;
                            held.time_quality = copy.time_quality;
                        }
                        groups[second].2 = first;
                    }
                }
                identities.insert(key, index);
            }
        }
    }
    for (i, (mut candidate, file, parent)) in groups.into_iter().enumerate() {
        if i != parent || candidate.record.timestamp_ms < since {
            continue;
        }
        let model = &candidate.record.model;
        let base = [
            "extra-low",
            "low",
            "medium",
            "high",
            "tiered",
            "control",
            "thinking",
            "agent",
        ]
        .into_iter()
        .find_map(|s| model.strip_suffix(&format!("-{s}")))
        .unwrap_or(model);
        let rate = if [
            "gemini-3-flash-agent",
            "gemini-3-flash-agent-a",
            "gemini-3-flash-agent-b",
            "gemini-3-flash-a",
            "gemini-3-flash-b",
        ]
        .contains(&model.as_str())
        {
            "gemini-3.5-flash"
        } else if model == "gemini-3.7-flash-safety-le" {
            "gemini-3.7-flash"
        } else if base == "gemini-3-flash" {
            "gemini-3-flash-preview"
        } else {
            base
        };
        if rate != model {
            candidate.record.rate_model = Some(rate.into());
        }
        out.files[file].1.push(candidate.record);
    }
    out
}

fn fallback_model(id: u64) -> Option<&'static str> {
    Some(match id {
        246 => "gemini-2.5-pro",
        312 => "gemini-2.5-flash",
        313 | 329 => "gemini-2.5-flash-thinking",
        330 => "gemini-2.5-flash-lite",
        281 | 282 => "claude-sonnet-4",
        290 | 291 => "claude-opus-4",
        333 | 334 => "claude-sonnet-4-5",
        340 | 341 => "claude-haiku-4-5",
        342 => "gpt-oss-120b-medium",
        1026 => "claude-opus-4-6",
        1035 => "claude-sonnet-4-6",
        1016 | 1036 | 1037 => "gemini-3.1-pro",
        1018 | 1084 | 1047 => "gemini-3-flash-preview",
        1020 => "gemini-3.5-flash-medium",
        1021 => "gemini-3.1-flash-image",
        1050 => "gemini-3.1-flash-lite",
        1071 => "gemini-3.6-flash-high",
        1072 => "gemini-3.6-flash-medium",
        1073 => "gemini-3.6-flash-low",
        1132 | 1133 => "gemini-3.5-flash-high",
        1187 => "gemini-3.5-flash-extra-low",
        1196 | 1266 => "gemini-3.6-flash",
        1298 => "gemini-3.7-flash-high",
        1299 => "gemini-3.7-flash-medium",
        1300 => "gemini-3.7-flash-low",
        1301 => "gemini-3.7-flash-tiered",
        1318 => "gemini-3.8-flash-high",
        1319 => "gemini-3.8-flash-medium",
        1320 => "gemini-3.8-flash-low",
        1322 => "gemini-3.8-flash-tiered",
        1402 => "claude-opus-5-5",
        1417..=1419 => "gemini-4-argon",
        _ => return None,
    })
}

#[cfg(test)]
mod snapshot_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::snapshot::connect;
    #[test]
    fn hostile_protobuf_field_counts_are_rejected_before_expanding() {
        let blob = [8, 0].repeat(MAX_ENTRIES + 1);
        assert!(fields(&blob).is_err());
    }
    #[test]
    fn scanning_snapshot_does_not_pin_the_owners_wal_or_block_writes() {
        for mode in ["WAL", "DELETE"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("owner.db");
            let owner = Connection::open(&path).unwrap();
            owner.busy_timeout(std::time::Duration::ZERO).unwrap();
            owner.execute_batch(&format!("PRAGMA journal_mode={mode}; PRAGMA wal_autocheckpoint=0; CREATE TABLE message(data TEXT); INSERT INTO message VALUES ('first');")).unwrap();
            let scan = connect(&path).unwrap();
            scan.execute_batch("BEGIN").unwrap();
            assert_eq!(
                scan.query_row("SELECT count(*) FROM message", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            owner
                .execute("INSERT INTO message VALUES ('second')", [])
                .unwrap();
            if mode == "WAL" {
                let (busy, log, done): (i64, i64, i64) = owner
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .unwrap();
                assert_eq!((busy, log, done), (0, 0, 0));
            }
            assert_eq!(
                scan.query_row("SELECT count(*) FROM message", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            // Windows must also allow the owner to rename the source while decoding continues.
            drop(owner);
            fs::rename(&path, temp.path().join("renamed.db")).unwrap();
        }
    }
}
