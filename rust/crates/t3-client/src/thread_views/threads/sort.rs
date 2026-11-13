use crate::types::{ProjectId, ThreadKey};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    CreatedAt,
    LatestUserMessage,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadSortInput<'a> {
    pub created_at: &'a str,
    pub updated_at: &'a str,
    pub latest_user_message_at: Option<&'a str>,
    pub latest_user_authored_message_at: Option<Option<&'a str>>,
    pub messages: &'a [(&'a str, &'a str)],
}

pub fn get_thread_sort_timestamp(
    input: &ThreadSortInput<'_>,
    order: SortOrder,
    parse: &impl Fn(&str) -> Option<i64>,
) -> Option<i64> {
    if order == SortOrder::CreatedAt {
        return parse(input.created_at).or_else(|| parse(input.updated_at));
    }
    if let Some(authored) = input.latest_user_authored_message_at {
        return authored
            .and_then(parse)
            .or_else(|| parse(input.created_at))
            .or_else(|| parse(input.updated_at));
    }
    input
        .latest_user_message_at
        .and_then(parse)
        .or_else(|| {
            input
                .messages
                .iter()
                .filter(|(role, _)| *role == "user")
                .filter_map(|(_, date)| parse(date))
                .max()
        })
        .or_else(|| parse(input.updated_at))
        .or_else(|| parse(input.created_at))
}

pub fn sort_threads<T>(
    rows: &mut [T],
    order: SortOrder,
    input: impl Fn(&T) -> ThreadSortInput<'_>,
    key: impl Fn(&T) -> &ThreadKey,
    parse: &impl Fn(&str) -> Option<i64>,
) {
    rows.sort_by_cached_key(|row| {
        (
            std::cmp::Reverse(get_thread_sort_timestamp(&input(row), order, parse)),
            std::cmp::Reverse(key(row).thread.clone()),
            key(row).environment.clone(),
        )
    });
}

pub fn latest_thread_for_project<'a, T>(
    rows: &'a [T],
    project: &ProjectId,
    project_id: impl Fn(&T) -> &ProjectId,
    archived: impl Fn(&T) -> bool,
    timestamp: impl Fn(&T) -> Option<i64>,
    key: impl Fn(&T) -> &ThreadKey,
) -> Option<&'a T> {
    rows.iter()
        .filter(|t| project_id(t) == project && !archived(t))
        .max_by(|a, b| {
            timestamp(a)
                .cmp(&timestamp(b))
                .then_with(|| key(a).thread.cmp(&key(b).thread))
                .then_with(|| key(a).environment.cmp(&key(b).environment))
        })
}

pub fn resolve_thread_activity_timestamp<'a>(
    latest_user_message: Option<&'a str>,
    run_dates: &[Option<&'a str>],
    updated: &'a str,
    parse: &impl Fn(&str) -> Option<i64>,
) -> Option<&'a str> {
    let mut latest = None;
    for date in std::iter::once(latest_user_message)
        .chain(run_dates.iter().copied())
        .flatten()
    {
        if let Some(millis) = parse(date)
            && latest.is_none_or(|(_, previous)| millis > previous)
        {
            latest = Some((date, millis));
        }
    }
    latest
        .map(|(date, _)| date)
        .or_else(|| parse(updated).map(|_| updated))
}

pub fn resolve_settled_thread_timestamp<'a>(
    settled: Option<&'a str>,
    latest_user_message: Option<&'a str>,
    run_dates: &[Option<&'a str>],
    updated: &'a str,
    parse: &impl Fn(&str) -> Option<i64>,
) -> Option<&'a str> {
    settled.filter(|s| parse(s).is_some()).or_else(|| {
        resolve_thread_activity_timestamp(latest_user_message, run_dates, updated, parse)
    })
}

pub fn sort_settled_threads<T>(
    rows: &mut [T],
    timestamp: impl Fn(&T) -> Option<i64>,
    key: impl Fn(&T) -> &ThreadKey,
) {
    rows.sort_by_cached_key(|row| {
        (
            std::cmp::Reverse(timestamp(row).unwrap_or(0)),
            key(row).thread.clone(),
            key(row).environment.clone(),
        )
    });
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|c| c.is_ascii_lowercase()) && !key.ends_with('a')
}

