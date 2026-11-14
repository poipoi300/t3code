use super::JsTrim;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AutoSettleDaysField {
    pub saved: u32,
    pub draft: String,
    pub dirty: bool,
    pub pending: Option<u32>,
    pub revision: u64,
}

impl AutoSettleDaysField {
    pub fn new(saved: u32) -> Self {
        Self {
            saved,
            draft: saved.to_string(),
            dirty: false,
            pending: None,
            revision: 0,
        }
    }
    pub fn edit(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
        self.dirty = true;
    }
    pub fn sync(&mut self, saved: u32) {
        if self.pending == Some(saved) {
            self.pending = None;
        }
        self.saved = saved;
        if !self.dirty {
            self.draft = self.pending.unwrap_or(saved).to_string();
        }
    }
    pub fn cancel(&mut self) {
        self.draft = self.pending.unwrap_or(self.saved).to_string();
        self.dirty = false;
    }
    pub fn commit(&mut self) -> Option<u32> {
        let text = self.draft.js_trim();
        let days = [
            ("0x", 16),
            ("0X", 16),
            ("0o", 8),
            ("0O", 8),
            ("0b", 2),
            ("0B", 2),
        ]
        .into_iter()
        .find_map(|(prefix, radix)| {
            text.strip_prefix(prefix).map(|digits| {
                (!digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)))
                    .then(|| u32::from_str_radix(digits, radix).ok())
                    .flatten()
                    .map(f64::from)
                    .unwrap_or(f64::NAN)
            })
        })
        .unwrap_or_else(|| text.parse::<f64>().unwrap_or(f64::NAN));
        if !days.is_finite() || days.fract() != 0.0 || !(1.0..=90.0).contains(&days) {
            self.cancel();
            return None;
        }
        let days = days as u32;
        self.draft = days.to_string();
        let submit = self.dirty && self.pending != Some(days);
        self.dirty = false;
        if !submit {
            return None;
        }
        self.pending = Some(days);
        self.revision += 1;
        Some(days)
    }
    pub fn fail(&mut self, revision: u64) {
        if self.revision != revision {
            return;
        }
        self.pending = None;
        if !self.dirty {
            self.draft = self.saved.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn submitted() -> AutoSettleDaysField {
        let mut field = AutoSettleDaysField::new(14);
        field.edit("30");
        assert_eq!(field.commit(), Some(30));
        field
    }
    #[test]
    fn saves_only_finished_edits() {
        let mut f = AutoSettleDaysField::new(14);
        for draft in ["1", "", "3", "30"] {
            f.edit(draft);
            assert_eq!(f.pending, None);
        }
        assert_eq!(f.commit(), Some(30));
        assert_eq!(f.commit(), None);
        f.sync(30);
        assert_eq!((f.saved, f.draft.as_str(), f.pending), (30, "30", None));
    }
    #[test]
    fn invalid_edits_restore_saved_or_pending() {
        for draft in ["", "0", "91", "1.5", "abc"] {
            let mut f = AutoSettleDaysField::new(14);
            f.edit(draft);
            assert_eq!(f.commit(), None);
            assert_eq!(f.draft, "14");
            let mut f = submitted();
            f.edit(draft);
            assert_eq!(f.commit(), None);
            assert_eq!(f.draft, "30");
        }
    }
    #[test]
    fn rejected_save_restores_saved_but_keeps_typing_and_allows_retry() {
        let mut f = submitted();
        f.fail(f.revision);
        assert_eq!(f.draft, "14");
        let mut f = submitted();
        f.edit("");
        f.fail(f.revision);
        assert_eq!(f.draft, "");
        assert_eq!(f.commit(), None);
        assert_eq!(f.draft, "14");
        f.edit("30");
        assert_eq!(f.commit(), Some(30));
    }
    #[test]
    fn partial_multi_environment_success_can_be_retyped() {
        let mut f = submitted();
        f.sync(30);
        f.fail(f.revision);
        assert_eq!(f.commit(), None);
        f.edit("3");
        f.edit("30");
        assert_eq!(f.commit(), Some(30));
    }
    #[test]
    fn returning_to_saved_value_submits_and_ignores_stale_failure() {
        let mut f = submitted();
        let old = f.revision;
        f.edit("14");
        assert_eq!(f.commit(), Some(14));
        let before = f.clone();
        f.fail(old);
        assert_eq!(f, before);
        f.sync(14);
        assert_eq!(f.pending, None);
        let before = f.clone();
        f.sync(14);
        assert_eq!(f, before);
        f.sync(20);
        assert_eq!(f.draft, "20");
    }
    #[test]
    fn earlier_confirmation_keeps_later_save_pending() {
        let mut f = submitted();
        f.edit("60");
        assert_eq!(f.commit(), Some(60));
        f.sync(30);
        assert_eq!((f.saved, f.draft.as_str(), f.pending), (30, "60", Some(60)));
        f.fail(f.revision);
        assert_eq!((f.draft.as_str(), f.pending), ("30", None));
    }
    #[test]
    fn escape_cancels_without_submission_and_sync_preserves_draft() {
        let mut f = AutoSettleDaysField::new(14);
        f.edit("1");
        f.cancel();
        assert_eq!(f.draft, "14");
        assert!(!f.dirty);
        assert_eq!(f.commit(), None);
        f.edit("4");
        f.sync(30);
        assert_eq!(f.saved, 30);
        assert_eq!(f.draft, "4");
        let mut f = submitted();
        f.edit("1");
        f.cancel();
        assert_eq!(f.draft, "30");
        assert_eq!(f.commit(), None);
    }
}
