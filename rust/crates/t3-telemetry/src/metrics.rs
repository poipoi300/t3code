use crate::TelemetryNumber;
use crate::privacy;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram, Meter},
};
use std::{collections::BTreeMap, sync::Mutex};
use t3_contracts::resource_telemetry::{
    ResourceTelemetryProcess, ResourceTelemetryProcessCategory as C,
};

pub struct Metrics {
    meter: Meter,
    counters: Mutex<BTreeMap<String, Counter<u64>>>,
    timers: Mutex<BTreeMap<String, Histogram<f64>>>,
    gauges: Mutex<BTreeMap<String, Gauge<f64>>>,
}
impl Metrics {
    pub fn new(meter: Meter) -> Self {
        Self {
            meter,
            counters: Mutex::new(BTreeMap::new()),
            timers: Mutex::new(BTreeMap::new()),
            gauges: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn increment(&self, name: &str, attributes: &BTreeMap<String, String>, amount: u64) {
        if let Ok(mut counters) = self.counters.lock() {
            counters
                .entry(name.to_owned())
                .or_insert_with(|| {
                    self.meter
                        .u64_counter(name.to_owned())
                        .with_unit("1")
                        .build()
                })
                .add(amount, &labels(attributes));
        }
    }
    pub fn duration(&self, name: &str, attributes: &BTreeMap<String, String>, seconds: f64) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        if let Ok(mut timers) = self.timers.lock() {
            timers
                .entry(name.to_owned())
                .or_insert_with(|| {
                    self.meter
                        .f64_histogram(name.to_owned())
                        .with_unit("ms")
                        .with_boundaries((0..35).map(|i| 0.5 * 2f64.powi(i)).collect())
                        .build()
                })
                .record(seconds * 1000.0, &{
                    let mut labels = labels(attributes);
                    labels.push(KeyValue::new("time_unit", "milliseconds"));
                    labels
                });
        }
    }
    pub fn gauge(&self, name: &str, unit: &str, attributes: &BTreeMap<String, String>, value: f64) {
        if !value.is_finite() {
            return;
        }
        if let Ok(mut gauges) = self.gauges.lock() {
            gauges
                .entry(name.to_owned())
                .or_insert_with(|| {
                    self.meter
                        .f64_gauge(name.to_owned())
                        .with_unit(if unit.is_empty() { "1" } else { unit }.to_owned())
                        .build()
                })
                .record(value, &{
                    let mut labels = labels(attributes);
                    if !unit.is_empty() {
                        labels.push(KeyValue::new("unit", unit.to_owned()));
                    }
                    labels
                });
        }
    }
}
fn labels(attributes: &BTreeMap<String, String>) -> Vec<KeyValue> {
    attributes
        .iter()
        .filter(|(key, _)| !privacy::private_field(key))
        .map(|(key, value)| KeyValue::new(key.clone(), privacy::redact_text(value)))
        .collect()
}

#[derive(Default, Debug, Clone)]
pub struct ProcessGroupTotals {
    pub resident_bytes: u64,
    pub cpu_percent: f64,
    pub count: u64,
}
fn child_group(name: &str, command: &str) -> &'static str {
    let name = name.to_lowercase();
    let script =
        ["node", "node.exe", "bun", "bun.exe", "deno", "deno.exe"].contains(&name.as_str());
    let text = if script {
        command.to_lowercase()
    } else {
        name.clone()
    };
    for (needle, group) in [
        ("codex", "codex"),
        ("claude", "claude"),
        ("cursor", "cursor"),
        ("opencode", "opencode"),
        ("grok", "grok"),
        ("antigravity", "antigravity"),
        ("gemini", "antigravity"),
    ] {
        if text.contains(needle) {
            return group;
        }
    }
    if script {
        return "other";
    }
    if name.contains("chrom") {
        return "browser";
    }
    if [
        "sh", "bash", "zsh", "fish", "dash", "csh", "sh.exe", "bash.exe", "zsh.exe", "fish.exe",
        "dash.exe", "csh.exe", "cmd.exe",
    ]
    .contains(&name.as_str())
        || name.starts_with("pwsh")
        || name.starts_with("powershell")
    {
        return "terminal";
    }
    "other"
}
pub fn group_processes(
    processes: &[ResourceTelemetryProcess],
) -> BTreeMap<String, ProcessGroupTotals> {
    let by_pid: BTreeMap<_, _> = processes.iter().map(|p| (p.identity.pid, p)).collect();
    let server_pid = processes
        .iter()
        .find(|p| p.category == C::Server)
        .map(|p| p.identity.pid);
    let mut totals = BTreeMap::<String, ProcessGroupTotals>::new();
    for p in processes {
        let group = match p.category {
            C::Server => "server",
            C::ResourceMonitor => "resource-monitor",
            C::ElectronMain | C::ElectronRenderer | C::ElectronGpu | C::ElectronUtility => {
                "electron"
            }
            _ => {
                let mut current = p;
                let mut group = "other";
                for _ in 0..processes.len() {
                    if Some(current.ppid) == server_pid {
                        group = child_group(&current.name, &current.command);
                        break;
                    }
                    let Some(parent) = by_pid.get(&current.ppid) else {
                        break;
                    };
                    current = parent;
                }
                group
            }
        };
        let total = totals.entry(group.into()).or_default();
        total.count += 1;
        total.resident_bytes += p.resident_bytes;
        total.cpu_percent += p.cpu_percent.value();
    }
    totals
}
pub fn record_process_groups(processes: &[ResourceTelemetryProcess]) {
    let metrics = Metrics::new(opentelemetry::global::meter("t3code-server"));
    let mut totals = group_processes(processes);
    for group in [
        "server",
        "resource-monitor",
        "electron",
        "codex",
        "claude",
        "cursor",
        "opencode",
        "grok",
        "antigravity",
        "browser",
        "terminal",
        "other",
    ] {
        let total = totals.remove(group).unwrap_or_default();
        let attributes = [("group".into(), group.into())].into_iter().collect();
        metrics.gauge(
            "t3_process_group_resident_bytes",
            "By",
            &attributes,
            total.resident_bytes as f64,
        );
        metrics.gauge(
            "t3_process_group_cpu_percent",
            "%",
            &attributes,
            total.cpu_percent,
        );
        metrics.gauge(
            "t3_process_group_count",
            "{process}",
            &attributes,
            total.count as f64,
        );
    }
}
