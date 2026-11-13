use icu_collator::{Collator, CollatorBorrowed};
use std::{cmp::Ordering, sync::OnceLock};

pub(crate) fn compare(left: &str, right: &str) -> Ordering {
    static COLLATOR: OnceLock<Option<CollatorBorrowed<'static>>> = OnceLock::new();
    let collator = COLLATOR.get_or_init(|| {
        Collator::try_new(Default::default(), Default::default())
            .inspect_err(|_| tracing::warn!("Usage collation data unavailable"))
            .ok()
    });
    collator.as_ref().map_or_else(
        || left.encode_utf16().cmp(right.encode_utf16()),
        |collator| collator.compare(left, right),
    )
}
