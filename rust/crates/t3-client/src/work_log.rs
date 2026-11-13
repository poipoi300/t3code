//! Pure work-log presentation and scroll decisions.
use crate::types::ThreadKey;
#[path = "view_logic/session/work_log_commands.rs"]
pub mod commands;
#[path = "view_logic/session/work_log_detail.rs"]
pub mod detail;
#[path = "view_logic/session/work_log_presentation.rs"]
pub mod presentation;
pub use commands::*;
pub use detail::*;
pub use presentation::*;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollAnchor {
    pub thread: ThreadKey,
    pub item_id: String,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkGroupScrollAnchor<'a> {
    pub row_id: &'a str,
    pub offset_within_row: f64,
    pub scroll_offset: f64,
}

pub fn resolve_work_group_scroll_anchor<'a>(
    row_ids: &[&'a str],
    scroll: f64,
    position_at_index: impl Fn(usize) -> Option<f64>,
) -> Option<WorkGroupScrollAnchor<'a>> {
    if row_ids.is_empty() || !scroll.is_finite() {
        return None;
    }
    let scroll_offset = scroll.max(0.0);
    let mut low = 0;
    let mut high = row_ids.len() - 1;
    let mut index = 0;
    let mut row_top = position_at_index(0).filter(|top| top.is_finite())?;
    while low <= high {
        let middle = low + (high - low) / 2;
        let top = position_at_index(middle).filter(|top| top.is_finite())?;
        if top <= scroll_offset {
            index = middle;
            row_top = top;
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    Some(WorkGroupScrollAnchor {
        row_id: row_ids[index],
        offset_within_row: (scroll_offset - row_top).max(0.0),
        scroll_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_flick_uses_actual_scroll_instead_of_cached_start_index() {
        let rows: Vec<_> = (0..18).map(|index| format!("call-{index}")).collect();
        let ids: Vec<_> = rows.iter().map(String::as_str).collect();
        assert_eq!(
            resolve_work_group_scroll_anchor(&ids, 0.0, |index| Some(index as f64 * 33.0)),
            Some(WorkGroupScrollAnchor {
                row_id: "call-0",
                offset_within_row: 0.0,
                scroll_offset: 0.0
            })
        );
        assert_eq!(
            resolve_work_group_scroll_anchor(&ids, 170.0, |index| Some(index as f64 * 33.0)),
            Some(WorkGroupScrollAnchor {
                row_id: "call-5",
                offset_within_row: 5.0,
                scroll_offset: 170.0
            })
        );
        assert_eq!(
            resolve_work_group_scroll_anchor(&ids, f64::INFINITY, |_| Some(0.0)),
            None
        );
        assert_eq!(resolve_work_group_scroll_anchor(&ids, 0.0, |_| None), None);
        assert_eq!(
            resolve_work_group_scroll_anchor(&ids, 0.0, |_| Some(f64::NAN)),
            None
        );
        assert_eq!(
            resolve_work_group_scroll_anchor(&[], 0.0, |_| Some(0.0)),
            None
        );
    }
}
