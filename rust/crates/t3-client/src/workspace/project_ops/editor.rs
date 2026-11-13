use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
    rpc::{RpcCallError, RpcEndpoint},
    types::EnvironmentId,
};
use t3_contracts::{
    editor::{EditorId, LaunchEditorInput},
    rpc::WsShellOpenInEditorRpc,
};

pub const LAST_EDITOR_KEY: &str = "t3code:last-editor";
pub const EDITOR_ORDER: &[EditorId] = &[
    EditorId::Cursor,
    EditorId::Trae,
    EditorId::Kiro,
    EditorId::Vscode,
    EditorId::VscodeInsiders,
    EditorId::Vscodium,
    EditorId::Zed,
    EditorId::Antigravity,
    EditorId::Idea,
    EditorId::Aqua,
    EditorId::Clion,
    EditorId::Datagrip,
    EditorId::Dataspell,
    EditorId::Goland,
    EditorId::Phpstorm,
    EditorId::Pycharm,
    EditorId::Rider,
    EditorId::Rubymine,
    EditorId::Rustrover,
    EditorId::Webstorm,
    EditorId::FileManager,
];
pub fn preferred_editor(available: &[EditorId], stored: Option<EditorId>) -> Option<EditorId> {
    stored
        .filter(|editor| available.contains(editor))
        .or_else(|| {
            EDITOR_ORDER
                .iter()
                .find(|editor| available.contains(editor))
                .copied()
        })
}
fn preference_key() -> StorageKey {
    StorageKey::new("preferences", None, LAST_EDITOR_KEY)
}
#[derive(Debug, thiserror::Error)]
pub enum PreferredEditorError {
    #[error("Cannot open {target_path} because no environment is selected.")]
    EnvironmentRequired { target_path: String },
    #[error("No available editor can open {target_path} in environment {environment:?}.")]
    Unavailable {
        environment: EnvironmentId,
        target_path: String,
        available_editor_ids: Vec<EditorId>,
    },
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Rpc(#[from] RpcCallError),
}
pub async fn open_in_preferred_editor(
    environment: Option<&EnvironmentId>,
    available: &[EditorId],
    project_root: Option<&str>,
    target_path: &str,
    endpoint: &impl RpcEndpoint,
    storage: &impl Storage,
    can_operate: impl Fn(&EnvironmentId) -> bool,
) -> Result<EditorId, PreferredEditorError> {
    let environment = environment.ok_or_else(|| PreferredEditorError::EnvironmentRequired {
        target_path: target_path.into(),
    })?;
    super::source_control::check_destination(endpoint, environment)?;
    let authorize = || {
        if can_operate(environment) {
            Ok(())
        } else {
            Err(ClientError::PermissionDenied {
                method: "shell.openInEditor".into(),
            })
        }
    };
    authorize()?;
    let stored = storage
        .get(StorageArea::Ordinary, preference_key())
        .await
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<EditorId>(&bytes).ok());
    authorize()?;
    let editor =
        preferred_editor(available, stored).ok_or_else(|| PreferredEditorError::Unavailable {
            environment: environment.clone(),
            target_path: target_path.into(),
            available_editor_ids: available.to_vec(),
        })?;
    if stored != Some(editor) {
        let _ = storage
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: preference_key(),
                    value: Some(serde_json::to_vec(&editor).map_err(ClientError::from)?),
                }],
            )
            .await;
    }
    authorize()?;
    endpoint
        .command::<WsShellOpenInEditorRpc>(LaunchEditorInput {
            cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target_path.into()),
            editor,
            project_root: project_root
                .filter(|s| !s.is_empty())
                .map(|s| Some(t3_contracts::base_schemas::TrimmedNonEmptyString(s.into()))),
            reveal: None,
        })
        .await?;
    Ok(editor)
}
