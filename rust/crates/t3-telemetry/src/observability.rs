use crate::datetime;
use std::collections::{BTreeMap, VecDeque};
use t3_contracts::server::ServerEventLoopStall;

#[derive(Debug, Clone)]
pub struct EventLoopReadings {
    pub delay_max_ns: u64,
    pub active_ms: u64,
    pub cpu_count: u64,
    pub load_average: Option<f64>,
}
pub fn stall_ms(readings: &EventLoopReadings) -> Option<u64> {
    let delay = (readings.delay_max_ns as f64 / 1_000_000.0).round() as u64;
    let stall = delay.saturating_sub(1000);
    (stall > 2000 && readings.active_ms >= stall).then_some(stall)
}
#[derive(Default)]
pub struct StallMonitor {
    latest: Option<(u64, ServerEventLoopStall)>,
}
impl StallMonitor {
    pub fn sample(&mut self, readings: &EventLoopReadings, now_ms: u64) -> Option<u64> {
        let duration = stall_ms(readings)?;
        if duration >= 5000 {
            self.latest = Some((
                now_ms,
                ServerEventLoopStall {
                    detected_at: datetime(now_ms),
                    duration_ms: duration,
                    load_average: readings
                        .load_average
                        .map(|v| crate::number((v * 10.0).round() / 10.0)),
                    cpu_count: readings.cpu_count,
                },
            ));
        }
        let span = tracing::warn_span!(
            "server.eventLoop.stall",
            delayMaxMs = duration,
            cpuCount = readings.cpu_count
        );
        let _entered = span.enter();
        tracing::warn!(duration_ms = duration, "event loop stalled");
        Some(duration)
    }
    pub fn latest_noticeable_stall(&self, now_ms: u64) -> Option<&ServerEventLoopStall> {
        self.latest
            .as_ref()
            .filter(|(at, _)| now_ms.saturating_sub(*at) <= 5 * 60_000)
            .map(|(_, stall)| stall)
    }
}

const MB: u64 = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeapSuspect {
    pub name: String,
    pub growth_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeapSpike {
    pub heap_used_bytes: u64,
    pub growth_bytes: u64,
    pub suspects: Vec<HeapSuspect>,
}
struct OpenSpan {
    name: String,
    start_heap: u64,
    start_tick: u64,
}
#[derive(Default)]
pub struct HeapAttribution {
    open: BTreeMap<u64, OpenSpan>,
    recent: VecDeque<(u64, HeapSuspect)>,
    samples: VecDeque<u64>,
    cooldown: u64,
    tick: u64,
}
impl HeapAttribution {
    pub fn open(&mut self, id: u64, name: &str, heap_used: u64) {
        self.open.insert(
            id,
            OpenSpan {
                name: name.into(),
                start_heap: heap_used,
                start_tick: self.tick,
            },
        );
    }
    pub fn end(&mut self, id: u64, heap_used: u64) -> Option<u64> {
        let span = self.open.remove(&id)?;
        let growth = heap_used.saturating_sub(span.start_heap);
        if growth < 8 * MB {
            return None;
        }
        self.recent.push_back((
            self.tick,
            HeapSuspect {
                name: span.name,
                growth_bytes: growth,
            },
        ));
        if self.recent.len() > 32 {
            self.recent.pop_front();
        }
        Some(growth)
    }
    pub fn sample(&mut self, heap_used: u64) -> Option<HeapSpike> {
        self.tick += 1;
        self.samples.push_back(heap_used);
        if self.samples.len() > 10 {
            self.samples.pop_front();
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return None;
        }
        let growth =
            heap_used.saturating_sub(self.samples.iter().copied().min().unwrap_or(heap_used));
        if growth < 128 * MB {
            return None;
        }
        self.cooldown = 60;
        let mut suspects: Vec<_> = self
            .open
            .values()
            .filter(|s| self.tick.saturating_sub(s.start_tick) <= self.samples.len() as u64)
            .map(|s| HeapSuspect {
                name: s.name.clone(),
                growth_bytes: heap_used.saturating_sub(s.start_heap),
            })
            .chain(
                self.recent
                    .iter()
                    .filter(|(at, _)| self.tick.saturating_sub(*at) <= self.samples.len() as u64)
                    .map(|(_, s)| s.clone()),
            )
            .filter(|s| s.growth_bytes >= 8 * MB)
            .collect();
        suspects.sort_by(|a, b| {
            b.growth_bytes
                .cmp(&a.growth_bytes)
                .then(a.name.cmp(&b.name))
        });
        suspects.truncate(8);
        self.recent.clear();
        Some(HeapSpike {
            heap_used_bytes: heap_used,
            growth_bytes: growth,
            suspects,
        })
    }
}
impl HeapSpike {
    pub fn record(&self) {
        let suspects: Vec<_> = self
            .suspects
            .iter()
            .map(|s| {
                format!(
                    "{} +{} MB",
                    s.name,
                    (s.growth_bytes as f64 / MB as f64).round()
                )
            })
            .collect();
        let span = tracing::warn_span!("server.heap.spike",heapUsedMb=(self.heap_used_bytes as f64/MB as f64).round() as u64,growthMb=(self.growth_bytes as f64/MB as f64).round() as u64,suspects=?suspects);
        let _entered = span.enter();
    }
}
pub fn normalize_model_metric_label(model: Option<&str>) -> Option<&'static str> {
    let model = model?.trim().to_lowercase();
    if model.is_empty() {
        return None;
    }
    Some(if model.contains("gpt") {
        "gpt"
    } else if model.contains("claude") {
        "claude"
    } else if model.contains("gemini") {
        "gemini"
    } else {
        "other"
    })
}
pub fn report_defect(tag: &str) {
    let tag = if tag.len() <= 128
        && !tag.is_empty()
        && tag
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:/-".contains(&c))
    {
        tag
    } else {
        "UnknownError"
    };
    tracing::error!(error_tag = tag, "Unhandled defect");
}
