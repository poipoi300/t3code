use super::thread_management_service::ThreadManagementService;
use crate::{
    api::*,
    effects::TitleKind,
    error::{Error, Result},
    ports::runs_ingestion::provider_event_ingestor::expanded_id,
    runtime::threads::ThreadServiceScope,
    stores::{StoreDatabase, projects},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_contracts::{chat_attachment::ChatAttachment, model_selection::ModelSelection};
use tokio_util::sync::CancellationToken;

const OMITTED: &str = "[Earlier content truncated]\n\n";
const TRUNCATED: &str = "\n[Content truncated]\n";
const BOOTSTRAP: &str = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";

fn js_whitespace(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

fn trim_js(text: &str) -> &str {
    text.trim_matches(js_whitespace)
}

fn decode_url_component(text: &str, form: bool) -> Option<String> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let next = bytes.clone().take(2).collect::<Vec<_>>();
            let digit = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
            if next.len() == 2
                && let Some((high, low)) = digit(next[0]).zip(digit(next[1]))
            {
                decoded.push(high * 16 + low);
                bytes.next();
                bytes.next();
            } else if form {
                decoded.push(byte);
            } else {
                return None;
            }
        } else {
            decoded.push(if form && byte == b'+' { b' ' } else { byte });
        }
    }
    if form {
        Some(String::from_utf8_lossy(&decoded).into_owned())
    } else {
        String::from_utf8(decoded).ok()
    }
}

fn citation(href: &str) -> Option<t3_contracts::assistant_citations::AssistantCitation> {
    use t3_contracts::assistant_citations::*;
    const MAX_HREF: usize = 9
        * (ASSISTANT_CITATION_MAX_TEXT_LENGTH
            + ASSISTANT_CITATION_MAX_COMMENT_LENGTH
            + 3 * ASSISTANT_CITATION_MAX_ID_LENGTH
            + 2 * ASSISTANT_CITATION_CONTEXT_LENGTH)
        + 1000;
    if utf16_len(href) > MAX_HREF || href.contains('#') || href.chars().any(js_whitespace) {
        return None;
    }
    let (path, query) = href.strip_prefix("t3-citation://v1/")?.split_once('?')?;
    let ids = path
        .split('/')
        .map(|part| decode_url_component(part, false))
        .collect::<Option<Vec<_>>>()?;
    if ids.len() != 3
        || ids
            .iter()
            .any(|id| id.is_empty() || utf16_len(id) > ASSISTANT_CITATION_MAX_ID_LENGTH)
    {
        return None;
    }
    let mut params = BTreeMap::new();
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if params
            .insert(
                decode_url_component(key, true)?,
                decode_url_component(value, true)?,
            )
            .is_some()
        {
            return None;
        }
    }
    let comment = params.get("comment");
    if params.len() != 5 + usize::from(comment.is_some()) {
        return None;
    }
    let number = |key: &str| -> Option<u64> {
        let value = params.get(key)?;
        if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        value
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= 9_007_199_254_740_991)
    };
    let (start, end) = (number("start")?, number("end")?);
    let text = params.get("text")?;
    let prefix = params.get("prefix")?;
    let suffix = params.get("suffix")?;
    if end <= start
        || trim_js(text).is_empty()
        || utf16_len(text) > ASSISTANT_CITATION_MAX_TEXT_LENGTH
        || comment.is_some_and(|comment| utf16_len(comment) > ASSISTANT_CITATION_MAX_COMMENT_LENGTH)
        || utf16_len(prefix) > ASSISTANT_CITATION_CONTEXT_LENGTH
        || utf16_len(suffix) > ASSISTANT_CITATION_CONTEXT_LENGTH
    {
        return None;
    }
    let mut value = json!({"version":1,"environmentId":ids[0],"threadId":ids[1],"messageId":ids[2],"text":text,"start":start,"end":end,"prefix":prefix,"suffix":suffix});
    if let Some(comment) = comment {
        value["comment"] = json!(comment);
    }
    serde_json::from_value(value).ok()
}

