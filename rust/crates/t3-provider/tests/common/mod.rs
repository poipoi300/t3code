#![allow(dead_code)]
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use t3_provider::environment::Environment;

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub config: PathBuf,
    pub log: PathBuf,
    pub environment: Environment,
}
impl Fixture {
    pub fn new(config: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.json");
        let log = directory.path().join("calls.jsonl");
        let mut environment = Environment::from([(
            "T3_PROVIDER_FIXTURE".into(),
            path.to_string_lossy().into_owned(),
        )]);
        if let Ok(system_root) = std::env::var("SystemRoot") {
            environment.insert("SystemRoot".into(), system_root);
        }
        let fixture = Self {
            directory,
            config: path,
            log,
            environment,
        };
        fixture.write(config);
        fixture
    }
    pub fn write(&self, mut config: Value) {
        if !config.is_object() {
            config = json!({});
        }
        config["log"] = json!(self.log);
        fs::write(&self.config, serde_json::to_vec(&config).unwrap()).unwrap();
    }
    pub fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn binary(&self) -> String {
        env!("CARGO_BIN_EXE_provider-fixture").into()
    }
}
