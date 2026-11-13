#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewGuideAttentionKind {
    #[serde(rename = "tradeoff")]
    Tradeoff,
    #[serde(rename = "contract")]
    Contract,
    #[serde(rename = "product")]
    Product,
    #[serde(rename = "risk")]
    Risk,
    #[serde(rename = "accessibility")]
    Accessibility,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideAttention {
    pub kind: PullRequestReviewGuideAttentionKind,
    pub note: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewGuideExcerptSide {
    #[serde(rename = "new")]
    New,
    #[serde(rename = "old")]
    Old,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideExcerpt {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub side: PullRequestReviewGuideExcerptSide,
    pub start: crate::base_schemas::PositiveInt,
    pub end: crate::base_schemas::PositiveInt,
    pub caption: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideStopFiles {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideNote {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub line: crate::base_schemas::PositiveInt,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideStop {
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub summary: String,
    #[serde(with = "crate::wire::nullable")]
    pub attention: Option<PullRequestReviewGuideAttention>,
    pub excerpts: Vec<PullRequestReviewGuideExcerpt>,
    pub files: Vec<PullRequestReviewGuideStopFiles>,
    pub notes: Vec<PullRequestReviewGuideNote>,
    pub questions: Vec<crate::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuide {
    pub head_sha: crate::base_schemas::TrimmedNonEmptyString,
    pub generated_at: crate::base_schemas::IsoDateTime,
    pub model_selection: crate::model_selection::ModelSelection,
    pub stops: Vec<PullRequestReviewGuideStop>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewGuideRunMember1Tag {
    #[serde(rename = "Running")]
    Running,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideRunMember1 {
    #[serde(rename = "_tag")]
    pub _tag: PullRequestReviewGuideRunMember1Tag,
    pub head_sha: crate::base_schemas::TrimmedNonEmptyString,
    pub started_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestReviewGuideRunMember2Tag {
    #[serde(rename = "Failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideRunMember2 {
    #[serde(rename = "_tag")]
    pub _tag: PullRequestReviewGuideRunMember2Tag,
    pub head_sha: crate::base_schemas::TrimmedNonEmptyString,
    pub failed_at: crate::base_schemas::IsoDateTime,
    pub detail: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PullRequestReviewGuideRun {
    Running(PullRequestReviewGuideRunMember1),
    Failed(PullRequestReviewGuideRunMember2),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewGuideResult {
    #[serde(with = "crate::wire::nullable")]
    pub guide: Option<PullRequestReviewGuide>,
    #[serde(with = "crate::wire::nullable")]
    pub run: Option<PullRequestReviewGuideRun>,
    #[serde(with = "crate::wire::nullable")]
    pub current_head_sha: Option<crate::base_schemas::TrimmedNonEmptyString>,
}
