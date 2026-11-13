#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationProjectShell {
    pub id: crate::base_schemas::ProjectId,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub has_custom_title: Option<Option<bool>>,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub repository_identity: Option<Option<crate::environment::RepositoryIdentity>>,
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
    pub project_icon: Option<Option<crate::project::ReceivedProjectIcon>>,
    pub scripts: Vec<crate::project::ProjectScript>,
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
}
