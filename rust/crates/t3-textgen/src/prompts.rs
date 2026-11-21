use crate::text::{head, limit_section, limit_title_message, tail, utf16_len};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};
use t3_contracts::chat_attachment::ChatAttachment;

#[derive(Deserialize)]
struct Fixtures {
    templates: BTreeMap<String, String>,
}
static FIXTURES: LazyLock<Fixtures> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/prompts.json"))
        .expect("extracted prompt fixture")
});

fn render(key: &str, values: &[(&str, String)]) -> String {
    let template = &FIXTURES.templates[key];
    let mut result = String::new();
    let mut rest = template.as_str();
    while let Some(start) = rest.find("__T3_SLOT_") {
        result.push_str(&rest[..start]);
        let body = &rest[start + 10..];
        let end = body.find("__").expect("fixture slot terminator");
        let name = &body[..end];
        result.push_str(
            &values
                .iter()
                .find(|(key, _)| *key == name)
                .expect("fixture slot binding")
                .1,
        );
        rest = &body[end + 2..];
    }
    result.push_str(rest);
    result
}

fn quoted_contents(value: &str) -> String {
    let json = serde_json::to_string(value).expect("string serialization");
    json[1..json.len() - 1].to_owned()
}

pub fn attachment_name(attachment: &ChatAttachment) -> &str {
    match attachment {
        ChatAttachment::Image(v) => &v.name,
        ChatAttachment::File(v) => &v.name,
        ChatAttachment::Unknown(v) => &v.name,
    }
}
pub fn attachment_id(attachment: &ChatAttachment) -> &str {
    match attachment {
        ChatAttachment::Image(v) => &v.id,
        ChatAttachment::File(v) => &v.id,
        ChatAttachment::Unknown(v) => &v.id,
    }
}
fn attachment_metadata(attachments: &[ChatAttachment]) -> String {
    limit_section(
        &attachments
            .iter()
            .map(|attachment| {
                let (name, mime, size) = match attachment {
                    ChatAttachment::Image(v) => (&v.name, &v.mime_type, &v.size_bytes),
                    ChatAttachment::File(v) => (&v.name, &v.mime_type, &v.size_bytes),
                    ChatAttachment::Unknown(v) => (&v.name, &v.mime_type, &v.size_bytes),
                };
                let size = serde_json::to_string(size).expect("wire number");
                format!("- {name} ({mime}, {size} bytes)")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        4_000,
    )
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationPolicy {
    #[serde(default)]
    pub commit_instructions: Option<String>,
    #[serde(default)]
    pub change_request_instructions: Option<String>,
    #[serde(default)]
    pub branch_instructions: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInput {
    pub branch: Option<String>,
    pub staged_summary: String,
    pub staged_patch: String,
    #[serde(default)]
    pub include_branch: bool,
    #[serde(default)]
    pub policy: GenerationPolicy,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrInput {
    pub base_branch: String,
    pub head_branch: String,
    pub commit_summary: String,
    pub diff_summary: String,
    pub diff_patch: String,
    pub change_request_template: Option<String>,
    pub thread_context: Option<String>,
    #[serde(default)]
    pub policy: GenerationPolicy,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum BranchNaming {
    #[default]
    Semantic,
    Static {
        prefix: String,
    },
    Custom {
        instructions: String,
    },
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInput {
    pub message: String,
    pub naming: Option<BranchNaming>,
    #[serde(default)]
    pub attachments: Vec<ChatAttachment>,
    #[serde(default)]
    pub policy: GenerationPolicy,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ForkSource {
    pub title: String,
    pub history: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTitleInput {
    pub message: String,
    pub project_title: Option<String>,
    pub previous_title: Option<String>,
    pub fork_source: Option<ForkSource>,
    pub linked_context: Option<String>,
    pub instructions: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ChatAttachment>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewLayer {
    pub number: u64,
    pub title: Option<String>,
    pub head_sha: Option<String>,
    pub state: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInput {
    pub number: u64,
    pub title: String,
    pub description: String,
    pub base_branch: String,
    pub base_commit: String,
    pub settled_review: String,
    pub instructions: Option<String>,
    #[serde(default)]
    pub stack: Vec<ReviewLayer>,
    #[serde(skip)]
    pub git_attr_source: Option<String>,
}

fn nonempty(value: Option<&str>) -> &str {
    value.unwrap_or("").trim()
}
pub fn commit_prompt(input: &CommitInput) -> String {
    let instructions = nonempty(input.policy.commit_instructions.as_deref());
    render(
        &format!(
            "commit_{}_{}",
            u8::from(input.include_branch),
            u8::from(!instructions.is_empty())
        ),
        &[
            (
                "branch",
                input.branch.clone().unwrap_or_else(|| "(detached)".into()),
            ),
            ("summary", limit_section(&input.staged_summary, 6000)),
            ("patch", limit_section(&input.staged_patch, 40000)),
            ("instructions", limit_section(instructions, 20000)),
        ],
    )
}
pub fn pr_prompt(input: &PrInput) -> String {
    let template = nonempty(input.change_request_template.as_deref());
    let context = nonempty(input.thread_context.as_deref());
    let instructions = nonempty(input.policy.change_request_instructions.as_deref());
    render(
        &format!(
            "pr_{}_{}_{}",
            u8::from(!template.is_empty()),
            u8::from(!context.is_empty()),
            u8::from(!instructions.is_empty())
        ),
        &[
            ("base", input.base_branch.clone()),
            ("head", input.head_branch.clone()),
            ("commits", limit_section(&input.commit_summary, 12000)),
            ("summary", limit_section(&input.diff_summary, 12000)),
            ("patch", limit_section(&input.diff_patch, 40000)),
            ("template", limit_section(template, 8000)),
            ("context", limit_section(context, 20000)),
            ("instructions", limit_section(instructions, 20000)),
        ],
    )
}
pub fn branch_prompt(input: &BranchInput) -> String {
    let (mode, instructions) = match &input.naming {
        None => (
            "default",
            input.policy.branch_instructions.as_deref().unwrap_or(""),
        ),
        Some(BranchNaming::Semantic) => (
            "semantic",
            input.policy.branch_instructions.as_deref().unwrap_or(""),
        ),
        Some(BranchNaming::Static { .. }) => (
            "static",
            input.policy.branch_instructions.as_deref().unwrap_or(""),
        ),
        Some(BranchNaming::Custom { instructions }) => ("custom", instructions.as_str()),
    };
    render(
        &format!(
            "branch_{mode}_{}_{}",
            u8::from(!instructions.trim().is_empty()),
            u8::from(!input.attachments.is_empty())
        ),
        &[
            ("message", limit_section(&input.message, 8000)),
            ("instructions", limit_section(instructions.trim(), 20000)),
            ("attachments", attachment_metadata(&input.attachments)),
        ],
    )
}
pub fn thread_title_prompt(input: &ThreadTitleInput) -> String {
    let project = nonempty(input.project_title.as_deref());
    let kind = if input.previous_title.is_some() {
        "regenerate"
    } else {
        "initial"
    };
    let mut prompt = render(
        &format!("title_{kind}_{}", u8::from(!project.is_empty())),
        &[
            ("project", quoted_contents(project)),
            (
                "previous",
                quoted_contents(input.previous_title.as_deref().unwrap_or("")),
            ),
        ],
    );
    if let Some(source) = &input.fork_source {
        prompt.push_str(&render(
            "title_fork",
            &[
                ("source_title", quoted_contents(&source.title)),
                ("history", limit_title_message(&source.history, 8000)),
            ],
        ));
    }
    let label = match (input.previous_title.is_some(), input.fork_source.is_some()) {
        (false, false) => "User message",
        (false, true) => "User message after the fork",
        (true, false) => "Thread contents",
        (true, true) => "Thread contents after the fork",
    };
    let message = if input.previous_title.is_some() {
        const MARKER: &str = "[Earlier content truncated]\n\n";
        if let Some(contents) = input.message.strip_prefix(MARKER) {
            format!("{MARKER}{}", tail(contents, 8000))
        } else if utf16_len(&input.message) > 8000 {
            format!("{MARKER}{}", tail(&input.message, 8000))
        } else {
            input.message.clone()
        }
    } else {
        limit_title_message(&input.message, 8000)
    };
    prompt.push_str(&format!("\n\n{label}:\n{message}"));
    let linked = input.linked_context.as_deref().unwrap_or("");
    let instructions = nonempty(input.instructions.as_deref());
    prompt.push_str(&render(
        &format!(
            "title_suffix_{}_{}_{}",
            u8::from(!linked.is_empty()),
            u8::from(!instructions.is_empty()),
            u8::from(!input.attachments.is_empty())
        ),
        &[
            ("linked", linked.to_owned()),
            ("instructions", limit_section(instructions, 20000)),
            ("attachments", attachment_metadata(&input.attachments)),
        ],
    ));
    prompt
}
pub fn review_prompt(input: &ReviewInput) -> String {
    let mut prompt = render(
        "review",
        &[
            ("number", input.number.to_string()),
            ("title", input.title.clone()),
            (
                "description",
                limit_section(
                    if input.description.trim().is_empty() {
                        "(none)"
                    } else {
                        input.description.trim()
                    },
                    8000,
                ),
            ),
            ("base", input.base_branch.clone()),
            ("commit", input.base_commit.clone()),
            (
                "settled",
                limit_section(
                    if input.settled_review.trim().is_empty() {
                        "(nothing)"
                    } else {
                        input.settled_review.trim()
                    },
                    8000,
                ),
            ),
        ],
    );
    if input.stack.len() >= 2 {
        let layers = input
            .stack
            .iter()
            .enumerate()
            .map(|(i, layer)| {
                let role = if layer.number == input.number {
                    " (this change)"
                } else {
                    ""
                };
                let location = if layer.number == input.number {
                    String::new()
                } else {
                    layer.head_sha.as_ref().map_or_else(
                        || ", head not available".into(),
                        |sha| format!(", head {sha}"),
                    )
                };
                let state = if layer.state == "open" {
                    String::new()
                } else {
                    format!(", {}", layer.state)
                };
                let title = nonempty(layer.title.as_deref());
                let title = if title.is_empty() {
                    "(untitled)"
                } else {
                    title
                };
                format!(
                    "{}. #{}{role}: {title}{state}{location}",
                    i + 1,
                    layer.number
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        prompt.push('\n');
        prompt.push_str(&render("review_stack_before", &[]));
        prompt.push_str(&layers);
        prompt.push_str(&render("review_stack_after", &[]));
    }
    let instructions = nonempty(input.instructions.as_deref());
    if !instructions.is_empty() {
        prompt.push('\n');
        prompt.push_str(&render(
            "review_preferences",
            &[("instructions", limit_section(instructions, 4000))],
        ));
    }
    prompt
}

pub fn voice_prompt(
    instructions: &str,
    transcript: &str,
    draft: &str,
    start: usize,
    end: usize,
    cue: &str,
) -> String {
    let before = head(draft, start);
    let through = head(draft, end.max(utf16_len(before)));
    let through = if through.len() < before.len() {
        before
    } else {
        through
    };
    render(
        &format!(
            "voice_{}_{}",
            u8::from(through.len() > before.len()),
            u8::from(!cue.trim().is_empty())
        ),
        &[
            ("instructions", instructions.to_owned()),
            ("transcript", transcript.to_owned()),
            ("before", before.to_owned()),
            ("selection", draft[before.len()..through.len()].to_owned()),
            ("after", draft[through.len()..].to_owned()),
            ("cue", quoted_contents(cue.trim())),
        ],
    )
}
pub fn default_voice_instructions() -> &'static str {
    &FIXTURES.templates["voice_default"]
}
