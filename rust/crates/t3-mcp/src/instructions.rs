use serde_json::Value;
use std::sync::OnceLock;

pub fn fixture() -> &'static Value {
    static INSTRUCTIONS: OnceLock<Value> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/instructions.json"))
            .expect("generated instructions are valid")
    })
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub struct RuntimeInfo<'a> {
    pub harness: &'a str,
    pub model: &'a str,
    pub model_name: &'a str,
    pub reasoning_effort: &'a str,
    pub pull_request_linking: bool,
}

pub fn runtime(info: &RuntimeInfo<'_>) -> String {
    let model = single_line(info.model);
    let name = single_line(info.model_name);
    let effort = single_line(info.reasoning_effort);
    let model_present = !model.is_empty() && model != "auto" && model != "default";
    let template = &fixture()[format!(
        "runtime/{}/{}/{}/{}",
        model_present,
        !name.is_empty() && name != model,
        !effort.is_empty(),
        info.pull_request_linking
    )];
    template
        .as_str()
        .expect("runtime template exists")
        .replace("__HARNESS__", &single_line(info.harness))
        .replace("__MODEL_NAME__", &name)
        .replace("__MODEL__", &model)
        .replace("__EFFORT__", &effort)
}

pub fn codex(info: &RuntimeInfo<'_>, browser: bool, device: bool) -> Value {
    let mut context = fixture()[format!("codex/{browser}/{device}")].clone();
    context["t3_code_runtime"]["value"] = runtime(info).into();
    context
}

pub fn claude(has_mcp: bool, device: bool) -> &'static str {
    fixture()[format!("claude/{has_mcp}/{device}")]
        .as_str()
        .expect("Claude template exists")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AcpState {
    pub plan: bool,
    pub has_mcp: bool,
    pub device: bool,
}

pub fn acp(prompt: &str, state: AcpState, previous: Option<AcpState>) -> String {
    if prompt.trim_start().starts_with('/') || previous == Some(state) {
        return prompt.into();
    }
    fixture()[format!(
        "acp/{}/{}/{}",
        if state.plan { "plan" } else { "default" },
        state.has_mcp,
        state.device
    )]
    .as_str()
    .expect("ACP template exists")
    .replace("__PROMPT__", prompt)
}

pub fn orchestration_prompt(
    prompt: &str,
    has_mcp: bool,
    device: bool,
    delivered: bool,
    device_delivered: bool,
) -> String {
    fixture()[format!("prompt/{has_mcp}/{device}/{delivered}/{device_delivered}")]
        .as_str()
        .expect("prompt template exists")
        .replace("__PROMPT__", prompt)
}

pub fn orchestration_system_prompt(has_mcp: bool, device: bool) -> Option<&'static str> {
    fixture()[format!("system/{has_mcp}/{device}")].as_str()
}
