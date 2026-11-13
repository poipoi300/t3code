use super::message_search_index::{
    indexed_message_condition, message_search_text, non_placeholder_message_condition,
};
use crate::{
    api::ShellQuery,
    error::{Error, Result},
    stores::StoreDatabase,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use t3_contracts::{base_schemas::*, orchestration_v2::*, thread_search::*};
use t3_persistence::rusqlite::{Connection, params};

pub struct MessageSearchRequest {
    pub match_query: Option<String>,
    pub substrings: Vec<String>,
    pub short_spellings: Vec<Vec<String>>,
}
#[derive(Clone)]
pub struct MessageHit {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub message_id: MessageId,
    pub source: OrchestrationThreadSearchSource,
    pub text: String,
    pub message_created_at: Option<IsoDateTime>,
    pub quality: f64,
    pub offset: usize,
    pub hits: u64,
    pub opens: Option<Value>,
    pub activity_at: Option<IsoDateTime>,
}
#[derive(Clone)]
pub struct TitleCandidate {
    pub thread: Value,
    pub project_title: String,
    pub latest_user_message_at: Option<IsoDateTime>,
}
/// Local shared/paletteSearch shim: normalization, word matching and ranking stay authoritative.
pub trait ThreadSearchPolicy: Send + Sync {
    fn message_search(&self, query: &str, has_index: bool) -> Option<MessageSearchRequest>;
    fn shown_message(&self, text: &str) -> String;
    fn match_message(&self, text: &str, query: &str) -> Option<(f64, usize)>;
    fn rank_messages(
        &self,
        hits: &[MessageHit],
        query: &str,
        now_ms: i64,
        picks: bool,
    ) -> Result<Vec<usize>>;
    fn rank_titles(
        &self,
        rows: &[TitleCandidate],
        query: &str,
        environment_label: Option<&str>,
        now_ms: i64,
        picks: bool,
    ) -> Result<Vec<usize>>;
}
#[derive(Clone)]
pub struct ThreadSearch {
    database: StoreDatabase,
    policy: Arc<dyn ThreadSearchPolicy>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSearchHit {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message_created_at: Option<Option<IsoDateTime>>,
}
#[derive(Debug, Serialize)]
pub struct AgentSearchResult {
    pub matches: Vec<ThreadSearchHit>,
}
impl ThreadSearch {
    pub fn new(database: StoreDatabase, policy: Arc<dyn ThreadSearchPolicy>) -> Self {
        Self { database, policy }
    }
    async fn messages(
        &self,
        query: String,
        project: Option<ProjectId>,
        excluded: BTreeSet<ThreadId>,
        limit: usize,
        picks: bool,
    ) -> Result<Vec<MessageHit>> {
        let policy = self.policy.clone();
        let request_query = query.clone();
        let mut hits=self.database.read(move |db| {
            let has_index:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='orchestration_v2_message_search')",[],|row|row.get(0))?;
            let Some(request)=policy.message_search(&request_query,has_index) else {return Ok(vec![]);};
            message_hits(db,&request,project.as_ref(),&excluded)
        }).await?;
        let mut best: Vec<MessageHit> = Vec::new();
        let mut positions: BTreeMap<ThreadId, usize> = BTreeMap::new();
        for mut hit in hits.drain(..) {
            hit.text = self.policy.shown_message(&hit.text);
            let Some((quality, offset)) = self.policy.match_message(&hit.text, &query) else {
                continue;
            };
            if !quality.is_finite() {
                return Err(search_error("Non-finite message quality"));
            }
            hit.quality = quality;
            hit.offset = offset;
            if let Some(index) = positions.get(&hit.thread_id).copied() {
                let current = &mut best[index];
                let count = if quality > current.quality {
                    1
                } else if quality == current.quality {
                    current.hits + 1
                } else {
                    current.hits
                };
                let better = quality > current.quality
                    || (quality == current.quality
                        && (hit.source == OrchestrationThreadSearchSource::User
                            && current.source != OrchestrationThreadSearchSource::User
                            || hit.source == current.source
                                && hit.message_created_at.as_ref().map(|d| &d.0)
                                    > current.message_created_at.as_ref().map(|d| &d.0)));
                if better {
                    *current = hit;
                }
                current.hits = count;
            } else {
                hit.hits = 1;
                positions.insert(hit.thread_id.clone(), best.len());
                best.push(hit);
            }
        }
        let ids: Vec<_> = best.iter().map(|hit| hit.thread_id.clone()).collect();
        let opens=self.database.read(move |db| {
            let mut statement=db.prepare(&format!("SELECT threads.thread_id,json_extract(threads.payload_json,'$.opens'),{} FROM orchestration_v2_projection_threads AS threads WHERE threads.thread_id IN(SELECT value FROM json_each(?1))",latest_user_message()))?;
            let rows=statement.query_map([serde_json::to_string(&ids)?],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(rows)
        }).await?;
        for (id, opens, activity) in opens {
            if let Some(hit) = best.iter_mut().find(|hit| hit.thread_id.0 == id) {
                hit.opens = opens.and_then(|text| serde_json::from_str(&text).ok());
                hit.activity_at = activity.map(t3_contracts::wire::WireDateTime);
            }
        }
        let order =
            self.policy
                .rank_messages(&best, &query, super::shell_stream::now_ms(), picks)?;
        validate_order(&order, best.len())?;
        Ok(order
            .into_iter()
            .take(limit)
            .map(|index| best[index].clone())
            .collect())
    }
    pub async fn search(
        &self,
        input: OrchestrationSearchThreadsInput,
    ) -> Result<OrchestrationSearchThreadsResult> {
        let hits = self
            .messages(
                input.query,
                None,
                BTreeSet::new(),
                usize::from(input.limit.unwrap_or(50)),
                true,
            )
            .await?;
        Ok(OrchestrationSearchThreadsResult {
            matches: hits.into_iter().map(message_match).collect::<Result<_>>()?,
        })
    }
    pub async fn search_threads(
        &self,
        input: OrchestrationSearchThreadsInput,
        project_id: Option<ProjectId>,
    ) -> Result<AgentSearchResult> {
        let limit = usize::from(input.limit.unwrap_or(50));
        let candidates = self.candidates(project_id.clone(), None, None).await?;
        let order = self.policy.rank_titles(
            &candidates,
            &input.query,
            None,
            super::shell_stream::now_ms(),
            false,
        )?;
        validate_order(&order, candidates.len())?;
        let titles = order
            .into_iter()
            .take(limit)
            .map(|index| title_hit(&candidates[index]))
            .collect::<Result<Vec<_>>>()?;
        let shown = titles.iter().map(|hit| hit.thread_id.clone()).collect();
        let messages = self
            .messages(input.query, project_id, shown, 50, false)
            .await?;
        let by_id: BTreeMap<_, _> = candidates
            .iter()
            .filter_map(|row| row.thread["id"].as_str().map(|id| (id.to_owned(), row)))
            .collect();
        let messages = messages
            .into_iter()
            .filter_map(|hit| {
                by_id.get(&hit.thread_id.0).map(|row| ThreadSearchHit {
                    thread_id: hit.thread_id.clone(),
                    project_id: hit.project_id.clone(),
                    title: row.thread["title"].as_str().unwrap_or("").into(),
                    source: if hit.source == OrchestrationThreadSearchSource::User {
                        "user"
                    } else {
                        "assistant"
                    }
                    .into(),
                    snippet: Some(build_search_snippet(&hit.text, hit.offset)),
                    message_created_at: Some(hit.message_created_at.clone()),
                })
            })
            .collect::<Vec<_>>();
        let slots = messages.len().min(limit / 5);
        Ok(AgentSearchResult {
            matches: titles
                .into_iter()
                .take(limit - slots)
                .chain(messages)
                .take(limit)
                .collect(),
        })
    }
    async fn candidates(
        &self,
        project: Option<ProjectId>,
        before: Option<String>,
        projects: Option<Vec<ProjectId>>,
    ) -> Result<Vec<TitleCandidate>> {
        self.database.read(move |db| {
            let sql=format!("SELECT threads.payload_json,projects.title,{} FROM orchestration_v2_projection_threads AS threads JOIN projection_projects AS projects ON projects.project_id=threads.project_id WHERE threads.deleted_at IS NULL AND threads.archived_at IS NULL AND projects.deleted_at IS NULL AND json_extract(threads.payload_json,'$.lineage.relationshipToParent') IS NOT 'subagent' AND json_extract(threads.payload_json,'$.lineage.sideChat') IS NOT 1 AND (?1 IS NULL OR threads.project_id=?1) AND (?2 IS NULL OR (json_extract(threads.payload_json,'$.settledOverride')='settled' AND json_extract(threads.payload_json,'$.settledAt')<?2)) AND (?3 IS NULL OR threads.project_id IN(SELECT value FROM json_each(?3))) ORDER BY threads.rowid",latest_user_message());
            let mut statement=db.prepare(&sql)?;
            let rows=statement.query_map(params![project.map(|id|id.0),before,projects.map(|ids|serde_json::to_string(&ids)).transpose()?],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<String>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            rows.into_iter().map(|(thread,project_title,date)|Ok(TitleCandidate {thread:crate::stores::event_store::parse_json(&thread)?,project_title,latest_user_message_at:date.map(t3_contracts::wire::WireDateTime)})).collect()
        }).await
    }
    pub async fn search_older_settled(
        &self,
        query: String,
        before: String,
        projects: Option<Vec<ProjectId>>,
        environment: Option<String>,
        message_ids: Vec<ThreadId>,
    ) -> Result<Vec<ThreadId>> {
        let rows = self
            .candidates(None, Some(before.clone()), projects.clone())
            .await?;
        let order = self.policy.rank_titles(
            &rows,
            &query,
            environment.as_deref(),
            super::shell_stream::now_ms(),
            true,
        )?;
        validate_order(&order, rows.len())?;
        let mut ids = order
            .into_iter()
            .take(15)
            .map(|index| title_hit(&rows[index]).map(|hit| hit.thread_id))
            .collect::<Result<Vec<_>>>()?;
        let mut seen: BTreeSet<_> = ids.iter().cloned().collect();
        let eligible: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| row.thread["id"].as_str().map(|id| ThreadId(id.into())))
            .collect();
        ids.extend(
            message_ids
                .into_iter()
                .filter(|id| eligible.contains(id) && seen.insert(id.clone()))
                .take(5),
        );
        Ok(ids)
    }
    pub async fn search_with_older_settled_rows(
        &self,
        input: OrchestrationV2SearchThreadsInput,
    ) -> Result<OrchestrationV2SearchThreadsResult> {
        let messages = if input.titles_only == Some(true) {
            vec![]
        } else {
            self.messages(
                input.query.0.clone(),
                None,
                BTreeSet::new(),
                input.limit.unwrap_or(50).max(0) as usize,
                true,
            )
            .await?
            .into_iter()
            .map(message_match)
            .collect::<Result<Vec<_>>>()?
        };
        let Some(before) = input.older_settled_before else {
            return Ok(OrchestrationV2SearchThreadsResult {
                matches: messages,
                snapshot_sequence: None,
                threads: None,
            });
        };
        let ids = self
            .search_older_settled(
                input.query.0,
                before.0,
                input.project_ids,
                input.environment_label,
                messages.iter().map(|hit| hit.thread_id.clone()).collect(),
            )
            .await?;
        let order: BTreeMap<_, _> = ids
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        let (raw, sequence) = self
            .database
            .read(move |db| {
                Ok((
                    crate::projections::read::read_shell_rows(
                        db,
                        &ShellQuery {
                            thread_ids: Some(ids),
                            ..Default::default()
                        },
                        true,
                    )?,
                    crate::stores::event_store::EventStore::new().latest_application_sequence(db)?
                        as u64,
                ))
            })
            .await?;
        let mut shells = crate::projections::read::decode_shell_rows(raw)?;
        shells
            .threads
            .sort_by_key(|thread| order.get(&thread.id).copied().unwrap_or(usize::MAX));
        Ok(OrchestrationV2SearchThreadsResult {
            matches: messages,
            snapshot_sequence: Some(sequence),
            threads: Some(shells.threads),
        })
    }
}
fn search_error(detail: &str) -> Error {
    Error::Service {
        operation: "ThreadSearch",
        detail: detail.into(),
    }
}
fn validate_order(order: &[usize], len: usize) -> Result<()> {
    let mut seen = BTreeSet::new();
    if order
        .iter()
        .any(|index| *index >= len || !seen.insert(*index))
    {
        return Err(search_error("Invalid ranking indices"));
    }
    Ok(())
}
fn latest_user_message() -> &'static str {
    "(SELECT messages.updated_at FROM orchestration_v2_projection_messages AS messages WHERE messages.thread_id=threads.thread_id AND messages.role='user' AND COALESCE(json_extract(messages.payload_json,'$.createdBy'),'user')='user' ORDER BY messages.updated_at DESC,messages.message_id DESC LIMIT 1)"
}
fn title_hit(row: &TitleCandidate) -> Result<ThreadSearchHit> {
    Ok(ThreadSearchHit {
        thread_id: serde_json::from_value(row.thread["id"].clone())?,
        project_id: serde_json::from_value(row.thread["projectId"].clone())?,
        title: serde_json::from_value(row.thread["title"].clone())?,
        source: "title".into(),
        snippet: None,
        message_created_at: None,
    })
}
fn message_match(hit: MessageHit) -> Result<OrchestrationThreadSearchMatch> {
    let snippet = build_search_snippet(&hit.text, hit.offset);
    Ok(OrchestrationThreadSearchMatch {
        thread_id: hit.thread_id,
        project_id: hit.project_id,
        source: hit.source,
        snippet,
        message_created_at: hit.message_created_at,
        quality: Some(serde_json::from_value(serde_json::json!(hit.quality))?),
        hits: Some(serde_json::from_value(serde_json::json!(hit.hits))?),
        message_id: Some(hit.message_id),
    })
}
pub fn build_search_snippet(text: &str, match_start: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let match_start = match_start.min(units.len());
    let mut start = match_start.saturating_sub(24);
    if let Some(offset) = units[start..match_start].iter().position(|unit| {
        char::from_u32(u32::from(*unit))
            .is_some_and(|c| t3_contracts::wire::trim_js(&c.to_string()).is_empty())
    }) && start > 0
    {
        start += offset + 1;
    }
    if units
        .get(start)
        .is_some_and(|unit| (0xdc00..=0xdfff).contains(unit))
    {
        start += 1;
    }
    let raw_end = units.len().min(start + 480);
    let raw = String::from_utf16_lossy(&units[start..raw_end]);
    let marked = raw.replace("**", "").replace('`', "");
    let shown = marked
        .split(|c: char| t3_contracts::wire::trim_js(&c.to_string()).is_empty())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let shown_units: Vec<u16> = shown.encode_utf16().collect();
    let mut end = shown_units.len().min(238);
    if shown_units
        .get(end)
        .is_some_and(|unit| (0xdc00..=0xdfff).contains(unit))
    {
        end -= 1;
    }
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        String::from_utf16_lossy(&shown_units[..end]),
        if end < shown_units.len() || raw_end < units.len() {
            "…"
        } else {
            ""
        }
    )
}
fn message_hits(
    db: &Connection,
    request: &MessageSearchRequest,
    project: Option<&ProjectId>,
    excluded: &BTreeSet<ThreadId>,
) -> Result<Vec<MessageHit>> {
    let text = message_search_text("messages");
    let source = if request.match_query.is_some() {
        format!(
            "FROM orchestration_v2_message_search JOIN orchestration_v2_projection_messages AS messages ON messages.rowid=orchestration_v2_message_search.rowid JOIN eligible ON eligible.thread_id=messages.thread_id WHERE orchestration_v2_message_search MATCH ?1 AND orchestration_v2_message_search.rowid>=COALESCE((SELECT rowid FROM orchestration_v2_message_search WHERE orchestration_v2_message_search MATCH ?1 ORDER BY rowid DESC LIMIT 1 OFFSET 9999),0) AND {}",
            non_placeholder_message_condition("messages")
        )
    } else {
        format!(
            "FROM orchestration_v2_projection_messages AS messages JOIN eligible ON eligible.thread_id=messages.thread_id WHERE ?1 IS NULL AND {} AND messages.rowid>(SELECT COALESCE(MAX(rowid),0) FROM orchestration_v2_projection_messages)-20000",
            indexed_message_condition("messages")
        )
    };
    let rowid = if request.match_query.is_some() {
        "orchestration_v2_message_search.rowid"
    } else {
        "messages.rowid"
    };
    let sql = format!(
        "WITH eligible AS MATERIALIZED(SELECT threads.thread_id,threads.project_id FROM orchestration_v2_projection_threads AS threads JOIN projection_projects AS projects ON projects.project_id=threads.project_id WHERE threads.deleted_at IS NULL AND threads.archived_at IS NULL AND projects.deleted_at IS NULL AND (?4 IS NULL OR threads.project_id=?4) AND threads.thread_id NOT IN(SELECT value FROM json_each(?5)) AND json_extract(threads.payload_json,'$.lineage.relationshipToParent') IS NOT 'subagent' AND json_extract(threads.payload_json,'$.lineage.sideChat') IS NOT 1) SELECT messages.message_id,messages.thread_id,eligible.project_id,messages.role,{text},messages.created_at,{rowid} AS messageRowid {source} AND messages.message_id NOT LIKE 'async-answer:%' AND NOT EXISTS(SELECT 1 FROM json_each(?2) AS token WHERE instr(lower({text}),token.value)=0) AND NOT EXISTS(SELECT 1 FROM json_each(?3) AS token WHERE NOT EXISTS(SELECT 1 FROM json_each(token.value) AS spelling WHERE instr({text},spelling.value)>0)) ORDER BY messageRowid DESC LIMIT 2000"
    );
    let mut statement = db.prepare(&sql)?;
    let rows = statement
        .query_map(
            params![
                request.match_query,
                serde_json::to_string(&request.substrings)?,
                serde_json::to_string(&request.short_spellings)?,
                project.map(|id| &id.0),
                serde_json::to_string(excluded)?
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(message, thread, project, source, text, date)| {
            Ok(MessageHit {
                message_id: MessageId(message),
                thread_id: ThreadId(thread),
                project_id: ProjectId(project),
                source: serde_json::from_value(source.into())?,
                text,
                message_created_at: date.map(t3_contracts::wire::WireDateTime),
                quality: 0.0,
                offset: 0,
                hits: 0,
                opens: None,
                activity_at: None,
            })
        })
        .collect()
}