fn citations_to_plain_text(text: &str) -> String {
    const OPEN: &str = "[Assistant quote](";
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find(OPEN) {
        let href = &remaining[start + OPEN.len()..];
        let Some(end) = href.find(')') else {
            break;
        };
        result.push_str(&remaining[..start]);
        match citation(&href[..end]) {
            Some(citation) => {
                result.push_str(&citation.text);
                if let Some(comment) = citation.comment.flatten() {
                    result.push_str("\nComment: ");
                    result.push_str(&comment);
                }
            }
            None => result.push_str(&remaining[start..start + OPEN.len() + end + 1]),
        }
        remaining = &href[end + 1..];
    }
    result.push_str(remaining);
    result
}

pub fn is_attachment_only_bootstrap_prompt(text: &str) -> bool {
    let text = if text
        .get(..11)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Ultrathink:"))
    {
        text[11..].trim_start_matches(js_whitespace)
    } else {
        text
    };
    text == BOOTSTRAP
}
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}
pub fn limit_title_message(text: &str, budget: usize) -> String {
    let text_units: Vec<_> = text.encode_utf16().collect();
    if text_units.len() <= budget {
        return text.into();
    }
    if budget <= TRUNCATED.len() {
        return String::new();
    }
    let available = budget - TRUNCATED.len();
    let mut head = available.div_ceil(2);
    let mut tail = available - head;
    if text_units
        .get(head.wrapping_sub(1))
        .is_some_and(|unit| (0xd800..=0xdbff).contains(unit))
    {
        head -= 1;
    }
    if tail > 0
        && text_units
            .get(text_units.len() - tail)
            .is_some_and(|unit| (0xdc00..=0xdfff).contains(unit))
    {
        tail -= 1;
    }
    format!(
        "{}{TRUNCATED}{}",
        String::from_utf16_lossy(&text_units[..head]),
        String::from_utf16_lossy(&text_units[text_units.len() - tail..])
    )
}

#[derive(Debug)]
pub struct TitleContext {
    pub message: String,
    pub attachments: Vec<ChatAttachment>,
}

