use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

#[path = "golden/mod.rs"]
mod golden;

type RoundTrip = fn(Value) -> Result<Value, serde_json::Error>;
type Registry = BTreeMap<&'static str, Codecs>;

/// One schema's Rust type in both JSON dialects (see `t3_contracts::stored`).
#[derive(Clone, Copy)]
struct Codecs {
    rpc: RoundTrip,
    stored: RoundTrip,
    stored_text: fn(Value) -> Result<String, serde_json::Error>,
}

fn round_trip<T: DeserializeOwned + Serialize>(value: Value) -> Result<Value, serde_json::Error> {
    serde_json::to_value(serde_json::from_value::<T>(value)?)
}

fn stored_round_trip<T: DeserializeOwned + Serialize>(
    value: Value,
) -> Result<Value, serde_json::Error> {
    t3_contracts::stored::to_value(&serde_json::from_value::<T>(value)?)
}

fn stored_text<T: DeserializeOwned + Serialize>(value: Value) -> Result<String, serde_json::Error> {
    t3_contracts::stored::to_string(&serde_json::from_value::<T>(value)?)
}

fn register<T: DeserializeOwned + Serialize>(registry: &mut Registry, name: &'static str) {
    let codecs = Codecs {
        rpc: round_trip::<T>,
        stored: stored_round_trip::<T>,
        stored_text: stored_text::<T>,
    };
    assert!(
        registry.insert(name, codecs).is_none(),
        "Duplicate registration: {name}"
    );
}

#[derive(Deserialize)]
struct Manifest {
    samples: Vec<SampleFile>,
    #[serde(default)]
    stored: Vec<StoredFile>,
}

#[derive(Deserialize)]
struct StoredFile {
    schema: String,
    parent: String,
    boundary: String,
    file: String,
    count: usize,
}

