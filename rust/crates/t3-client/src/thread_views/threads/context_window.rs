use t3_contracts::{
    model_selection::ModelSelection,
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    provider_runtime::ThreadTokenUsageSnapshot,
    server::ServerProvider,
    settings::{
        ClaudeSettings, IdleCompactionMode, InstanceIdleCompaction, LegacyCompactIdleThreads,
        ServerSettings,
    },
};
#[derive(Debug, Clone, PartialEq)]
pub struct ContextWindowSnapshot {
    pub usage: ThreadTokenUsageSnapshot,
    pub remaining_tokens: Option<u64>,
    pub used_percentage: Option<f64>,
    pub remaining_percentage: Option<f64>,
    pub updated_at: String,
}
pub fn latest_provider_turn_token_usage<'a>(
    turns: impl IntoIterator<Item = &'a OrchestrationV2ProviderTurn>,
    projection: Option<&'a OrchestrationV2ThreadProjection>,
    provider: Option<&OrchestrationV2ProviderThread>,
    parse: impl Fn(&str) -> Option<i64>,
) -> Option<&'a OrchestrationV2ProviderTurnTokenUsage> {
    let mut latest: Option<&OrchestrationV2ProviderTurnTokenUsage> = None;
    for usage in turns
        .into_iter()
        .filter_map(|t| t.token_usage.as_ref().and_then(Option::as_ref))
    {
        if latest.is_none_or(|last| {
            parse(&usage.updated_at)
                .zip(parse(&last.updated_at))
                .is_some_and(|(a, b)| a >= b)
        }) {
            latest = Some(usage);
        }
    }
    if latest.is_some()
        || provider.is_some_and(|p| p.context_usage.as_ref().is_some_and(Option::is_some))
    {
        return latest;
    }
    let p = projection?;
    let Some(AppThreadForkedFrom::Run(source)) = &p.thread.forked_from else {
        return None;
    };
    p.context_transfers
        .iter()
        .find(|t| {
            t.r#type == OrchestrationV2ContextTransferType::Fork
                && t.target_thread_id == p.thread.id
                && t.source_thread_id == IdOrIndex::Id(source.thread_id.clone())
                && t.source_point.run_id.as_ref().and_then(Option::as_ref) == Some(&source.run_id)
                && t.source_provider_instance_id.as_ref()
                    == Some(&IdOrIndex::Id(p.thread.model_selection.instance_id.clone()))
                && (t.status == ContextTransferStatus::Pending
                    || matches!(
                        t.resolution,
                        Some(OrchestrationV2ContextTransferResolution::NativeFork(_))
                    ))
        })
        .and_then(|t| t.source_token_usage.as_ref().and_then(Option::as_ref))
}
fn empty_usage(used_tokens: u64) -> ThreadTokenUsageSnapshot {
    ThreadTokenUsageSnapshot {
        used_tokens,
        total_processed_tokens: None,
        max_tokens: None,
        input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        reasoning_output_tokens: None,
        last_used_tokens: None,
        last_input_tokens: None,
        last_cached_input_tokens: None,
        last_output_tokens: None,
        last_reasoning_output_tokens: None,
        tool_uses: None,
        duration_ms: None,
        compacts_automatically: Some(Some(true)),
        auto_compact_threshold: None,
        cost: None,
    }
}
pub fn derive_latest_context_window_snapshot(
    rows: &[OrchestrationV2ProjectedTurnItem],
    live: Option<&OrchestrationV2ProviderTurnTokenUsage>,
    provider: Option<&OrchestrationV2ProviderThread>,
) -> Option<ContextWindowSnapshot> {
    let (usage, updated_at) = if let Some(live) = live {
        let mut usage = empty_usage(live.used_tokens);
        usage.max_tokens = live.max_tokens;
        usage.input_tokens = live.input_tokens;
        usage.cached_input_tokens = live.cached_input_tokens;
        usage.output_tokens = live.output_tokens;
        usage.reasoning_output_tokens = live.reasoning_output_tokens;
        usage.auto_compact_threshold = live.auto_compact_threshold;
        (usage, live.updated_at.clone())
    } else if let Some((usage, updated_at)) = provider.and_then(|p| {
        p.context_usage
            .as_ref()?
            .as_ref()
            .map(|u| (u, &p.updated_at))
    }) {
        (usage.clone(), updated_at.0.clone())
    } else {
        let item = rows.iter().rev().find_map(|row| match &row.item {
            OrchestrationV2TurnItem::Compaction(i) if i.after_token_count.flatten().is_some() => {
                Some(i)
            }
            _ => None,
        })?;
        let mut usage = empty_usage(item.after_token_count.flatten()?);
        usage.total_processed_tokens = item.before_token_count;
        (
            usage,
            item.started_at
                .as_ref()
                .unwrap_or(&item.updated_at)
                .0
                .clone(),
        )
    };
    let max = usage.max_tokens.flatten();
    let used_percentage = max
        .filter(|max| *max > 0)
        .map(|max| (usage.used_tokens as f64 / max as f64 * 100.0).min(100.0));
    Some(ContextWindowSnapshot {
        remaining_tokens: max.map(|max| max.saturating_sub(usage.used_tokens)),
        remaining_percentage: used_percentage.map(|p| (100.0 - p).max(0.0)),
        used_percentage,
        usage,
        updated_at,
    })
}
fn is_resume_question(text: &str) -> bool {
    let Some(text) = text
        .strip_prefix("This session is ")
        .and_then(|s| s.strip_suffix(" tokens. Compact it before continuing?"))
    else {
        return false;
    };
    let Some((age, tokens)) = text.split_once(" old and uses ") else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let age = age.strip_suffix('m').is_some_and(|s| {
        s.split_once("h ")
            .map_or_else(|| digits(s), |(h, m)| digits(h) && digits(m))
    });
    let mut groups = tokens.split(',');
    let first = groups.next().unwrap_or_default();
    age && digits(first) && first.len() <= 3 && groups.all(|g| g.len() == 3 && digits(g))
}
pub fn has_dismissed_resume_compaction(requests: &[OrchestrationV2RuntimeRequest]) -> bool {
    requests.iter().filter(|r| r.status==RuntimeRequestStatus::Resolved && matches!(&r.kind,RuntimeRequestKind::Member2(kind) if **kind==RuntimeRequestKindMember2::UserInput)).filter_map(|r| r.answers.as_ref().and_then(Option::as_ref)).any(|answers| answers.iter().any(|(question,answer)| is_resume_question(question) && *answer==serde_json::Value::String("Don't ask again".into())))
}
pub fn should_offer_resume_compaction(
    provider: &str,
    tokens: Option<u64>,
    updated_at: Option<&str>,
    ttl: Option<u64>,
    now: &str,
    parse: impl Fn(&str) -> Option<i64>,
) -> bool {
    provider == "claudeAgent"
        && tokens.unwrap_or(0) >= 200_000
        && updated_at
            .and_then(&parse)
            .zip(parse(now))
            .is_some_and(|(at, now)| {
                i128::from(now) - i128::from(at) >= i128::from(ttl.unwrap_or(70 * 60_000))
            })
}
pub struct CompactBeforeSendInput<'a> {
    pub projection: &'a OrchestrationV2ThreadProjection,
    pub selection: &'a ModelSelection,
    pub providers: &'a [ServerProvider],
    pub settings: &'a ServerSettings,
    pub text: &'a str,
    pub thread_idle: bool,
    pub now: &'a str,
}
pub fn compact_before_send_tokens(
    input: CompactBeforeSendInput<'_>,
    parse: impl Fn(&str) -> Option<i64>,
) -> Option<u64> {
    let p = input.projection;
    let selected = input
        .providers
        .iter()
        .find(|provider| provider.instance_id == input.selection.instance_id)?;
    let typed_compact = input
        .text
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("/compact"))
        && input.text.get(8..).is_some_and(|s| {
            s.is_empty()
                || s.chars()
                    .next()
                    .is_some_and(crate::view_logic::is_js_whitespace)
        });
    let override_mode = input
        .settings
        .provider_instances
        .get(&selected.instance_id)
        .and_then(|s| s.config.clone())
        .and_then(|v| serde_json::from_value::<ClaudeSettings>(v).ok())
        .map(|c| c.idle_compaction);
    let off = match override_mode {
        Some(InstanceIdleCompaction::Off) => true,
        Some(InstanceIdleCompaction::OnReturn | InstanceIdleCompaction::BeforeCacheExpires) => {
            false
        }
        _ => input.settings.idle_compaction.as_ref().map_or_else(
            || match &input.settings.compact_idle_threads {
                LegacyCompactIdleThreads::Member1(_) => false,
                LegacyCompactIdleThreads::IdleCompactionMode(mode) => {
                    **mode == IdleCompactionMode::Off
                }
            },
            |m| *m == IdleCompactionMode::Off,
        ),
    };
    if !input.thread_idle
        || typed_compact
        || off
        || !selected
            .slash_commands
            .iter()
            .any(|c| c.name.0 == "compact")
        || p.runtime_requests
            .iter()
            .any(|r| r.status == RuntimeRequestStatus::Pending)
        || p.runs.iter().any(|r| {
            r.status == OrchestrationV2RunStatus::Queued && r.queue_held == Some(Some(true))
        })
        || has_dismissed_resume_compaction(&p.runtime_requests)
    {
        return None;
    }
    let from = p.runs.iter().rfind(|r| {
        r.status != OrchestrationV2RunStatus::RolledBack
            && (r.started_at.is_some()
                || (r.status == OrchestrationV2RunStatus::Starting
                    && r.context_handoff_id.is_some()))
    });
    if !p.visible_turn_items.is_empty()
        && let Some(from) = from
        && from.model_selection.instance_id != input.selection.instance_id
    {
        let source = input
            .providers
            .iter()
            .find(|s| s.instance_id == from.model_selection.instance_id);
        let source_group = source.and_then(|s| s.continuation.as_ref()?.as_ref());
        let target_group = selected.continuation.as_ref().and_then(Option::as_ref);
        if source_group.is_none()
            || source.is_none_or(|s| s.driver != selected.driver)
            || source_group != target_group
        {
            return None;
        }
    }
    let has_prompt=p.visible_turn_items.iter().any(|r| matches!(&r.item,OrchestrationV2TurnItem::UserMessage(i) if !t3_contracts::wire::trim_js(&i.text).eq_ignore_ascii_case("/compact") || !i.attachments.is_empty())) || p.runs.iter().any(|r| r.status!=OrchestrationV2RunStatus::RolledBack && r.started_at.is_some() && !p.visible_turn_items.iter().any(|row| matches!(&row.item,OrchestrationV2TurnItem::UserMessage(i) if i.run_id.as_ref()==Some(&r.id))));
    if !has_prompt {
        return None;
    }
    let provider = p
        .provider_threads
        .iter()
        .find(|t| Some(&t.id) == p.thread.active_provider_thread_id.as_ref());
    let live = latest_provider_turn_token_usage(
        p.provider_turns
            .iter()
            .filter(|t| Some(&t.provider_thread_id) == p.thread.active_provider_thread_id.as_ref()),
        (input.selection.instance_id == p.thread.model_selection.instance_id).then_some(p),
        provider,
        &parse,
    );
    let snapshot = derive_latest_context_window_snapshot(&p.visible_turn_items, live, provider)?;
    should_offer_resume_compaction(
        &selected.driver.0,
        Some(snapshot.usage.used_tokens),
        Some(&snapshot.updated_at),
        live.and_then(|u| u.prompt_cache_ttl_ms.flatten()),
        input.now,
        parse,
    )
    .then_some(snapshot.usage.used_tokens)
}