fn midpoint(a: &[u8], b: &[u8], out: &mut String) {
    if !b.is_empty() {
        let common = b
            .iter()
            .enumerate()
            .take_while(|(i, ch)| a.get(*i).copied().unwrap_or(b'a') == **ch)
            .count();
        if common > 0 {
            for &ch in &b[..common] {
                out.push(ch as char);
            }
            midpoint(a.get(common..).unwrap_or(&[]), &b[common..], out);
            return;
        }
    }
    let da = a.first().copied().unwrap_or(b'a') - b'a';
    let db = b.first().map_or(26, |v| v - b'a');
    if db - da > 1 {
        out.push((b'a' + (da + db).div_ceil(2)) as char);
    } else if b.len() > 1 {
        out.push(b[0] as char);
    } else {
        out.push((b'a' + da) as char);
        midpoint(a.get(1..).unwrap_or(&[]), &[], out);
    }
}

pub fn pin_order_key_between(before: Option<&str>, after: Option<&str>) -> Option<String> {
    let a = before.unwrap_or("");
    let b = after.unwrap_or("");
    if (!a.is_empty() && !valid_key(a)) || (!b.is_empty() && (!valid_key(b) || a >= b)) {
        return None;
    }
    let mut out = String::new();
    midpoint(a.as_bytes(), b.as_bytes(), &mut out);
    Some(out)
}

pub fn generate_spread_pin_order_keys(count: usize) -> Vec<String> {
    let mut width = 2;
    let mut space = 26_u128.pow(width);
    while space <= (count as u128 + 1) * 2 {
        width += 1;
        space *= 26;
    }
    let step = space as f64 / (count as f64 + 1.0);
    (0..count)
        .map(|i| {
            let mut value = (step * (i as f64 + 1.0)).round() as u128;
            if value.is_multiple_of(26) {
                value += 1;
            }
            let mut bytes = vec![b'a'; width as usize];
            for ch in bytes.iter_mut().rev() {
                *ch = b'a' + (value % 26) as u8;
                value /= 26;
            }
            bytes.into_iter().map(char::from).collect()
        })
        .collect()
}

pub fn plan_pinned_reorder(
    ordered_ids: &[String],
    keys_by_id: &BTreeMap<String, Option<String>>,
    moved_id: &str,
) -> Vec<(String, String)> {
    let Some(index) = ordered_ids.iter().position(|id| id == moved_id) else {
        return vec![];
    };
    let visible: BTreeSet<_> = ordered_ids.iter().collect();
    let reserved: BTreeSet<_> = keys_by_id
        .iter()
        .filter(|(id, _)| !visible.contains(id))
        .filter_map(|(_, key)| key.as_deref())
        .collect();
    let before = index
        .checked_sub(1)
        .and_then(|i| keys_by_id.get(&ordered_ids[i]))
        .and_then(Option::as_deref);
    let after = ordered_ids
        .get(index + 1)
        .and_then(|id| keys_by_id.get(id))
        .and_then(Option::as_deref);
    if (index == 0 || before.is_some()) && (index + 1 == ordered_ids.len() || after.is_some()) {
        let mut candidate = pin_order_key_between(before, after);
        while candidate
            .as_deref()
            .is_some_and(|key| reserved.contains(key))
        {
            candidate = pin_order_key_between(candidate.as_deref(), after);
        }
        if let Some(key) = candidate {
            return vec![(moved_id.into(), key)];
        }
    }
    let keys = generate_spread_pin_order_keys(ordered_ids.len() + reserved.len());
    ordered_ids
        .iter()
        .zip(keys.into_iter().filter(|k| !reserved.contains(k.as_str())))
        .filter(|(id, key)| keys_by_id.get(*id).and_then(Option::as_ref) != Some(key))
        .map(|(id, key)| (id.clone(), key))
        .collect()
}

pub struct OrderedThread<'a> {
    pub key: &'a ThreadKey,
    pub created_at: Option<i64>,
    pub unsettled_at: Option<i64>,
    pub order_key: Option<&'a str>,
}

pub fn sort_pinned_threads(rows: &mut [OrderedThread<'_>]) {
    rows.sort_by(|a, b| {
        match (a.order_key, b.order_key) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.created_at.unwrap_or(0).cmp(&a.created_at.unwrap_or(0)),
        }
        .then(a.key.thread.cmp(&b.key.thread))
        .then(a.key.environment.cmp(&b.key.environment))
    });
}
pub fn sort_active_threads(rows: &mut [OrderedThread<'_>]) {
    rows.sort_by(|a, b| {
        match (a.order_key, b.order_key) {
            (Some(a), Some(b)) => a.cmp(b),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, None) => b
                .created_at
                .unwrap_or(0)
                .max(b.unsettled_at.unwrap_or(0))
                .cmp(&a.created_at.unwrap_or(0).max(a.unsettled_at.unwrap_or(0))),
        }
        .then(a.key.thread.cmp(&b.key.thread))
        .then(a.key.environment.cmp(&b.key.environment))
    });
}
