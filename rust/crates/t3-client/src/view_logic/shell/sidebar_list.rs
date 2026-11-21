//! Port of apps/web/src/components/Sidebar.tsx's shelf composition: `visibleWorkingThreads`,
//! `visibleSnoozedThreads`, `visibleSettledThreads` / `renderedSettledThreads` (`routeThreadOnly`
//! and the settled tail of 10 rows growing by 25), `hiddenSettledCount`, the marker sequence of
//! `sidebarListItemsFresh`, and the three shelves' expanded flags (`useLocalStorage` records
//! `t3code:sidebar:{working,snoozed,settled}-expanded`, written alone as the web writes them).
//!
//! Older settled pages from the server (`useOlderSettledShelf`) are not composed here yet: the
//! settled count and tail cover the loaded rows only.

use super::sidebar::{Marker, Section};
use super::ui_state::{
    PREFERENCES_NAMESPACE, SETTLED_SHELF_EXPANDED_KEY, SNOOZED_SHELF_EXPANDED_KEY,
    WORKING_SHELF_EXPANDED_KEY,
};
use crate::platform::{PlatformError, Storage, StorageArea, StorageKey, StorageWrite};

/// `SETTLED_TAIL_INITIAL_COUNT`.
pub const SETTLED_TAIL_INITIAL_COUNT: usize = 10;
/// `SETTLED_TAIL_PAGE_COUNT`.
pub const SETTLED_TAIL_PAGE_COUNT: usize = 25;

/// The three collapsible shelves' expanded flags; every shelf starts collapsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShelfExpansion {
    pub working: bool,
    pub snoozed: bool,
    pub settled: bool,
}

impl ShelfExpansion {
    /// The flag of a shelf header marker; `None` for the drag-only markers.
    pub fn of(self, marker: Marker) -> Option<bool> {
        match marker {
            Marker::WorkingHeader => Some(self.working),
            Marker::SnoozedHeader => Some(self.snoozed),
            Marker::SettledHeader => Some(self.settled),
            _ => None,
        }
    }

    pub fn with(mut self, marker: Marker, expanded: bool) -> Self {
        match marker {
            Marker::WorkingHeader => self.working = expanded,
            Marker::SnoozedHeader => self.snoozed = expanded,
            Marker::SettledHeader => self.settled = expanded,
            _ => {}
        }
        self
    }
}

/// One entry of the sidebar's list, in order: a thread row (its index in the caller's ordered
/// rows), a marker, or the settled shelf's "Show more" row at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarListItem {
    Thread(usize),
    Marker(Marker),
    SettledMore,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarListItems {
    pub items: Vec<SidebarListItem>,
    /// Header counts include rows a collapsed shelf hides.
    pub working_count: usize,
    pub snoozed_count: usize,
    pub settled_count: usize,
    /// `hiddenSettledCount`: settled rows past the tail.
    pub hidden_settled: usize,
}

/// `routeThreadOnly`.
fn route_only(rows: &[usize], route: Option<usize>) -> Vec<usize> {
    route
        .filter(|route| rows.contains(route))
        .into_iter()
        .collect()
}

/// `sidebarListItemsFresh` over `sections` (each row's section, rows already in the client's
/// order). `route` is the index of the open thread's row, which a collapsed shelf still shows.
/// `settled_visible` is the settled tail's length (`settledVisibleCount`).
pub fn sidebar_list_items(
    sections: &[Section],
    route: Option<usize>,
    expansion: ShelfExpansion,
    settled_visible: usize,
) -> SidebarListItems {
    let of = |section: Section| -> Vec<usize> {
        sections
            .iter()
            .enumerate()
            .filter(|(_, row)| **row == section)
            .map(|(index, _)| index)
            .collect()
    };
    let (pinned, active, working, snoozed, settled) = (
        of(Section::Pinned),
        of(Section::Active),
        of(Section::Working),
        of(Section::Snoozed),
        of(Section::Settled),
    );
    let mut visible_settled: Vec<usize> = settled.iter().copied().take(settled_visible).collect();
    if let Some(route) = route
        && settled.contains(&route)
        && !visible_settled.contains(&route)
    {
        visible_settled.push(route);
    }
    if visible_settled.len() == settled.len() {
        visible_settled = settled.clone();
    }
    let hidden_settled = settled.len().saturating_sub(visible_settled.len());
    let mut list = SidebarListItems {
        items: Vec::new(),
        working_count: working.len(),
        snoozed_count: snoozed.len(),
        settled_count: settled.len(),
        hidden_settled,
    };
    if sections.is_empty() {
        return list;
    }
    let rendered_settled = if expansion.settled {
        visible_settled
    } else {
        route_only(&visible_settled, route)
    };
    let rows = |indices: Vec<usize>| indices.into_iter().map(SidebarListItem::Thread);
    let items = &mut list.items;
    items.push(SidebarListItem::Marker(Marker::PinnedHeader));
    items.extend(rows(pinned));
    items.push(SidebarListItem::Marker(Marker::PinnedDivider));
    items.push(SidebarListItem::Marker(Marker::ActivePlaceholder));
    items.extend(rows(active));
    if !working.is_empty() {
        items.push(SidebarListItem::Marker(Marker::WorkingHeader));
        items.extend(rows(if expansion.working {
            working
        } else {
            route_only(&working, route)
        }));
    }
    if !snoozed.is_empty() {
        items.push(SidebarListItem::Marker(Marker::SnoozedHeader));
        items.extend(rows(if expansion.snoozed {
            snoozed
        } else {
            route_only(&snoozed, route)
        }));
    }
    items.push(SidebarListItem::Marker(Marker::SettledHeader));
    items.push(SidebarListItem::Marker(Marker::SettledPlaceholder));
    items.extend(rows(rendered_settled));
    if expansion.settled && hidden_settled > 0 {
        items.push(SidebarListItem::SettledMore);
    }
    list
}

