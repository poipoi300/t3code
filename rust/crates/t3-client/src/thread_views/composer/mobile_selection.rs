//! One application-lifetime registry keeps caret memory and reset generations out of persisted drafts.
use super::mobile_content::Insertion;
use crate::{ClientError, thread_views::document::utf16_len};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct SelectionRegistry {
    last_reset: u64,
    resets: BTreeMap<String, u64>,
    selections: BTreeMap<String, Insertion>,
}
pub struct EditorBinding {
    key: Option<String>,
    generation: u64,
}
impl EditorBinding {
    pub fn new(key: Option<String>) -> Self {
        Self { key, generation: 0 }
    }
}
impl SelectionRegistry {
    fn next_reset(&mut self) -> Result<u64, ClientError> {
        self.last_reset = self.last_reset.checked_add(1).ok_or_else(|| {
            ClientError::InvalidPayload("Composer reset generation overflow.".into())
        })?;
        Ok(self.last_reset)
    }
    pub fn reset(&mut self, key: &str) -> Result<u64, ClientError> {
        let generation = self.next_reset()?;
        self.resets.insert(key.into(), generation);
        Ok(generation)
    }
    pub fn remember(&mut self, key: &str, text: String, start: usize, end: usize) {
        self.selections
            .insert(key.into(), Insertion { text, start, end });
    }
    pub fn selection(&self, key: &str, text: &str) -> Option<(usize, usize)> {
        self.selections
            .get(key)
            .filter(|selection| selection.text == text)
            .map(|selection| (selection.start, selection.end))
    }
    pub fn capture(&self, key: &str, text: &str, selection: Option<(usize, usize)>) -> Insertion {
        let (start, end) = selection
            .or_else(|| self.selection(key, text))
            .unwrap_or_else(|| {
                let end = utf16_len(text);
                (end, end)
            });
        Insertion {
            text: text.into(),
            start,
            end,
        }
    }
    pub fn bind(
        &mut self,
        binding: &mut EditorBinding,
        key: Option<&str>,
    ) -> Result<u64, ClientError> {
        if binding.key.as_deref() != key {
            binding.key = key.map(str::to_owned);
            binding.generation = self.next_reset()?;
        }
        Ok(binding.generation.max(
            key.and_then(|key| self.resets.get(key).copied())
                .unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_switches_and_resets_share_one_monotone_generation_and_carets_require_matching_text() {
        let mut registry = SelectionRegistry::default();
        let mut binding = EditorBinding::new(Some("first".into()));
        assert_eq!(
            registry.bind(&mut binding, Some("first")).expect("initial"),
            0
        );
        registry.remember("first", "😀 one".into(), 2, 2);
        assert_eq!(registry.capture("first", "😀 one", None).start, 2);
        assert!(registry.selection("second", "😀 one").is_none());
        assert_eq!(registry.capture("first", "changed", None).start, 7);
        let first = registry.reset("first").expect("reset");
        assert_eq!(
            registry.bind(&mut binding, Some("first")).expect("first"),
            first
        );
        let second = registry.bind(&mut binding, Some("second")).expect("switch");
        assert!(second > first);
        assert!(registry.bind(&mut binding, Some("first")).expect("back") > second);
        let detached = registry.bind(&mut binding, None).expect("detach");
        assert!(detached > second);
        assert_eq!(registry.bind(&mut binding, None).expect("stable"), detached);
    }
}
