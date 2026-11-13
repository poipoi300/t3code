crate::string_id!(ComposerContextId);

pub type ComposerContextKind = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ImageContextRecordVersion;
impl TryFrom<u64> for ImageContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ImageContextRecordVersion> for u64 {
    fn from(_: ImageContextRecordVersion) -> Self {
        1
    }
}

pub type ContextLabel = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImageContextRecordKind {
    #[serde(rename = "image")]
    Image,
}

pub type ContextAttachmentId = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContextRecord {
    pub version: ImageContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ImageContextRecordKind,
    pub attachment_id: ContextAttachmentId,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub mime_type: crate::base_schemas::TrimmedNonEmptyString,
    pub size_bytes: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct FileContextRecordVersion;
impl TryFrom<u64> for FileContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<FileContextRecordVersion> for u64 {
    fn from(_: FileContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileContextRecordKind {
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContextRecord {
    pub version: FileContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: FileContextRecordKind,
    pub attachment_id: ContextAttachmentId,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub mime_type: crate::base_schemas::TrimmedNonEmptyString,
    pub size_bytes: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct TerminalContextRecordVersion;
impl TryFrom<u64> for TerminalContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<TerminalContextRecordVersion> for u64 {
    fn from(_: TerminalContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TerminalContextRecordKind {
    #[serde(rename = "terminal")]
    Terminal,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalContextRecordFields {
    pub version: TerminalContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: TerminalContextRecordKind,
    pub terminal_id: crate::base_schemas::TrimmedNonEmptyString,
    pub terminal_label: crate::base_schemas::TrimmedNonEmptyString,
    pub line_start: crate::base_schemas::NonNegativeInt,
    pub line_end: crate::base_schemas::NonNegativeInt,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", try_from = "TerminalContextRecordFields")]
pub struct TerminalContextRecord {
    pub version: TerminalContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: TerminalContextRecordKind,
    pub terminal_id: crate::base_schemas::TrimmedNonEmptyString,
    pub terminal_label: crate::base_schemas::TrimmedNonEmptyString,
    pub line_start: crate::base_schemas::NonNegativeInt,
    pub line_end: crate::base_schemas::NonNegativeInt,
    pub text: String,
}
impl TryFrom<TerminalContextRecordFields> for TerminalContextRecord {
    type Error = &'static str;
    fn try_from(fields: TerminalContextRecordFields) -> Result<Self, Self::Error> {
        if !(fields.line_end >= fields.line_start) {
            return Err("reversed context range");
        }
        Ok(Self {
            version: fields.version,
            context_id: fields.context_id,
            label: fields.label,
            kind: fields.kind,
            terminal_id: fields.terminal_id,
            terminal_label: fields.terminal_label,
            line_start: fields.line_start,
            line_end: fields.line_end,
            text: fields.text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ElementContextRecordVersion;
impl TryFrom<u64> for ElementContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ElementContextRecordVersion> for u64 {
    fn from(_: ElementContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ElementContextRecordKind {
    #[serde(rename = "element")]
    Element,
}

pub type ShortString = String;

pub type NullableShortString = Option<ShortString>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementContextSource {
    #[serde(with = "crate::wire::nullable")]
    pub function_name: NullableShortString,
    #[serde(with = "crate::wire::nullable")]
    pub file_name: NullableShortString,
    #[serde(with = "crate::wire::nullable")]
    pub line_number: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(with = "crate::wire::nullable")]
    pub column_number: Option<crate::base_schemas::NonNegativeInt>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementContextRecord {
    pub version: ElementContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ElementContextRecordKind,
    pub page_url: ShortString,
    #[serde(with = "crate::wire::nullable")]
    pub page_title: NullableShortString,
    pub tag_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub selector: NullableShortString,
    pub html_preview: String,
    #[serde(with = "crate::wire::nullable")]
    pub component_name: NullableShortString,
    #[serde(with = "crate::wire::nullable")]
    pub source: Option<ElementContextSource>,
    pub styles: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct PreviewAnnotationContextRecordVersion;
impl TryFrom<u64> for PreviewAnnotationContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<PreviewAnnotationContextRecordVersion> for u64 {
    fn from(_: PreviewAnnotationContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PreviewAnnotationContextRecordKind {
    #[serde(rename = "preview-annotation")]
    PreviewAnnotation,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementContextDetails {
    pub page_url: ShortString,
    #[serde(with = "crate::wire::nullable")]
    pub page_title: NullableShortString,
    pub tag_name: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(with = "crate::wire::nullable")]
    pub selector: NullableShortString,
    pub html_preview: String,
    #[serde(with = "crate::wire::nullable")]
    pub component_name: NullableShortString,
    #[serde(with = "crate::wire::nullable")]
    pub source: Option<ElementContextSource>,
    pub styles: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationContextRecordStyleChangeDetails {
    pub target_id: ShortString,
    #[serde(with = "crate::wire::nullable")]
    pub selector: NullableShortString,
    pub property: ShortString,
    pub previous_value: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnnotationContextRecord {
    pub version: PreviewAnnotationContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: PreviewAnnotationContextRecordKind,
    pub annotation_id: ShortString,
    pub page_url: ShortString,
    #[serde(with = "crate::wire::nullable")]
    pub page_title: NullableShortString,
    pub comment: String,
    pub target_summary: ShortString,
    pub style_changes: Vec<ShortString>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub elements: Option<Vec<ElementContextDetails>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub element_ids: Option<Vec<ShortString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub region_count: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub stroke_count: Option<crate::base_schemas::NonNegativeInt>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub style_change_details: Option<Vec<PreviewAnnotationContextRecordStyleChangeDetails>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub screenshot_context_id: Option<ComposerContextId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ReviewCommentContextRecordVersion;
impl TryFrom<u64> for ReviewCommentContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ReviewCommentContextRecordVersion> for u64 {
    fn from(_: ReviewCommentContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReviewCommentContextRecordKind {
    #[serde(rename = "review-comment")]
    ReviewComment,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullRequestContextMetadataState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "merged")]
    Merged,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestContextMetadata {
    pub number: crate::base_schemas::PositiveInt,
    pub title: ShortString,
    pub url: ShortString,
    pub head_branch: ShortString,
    pub base_branch: ShortString,
    pub state: PullRequestContextMetadataState,
    pub is_draft: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewCommentContextRecordFields {
    pub version: ReviewCommentContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ReviewCommentContextRecordKind,
    pub section_id: crate::base_schemas::TrimmedNonEmptyString,
    pub section_title: ShortString,
    pub file_path: crate::base_schemas::TrimmedNonEmptyString,
    pub start_index: crate::base_schemas::NonNegativeInt,
    pub end_index: crate::base_schemas::NonNegativeInt,
    pub range_label: ShortString,
    pub text: String,
    pub diff: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub fence_language: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pull_request: Option<PullRequestContextMetadata>,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    rename_all = "camelCase",
    try_from = "ReviewCommentContextRecordFields"
)]
pub struct ReviewCommentContextRecord {
    pub version: ReviewCommentContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ReviewCommentContextRecordKind,
    pub section_id: crate::base_schemas::TrimmedNonEmptyString,
    pub section_title: ShortString,
    pub file_path: crate::base_schemas::TrimmedNonEmptyString,
    pub start_index: crate::base_schemas::NonNegativeInt,
    pub end_index: crate::base_schemas::NonNegativeInt,
    pub range_label: ShortString,
    pub text: String,
    pub diff: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub fence_language: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub pull_request: Option<PullRequestContextMetadata>,
}
impl TryFrom<ReviewCommentContextRecordFields> for ReviewCommentContextRecord {
    type Error = &'static str;
    fn try_from(fields: ReviewCommentContextRecordFields) -> Result<Self, Self::Error> {
        if !(fields.end_index >= fields.start_index) {
            return Err("reversed context range");
        }
        Ok(Self {
            version: fields.version,
            context_id: fields.context_id,
            label: fields.label,
            kind: fields.kind,
            section_id: fields.section_id,
            section_title: fields.section_title,
            file_path: fields.file_path,
            start_index: fields.start_index,
            end_index: fields.end_index,
            range_label: fields.range_label,
            text: fields.text,
            diff: fields.diff,
            fence_language: fields.fence_language,
            pull_request: fields.pull_request,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct MentionContextRecordVersion;
impl TryFrom<u64> for MentionContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<MentionContextRecordVersion> for u64 {
    fn from(_: MentionContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MentionContextRecordKind {
    #[serde(rename = "mention")]
    Mention,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionContextRecord {
    pub version: MentionContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: MentionContextRecordKind,
    pub path: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct SkillContextRecordVersion;
impl TryFrom<u64> for SkillContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<SkillContextRecordVersion> for u64 {
    fn from(_: SkillContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SkillContextRecordKind {
    #[serde(rename = "skill")]
    Skill,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillContextRecord {
    pub version: SkillContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: SkillContextRecordKind,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ThreadContextRecordVersion;
impl TryFrom<u64> for ThreadContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ThreadContextRecordVersion> for u64 {
    fn from(_: ThreadContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadContextRecordKind {
    #[serde(rename = "thread")]
    Thread,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadContextRecord {
    pub version: ThreadContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ThreadContextRecordKind,
    pub environment_id: crate::base_schemas::EnvironmentId,
    pub thread_id: crate::base_schemas::ThreadId,
    pub title: ContextLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct EnvironmentContextRecordVersion;
impl TryFrom<u64> for EnvironmentContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<EnvironmentContextRecordVersion> for u64 {
    fn from(_: EnvironmentContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EnvironmentContextRecordKind {
    #[serde(rename = "environment")]
    Environment,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentContextRecord {
    pub version: EnvironmentContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: EnvironmentContextRecordKind,
    pub environment_id: crate::base_schemas::EnvironmentId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ResponseContextRecordVersion;
impl TryFrom<u64> for ResponseContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<ResponseContextRecordVersion> for u64 {
    fn from(_: ResponseContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResponseContextRecordKind {
    #[serde(rename = "response")]
    Response,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseContextRecordSubagents {
    pub thread_id: crate::base_schemas::ThreadId,
    pub title: ContextLabel,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub environment_id: Option<Option<crate::base_schemas::EnvironmentId>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseContextRecord {
    pub version: ResponseContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: ResponseContextRecordKind,
    pub citation: crate::assistant_citations::AssistantCitation,
    pub item_id: crate::base_schemas::TurnItemId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub subagents: Option<Option<Vec<ResponseContextRecordSubagents>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct UnknownContextRecordVersion;
impl TryFrom<u64> for UnknownContextRecordVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<UnknownContextRecordVersion> for u64 {
    fn from(_: UnknownContextRecordVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownContextRecord {
    pub version: UnknownContextRecordVersion,
    pub context_id: ComposerContextId,
    pub label: ContextLabel,
    pub kind: UnknownComposerContextKind,
    #[serde(deserialize_with = "deserialize_unknown_payload")]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ComposerContextRecord {
    Image(ImageContextRecord),
    File(FileContextRecord),
    Terminal(TerminalContextRecord),
    Element(ElementContextRecord),
    PreviewAnnotation(PreviewAnnotationContextRecord),
    ReviewComment(ReviewCommentContextRecord),
    Mention(MentionContextRecord),
    Skill(SkillContextRecord),
    Thread(ThreadContextRecord),
    Environment(EnvironmentContextRecord),
    Response(ResponseContextRecord),
    Unknown(UnknownContextRecord),
}

crate::string_id!(ComposerContextReferenceId);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestMessageAttribution {
    pub guest_message_id: crate::base_schemas::GuestMessageId,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub share_id: Option<Option<crate::base_schemas::GuestShareId>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum KnownComposerContextRecord {
    Image(ImageContextRecord),
    File(FileContextRecord),
    Terminal(TerminalContextRecord),
    Element(ElementContextRecord),
    PreviewAnnotation(PreviewAnnotationContextRecord),
    ReviewComment(ReviewCommentContextRecord),
    Mention(MentionContextRecord),
    Skill(SkillContextRecord),
    Thread(ThreadContextRecord),
    Environment(EnvironmentContextRecord),
    Response(ResponseContextRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct OrchestrationMessageContextVersion;
impl TryFrom<u64> for OrchestrationMessageContextVersion {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unexpected literal")
        }
    }
}
impl From<OrchestrationMessageContextVersion> for u64 {
    fn from(_: OrchestrationMessageContextVersion) -> Self {
        1
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationMessageContext {
    pub version: OrchestrationMessageContextVersion,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub guest: Option<Option<GuestMessageAttribution>>,
    #[serde(deserialize_with = "deserialize_message_records")]
    pub records: Vec<ComposerContextRecord>,
}

pub const COMPOSER_CONTEXT_MAX_RECORDS: usize = 200;
pub const COMPOSER_CONTEXT_LABEL_MAX_CHARS: usize = 200;
pub const COMPOSER_CONTEXT_TERMINAL_TEXT_MAX_CHARS: usize = 64_000;
pub const COMPOSER_CONTEXT_REVIEW_TEXT_MAX_CHARS: usize = 16_000;
pub const COMPOSER_CONTEXT_REVIEW_DIFF_MAX_CHARS: usize = 32_000;
pub const COMPOSER_CONTEXT_KINDS: &[&str] = &[
    "image",
    "file",
    "terminal",
    "element",
    "preview-annotation",
    "review-comment",
    "mention",
    "skill",
    "thread",
    "environment",
    "response",
];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ComposerContextKind", into = "ComposerContextKind")]
pub struct UnknownComposerContextKind(pub ComposerContextKind);

impl TryFrom<ComposerContextKind> for UnknownComposerContextKind {
    type Error = &'static str;
    fn try_from(kind: ComposerContextKind) -> Result<Self, Self::Error> {
        let value = kind.0.as_str();
        if value.is_empty()
            || value.len() > 40
            || !value.as_bytes()[0].is_ascii_lowercase()
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || COMPOSER_CONTEXT_KINDS.contains(&value)
        {
            return Err("expected an unknown context kind");
        }
        Ok(Self(kind))
    }
}
impl From<UnknownComposerContextKind> for ComposerContextKind {
    fn from(kind: UnknownComposerContextKind) -> Self {
        kind.0
    }
}

impl ComposerContextRecord {
    pub fn context_id(&self) -> &ComposerContextId {
        match self {
            Self::Image(record) => &record.context_id,
            Self::File(record) => &record.context_id,
            Self::Terminal(record) => &record.context_id,
            Self::Element(record) => &record.context_id,
            Self::PreviewAnnotation(record) => &record.context_id,
            Self::ReviewComment(record) => &record.context_id,
            Self::Mention(record) => &record.context_id,
            Self::Skill(record) => &record.context_id,
            Self::Thread(record) => &record.context_id,
            Self::Environment(record) => &record.context_id,
            Self::Response(record) => &record.context_id,
            Self::Unknown(record) => &record.context_id,
        }
    }
}

fn decode_context_records<E: serde::de::Error>(
    values: Vec<serde_json::Value>,
) -> Result<Vec<ComposerContextRecord>, E> {
    if values.len() > COMPOSER_CONTEXT_MAX_RECORDS {
        return Err(E::custom("too many context records"));
    }
    Ok(values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

pub(crate) fn deserialize_context_records<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ComposerContextRecord>, D::Error> {
    let values = <Vec<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    decode_context_records(values)
}

fn deserialize_message_records<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ComposerContextRecord>, D::Error> {
    let values = <Vec<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    let encoded = serde_json::to_string(&values).map_err(serde::de::Error::custom)?;
    if encoded.encode_utf16().count() > 16_000_000 {
        return Err(serde::de::Error::custom(
            "context records exceed the serialized size limit",
        ));
    }
    let records = decode_context_records::<D::Error>(values)?;
    let mut ids = std::collections::BTreeSet::new();
    if records
        .iter()
        .any(|record| !ids.insert(record.context_id()))
    {
        return Err(serde::de::Error::custom("duplicate context ids"));
    }
    Ok(records)
}

fn deserialize_unknown_payload<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<serde_json::Value, D::Error> {
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    let encoded = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
    if encoded.encode_utf16().count() > 64_000 {
        return Err(serde::de::Error::custom(
            "unknown context payload exceeds size limit",
        ));
    }
    Ok(value)
}