fn shelf_key(marker: Marker) -> Option<StorageKey> {
    let record = match marker {
        Marker::WorkingHeader => WORKING_SHELF_EXPANDED_KEY,
        Marker::SnoozedHeader => SNOOZED_SHELF_EXPANDED_KEY,
        Marker::SettledHeader => SETTLED_SHELF_EXPANDED_KEY,
        _ => return None,
    };
    Some(StorageKey {
        namespace: PREFERENCES_NAMESPACE.into(),
        environment: None,
        record: record.into(),
    })
}

/// The stored flags; an unreadable or missing record is collapsed, as `useLocalStorage`'s
/// default is.
pub async fn load_shelf_expansion(storage: &(impl Storage + ?Sized)) -> ShelfExpansion {
    let mut expansion = ShelfExpansion::default();
    for marker in [
        Marker::WorkingHeader,
        Marker::SnoozedHeader,
        Marker::SettledHeader,
    ] {
        let Some(key) = shelf_key(marker) else {
            continue;
        };
        let stored = storage
            .get(StorageArea::Ordinary, key)
            .await
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
            .unwrap_or(false);
        expansion = expansion.with(marker, stored);
    }
    expansion
}

/// One shelf's toggle, written alone. Markers other than the three shelf headers are refused.
pub async fn save_shelf_expanded(
    storage: &(impl Storage + ?Sized),
    marker: Marker,
    expanded: bool,
) -> Result<(), PlatformError> {
    let key = shelf_key(marker)
        .ok_or_else(|| PlatformError::InvalidData(format!("{} is not a shelf", marker.name())))?;
    let value = serde_json::to_vec(&expanded)
        .map_err(|error| PlatformError::InvalidData(error.to_string()))?;
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key,
                value: Some(value),
            }],
        )
        .await
}

/// The shelves' flags for the UI, reached with `ClientRuntime::sidebar_shelves`.
#[derive(Clone)]
pub struct SidebarShelves<P> {
    platform: P,
}