pub fn format_thread_title_context(
    messages: &[OrchestrationV2ConversationMessage],
) -> Result<TitleContext> {
    struct Section<'a> {
        source: &'a OrchestrationV2ConversationMessage,
        user: bool,
        prefix: &'static str,
        content: String,
    }
    let mut sections = Vec::new();
    for message in messages {
        let role = json!(message.role);
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = if is_attachment_only_bootstrap_prompt(&message.text) {
            String::new()
        } else {
            citations_to_plain_text(&message.text)
        };
        let text = trim_js(&text);
        if text.is_empty() && message.attachments.is_empty() {
            continue;
        }
        let mut content = text.to_owned();
        if !message.attachments.is_empty() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&format!(
                "[Attachments: {}]",
                message
                    .attachments
                    .iter()
                    .map(|attachment| serde_json::to_value(attachment)
                        .map(|value| value["name"].as_str().unwrap_or_default().to_owned()))
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .join(", ")
            ));
        }
        let user = role == "user";
        sections.push(Section {
            source: message,
            user,
            prefix: if user { "USER:\n" } else { "ASSISTANT:\n" },
            content,
        });
    }
    let mut selected = BTreeMap::<usize, String>::new();
    let mut remaining = 8000 - OMITTED.len();
    let first = sections.iter().position(|section| section.user);
    let add = |index: usize,
               budget: usize,
               remaining: &mut usize,
               selected: &mut BTreeMap<usize, String>| {
        if selected.contains_key(&index) {
            return;
        }
        let section = &sections[index];
        let limit = budget
            .min(*remaining)
            .saturating_sub(section.prefix.len() + 2);
        if limit <= TRUNCATED.len() {
            return;
        }
        let content = limit_title_message(&section.content, limit);
        if content.is_empty() {
            return;
        }
        let text = format!("{}{content}", section.prefix);
        *remaining -= utf16_len(&text) + 2;
        selected.insert(index, text);
    };
    if let Some(index) = first {
        add(index, 2000, &mut remaining, &mut selected);
    }
    for index in (0..sections.len())
        .rev()
        .filter(|index| sections[*index].user)
    {
        add(
            index,
            2000.min(remaining.saturating_sub(2000)),
            &mut remaining,
            &mut selected,
        );
    }
    for index in (0..sections.len())
        .rev()
        .filter(|index| !sections[*index].user)
    {
        add(index, 2000, &mut remaining, &mut selected);
    }
    for user in [true, false] {
        for index in (0..sections.len())
            .rev()
            .filter(|index| sections[*index].user == user)
        {
            if let Some(previous) = selected.get(&index) {
                let section = &sections[index];
                let expanded = format!(
                    "{}{}",
                    section.prefix,
                    limit_title_message(
                        &section.content,
                        utf16_len(previous) + remaining - section.prefix.len()
                    )
                );
                remaining = remaining + utf16_len(previous) - utf16_len(&expanded);
                selected.insert(index, expanded);
            }
        }
    }
    let truncated = selected.len() < sections.len()
        || selected.iter().any(|(index, text)| {
            *text != format!("{}{}", sections[*index].prefix, sections[*index].content)
        });
    let retained_attachments: Vec<_> = selected
        .keys()
        .flat_map(|index| sections[*index].source.attachments.iter())
        .collect();
    let first_attachment = first.and_then(|index| sections[index].source.attachments.first());
    let mut recent: Vec<_> = retained_attachments
        .into_iter()
        .filter(|attachment| {
            first_attachment.is_none_or(|first| json!(first)["id"] != json!(attachment)["id"])
        })
        .cloned()
        .collect();
    let take = if first_attachment.is_some() { 3 } else { 4 };
    if recent.len() > take {
        recent.drain(..recent.len() - take);
    }
    let mut attachments = Vec::new();
    if let Some(first) = first_attachment {
        attachments.push(first.clone());
    }
    attachments.extend(recent);
    Ok(TitleContext {
        message: format!(
            "{}{}",
            if truncated { OMITTED } else { "" },
            selected.into_values().collect::<Vec<_>>().join("\n\n")
        ),
        attachments,
    })
}

pub struct ThreadTitleRegenerationService {
    pub management: ThreadManagementService,
    pub database: StoreDatabase,
    pub settings: Arc<dyn SettingsService>,
    pub generator: Arc<dyn TitleGenerator>,
    scope: ThreadServiceScope,
}
enum Outcome {
    Stale,
    Renamed(String),
    Unchanged,
    Failed(String),
}

