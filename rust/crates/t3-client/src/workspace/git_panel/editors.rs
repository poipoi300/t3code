use super::*;
use crate::{
    connection::{ConnectionMode, ConnectionProfile, ConnectionStores},
    platform::{StorageArea, StorageKey, StorageWrite},
    rpc::RpcEndpoint,
    workspace::project_ops::editor::{EDITOR_ORDER, LAST_EDITOR_KEY, preferred_editor},
};
use futures::{future::LocalBoxFuture, stream};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    editor::{EditorId, LaunchEditorInput},
    rpc::WsShellOpenInEditorRpc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOpenState {
    LocalExec,
    RemoteLinks {
        host: String,
        username: Option<String>,
    },
    RemoteUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorOption {
    pub editor: EditorId,
    pub label: String,
    pub icon_identity: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorPickerSnapshot {
    pub options: Vec<EditorOption>,
    pub preferred: Option<EditorId>,
    pub remote: RemoteOpenState,
    pub can_open: bool,
    pub unavailable_reason: Option<String>,
    pub hint_seen: bool,
    pub shortcut: Option<String>,
    pub target_path: Option<String>,
    /// The environment's saved label, or "this machine"; OpenInPicker's copy interpolates it.
    pub environment_label: String,
}
#[derive(Clone)]
pub struct EditorPicker<P: Platform + Clone + 'static> {
    service: GitPanel<P>,
    key: ThreadKey,
}
fn preference_key() -> StorageKey {
    StorageKey::new("preferences", None, LAST_EDITOR_KEY)
}
fn hint_key() -> StorageKey {
    StorageKey::new("preferences", None, "t3code:remote-open-hint-seen")
}
pub fn editor_label(editor: EditorId, platform: &str) -> &'static str {
    use EditorId::*;
    match editor {
        Cursor => "Cursor",
        Trae => "Trae",
        Kiro => "Kiro",
        Vscode => "VS Code",
        VscodeInsiders => "VS Code Insiders",
        Vscodium => "VSCodium",
        Zed => "Zed",
        Antigravity => "Antigravity",
        Idea => "IntelliJ IDEA",
        Aqua => "Aqua",
        Clion => "CLion",
        Datagrip => "DataGrip",
        Dataspell => "DataSpell",
        Goland => "GoLand",
        Phpstorm => "PhpStorm",
        Pycharm => "PyCharm",
        Rider => "Rider",
        Rubymine => "RubyMine",
        Rustrover => "RustRover",
        Webstorm => "WebStorm",
        FileManager => {
            if crate::view_logic::shell::keybindings::is_mac_platform(platform) {
                "Finder"
            } else if platform.to_lowercase().contains("win") {
                "File Explorer"
            } else {
                "Files"
            }
        }
    }
}
pub fn should_show_open_in_picker(
    project_name: Option<&str>,
    environment: &EnvironmentId,
    primary: Option<&EnvironmentId>,
    host: Option<&EnvironmentId>,
    remote: &RemoteOpenState,
) -> bool {
    project_name.is_some_and(|n| !n.is_empty())
        && (Some(environment) == primary
            || Some(environment) == host
            || !matches!(remote, RemoteOpenState::LocalExec))
}
pub fn remote_fallback_editors(os: &str, mobile: bool) -> Vec<EditorId> {
    if mobile || matches!(os.to_lowercase().as_str(), "ios" | "android") {
        vec![]
    } else {
        vec![EditorId::Vscode]
    }
}
pub fn ssh_host(alias: &str, username: Option<&str>) -> (String, Option<String>) {
    let (inline, host) = alias
        .rsplit_once('@')
        .map_or((None, alias), |(u, h)| (Some(u), h));
    (
        host.into(),
        username
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .or(inline)
            .map(str::to_owned),
    )
}
fn encode(value: &str) -> String {
    let mut s = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            s.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(s, "%{byte:02X}");
        }
    }
    s
}
pub fn remote_open_url(
    editor: EditorId,
    host: &str,
    username: Option<&str>,
    path: &str,
    is_file: bool,
) -> Option<String> {
    use EditorId::*;
    let product = match editor {
        Idea => Some("IU"),
        Aqua => Some("QA"),
        Clion => Some("CL"),
        Datagrip => Some("DG"),
        Dataspell => Some("DS"),
        Goland => Some("GO"),
        Phpstorm => Some("PS"),
        Pycharm => Some("PY"),
        Rider => Some("RD"),
        Rubymine => Some("RM"),
        Rustrover => Some("RR"),
        Webstorm => Some("WS"),
        _ => None,
    };
    let path = path.replace('\\', "/");
    if let Some(product) = product {
        let mut params = url::form_urlencoded::Serializer::new(String::new());
        params
            .append_pair("h", host)
            .append_pair("launchIde", "true")
            .append_pair("ideHint", product)
            .append_pair("projectHint", &path);
        if let Some(user) = username {
            params.append_pair("u", user);
        }
        return Some(format!(
            "jetbrains://gateway/ssh/environment?{}",
            params.finish()
        ));
    }
    let scheme = match editor {
        Cursor => "cursor",
        Vscode => "vscode",
        VscodeInsiders => "vscode-insiders",
        Vscodium => "vscodium",
        Zed => "zed",
        _ => return None,
    };
    let rooted = if path.starts_with('/') {
        path.clone()
    } else {
        format!("/{path}")
    };
    let destination = username.map_or_else(|| host.to_owned(), |u| format!("{u}@{host}"));
    if editor == Zed {
        let rooted = if path.as_bytes().get(1) == Some(&b':')
            && path.as_bytes().get(2) == Some(&b'/')
            && path.as_bytes()[0].eq_ignore_ascii_case(&b'c')
        {
            &rooted[3..]
        } else {
            &rooted
        };
        let destination = username.map_or_else(
            || encode(host),
            |u| format!("{}@{}", encode(u), encode(host)),
        );
        return Some(format!(
            "zed://ssh/{destination}{}",
            rooted.split('/').map(encode).collect::<Vec<_>>().join("/")
        ));
    }
    let destination = if destination.contains('+') {
        let mut value = serde_json::json!({"hostName":host});
        if let Some(user) = username {
            value["user"] = serde_json::json!(user);
        }
        serde_json::to_string(&value)
            .ok()?
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    } else {
        destination
    };
    Some(format!(
        "{scheme}://vscode-remote/ssh-remote+{}{}{}",
        encode(&destination),
        rooted.split('/').map(encode).collect::<Vec<_>>().join("/"),
        if is_file { ":1:1" } else { "" }
    ))
}
impl<P: Platform + Clone + 'static> GitPanel<P> {
    pub fn editor_picker(&self, key: &ThreadKey) -> EditorPicker<P> {
        EditorPicker {
            service: self.clone(),
            key: key.clone(),
        }
    }
}
impl<P: Platform + Clone + 'static> EditorPicker<P> {
    async fn remote(&self) -> Result<RemoteOpenState, ClientError> {
        let entry = self
            .service
            .runtime
            .environments()
            .into_iter()
            .find(|e| e.id == self.key.environment)
            .ok_or(ClientError::Disconnected)?;
        let native = self.service.platform.external_url_surface()
            == crate::persistence::external_links::ExternalUrlSurface::Desktop;
        let local = entry.routes.iter().any(|r| {
            let loopback = url::Url::parse(&r.http_base)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .is_some_and(|h| {
                    h == "localhost" || h == "[::1]" || h == "::1" || h.starts_with("127.")
                });
            loopback || (native && r.mode == ConnectionMode::Local)
        });
        if local {
            return Ok(RemoteOpenState::LocalExec);
        }
        let stores = ConnectionStores::new(Rc::new(self.service.platform.clone()));
        for route in &entry.routes {
            if route.mode == ConnectionMode::Ssh
                && let Ok(Some(ConnectionProfile::Ssh { target, .. })) =
                    stores.profile(&self.key.environment, &route.id).await
            {
                let (host, username) = ssh_host(&target.alias, target.username.as_deref());
                return Ok(RemoteOpenState::RemoteLinks { host, username });
            }
        }
        let config =
            super::details::environment_config(self.service.runtime.store(), &self.key.environment);
        Ok(config
            .and_then(|c| {
                c.remote_open_targets
                    .as_ref()
                    .and_then(|t| t.0.first())
                    .cloned()
            })
            .map_or(RemoteOpenState::RemoteUnavailable, |t| {
                RemoteOpenState::RemoteLinks {
                    host: t.host.0,
                    username: t.username.map(|u| u.0),
                }
            }))
    }
    pub fn snapshot(&self) -> LocalBoxFuture<'static, Result<EditorPickerSnapshot, ClientError>> {
        let picker = self.clone();
        async move {
            let remote = picker.remote().await?;
            let config = super::details::environment_config(
                picker.service.runtime.store(),
                &picker.key.environment,
            );
            let identity = picker.service.platform.client_presentation().await.ok();
            let os = identity
                .as_ref()
                .and_then(|p| p.os.as_ref())
                .map(|o| o.0.as_str())
                .unwrap_or_default();
            let mobile = identity.as_ref().is_some_and(|p| {
                matches!(
                    p.device_type,
                    Some(
                        t3_contracts::auth::AuthClientMetadataDeviceType::Mobile
                            | t3_contracts::auth::AuthClientMetadataDeviceType::Tablet
                    )
                )
            });
            let available = if matches!(remote, RemoteOpenState::LocalExec) {
                config
                    .as_ref()
                    .map(|c| c.available_editors.0.clone())
                    .unwrap_or_default()
            } else {
                remote_fallback_editors(os, mobile)
            };
            let stored = picker
                .service
                .platform
                .get(StorageArea::Ordinary, preference_key())
                .await?
                .and_then(|b| serde_json::from_slice(&b).ok());
            let preferred = preferred_editor(&available, stored);
            let options = EDITOR_ORDER
                .iter()
                .filter(|e| available.contains(e))
                .map(|e| EditorOption {
                    editor: *e,
                    label: editor_label(*e, os).into(),
                    icon_identity: serde_json::to_value(e)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>();
            let target = picker
                .service
                .workspace_context(&picker.key, None)
                .map(|c| {
                    c.worktree_path
                        .unwrap_or_else(|| c.project.workspace_root.0.clone())
                });
            let local_allowed = target.as_ref().zip(preferred).is_some_and(|(cwd, editor)| {
                picker
                    .service
                    .session(&picker.key.environment)
                    .ok()
                    .is_some_and(|e| {
                        e.available::<WsShellOpenInEditorRpc>(&LaunchEditorInput {
                            cwd: TrimmedNonEmptyString(cwd.clone()),
                            editor,
                            project_root: None,
                            reveal: None,
                        })
                    })
            });
            let label = picker
                .service
                .runtime
                .environments()
                .into_iter()
                .find(|e| e.id == picker.key.environment)
                .map(|e| e.label)
                .unwrap_or_else(|| "this machine".into());
            let unavailable_reason = match remote {
                RemoteOpenState::RemoteUnavailable => Some(format!("No SSH route to {label}")),
                RemoteOpenState::RemoteLinks { .. } if options.is_empty() => {
                    Some(format!("Open from a computer to edit on {label}"))
                }
                _ => None,
            };
            let hint_seen = picker
                .service
                .platform
                .get(StorageArea::Ordinary, hint_key())
                .await?
                .and_then(|b| serde_json::from_slice::<bool>(&b).ok())
                .unwrap_or(false);
            let shortcut = config.as_ref().and_then(|c| {
                crate::view_logic::shell::keybindings::shortcut_label_for_command(
                    &c.keybindings.0,
                    Some("editor.openFavorite"),
                    os,
                    &Default::default(),
                )
            });
            Ok(EditorPickerSnapshot {
                options,
                preferred,
                can_open: target.is_some()
                    && preferred.is_some()
                    && match remote {
                        RemoteOpenState::LocalExec => local_allowed,
                        RemoteOpenState::RemoteLinks { .. } => true,
                        RemoteOpenState::RemoteUnavailable => false,
                    },
                remote,
                unavailable_reason,
                hint_seen,
                shortcut,
                target_path: target,
                environment_label: label,
            })
        }
        .boxed_local()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, Result<EditorPickerSnapshot, ClientError>> {
        let picker = self.clone();
        let changes = stream::select(
            stream::select(
                picker.service.changes(),
                picker.service.runtime.composer_drafts().changes(),
            ),
            picker.service.state.editor_changes.subscribe(),
        );
        stream::unfold(
            (true, picker, changes),
            |(first, picker, mut changes)| async move {
                if !first {
                    changes.next().await?;
                }
                let value = picker.snapshot().await;
                Some((value, (false, picker, changes)))
            },
        )
        .boxed_local()
    }
    pub fn select_editor(
        &self,
        editor: EditorId,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let picker = self.clone();
        async move {
            let snapshot = picker.snapshot().await?;
            if !snapshot.options.iter().any(|o| o.editor == editor) {
                return Err(ClientError::Cancelled);
            }
            picker
                .service
                .platform
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: preference_key(),
                        value: Some(serde_json::to_vec(&editor)?),
                    }],
                )
                .await?;
            picker.service.state.editor_changes.notify();
            Ok(())
        }
        .boxed_local()
    }
    pub fn open_editor(
        &self,
        editor: EditorId,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        let picker = self.clone();
        async move {
            let snapshot = picker.snapshot().await?;
            if !snapshot.can_open || !snapshot.options.iter().any(|o| o.editor == editor) {
                return Ok(false);
            }
            let Some(path) = snapshot.target_path else {
                return Ok(false);
            };
            match snapshot.remote {
                RemoteOpenState::RemoteUnavailable => Ok(false),
                RemoteOpenState::LocalExec => {
                    picker.select_editor(editor).await?;
                    picker
                        .service
                        .session(&picker.key.environment)?
                        .call::<WsShellOpenInEditorRpc>(LaunchEditorInput {
                            cwd: TrimmedNonEmptyString(path),
                            editor,
                            project_root: None,
                            reveal: None,
                        })
                        .await?;
                    Ok(true)
                }
                RemoteOpenState::RemoteLinks { host, username } => {
                    let Some(url) =
                        remote_open_url(editor, &host, username.as_deref(), &path, false)
                    else {
                        return Ok(false);
                    };
                    if picker.service.platform.open_url(url).await.is_err() {
                        return Ok(false);
                    }
                    picker.select_editor(editor).await?;
                    picker
                        .service
                        .platform
                        .write(
                            StorageArea::Ordinary,
                            vec![StorageWrite {
                                key: hint_key(),
                                value: Some(serde_json::to_vec(&true)?),
                            }],
                        )
                        .await?;
                    Ok(true)
                }
            }
        }
        .boxed_local()
    }
}
