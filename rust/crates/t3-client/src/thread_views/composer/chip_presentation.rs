//! Port of composerContextRecords.ts and chat/assistantCitationCommentDismissal.ts.

use t3_contracts::{
    assistant_citations::ASSISTANT_CITATION_MAX_COMMENT_LENGTH,
    composer_context::{PullRequestContextMetadataState, ReviewCommentContextRecord},
    wire::trim_js,
};

use super::super::document::utf16_len;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRequestDisplayState {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewChipPresentation {
    pub label: String,
    pub summary: bool,
    pub state: Option<PullRequestDisplayState>,
}

pub fn review_comment_presentation(record: &ReviewCommentContextRecord) -> ReviewChipPresentation {
    let legacy_number =
        record.file_path.0.strip_prefix("PR #").filter(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        });
    let summary = record.pull_request.is_some()
        || (record.section_id.0.starts_with("pull-request:")
            && trim_js(&record.diff).is_empty()
            && legacy_number.is_some());
    let state = record
        .pull_request
        .as_ref()
        .map(|request| match request.state {
            PullRequestContextMetadataState::Open if request.is_draft => {
                PullRequestDisplayState::Draft
            }
            PullRequestContextMetadataState::Open => PullRequestDisplayState::Open,
            PullRequestContextMetadataState::Merged => PullRequestDisplayState::Merged,
            PullRequestContextMetadataState::Closed => PullRequestDisplayState::Closed,
        });
    // The existing label helper reads only patch emptiness; do not clone a captured diff for a label.
    let label = super::web_drafts::review_label(&serde_json::json!({
        "sectionId":record.section_id, "filePath":record.file_path,
        "rangeLabel":record.range_label, "diff":if trim_js(&record.diff).is_empty() { "" } else { "patch" }, "pullRequest":record.pull_request,
    }));
    ReviewChipPresentation {
        label,
        summary,
        state,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CitationCommentDismissal {
    Commit(String),
    Close,
    KeepOpen,
}

pub fn resolve_assistant_citation_comment_dismissal(
    reason: &str,
    draft: Option<&str>,
    saved: Option<&str>,
) -> CitationCommentDismissal {
    if reason == "escape-key" {
        return CitationCommentDismissal::Close;
    }
    let Some(draft) = draft else {
        return CitationCommentDismissal::Close;
    };
    if trim_js(draft) == trim_js(saved.unwrap_or_default()) {
        return CitationCommentDismissal::Close;
    }
    if utf16_len(draft) > ASSISTANT_CITATION_MAX_COMMENT_LENGTH {
        return CitationCommentDismissal::KeepOpen;
    }
    CitationCommentDismissal::Commit(draft.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn citation_dismissal_preserves_changed_and_oversized_drafts() {
        // assistantCitationCommentDismissal.test.ts: outside press, focus-out, over-length.
        for (reason, draft, saved, expected) in [
            (
                "outside-press",
                Some("needs a retry"),
                None,
                CitationCommentDismissal::Commit("needs a retry".into()),
            ),
            (
                "focus-out",
                Some("second thought"),
                Some("first thought"),
                CitationCommentDismissal::Commit("second thought".into()),
            ),
            (
                "escape-key",
                Some("discard"),
                None,
                CitationCommentDismissal::Close,
            ),
            ("outside-press", None, None, CitationCommentDismissal::Close),
            (
                "outside-press",
                Some("\u{feff}same\u{a0}"),
                Some("same"),
                CitationCommentDismissal::Close,
            ),
        ] {
            assert_eq!(
                resolve_assistant_citation_comment_dismissal(reason, draft, saved),
                expected
            );
        }
        let oversized = "x".repeat(ASSISTANT_CITATION_MAX_COMMENT_LENGTH + 1);
        assert_eq!(
            resolve_assistant_citation_comment_dismissal("outside-press", Some(&oversized), None),
            CitationCommentDismissal::KeepOpen
        );
        let unicode = "😀".repeat(ASSISTANT_CITATION_MAX_COMMENT_LENGTH / 2 + 1);
        assert_eq!(
            resolve_assistant_citation_comment_dismissal("outside-press", Some(&unicode), None),
            CitationCommentDismissal::KeepOpen
        );
        assert_eq!(
            resolve_assistant_citation_comment_dismissal(
                "outside-press",
                Some(&oversized),
                Some(&oversized)
            ),
            CitationCommentDismissal::Close
        );
    }

    #[test]
    fn review_contexts_keep_diff_ranges_and_legacy_and_current_pr_states() {
        let mut value = json!({"version":1,"contextId":"review","label":"stored","kind":"review-comment","sectionId":"pull-request:7","sectionTitle":"Changes","filePath":"src/main.rs","startIndex":0,"endIndex":1,"rangeLabel":"-3 to -5","text":"Comment","diff":"patch"});
        let present = |value: &serde_json::Value| {
            review_comment_presentation(&serde_json::from_value(value.clone()).expect("record"))
        };
        assert_eq!(
            present(&value),
            ReviewChipPresentation {
                label: "#7 main.rs L3 to L5 (before)".into(),
                summary: false,
                state: None
            }
        );
        value["filePath"] = json!("PR #7");
        value["diff"] = json!("\u{feff}\u{a0}");
        assert_eq!(
            present(&value),
            ReviewChipPresentation {
                label: "#7".into(),
                summary: true,
                state: None
            }
        );
        value["pullRequest"] = json!({"number":7,"title":"Change","url":"https://example.test/pull/7","headBranch":"head","baseBranch":"main","state":"open","isDraft":true});
        for (state, draft, expected) in [
            ("open", true, PullRequestDisplayState::Draft),
            ("open", false, PullRequestDisplayState::Open),
            ("merged", false, PullRequestDisplayState::Merged),
            ("closed", false, PullRequestDisplayState::Closed),
        ] {
            value["pullRequest"]["state"] = json!(state);
            value["pullRequest"]["isDraft"] = json!(draft);
            assert_eq!(
                present(&value),
                ReviewChipPresentation {
                    label: "#7".into(),
                    summary: true,
                    state: Some(expected)
                }
            );
        }
    }
}