impl<P: crate::platform::Platform + Clone + 'static> SidebarShelves<P> {
    pub fn new(platform: P) -> Self {
        Self { platform }
    }

    pub fn load(&self) -> futures::future::LocalBoxFuture<'static, ShelfExpansion> {
        let platform = self.platform.clone();
        Box::pin(async move { load_shelf_expansion(&platform).await })
    }

    pub fn project_expansion(
        &self,
    ) -> futures::future::LocalBoxFuture<'static, std::collections::BTreeMap<String, bool>> {
        let platform = self.platform.clone();
        Box::pin(async move {
            super::ui_state::UiPreferences::load(&platform)
                .await
                .map_or_else(
                    |_| Default::default(),
                    |prefs| prefs.state.project_expanded_by_id,
                )
        })
    }

    pub fn save_project_expansion(
        &self,
        aliases: Vec<String>,
        expanded: bool,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), PlatformError>> {
        use super::ui_state::{STORAGE_KEY, UiPreferences, UiState};
        use crate::platform::{StorageCondition, StorageTransaction};
        let platform = self.platform.clone();
        Box::pin(async move {
            let key = StorageKey::new(PREFERENCES_NAMESPACE, None, STORAGE_KEY);
            let invalid =
                |error: &dyn std::fmt::Display| PlatformError::InvalidData(error.to_string());
            for _ in 0..3 {
                let expected = platform.get(StorageArea::Ordinary, key.clone()).await?;
                let mut state = if let Some(raw) = expected.as_deref() {
                    let raw = std::str::from_utf8(raw).map_err(|error| invalid(&error))?;
                    UiState::decode(raw, &crate::time::epoch_ms).map_err(|error| invalid(&error))?
                } else {
                    UiPreferences::load(&platform).await?.state
                };
                state.set_project_expanded(&aliases, expanded);
                let value = state
                    .encode()
                    .map_err(|error| invalid(&error))?
                    .into_bytes();
                if platform
                    .transaction(StorageTransaction {
                        conditions: vec![StorageCondition {
                            area: StorageArea::Ordinary,
                            key: key.clone(),
                            expected,
                        }],
                        ordinary: vec![StorageWrite {
                            key: key.clone(),
                            value: Some(value),
                        }],
                        secure: vec![],
                    })
                    .await?
                {
                    return Ok(());
                }
            }
            Err(PlatformError::Storage(
                "Project expansion changed repeatedly while saving.".into(),
            ))
        })
    }

    pub fn save(
        &self,
        marker: Marker,
        expanded: bool,
    ) -> futures::future::LocalBoxFuture<'static, Result<(), PlatformError>> {
        let platform = self.platform.clone();
        Box::pin(async move { save_shelf_expanded(&platform, marker, expanded).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakePlatform;
    use Section::*;
    use futures::executor::block_on;

    #[test]
    fn writing_project_expansion_preserves_the_other_sidebar_records() {
        let platform = FakePlatform::default();
        block_on(async {
            let preferences = super::super::ui_state::UiPreferences {
                sidebar_width: Some(336.0),
                settled_shelf_expanded: true,
                ..Default::default()
            };
            preferences
                .save(&platform)
                .await
                .expect("seed other settings");
            let shelves = SidebarShelves::new(platform.clone());
            shelves
                .save_project_expansion(vec!["group".into(), "checkout".into()], false)
                .await
                .expect("save folder expansion");
            let stored = super::super::ui_state::UiPreferences::load(&platform)
                .await
                .expect("read sidebar preferences");
            assert_eq!(stored.sidebar_width, Some(336.0));
            assert!(stored.settled_shelf_expanded);
            assert_eq!(
                stored.state.project_expanded_by_id.get("group"),
                Some(&false)
            );
            assert_eq!(
                stored.state.project_expanded_by_id.get("checkout"),
                Some(&false)
            );
        });
    }

    fn markers(list: &SidebarListItems) -> Vec<String> {
        list.items
            .iter()
            .map(|item| match item {
                SidebarListItem::Thread(index) => index.to_string(),
                SidebarListItem::Marker(marker) => marker.name().to_owned(),
                SidebarListItem::SettledMore => "more".to_owned(),
            })
            .collect()
    }

    #[test]
    fn markers_follow_the_web_sequence_and_collapsed_shelves_keep_the_route_row() {
        let sections = [Pinned, Active, Working, Working, Snoozed, Settled, Settled];
        let list = sidebar_list_items(&sections, Some(3), ShelfExpansion::default(), 10);
        assert_eq!(
            markers(&list),
            [
                "pinned-header",
                "0",
                "pinned-divider",
                "active-placeholder",
                "1",
                "working-header",
                "3",
                "snoozed-header",
                "settled-header",
                "settled-placeholder",
            ]
        );
        assert_eq!(
            (list.working_count, list.snoozed_count, list.settled_count),
            (2, 1, 2)
        );
        let open = sidebar_list_items(
            &sections,
            None,
            ShelfExpansion {
                working: true,
                snoozed: true,
                settled: true,
            },
            10,
        );
        assert_eq!(
            markers(&open)[5..],
            [
                "working-header",
                "2",
                "3",
                "snoozed-header",
                "4",
                "settled-header",
                "settled-placeholder",
                "5",
                "6"
            ]
        );
    }

    #[test]
    fn empty_shelves_drop_their_headers_and_an_empty_list_has_no_markers() {
        let list = sidebar_list_items(&[Active], None, ShelfExpansion::default(), 10);
        assert_eq!(
            markers(&list),
            [
                "pinned-header",
                "pinned-divider",
                "active-placeholder",
                "0",
                "settled-header",
                "settled-placeholder"
            ]
        );
        assert!(
            sidebar_list_items(&[], None, ShelfExpansion::default(), 10)
                .items
                .is_empty()
        );
    }

    #[test]
    fn the_settled_tail_pages_and_appends_a_routed_row_beyond_it() {
        let sections = vec![Settled; 40];
        let expanded = ShelfExpansion {
            settled: true,
            ..Default::default()
        };
        let list = sidebar_list_items(&sections, Some(30), expanded, SETTLED_TAIL_INITIAL_COUNT);
        let rows: Vec<_> = markers(&list).into_iter().skip(5).collect();
        assert_eq!(rows.len(), 12, "ten rows, the routed one, and Show more");
        assert_eq!(rows[10], "30");
        assert_eq!(rows[11], "more");
        assert_eq!(list.hidden_settled, 29);
        let all = sidebar_list_items(&sections, None, expanded, 40);
        assert_eq!(all.hidden_settled, 0);
        assert!(!all.items.contains(&SidebarListItem::SettledMore));
        let collapsed = sidebar_list_items(&sections, None, ShelfExpansion::default(), 10);
        assert!(!collapsed.items.contains(&SidebarListItem::SettledMore));
    }

    #[test]
    fn each_shelf_flag_is_its_own_record() {
        let platform = FakePlatform::default();
        assert_eq!(
            block_on(load_shelf_expansion(&platform)),
            ShelfExpansion::default()
        );
        block_on(save_shelf_expanded(&platform, Marker::SnoozedHeader, true)).expect("save");
        assert_eq!(
            block_on(load_shelf_expansion(&platform)),
            ShelfExpansion {
                snoozed: true,
                ..Default::default()
            }
        );
        assert!(block_on(save_shelf_expanded(&platform, Marker::PinnedHeader, true)).is_err());
    }
}