impl ThreadTitleRegenerationService {
    pub fn new(
        management: ThreadManagementService,
        database: StoreDatabase,
        settings: Arc<dyn SettingsService>,
        generator: Arc<dyn TitleGenerator>,
    ) -> Self {
        Self {
            management,
            database,
            settings,
            generator,
            scope: ThreadServiceScope::default(),
        }
    }
    pub async fn shutdown(&self) {
        self.scope.shutdown().await;
    }
    pub async fn execute(
        &self,
        thread_id: ThreadId,
        request_id: CommandId,
        kind: TitleKind,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let scoped_cancellation = self.scope.cancellation();
        let _cancel_on_exit = scoped_cancellation.clone().drop_guard();
        self.scope.run(async {
            tokio::select! { biased;
                _ = cancellation.cancelled() => Err(Error::Cancelled),
                result = self.execute_inner(thread_id, request_id, kind, scoped_cancellation) => result,
            }
        }).await
    }
    async fn execute_inner(
        &self,
        thread_id: ThreadId,
        request_id: CommandId,
        kind: TitleKind,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let attempts = if matches!(kind, TitleKind::Initial { .. }) {
            3
        } else {
            1
        };
        let mut outcome = Outcome::Stale;
        for attempt in 0..attempts {
            let result = tokio::select! {
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                result = self.generate(thread_id.clone(), &request_id, &kind, cancellation.clone()) => result,
            };
            match result {
                Ok(result) => {
                    outcome = result;
                    break;
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) if attempt + 1 == attempts => {
                    outcome = Outcome::Failed(failure_message(&error))
                }
                Err(_) => {
                    tokio::select! { _ = cancellation.cancelled() => return Err(Error::Cancelled), _ = tokio::time::sleep(Duration::from_secs(2 << attempt)) => {} }
                }
            }
        }
        if matches!(outcome, Outcome::Stale) {
            return Ok(());
        }
        let mut command = json!({"type":"thread.title.regeneration.complete","commandId":format!("{}:title-complete",request_id.0),"threadId":thread_id,"requestId":request_id});
        if let Outcome::Renamed(title) = &outcome {
            command["title"] = json!(title);
        }
        if matches!(kind, TitleKind::Regenerate) {
            command["outcome"] = match outcome {
                Outcome::Renamed(_) => json!({"status":"renamed"}),
                Outcome::Unchanged => json!({"status":"unchanged"}),
                Outcome::Failed(message) => json!({"status":"failed","message":message}),
                Outcome::Stale => unreachable!(),
            };
        }
        self.management
            .dispatch(serde_json::from_value(command)?)
            .await?;
        Ok(())
    }
    async fn generate(
        &self,
        thread_id: ThreadId,
        request: &CommandId,
        kind: &TitleKind,
        cancellation: CancellationToken,
    ) -> Result<Outcome> {
        let source = self
            .management
            .query_records(ProjectionRecordQuery {
                thread_id: thread_id.clone(),
                fields: vec![RecordKind::Messages],
                filter: RecordFilter::default(),
                messages: match kind {
                    TitleKind::Initial { message_id } => MessageFilter {
                        ids: Some(vec![message_id.clone()]),
                        ..Default::default()
                    },
                    TitleKind::Regenerate => MessageFilter {
                        roles: Some(vec![
                            ConversationMessageRole::User,
                            ConversationMessageRole::Assistant,
                        ]),
                        ..Default::default()
                    },
                },
                run_statuses: None,
                include_latest_run: false,
            })
            .await?;
        let thread = serde_json::to_value(&source.thread)?;
        if thread["titleRegeneration"]["requestId"] != json!(request) {
            return Ok(Outcome::Stale);
        }
        let project_id = expanded_id(&source.thread.project_id)?;
        let project = self
            .database
            .read({
                let id = project_id.clone();
                move |db| projects::get(db, &id, false)
            })
            .await?;
        let Some(project) = project else {
            return Ok(Outcome::Failed(
                "This thread's project no longer exists.".into(),
            ));
        };
        let messages: Vec<_> = source
            .messages
            .into_iter()
            .filter(|message| !message.streaming)
            .collect();
        let context = match kind {
            TitleKind::Initial { message_id } => messages
                .iter()
                .find(|message| json!(message.id) == json!(message_id))
                .map(|message| TitleContext {
                    message: if is_attachment_only_bootstrap_prompt(&message.text) {
                        String::new()
                    } else {
                        message.text.clone()
                    },
                    attachments: message.attachments.clone(),
                })
                .unwrap_or(TitleContext {
                    message: String::new(),
                    attachments: vec![],
                }),
            TitleKind::Regenerate => format_thread_title_context(&messages)?,
        };
        if context.message.is_empty() && context.attachments.is_empty() {
            return Ok(Outcome::Failed(
                "This thread has no messages to title yet.".into(),
            ));
        }
        let fork_source = self.fork_source(&source.thread).await.unwrap_or(None);
        let settings = serde_json::to_value(self.settings.get().await?)?;
        let overrides = &settings["projectSettingsOverrides"][&project_id.0];
        let instructions = overrides
            .get("threadTitleInstructions")
            .unwrap_or(&settings["threadTitleInstructions"])
            .as_str()
            .unwrap_or_default()
            .into();
        let model_selection: ModelSelection = serde_json::from_value(
            overrides
                .get("textGenerationModelSelection")
                .filter(|value| !value.is_null())
                .unwrap_or(&settings["textGenerationModelSelection"])
                .clone(),
        )?;
        let worktree = source
            .thread
            .worktree_path
            .as_ref()
            .map(|path| PathBuf::from(&path.0));
        let cwd = match worktree {
            Some(path)
                if tokio::task::spawn_blocking({
                    let path = path.clone();
                    move || path.try_exists().unwrap_or(false)
                })
                .await
                .map_err(|error| Error::Service {
                    operation: "TitleRegeneration.cwd",
                    detail: error.to_string(),
                })? =>
            {
                path
            }
            _ => PathBuf::from(project.workspace_root),
        };
        let result = self
            .generator
            .generate_with_input(
                TitleGenerationInput {
                    thread_id,
                    project_id,
                    cwd,
                    project_title: project.title,
                    message: context.message,
                    attachments: context.attachments,
                    previous_title: matches!(kind, TitleKind::Regenerate)
                        .then(|| source.thread.title.0.clone()),
                    fork_source,
                    instructions,
                    model_selection,
                },
                cancellation,
            )
            .await?;
        if trim_js(&result) == "New thread"
            || (matches!(kind, TitleKind::Regenerate)
                && trim_js(&result) == trim_js(&source.thread.title.0))
        {
            Ok(Outcome::Unchanged)
        } else {
            Ok(Outcome::Renamed(result))
        }
    }
    async fn fork_source(
        &self,
        thread: &OrchestrationV2AppThread,
    ) -> Result<Option<ForkTitleContext>> {
        let mut forked = serde_json::to_value(&thread.forked_from)?;
        let mut title = None;
        let mut history = Vec::new();
        let mut visited = HashSet::new();
        while forked["type"] == "run" {
            let id: ThreadId = serde_json::from_value(forked["threadId"].clone())?;
            if !visited.insert(id.clone()) {
                return Err(Error::Service {
                    operation: "TitleRegeneration.forkSource",
                    detail: "Cyclic fork ancestry.".into(),
                });
            }
            let source = self
                .management
                .query_records(ProjectionRecordQuery {
                    thread_id: id,
                    fields: vec![RecordKind::Runs, RecordKind::Messages],
                    filter: RecordFilter::default(),
                    messages: MessageFilter {
                        roles: Some(vec![
                            ConversationMessageRole::User,
                            ConversationMessageRole::Assistant,
                        ]),
                        ..Default::default()
                    },
                    run_statuses: None,
                    include_latest_run: false,
                })
                .await?;
            let ordinals: BTreeMap<_, _> = source
                .runs
                .iter()
                .filter(|run| run.status != OrchestrationV2RunStatus::RolledBack)
                .map(|run| (json!(run.id).to_string(), run.ordinal))
                .collect();
            let fork_ordinal = ordinals.get(&forked["runId"].to_string()).copied();
            let imported = json!(source.thread.history_origin) == "v1_import";
            let messages: Vec<_> = source
                .messages
                .into_iter()
                .filter(|message| {
                    !message.streaming
                        && if let Some(run) = &message.run_id {
                            ordinals
                                .get(&json!(run).to_string())
                                .zip(fork_ordinal)
                                .is_some_and(|(ordinal, fork)| *ordinal <= fork)
                        } else {
                            imported
                        }
                })
                .collect();
            history.push(messages);
            title.get_or_insert(source.thread.title.0);
            forked = serde_json::to_value(source.thread.forked_from)?;
        }
        let messages: Vec<_> = history.into_iter().rev().flatten().collect();
        let context = format_thread_title_context(&messages)?;
        Ok(title
            .filter(|_| !context.message.is_empty())
            .map(|title| ForkTitleContext {
                title,
                history: context.message,
            }))
    }
}

pub fn failure_message(error: &Error) -> String {
    let detail = match error {
        Error::Service {
            operation: "TextGeneration.generateThreadTitle",
            detail,
        } => detail.lines().map(trim_js).rfind(|line| !line.is_empty()),
        _ => None,
    };
    let Some(detail) = detail else {
        return "Title generation failed.".into();
    };
    if utf16_len(detail) <= 300 {
        return detail.into();
    }
    let units: Vec<_> = detail.encode_utf16().take(299).collect();
    format!("{}…", String::from_utf16_lossy(&units))
}
