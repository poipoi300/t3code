use std::{cmp::Ordering, collections::HashSet};
use t3_contracts::{
    model_selection::ModelSelection, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};

pub fn run_ran_after(run: &OrchestrationV2Run, other: &OrchestrationV2Run) -> bool {
    let end = |run: &OrchestrationV2Run| {
        run.completed_at
            .as_ref()
            .and_then(|date| super::orchestrator::epoch_millis(&date.0))
            .unwrap_or(i64::MAX)
    };
    end(run)
        .cmp(&end(other))
        .then_with(|| run.ordinal.cmp(&other.ordinal))
        == Ordering::Greater
}

pub fn latest_executed_run(runs: &[OrchestrationV2Run]) -> Option<&OrchestrationV2Run> {
    runs.iter()
        .filter(|run| {
            run.status != OrchestrationV2RunStatus::Queued
                && !(run.status == OrchestrationV2RunStatus::Cancelled && run.started_at.is_none())
        })
        .fold(None, |latest, run| match latest {
            None => Some(run),
            Some(previous) if run_ran_after(run, previous) => Some(run),
            _ => latest,
        })
}

pub fn model_selections_equal(left: &ModelSelection, right: &ModelSelection) -> bool {
    if left.instance_id != right.instance_id || left.model != right.model {
        return false;
    }
    static COLLATOR: std::sync::OnceLock<icu_collator::CollatorBorrowed<'static>> =
        std::sync::OnceLock::new();
    let collator = COLLATOR.get_or_init(|| {
        // Root collation data is compiled into ICU; construction has no external input.
        icu_collator::Collator::try_new(Default::default(), Default::default())
            .expect("compiled ICU root collation")
    });
    let options = |selection: &ModelSelection| {
        let mut values: Vec<_> = selection
            .options
            .as_ref()
            .into_iter()
            .flat_map(|options| &options.0)
            .map(|option| {
                (
                    option.id.clone(),
                    match &option.value {
                        t3_contracts::model::ProviderOptionSelectionValue::String(value) => {
                            (false, value.clone())
                        }
                        t3_contracts::model::ProviderOptionSelectionValue::Boolean(value) => {
                            (true, value.to_string())
                        }
                    },
                )
            })
            .collect();
        values.sort_by(|(left_id, (_, left)), (right_id, (_, right))| {
            collator
                .compare(left_id, right_id)
                .then_with(|| collator.compare(left, right))
        });
        values
    };
    options(left) == options(right)
}

pub fn model_switch_since(
    runs: &[OrchestrationV2Run],
    items: &[OrchestrationV2TurnItem],
    to: &ModelSelection,
    baseline: Option<&ModelSelection>,
) -> Option<OrchestrationV2ModelSwitch> {
    let shown: HashSet<_> = items
        .iter()
        .filter_map(|item| {
            let OrchestrationV2TurnItem::UserMessage(item) = item else {
                return None;
            };
            let IdOrIndex::Id(id) = &item.message_id else {
                return None;
            };
            if (item.input_intent == OrchestrationV2UserMessageInputIntent::QueuedTurn
                && item.status == OrchestrationV2TurnItemStatus::Pending)
                || id.0.starts_with("async-answer:")
                || (t3_contracts::wire::trim_js(&item.text).eq_ignore_ascii_case("/compact")
                    && item.attachments.is_empty())
            {
                return None;
            }
            Some(id.0.as_str())
        })
        .collect();
    let previous = runs
        .iter()
        .filter(|run| {
            !matches!(
                run.status,
                OrchestrationV2RunStatus::RolledBack | OrchestrationV2RunStatus::Queued
            )
        })
        .filter(|run| match &run.user_message_id {
            IdOrIndex::Id(id) => shown.contains(id.0.as_str()) || run.context_handoff_id.is_some(),
            IdOrIndex::Index(_) => run.context_handoff_id.is_some(),
        })
        .fold(None, |latest, run| match latest {
            None => Some(run),
            Some(previous) if run_ran_after(run, previous) => Some(run),
            _ => latest,
        });
    let from = previous.map(|run| &run.model_selection).or(baseline)?;
    (!model_selections_equal(from, to)).then(|| OrchestrationV2ModelSwitch {
        from: from.clone(),
        to: to.clone(),
    })
}
