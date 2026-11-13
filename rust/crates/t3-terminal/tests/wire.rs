use serde::{Serialize, de::DeserializeOwned};
use t3_terminal::*;

fn check<T: DeserializeOwned + Serialize>(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("fixtures/manifest.json").is_file())
        .ok_or("Rust fixtures not found")?;
    let path = root.join("fixtures/samples").join(format!("{name}.jsonl"));
    let contents = std::fs::read_to_string(path)?;
    assert!(!contents.is_empty());
    for (line, sample) in contents.lines().enumerate() {
        let expected: serde_json::Value = serde_json::from_str(sample)?;
        let actual = serde_json::to_value(
            serde_json::from_value::<T>(expected.clone())
                .unwrap_or_else(|e| panic!("{name}:{}: {e}", line + 1)),
        )?;
        assert_eq!(actual, expected, "{name}:{}", line + 1);
    }
    Ok(())
}
#[test]
fn terminal_types_roundtrip_typescript_golden_samples() -> Result<(), Box<dyn std::error::Error>> {
    macro_rules! check {($($ty:ty),*$(,)?)=>{$(check::<$ty>(stringify!($ty))?;)*};}
    check!(
        TerminalThreadInput,
        TerminalOpenInput,
        TerminalAttachInput,
        TerminalObserveInput,
        TerminalWriteInput,
        TerminalResizeInput,
        TerminalClearInput,
        TerminalCloseInput,
        TerminalSessionStatus,
        TerminalSessionSnapshot,
        TerminalSummary,
        TerminalMetadataStreamEvent,
        TerminalEvent,
        TerminalAttachStreamEvent
    );
    Ok(())
}
