#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectLucideIconWithLegacyMonogramKind {
    #[serde(rename = "lucide")]
    Lucide,
}

pub type ProjectLucideIconName = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectIconColor {
    #[serde(rename = "gray")]
    Gray,
    #[serde(rename = "red")]
    Red,
    #[serde(rename = "orange")]
    Orange,
    #[serde(rename = "amber")]
    Amber,
    #[serde(rename = "yellow")]
    Yellow,
    #[serde(rename = "lime")]
    Lime,
    #[serde(rename = "green")]
    Green,
    #[serde(rename = "emerald")]
    Emerald,
    #[serde(rename = "teal")]
    Teal,
    #[serde(rename = "cyan")]
    Cyan,
    #[serde(rename = "sky")]
    Sky,
    #[serde(rename = "blue")]
    Blue,
    #[serde(rename = "indigo")]
    Indigo,
    #[serde(rename = "violet")]
    Violet,
    #[serde(rename = "purple")]
    Purple,
    #[serde(rename = "fuchsia")]
    Fuchsia,
    #[serde(rename = "pink")]
    Pink,
    #[serde(rename = "rose")]
    Rose,
}

pub type ProjectMonogramText = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLucideIconWithLegacyMonogram {
    pub kind: ProjectLucideIconWithLegacyMonogramKind,
    pub name: ProjectLucideIconName,
    pub color: ProjectIconColor,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub monogram_text: Option<Option<ProjectMonogramText>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub monogram: Option<Option<ProjectMonogramText>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectEmojiIconKind {
    #[serde(rename = "emoji")]
    Emoji,
}

pub type ProjectEmoji = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEmojiIcon {
    pub kind: ProjectEmojiIconKind,
    pub emoji: ProjectEmoji,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectMonogramIconKind {
    #[serde(rename = "monogram")]
    Monogram,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMonogramIcon {
    pub kind: ProjectMonogramIconKind,
    pub text: ProjectMonogramText,
    pub color: ProjectIconColor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ReceivedProjectIconKnown {
    Lucide(ProjectLucideIconWithLegacyMonogram),
    Emoji(ProjectEmojiIcon),
    Monogram(ProjectMonogramIcon),
}

impl crate::wire::TaggedUnion for ReceivedProjectIconKnown {
    const TAG: &'static str = "kind";
    const KNOWN_TAGS: &'static [&'static str] = &["lucide", "emoji", "monogram"];
}
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(transparent)]
pub struct ReceivedProjectIcon(
    #[serde(deserialize_with = "deserialize_received_icon")] pub Option<ProjectIconOverride>,
);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectScriptIcon {
    #[serde(rename = "play")]
    Play,
    #[serde(rename = "test")]
    Test,
    #[serde(rename = "lint")]
    Lint,
    #[serde(rename = "configure")]
    Configure,
    #[serde(rename = "build")]
    Build,
    #[serde(rename = "debug")]
    Debug,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScript {
    pub id: crate::base_schemas::TrimmedNonEmptyString,
    pub name: crate::base_schemas::TrimmedNonEmptyString,
    pub command: crate::base_schemas::TrimmedNonEmptyString,
    pub icon: ProjectScriptIcon,
    pub run_on_worktree_create: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_on_settle: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub stop_on_settle: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub r#async: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub preview_url: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_open_preview: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: crate::base_schemas::ProjectId,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub repository_identity: Option<Option<crate::environment::RepositoryIdentity>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub favicon_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::project::optional_received_icon"
    )]
    pub project_icon: Option<Option<ReceivedProjectIcon>>,
    #[serde(with = "crate::wire::nullable")]
    pub default_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_pull: Option<Option<bool>>,
    pub scripts: Vec<ProjectScript>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pinned_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub archived_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub updated_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub deleted_at: Option<crate::base_schemas::IsoDateTime>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContentMatchRange {
    pub start: crate::base_schemas::NonNegativeInt,
    pub end: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContentMatch {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub line_number: crate::base_schemas::PositiveInt,
    pub line_content: String,
    pub match_ranges: Vec<ProjectContentMatchRange>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateFromThreadInput {
    pub thread_id: crate::base_schemas::ThreadId,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateFromThreadResult {
    pub project_id: crate::base_schemas::ProjectId,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateNewInput {
    pub name: crate::base_schemas::TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateNewResult {
    pub project_id: crate::base_schemas::ProjectId,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub commit_error: Option<crate::base_schemas::TrimmedNonEmptyString>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreatePayload {
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub create_workspace_root_if_missing: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scripts: Option<Option<Vec<ProjectScript>>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEnsureScratchResult {
    pub project_id: crate::base_schemas::ProjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectEntriesFailure {
    #[serde(rename = "workspace_root_not_found")]
    WorkspaceRootNotFound,
    #[serde(rename = "workspace_root_create_failed")]
    WorkspaceRootCreateFailed,
    #[serde(rename = "workspace_root_stat_failed")]
    WorkspaceRootStatFailed,
    #[serde(rename = "workspace_root_not_directory")]
    WorkspaceRootNotDirectory,
    #[serde(rename = "search_index_create_failed")]
    SearchIndexCreateFailed,
    #[serde(rename = "search_index_scan_timed_out")]
    SearchIndexScanTimedOut,
    #[serde(rename = "search_index_search_failed")]
    SearchIndexSearchFailed,
    #[serde(rename = "directory_list_failed")]
    DirectoryListFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectEntryKind {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "directory")]
    Directory,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub kind: ProjectEntryKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ignored: Option<Option<bool>>,
}

pub type ProjectFaviconPath = crate::base_schemas::TrimmedNonEmptyString;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectFileFailure {
    #[serde(rename = "workspace_path_outside_root")]
    WorkspacePathOutsideRoot,
    #[serde(rename = "resolved_path_outside_root")]
    ResolvedPathOutsideRoot,
    #[serde(rename = "path_not_file")]
    PathNotFile,
    #[serde(rename = "binary_file")]
    BinaryFile,
    #[serde(rename = "operation_failed")]
    OperationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectFileOperation {
    #[serde(rename = "realpath-workspace-root")]
    RealpathWorkspaceRoot,
    #[serde(rename = "realpath-target")]
    RealpathTarget,
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "stat")]
    Stat,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "close")]
    Close,
    #[serde(rename = "make-directory")]
    MakeDirectory,
    #[serde(rename = "write-file")]
    WriteFile,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectLucideIconKind {
    #[serde(rename = "lucide")]
    Lucide,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLucideIcon {
    pub kind: ProjectLucideIconKind,
    pub name: ProjectLucideIconName,
    pub color: ProjectIconColor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProjectIconOverride {
    Lucide(ProjectLucideIcon),
    Emoji(ProjectEmojiIcon),
    Monogram(ProjectMonogramIcon),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIgnoredFile {
    pub path: crate::base_schemas::TrimmedNonEmptyString,
    pub byte_length: crate::base_schemas::NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectListEntriesErrorTag {
    #[serde(rename = "ProjectListEntriesError")]
    ProjectListEntriesError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListEntriesError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectListEntriesErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<ProjectEntriesFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub normalized_cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListEntriesInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub directory_path: Option<Option<crate::base_schemas::TrimmedString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListEntriesResult {
    pub entries: Vec<ProjectEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListIgnoredFilesInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        rename = "query",
        default,
        skip_serializing_if = "Option::is_none",
        with = "self::ignored_file_query"
    )]
    pub query: Option<crate::base_schemas::TrimmedString>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListIgnoredFilesResult {
    pub files: Vec<ProjectIgnoredFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectMutationMember1Type {
    #[serde(rename = "project.create")]
    ProjectCreate,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMutationMember1 {
    pub r#type: ProjectMutationMember1Type,
    pub command_id: crate::base_schemas::CommandId,
    pub project_id: crate::base_schemas::ProjectId,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub create_workspace_root_if_missing: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scripts: Option<Option<Vec<ProjectScript>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectMutationMember2Type {
    #[serde(rename = "project.update")]
    ProjectUpdate,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMutationMember2 {
    pub r#type: ProjectMutationMember2Type,
    pub command_id: crate::base_schemas::CommandId,
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_root: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_pull: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub project_icon: Option<Option<ProjectIconOverride>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub favicon_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scripts: Option<Option<Vec<ProjectScript>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pinned: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub archived: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectMutationMember3Type {
    #[serde(rename = "project.delete")]
    ProjectDelete,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMutationMember3 {
    pub r#type: ProjectMutationMember3Type,
    pub command_id: crate::base_schemas::CommandId,
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub force: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProjectMutation {
    Create(ProjectMutationMember1),
    Update(ProjectMutationMember2),
    Delete(ProjectMutationMember3),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectMutationErrorTag {
    #[serde(rename = "ProjectMutationError")]
    ProjectMutationError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMutationError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectMutationErrorTag,
    pub command_id: crate::base_schemas::CommandId,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectReadFileErrorTag {
    #[serde(rename = "ProjectReadFileError")]
    ProjectReadFileError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReadFileError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectReadFileErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub relative_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<ProjectFileFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resolved_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resolved_workspace_root: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub operation: Option<Option<ProjectFileOperation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub operation_path: Option<Option<String>>,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReadFileInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub relative_path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_missing: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReadFileResult {
    pub relative_path: String,
    pub contents: String,
    pub byte_length: crate::base_schemas::NonNegativeInt,
    pub truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub exists: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectSearchContentsErrorTag {
    #[serde(rename = "ProjectSearchContentsError")]
    ProjectSearchContentsError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchContentsError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectSearchContentsErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query_length: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<ProjectEntriesFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub normalized_cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchContentsInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub query: String,
    pub limit: crate::base_schemas::PositiveInt,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchContentsResult {
    pub matches: Vec<ProjectContentMatch>,
    pub truncated: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub regex_fallback_error: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectSearchEntriesErrorTag {
    #[serde(rename = "ProjectSearchEntriesError")]
    ProjectSearchEntriesError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchEntriesError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectSearchEntriesErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub query_length: Option<Option<crate::base_schemas::NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub limit: Option<Option<crate::base_schemas::PositiveInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<ProjectEntriesFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub normalized_cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub timeout: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub detail: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchEntriesInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub query: crate::base_schemas::TrimmedString,
    pub limit: crate::base_schemas::PositiveInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub kind: Option<Option<ProjectEntryKind>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub image_only: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchEntriesResult {
    pub entries: Vec<ProjectEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub projects: Vec<Project>,
    pub updated_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdatePayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_root: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_pull: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub project_icon: Option<Option<ProjectIconOverride>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub favicon_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scripts: Option<Option<Vec<ProjectScript>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub pinned: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub archived: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectWriteFileErrorTag {
    #[serde(rename = "ProjectWriteFileError")]
    ProjectWriteFileError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWriteFileError {
    #[serde(rename = "_tag")]
    pub _tag: ProjectWriteFileErrorTag,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cwd: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub relative_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub failure: Option<Option<ProjectFileFailure>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resolved_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub resolved_workspace_root: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub operation: Option<Option<ProjectFileOperation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub operation_path: Option<Option<String>>,
    pub message: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub cause: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWriteFileInput {
    pub cwd: crate::base_schemas::TrimmedNonEmptyString,
    pub relative_path: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWriteFileResult {
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "ReceivedProjectIconKnown", into = "ProjectIconOverride")]
pub struct StoredProjectIcon(pub ProjectIconOverride);

impl From<ReceivedProjectIconKnown> for ProjectIconOverride {
    fn from(icon: ReceivedProjectIconKnown) -> Self {
        match icon {
            ReceivedProjectIconKnown::Lucide(icon) => {
                match icon.monogram_text.flatten().or(icon.monogram.flatten()) {
                    Some(text) => Self::Monogram(ProjectMonogramIcon {
                        kind: ProjectMonogramIconKind::Monogram,
                        text,
                        color: icon.color,
                    }),
                    None => Self::Lucide(ProjectLucideIcon {
                        kind: ProjectLucideIconKind::Lucide,
                        name: icon.name,
                        color: icon.color,
                    }),
                }
            }
            ReceivedProjectIconKnown::Emoji(icon) => Self::Emoji(icon),
            ReceivedProjectIconKnown::Monogram(icon) => Self::Monogram(icon),
        }
    }
}
impl From<ReceivedProjectIconKnown> for StoredProjectIcon {
    fn from(icon: ReceivedProjectIconKnown) -> Self {
        Self(icon.into())
    }
}
impl From<StoredProjectIcon> for ProjectIconOverride {
    fn from(icon: StoredProjectIcon) -> Self {
        icon.0
    }
}
fn deserialize_received_icon<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ProjectIconOverride>, D::Error> {
    let icon = <crate::wire::ForwardCompatible<ReceivedProjectIconKnown> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(match icon {
        crate::wire::ForwardCompatible::Known(icon) => Some(icon.into()),
        crate::wire::ForwardCompatible::Unknown(_) => None,
    })
}

pub const PROJECT_SEARCH_CONTENTS_QUERY_MAX_LENGTH: usize = 256;

impl serde::Serialize for ReceivedProjectIcon {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Some(icon) => serde::Serialize::serialize(icon, serializer),
            None => Err(serde::ser::Error::custom(
                "unknown project icons cannot be sent",
            )),
        }
    }
}

pub(crate) mod optional_received_icon {
    use super::ReceivedProjectIcon;
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<ReceivedProjectIcon>>, D::Error> {
        let icon = <Option<ReceivedProjectIcon> as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Some(icon.filter(|icon| icon.0.is_some())))
    }
    pub use crate::wire::double_option::serialize;
}

mod ignored_file_query {
    use crate::base_schemas::TrimmedString;
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(try_from = "TrimmedString")]
    struct Query(TrimmedString);
    impl TryFrom<TrimmedString> for Query {
        type Error = &'static str;
        fn try_from(value: TrimmedString) -> Result<Self, Self::Error> {
            if value.0.encode_utf16().count() > 256 {
                Err("ignored-file query exceeds 256 UTF-16 units")
            } else {
                Ok(Self(value))
            }
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<TrimmedString>, D::Error> {
        crate::wire::optional::deserialize::<D, Query>(d).map(|v| v.map(|v| v.0))
    }
    pub use crate::wire::optional::serialize;
}
