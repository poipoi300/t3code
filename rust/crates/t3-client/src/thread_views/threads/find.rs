//! Find text uses the same rendered-text and literal matching policy as the server.

pub use t3_shared::find_text::{markdown_segments, rendered_segments};
pub use t3_shared::thread_search::find_thread_search_occurrences;

pub fn step_thread_find_index(index: usize, total: usize, delta: isize) -> usize {
    if total == 0 {
        return 0;
    }
    let index = index.min(total - 1);
    if delta >= 0 {
        (index + delta as usize % total) % total
    } else {
        (index + total - delta.unsigned_abs() % total) % total
    }
}

pub fn format_thread_find_count(index: usize, total: usize) -> String {
    if total == 0 {
        "0/0".into()
    } else {
        format!("{}/{total}", index.min(total - 1) + 1)
    }
}