#[derive(Deserialize)]
struct SampleFile {
    schema: String,
    file: String,
    count: usize,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn manifest() -> Result<Manifest, Box<dyn Error>> {
    Ok(serde_json::from_str(&fs::read_to_string(
        fixtures().join("manifest.json"),
    )?)?)
}

fn owners() -> Result<BTreeMap<String, String>, serde_json::Error> {
    serde_json::from_str(include_str!("schema_modules.json"))
}

fn check_module(module: &str, register_module: fn(&mut Registry)) -> Result<(), Box<dyn Error>> {
    let mut registry = Registry::new();
    register_module(&mut registry);
    let manifest = manifest()?;
    let owners = owners()?;
    for name in registry.keys() {
        assert_eq!(
            owners.get(*name).map(String::as_str),
            Some(module),
            "{name} registered in wrong module"
        );
    }
    for entry in &manifest.samples {
        let Some(check) = registry.get(entry.schema.as_str()) else {
            continue;
        };
        let contents = fs::read_to_string(fixtures().join(&entry.file))?;
        assert_eq!(
            contents.lines().count(),
            entry.count,
            "{} sample count",
            entry.schema
        );
        assert!(entry.count > 0, "{} has no samples", entry.schema);
        for (line, sample) in contents.lines().enumerate() {
            let before: Value = serde_json::from_str(sample)?;
            let after = (check.rpc)(before.clone())
                .unwrap_or_else(|error| panic!("{}:{}: {error}", entry.schema, line + 1));
            assert_eq!(before, after, "{}:{}", entry.schema, line + 1);
        }
    }
    Ok(())
}

#[test]
fn coverage() -> Result<(), Box<dyn Error>> {
    let registry = golden::registry();
    let manifest = manifest()?;
    let owners = owners()?;
    let mut breakdown: BTreeMap<&str, (usize, usize, usize)> = golden::MODULES
        .iter()
        .map(|module| (*module, (0, 0, 0)))
        .collect();
    let mut missing = Vec::new();
    let mut samples = 0;
    for entry in &manifest.samples {
        let module = owners
            .get(&entry.schema)
            .unwrap_or_else(|| panic!("{} missing from schema_modules.json", entry.schema));
        let counts = breakdown
            .get_mut(module.as_str())
            .unwrap_or_else(|| panic!("Unknown module {module}"));
        counts.1 += 1;
        if registry.contains_key(entry.schema.as_str()) {
            counts.0 += 1;
            counts.2 += entry.count;
            samples += entry.count;
        } else {
            missing.push(entry.schema.as_str());
        }
    }
    assert_eq!(
        owners.len(),
        manifest.samples.len(),
        "Stale schema ownership table"
    );
    for name in registry.keys() {
        assert!(
            manifest.samples.iter().any(|entry| entry.schema == *name),
            "Unknown registered schema {name}"
        );
    }
    eprintln!(
        "Rust wire coverage: {}/{} schemas, {samples} samples; {} unregistered",
        registry.len(),
        manifest.samples.len(),
        missing.len()
    );
    for (module, (registered, total, samples)) in breakdown {
        eprintln!("  {module}: {registered}/{total} schemas, {samples} samples");
    }
    // ACP JSON-RPC groups are carried by t3-acp's own types, not by this crate.
    let unexpected: Vec<_> = missing
        .iter()
        .filter(|name| {
            ![
                "rpc.AgentRpcs.",
                "rpc.ClientRpcs.",
                "rpc.CompatAgentRpcs.",
                "rpc.CompatClientRpcs.",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "unregistered schemas: {unexpected:?}"
    );
    Ok(())
}

/// Mutated samples (`fixtures/differential.jsonl`, generated by the real
/// TypeScript decoder) are accepted or rejected, and re-encoded, exactly as
/// TypeScript does. `differential_gaps.txt` lists the known, not yet fixed
/// differences as `schema<TAB>mutation<TAB>class`; any other mismatch fails, and
/// so does a listed gap that now matches (remove it). Regenerate the list with
/// `T3_CONTRACT_DIFF_BLESS=1` only after reviewing every added line.
#[test]
fn differential() -> Result<(), Box<dyn Error>> {
    let registry = golden::registry();
    let text = fs::read_to_string(fixtures().join("differential.jsonl"))?;
    let gaps_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/differential_gaps.txt");
    let known: std::collections::BTreeSet<String> = fs::read_to_string(&gaps_path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect();
    let mut failures = Vec::new();
    let mut actual = std::collections::BTreeSet::new();
    let mut total = 0;
    for line in text.lines() {
        let row: Value = serde_json::from_str(line)?;
        let name = row["s"].as_str().ok_or("schema")?;
        let check = registry
            .get(name)
            .ok_or_else(|| format!("unregistered {name}"))?;
        let result = (check.rpc)(row["i"].clone());
        let accepted = row["t"] == true;
        let class = match &result {
            Ok(value) if accepted && *value == row["o"] => None,
            Ok(_) if accepted => Some("output-differs"),
            Ok(_) => Some("rust-accepts"),
            Err(_) if accepted => Some("rust-rejects"),
            Err(_) => None,
        };
        if let Some(class) = class {
            let key = format!("{name}	{}	{class}", row["k"].as_str().unwrap_or_default());
            if !known.contains(&key) {
                failures.push(serde_json::json!({"s": name, "k": row["k"], "i": row["i"], "ts": row["o"], "accepted": accepted, "rust": result.map_err(|e| e.to_string())}));
            }
            actual.insert(key);
        }
        total += 1;
    }
    if std::env::var_os("T3_CONTRACT_DIFF_BLESS").is_some() {
        let header = "# Known differential gaps: schema, mutation, class. Each is a defect to fix, not an accepted difference.
";
        fs::write(
            &gaps_path,
            header.to_owned()
                + &actual.iter().cloned().collect::<Vec<_>>().join(
                    "
",
                )
                + "
",
        )?;
        return Ok(());
    }
    if let Ok(path) = std::env::var("T3_CONTRACT_DIFF_REPORT") {
        fs::write(path, serde_json::to_string_pretty(&failures)?)?;
    }
    let fixed: Vec<_> = known.difference(&actual).collect();
    assert!(
        failures.is_empty(),
        "{} / {total} new differential mismatches; first: {:?}",
        failures.len(),
        failures.first()
    );
    assert!(
        fixed.is_empty(),
        "{} listed gaps now match TypeScript; remove them from differential_gaps.txt: {:?}",
        fixed.len(),
        &fixed[..fixed.len().min(5)]
    );
    eprintln!(
        "Differential parity: {total} vectors, {} known gaps",
        known.len()
    );
    Ok(())
}

/// Stored-dialect samples (plain TypeScript codec) decode and re-encode
/// byte-for-byte equal through `t3_contracts::stored`, so a Rust-written row
/// never carries the `null` the TypeScript store rejects.
#[test]
fn stored_dialect() -> Result<(), Box<dyn Error>> {
    let registry = golden::registry();
    let manifest = manifest()?;
    assert!(
        !manifest.stored.is_empty(),
        "manifest has no stored samples"
    );
    let mut failures = Vec::new();
    let mut total = 0;
    // For rust/tools/export/verifyStored.ts: Rust's stored text, checked by the real decoder.
    let mut rust_rows = Vec::new();
    for entry in &manifest.stored {
        let check = registry
            .get(entry.parent.as_str())
            .ok_or_else(|| format!("unregistered stored schema {}", entry.parent))?;
        let contents = fs::read_to_string(fixtures().join(&entry.file))?;
        assert_eq!(contents.lines().count(), entry.count, "{}", entry.file);
        for (line, sample) in contents.lines().enumerate() {
            total += 1;
            let before: Value = serde_json::from_str(sample)?;
            if let Ok(text) = (check.stored_text)(before.clone()) {
                rust_rows.push(serde_json::to_string(
                    &serde_json::json!({"schema": entry.parent, "sample": entry.file, "line": line + 1, "rust": text}),
                )?);
            }
            match (check.stored)(before.clone()) {
                Ok(after) if after == before => {}
                Ok(after) => failures.push(format!(
                    "{} [{}] {}:{}: stored {after}, TypeScript {before}",
                    entry.schema,
                    entry.boundary,
                    entry.file,
                    line + 1
                )),
                Err(error) => failures.push(format!(
                    "{} [{}] {}:{}: {error}",
                    entry.schema,
                    entry.boundary,
                    entry.file,
                    line + 1
                )),
            }
        }
    }
    if let Ok(path) = std::env::var("T3_CONTRACT_STORED_REPORT") {
        fs::write(
            path,
            failures.join(
                "
",
            ),
        )?;
    }
    if let Ok(path) = std::env::var("T3_CONTRACT_STORED_OUT") {
        fs::write(
            path,
            rust_rows.join(
                "
",
            ) + "
",
        )?;
    }
    assert!(
        failures.is_empty(),
        "{} / {total} stored samples differ; first: {}",
        failures.len(),
        failures.first().map(String::as_str).unwrap_or_default()
    );
    eprintln!("Stored dialect: {total} samples");
    Ok(())
}
