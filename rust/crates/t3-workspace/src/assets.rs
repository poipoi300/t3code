use crate::{
    Error, Result, attachments, blocking,
    filesystem::open_read,
    io_error,
    paths::{self, InternalPaths},
    project::FaviconResolver,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    future::Future,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use t3_contracts::{assets::*, base_schemas::TrimmedNonEmptyString as Text};

pub const ICON_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const TOOL_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;

const URL_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Supply the bytes from the server's `asset-access-signing-key` secret; never stores keys here.
#[derive(Clone)]
pub struct AssetSigner {
    key: Arc<[u8]>,
}
impl AssetSigner {
    pub fn new(key: impl Into<Vec<u8>>) -> Result<Self> {
        let key = key.into();
        if key.len() < 32 {
            return Err(Error::Invalid(
                "asset signing key must contain at least 32 bytes".into(),
            ));
        }
        Ok(Self { key: key.into() })
    }
    pub fn sign(&self, payload: &str) -> Result<String> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).map_err(|e| Error::Invalid(e.to_string()))?;
        mac.update(payload.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
    pub fn verify(&self, payload: &str, signature: &str) -> bool {
        let Ok(tag) = URL_SAFE_NO_PAD.decode(signature) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.key) else {
            return false;
        };
        mac.update(payload.as_bytes());
        mac.verify_slice(&tag).is_ok()
    }
    pub fn sign_json<T: Serialize>(&self, claims: &T) -> Result<String> {
        let payload = attachments::encoded_payload(claims)?;
        Ok(format!("{payload}.{}", self.sign(&payload)?))
    }
    pub fn verify_json<T: DeserializeOwned>(&self, token: &str) -> Option<T> {
        let (payload, signature) = token.split_once('.')?;
        if signature.contains('.') || !self.verify(payload, signature) {
            return None;
        }
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum AssetGrant {
    WorkspaceFile {
        workspace_root: String,
        base_relative_path: String,
    },
    WorkspaceFileExact {
        workspace_root: String,
        relative_path: String,
    },
    MediaFileExact {
        file_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        download: Option<bool>,
        device: String,
        inode: String,
    },
    Attachment {
        attachment_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        download: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_intent: Option<String>,
    },
    ProjectFavicon {
        workspace_root: String,
        relative_path: Option<String>,
    },
    ProjectFaviconExternal {
        file_path: String,
    },
    ToolOutputImage {
        thread_id: String,
        item_id: String,
        index: u64,
    },
    NativeAppIcon {
        app: NativeAppIconReference,
    },
    #[serde(rename = "github-media")]
    SourceControlMedia {
        url: String,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetClaims {
    version: u8,
    #[serde(flatten)]
    grant: AssetGrant,
    expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceControlMedia {
    pub fetch_url: String,
    pub file_name: String,
}
pub type AssetFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
/// External capabilities owned by orchestration, native shell, and source-control services.
pub trait ExternalAssets: Send + Sync {
    fn tool_image<'a>(
        &'a self,
        thread: &'a str,
        item: &'a str,
        index: u64,
    ) -> AssetFuture<'a, Option<(String, Vec<u8>)>>;
    fn native_icon<'a>(
        &'a self,
        app: &'a NativeAppIconReference,
    ) -> AssetFuture<'a, Option<PathBuf>>;
    fn validate_source_control<'a>(
        &'a self,
        cwd: &'a Path,
        url: &'a str,
        repository: Option<&'a str>,
    ) -> AssetFuture<'a, Option<SourceControlMedia>>;
}
#[derive(Default)]
pub struct AssetContext<'a> {
    pub workspace_root: Option<&'a Path>,
    pub project_favicon_path: Option<&'a str>,
    pub checkout: Checkout,
}
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Checkout {
    Pending,
    Cloned,
    #[default]
    Ready,
}

pub enum ResolvedAsset {
    File {
        path: PathBuf,
        download: bool,
        file_name: Option<String>,
        mime_type: Option<String>,
        file: OpenMediaFile,
        render_intent: Option<String>,
    },
    Bytes {
        bytes: Vec<u8>,
        mime_type: String,
    },
    SourceControlMedia {
        url: String,
        cwd: PathBuf,
        repository: Option<String>,
        expires_at: u64,
    },
}
fn file_asset(path: PathBuf) -> Option<ResolvedAsset> {
    let file = OpenMediaFile::open(&path, None).ok().flatten()?;
    Some(ResolvedAsset::File {
        path,
        download: false,
        file_name: None,
        mime_type: None,
        file,
        render_intent: None,
    })
}

#[derive(Clone)]
pub struct AssetAccess {
    pub signer: AssetSigner,
    pub attachments_dir: PathBuf,
    pub home: PathBuf,
    pub internal: InternalPaths,
    pub favicons: Arc<FaviconResolver>,
    pub external: Arc<dyn ExternalAssets>,
}
impl AssetAccess {
    pub async fn issue_url(
        &self,
        resource: AssetResource,
        context: AssetContext<'_>,
        now_ms: u64,
    ) -> Result<AssetCreateUrlResult> {
        let mut expires_at = now_ms + 60 * 60_000;
        let mut source_path = None;
        let mut revision = None;
        let mut inline_dimensions = None;
        let (grant, name) = match resource {
            AssetResource::Attachment {
                attachment_id,
                file_name,
                mime_type,
                disposition,
                render_intent,
            } => {
                let path = attachment_file(&self.attachments_dir, &attachment_id.0, &self.internal)
                    .ok_or(Error::AssetDenied)?;
                let extension = attachments::parse_id(&attachment_id.0).and_then(|p| p.extension);
                let supplied_mime = mime_type.map(|m| m.0);
                let video = supplied_mime
                    .as_deref()
                    .and_then(|m| m.split(';').next())
                    .map(str::trim)
                    .filter(|m| {
                        regex::Regex::new(r"(?i)^video/[\w!#$&^.+-]+$").is_ok_and(|r| r.is_match(m))
                    })
                    .map(str::to_owned);
                let inline = if disposition == Some(AssetDisposition::Inline) {
                    extension
                        .as_ref()
                        .and_then(|ext| preview_mime(ext))
                        .map(str::to_owned)
                } else {
                    None
                };
                let download = extension.is_some() && video.is_none() && inline.is_none();
                // The type comes from the stored extension; a client's type is kept only for
                // video playback and for downloads, which never render.
                let mime = inline
                    .clone()
                    .or(video)
                    .or(supplied_mime.filter(|_| download));
                let render = (render_intent == Some(AssetRenderIntent::McpApp)
                    && inline.as_deref() == Some("text/html"))
                .then(|| "mcp-app".into());
                if mime.as_deref() == Some("text/html") {
                    expires_at = expires_at.div_ceil(30 * 60_000) * (30 * 60_000);
                }
                let name = file_name
                    .as_ref()
                    .map(|s| s.0.clone())
                    .unwrap_or_else(|| basename(&path));
                (
                    AssetGrant::Attachment {
                        attachment_id: attachment_id.0,
                        download: download.then_some(true),
                        file_name: file_name.map(|n| n.0),
                        mime_type: mime,
                        render_intent: render,
                    },
                    name,
                )
            }
            AssetResource::WorkspaceFile { path, .. } => {
                let root = context.workspace_root.ok_or(Error::AssetDenied)?;
                self.workspace_grant(root, &path.0, expires_at)?
            }
            AssetResource::DraftWorkspaceFile { cwd, path } => {
                let root = context.workspace_root.unwrap_or_else(|| Path::new(&cwd.0));
                let host = paths::expand_home(&path.0, &self.home);
                if host.is_absolute() {
                    let (grant, name, rev) = self.media_grant(&host, Some(root), false, None)?;
                    revision = Some(rev);
                    (grant, name)
                } else if is_audio_video(&path.0) {
                    let canonical = canonical_workspace_file(root, &path.0, &self.internal)
                        .ok_or(Error::AssetDenied)?;
                    #[cfg(test)]
                    draft_hook();
                    let (grant, name, rev) =
                        self.media_grant(&canonical, Some(root), false, Some(&canonical))?;
                    revision = Some(rev);
                    (grant, name)
                } else {
                    self.workspace_grant(root, &path.0, expires_at)?
                }
            }
            AssetResource::MediaFile {
                path, disposition, ..
            } => {
                let requested = paths::expand_home(&path.0, &self.home);
                let requested = if requested.is_absolute() {
                    requested
                } else {
                    context
                        .workspace_root
                        .ok_or(Error::AssetDenied)?
                        .join(requested)
                };
                let (grant, name, rev) = self.media_grant(
                    &requested,
                    context.workspace_root,
                    disposition.is_some(),
                    None,
                )?;
                revision = Some(rev);
                (grant, name)
            }
            AssetResource::ProjectFavicon { cwd, .. } => {
                let root = paths::normalize_root(&cwd.0, &self.home, false).await?;
                if context.checkout == Checkout::Cloned {
                    self.favicons.invalidate(&root).await;
                }
                let found = if context.checkout == Checkout::Pending {
                    None
                } else {
                    self.favicons
                        .resolve(&cwd.0, context.project_favicon_path)
                        .await?
                };
                let canonical_root = paths::canonicalize(&root)
                    .map_err(|e| io_error("realpath-workspace-root", &root, e))?;
                let external = context
                    .project_favicon_path
                    .is_some_and(|s| Path::new(s).is_absolute())
                    && found.as_ref().is_some_and(|p| {
                        Some(p.as_path()) == context.project_favicon_path.map(Path::new)
                    });
                let (grant, name) = if let Some(found) = found {
                    if !crate::search::is_image_path(&found.to_string_lossy()) {
                        return Err(Error::AssetDenied);
                    }
                    let path = if external {
                        canonical_file(&found, &self.internal)
                    } else {
                        canonical_workspace_file(
                            &root,
                            &found
                                .strip_prefix(&root)
                                .map_err(|_| Error::AssetDenied)?
                                .to_string_lossy(),
                            &self.internal,
                        )
                    }
                    .ok_or(Error::AssetDenied)?;
                    let source = if external {
                        found.to_string_lossy().into_owned()
                    } else {
                        paths::display_path(
                            found.strip_prefix(&root).map_err(|_| Error::AssetDenied)?,
                        )
                    };
                    source_path = Some(Some(Text(source.clone())));
                    let mut opened = OpenMediaFile::open(&path, None)?.ok_or(Error::AssetDenied)?;
                    let info = opened
                        .file
                        .metadata()
                        .map_err(|e| io_error("stat", &path, e))?;
                    if info.len() > ICON_MAX_BYTES {
                        return Err(Error::AssetDenied);
                    }
                    let bytes = opened.read_range(0, ICON_MAX_BYTES)?;
                    let name = format!(
                        "v{}-{}",
                        attachments::hex(&Sha256::digest(bytes)),
                        basename(&found)
                    );
                    (
                        if external {
                            AssetGrant::ProjectFaviconExternal {
                                file_path: path.to_string_lossy().into(),
                            }
                        } else {
                            AssetGrant::ProjectFavicon {
                                workspace_root: canonical_root.to_string_lossy().into(),
                                relative_path: Some(source),
                            }
                        },
                        name,
                    )
                } else {
                    (
                        AssetGrant::ProjectFavicon {
                            workspace_root: canonical_root.to_string_lossy().into(),
                            relative_path: None,
                        },
                        "project-favicon-missing".into(),
                    )
                };
                expires_at = (now_ms / (30 * 60_000) + 2) * (30 * 60_000);
                (grant, name)
            }
            AssetResource::NativeAppIcon { app } => {
                let path = self
                    .external
                    .native_icon(&app)
                    .await?
                    .ok_or(Error::AssetDenied)?;
                let _ = path;
                (
                    AssetGrant::NativeAppIcon { app },
                    "native-app-icon.png".into(),
                )
            }
            AssetResource::ToolOutputImage {
                thread_id,
                item_id,
                index,
            } => {
                let Some((mime, bytes)) = self
                    .external
                    .tool_image(&thread_id.0, &item_id.0, index)
                    .await?
                else {
                    return Err(Error::AssetDenied);
                };
                if bytes.len() > TOOL_IMAGE_MAX_BYTES {
                    return Err(Error::AssetDenied);
                }
                inline_dimensions = crate::image::dimensions(&bytes).map(Some);
                (
                    AssetGrant::ToolOutputImage {
                        thread_id: thread_id.0,
                        item_id: item_id.0,
                        index,
                    },
                    format!(
                        "image-{}.{}",
                        index + 1,
                        mime.strip_prefix("image/").unwrap_or("bin")
                    ),
                )
            }
            AssetResource::SourceControlMedia {
                cwd,
                url,
                repository,
            } => {
                let media = self
                    .external
                    .validate_source_control(
                        Path::new(&cwd.0),
                        &url.0,
                        repository.as_ref().map(|r| r.0.as_str()),
                    )
                    .await?
                    .ok_or(Error::AssetDenied)?;
                let name = media.file_name;
                (
                    AssetGrant::SourceControlMedia {
                        url: media.fetch_url,
                        cwd: cwd.0,
                        repository: repository.map(|s| s.0),
                    },
                    name,
                )
            }
        };
        let dimensions_path = match &grant {
            AssetGrant::MediaFileExact { file_path, .. } => Some(PathBuf::from(file_path)),
            AssetGrant::WorkspaceFileExact {
                workspace_root,
                relative_path,
            } => canonical_workspace_file(Path::new(workspace_root), relative_path, &self.internal),
            AssetGrant::Attachment { attachment_id, .. }
                if attachments::parse_id(attachment_id)
                    .is_some_and(|id| id.extension.is_none()) =>
            {
                attachment_file(&self.attachments_dir, attachment_id, &self.internal)
            }
            _ => None,
        };
        let image_dimensions = if inline_dimensions.is_some() {
            inline_dimensions
        } else if let Some(path) = dimensions_path.filter(|p| {
            matches!(
                extension(&p.to_string_lossy()).as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
        }) {
            blocking(move || {
                let Some(mut file) = OpenMediaFile::open(&path, None)? else {
                    return Ok(None);
                };
                Ok(crate::image::dimensions(
                    &file.read_range(0, crate::image::HEADER_BYTES)?,
                ))
            })
            .await
            .ok()
            .flatten()
            .map(Some)
        } else {
            None
        };
        let claims = AssetClaims {
            version: 1,
            grant,
            expires_at,
        };
        let token = self.signer.sign_json(&claims)?;
        let name = percent_encoding::utf8_percent_encode(&name, URL_COMPONENT).to_string();
        Ok(AssetCreateUrlResult {
            relative_url: Text(format!("/api/assets/{token}/{name}")),
            expires_at: serde_json::from_value(expires_at.into())
                .map_err(|e| Error::Invalid(e.to_string()))?,
            source_path,
            image_dimensions,
            file_revision: revision.map(|s| Some(Text(s))),
        })
    }
    fn workspace_grant(
        &self,
        root: &Path,
        requested: &str,
        _expires: u64,
    ) -> Result<(AssetGrant, String)> {
        let root = paths::absolute(root)?;
        let relative = if Path::new(requested).is_absolute() {
            paths::display_path(&paths::relative_to(
                &paths::absolute(Path::new(requested))?,
                &root,
            )?)
        } else {
            requested.into()
        };
        let (_, relative) = paths::resolve_relative(&root, &relative)?;
        let ext = extension(&relative);
        if preview_mime(&ext).is_none() || is_audio_video(&relative) {
            return Err(Error::AssetDenied);
        }
        canonical_workspace_file(&root, &relative, &self.internal).ok_or(Error::AssetDenied)?;
        let real_root = paths::canonicalize(&root)
            .map_err(|e| io_error("realpath-workspace-root", &root, e))?
            .to_string_lossy()
            .into_owned();
        let name = basename(Path::new(&relative));
        let grant = if ["html", "htm"].contains(&ext.as_str()) {
            let base = Path::new(&relative)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            AssetGrant::WorkspaceFile {
                workspace_root: real_root,
                base_relative_path: paths::display_path(base),
            }
        } else {
            AssetGrant::WorkspaceFileExact {
                workspace_root: real_root,
                relative_path: relative,
            }
        };
        Ok((grant, name))
    }
    fn media_grant(
        &self,
        requested: &Path,
        root: Option<&Path>,
        download: bool,
        expected: Option<&Path>,
    ) -> Result<(AssetGrant, String, String)> {
        let path = paths::absolute(requested)?;
        if paths::is_network_path(&requested.to_string_lossy())
            && !root.is_some_and(|r| paths::absolute(r).is_ok_and(|r| paths::within(&path, &r)))
        {
            return Err(Error::AssetDenied);
        }
        let canonical = canonical_file(&path, &self.internal).ok_or(Error::AssetDenied)?;
        if expected.is_some_and(|expected| expected != canonical) {
            return Err(Error::AssetDenied);
        }
        if !download && preview_mime(&extension(&canonical.to_string_lossy())).is_none() {
            return Err(Error::AssetDenied);
        }
        let file = OpenMediaFile::open(&canonical, None)?.ok_or(Error::AssetDenied)?;
        let identity = file.identity.clone();
        let info = file
            .file
            .metadata()
            .map_err(|e| io_error("stat", &canonical, e))?;
        let modified = info
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|t| t.as_nanos())
            .unwrap_or(0);
        let revision = format!(
            "{}:{}:{}:{modified}",
            identity.device,
            identity.inode,
            info.len()
        );
        let name = basename(&canonical);
        Ok((
            AssetGrant::MediaFileExact {
                file_path: canonical.to_string_lossy().into(),
                download: download.then_some(true),
                device: identity.device,
                inode: identity.inode,
            },
            name,
            revision,
        ))
    }
    pub async fn resolve(
        &self,
        token: &str,
        relative_path: &str,
        now_ms: u64,
    ) -> Result<Option<ResolvedAsset>> {
        let Some(claims) = self.signer.verify_json::<AssetClaims>(token) else {
            return Ok(None);
        };
        if claims.version != 1 || claims.expires_at <= now_ms {
            return Ok(None);
        }
        let decoded = decode_component(relative_path);
        let asset = match claims.grant {
            AssetGrant::Attachment {
                attachment_id,
                download,
                file_name,
                mime_type,
                render_intent,
            } => attachment_file(&self.attachments_dir, &attachment_id, &self.internal).and_then(
                |path| {
                    let file = OpenMediaFile::open(&path, None).ok().flatten()?;
                    Some(ResolvedAsset::File {
                        path,
                        download: download.unwrap_or(false),
                        file_name,
                        mime_type,
                        render_intent,
                        file,
                    })
                },
            ),
            AssetGrant::WorkspaceFileExact {
                workspace_root,
                relative_path,
            } => {
                if decoded.as_deref() != Some(&basename(Path::new(&relative_path))) {
                    return Ok(None);
                }
                canonical_granted_file(Path::new(&workspace_root), &relative_path, &self.internal)
                    .and_then(file_asset)
            }
            AssetGrant::WorkspaceFile {
                workspace_root,
                base_relative_path,
            } => {
                let Some(decoded) = decoded else {
                    return Ok(None);
                };
                if decoded.is_empty()
                    || decoded.contains('\0')
                    || decoded.split(['/', '\\']).any(|s| s.starts_with('.'))
                    || !dependency_extension(&extension(&decoded))
                {
                    return Ok(None);
                }
                let relative = if base_relative_path == "." {
                    decoded
                } else {
                    format!("{base_relative_path}/{decoded}")
                };
                canonical_granted_file(Path::new(&workspace_root), &relative, &self.internal)
                    .filter(|path| {
                        paths::canonicalize(&workspace_root)
                            .ok()
                            .and_then(|root| paths::relative_to(path, &root).ok())
                            .is_some_and(|relative| {
                                !relative
                                    .components()
                                    .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                                    && dependency_extension(&extension(&relative.to_string_lossy()))
                            })
                    })
                    .and_then(file_asset)
            }
            AssetGrant::MediaFileExact {
                file_path,
                download,
                device,
                inode,
            } => {
                let path = PathBuf::from(file_path);
                if decoded.as_deref() != Some(&basename(&path)) {
                    return Ok(None);
                }
                let Some(path) = canonical_file(&path, &self.internal) else {
                    return Ok(None);
                };
                let mime = if download == Some(true) {
                    Some("application/octet-stream")
                } else {
                    preview_mime(&extension(&path.to_string_lossy()))
                };
                let Some(mime) = mime else {
                    return Ok(None);
                };
                let Some(file) = OpenMediaFile::open(&path, Some(&FileIdentity { device, inode }))?
                else {
                    return Ok(None);
                };
                Some(ResolvedAsset::File {
                    file_name: download.filter(|b| *b).map(|_| basename(&path)),
                    path,
                    download: download.unwrap_or(false),
                    mime_type: Some(mime.into()),
                    file,
                    render_intent: None,
                })
            }
            AssetGrant::ProjectFavicon {
                workspace_root,
                relative_path,
            } => {
                let Some(relative) = relative_path else {
                    return Ok(None);
                };
                match canonical_granted_file(Path::new(&workspace_root), &relative, &self.internal)
                {
                    Some(p) => favicon_asset(p).await?,
                    None => None,
                }
            }
            AssetGrant::ProjectFaviconExternal { file_path } => {
                let path = PathBuf::from(file_path);
                if canonical_file(&path, &self.internal).as_ref() == Some(&path) {
                    favicon_asset(path).await?
                } else {
                    None
                }
            }
            AssetGrant::ToolOutputImage {
                thread_id,
                item_id,
                index,
            } => self
                .external
                .tool_image(&thread_id, &item_id, index)
                .await?
                .filter(|(_, bytes)| bytes.len() <= TOOL_IMAGE_MAX_BYTES)
                .map(|(mime_type, bytes)| ResolvedAsset::Bytes { mime_type, bytes }),
            AssetGrant::NativeAppIcon { app } => self
                .external
                .native_icon(&app)
                .await?
                .and_then(|p| canonical_file(&p, &self.internal))
                .and_then(file_asset),
            AssetGrant::SourceControlMedia {
                url,
                cwd,
                repository,
            } => Some(ResolvedAsset::SourceControlMedia {
                url,
                cwd: PathBuf::from(cwd),
                repository,
                expires_at: claims.expires_at,
            }),
        };
        Ok(asset)
    }
}
/// A stored attachment that is the file its entry names, opened under the attachments directory
/// without following a link ([`ContainedFiles`](crate::contained_files::ContainedFiles)): the
/// opened file must be the entry itself, so a planted link to a file outside or to another
/// attachment is neither signed nor served.
fn attachment_file(directory: &Path, id: &str, internal: &InternalPaths) -> Option<PathBuf> {
    let path = attachments::resolve_by_id(directory, id)?;
    let files = crate::contained_files::ContainedFiles::new(directory);
    let real = files.realpath(&path).ok()?;
    let stat = files.open(&real).ok()?.stat().ok()?;
    (stat.regular
        && files.lstat_identity(&path).ok()? == stat.identity
        && !internal.contains(&real))
    .then_some(real)
}
pub fn canonical_file(path: &Path, internal: &InternalPaths) -> Option<PathBuf> {
    let canonical = paths::canonicalize(path).ok()?;
    (canonical.is_file() && !internal.contains(&canonical)).then_some(canonical)
}
pub fn canonical_workspace_file(
    root: &Path,
    relative: &str,
    internal: &InternalPaths,
) -> Option<PathBuf> {
    let scope = crate::scoped::ScopedDir::open(root).ok()?;
    canonical_scoped_file(&scope, relative, internal)
}
fn canonical_granted_file(
    root: &Path,
    relative: &str,
    internal: &InternalPaths,
) -> Option<PathBuf> {
    // Signed canonical roots may use short names, but must never acquire a symlink expansion.
    let scope = crate::scoped::ScopedDir::open_unlinked(root).ok()?;
    canonical_scoped_file(&scope, relative, internal)
}
fn canonical_scoped_file(
    scope: &crate::scoped::ScopedDir,
    relative: &str,
    internal: &InternalPaths,
) -> Option<PathBuf> {
    let relative = scope.resolve(relative, false).ok()?;
    let file = scope.open_file(&relative).ok()?;
    let path = crate::scoped::final_path(&file, &scope.root.join(relative)).ok()?;
    (!internal.contains(&path)).then_some(path)
}
fn basename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into()
}
fn decode_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !bytes
                .get(index + 1..index + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}
fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}
fn is_audio_video(path: &str) -> bool {
    preview_mime(&extension(path))
        .is_some_and(|m| m.starts_with("audio/") || m.starts_with("video/"))
}
fn dependency_extension(ext: &str) -> bool {
    matches!(
        ext,
        "htm"
            | "html"
            | "pdf"
            | "avif"
            | "gif"
            | "ico"
            | "jpeg"
            | "jpg"
            | "png"
            | "svg"
            | "webp"
            | "css"
            | "js"
            | "mjs"
            | "otf"
            | "ttf"
            | "woff"
            | "woff2"
    )
}
pub fn preview_mime(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().trim_start_matches('.') {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "aiff" => "audio/aiff",
        "weba" | "webm-audio" => "audio/webm",
        "zip" => "application/zip",
        "jar" => "application/java-archive",
        "tar" => "application/x-tar",
        "gz" | "tgz" => "application/gzip",
        "sqlite" | "sqlite3" | "db" => "application/vnd.sqlite3",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",
        "epub" => "application/epub+zip",
        "ipynb" => "application/x-ipynb+json",
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: String,
    pub inode: String,
}
fn file_identity(path: &Path) -> Result<FileIdentity> {
    let file = open_read(path, true)?;
    let (device, inode, _) = crate::scoped::identity(&file)?;
    Ok(FileIdentity { device, inode })
}
pub struct OpenMediaFile {
    pub file: File,
    pub identity: FileIdentity,
}
impl OpenMediaFile {
    pub fn open(path: &Path, identity: Option<&FileIdentity>) -> Result<Option<Self>> {
        let before = fs::symlink_metadata(path).map_err(|e| io_error("stat", path, e))?;
        if !before.is_file() {
            return Ok(None);
        }
        let before_id = file_identity(path)?;
        if before_id.inode == "0" || identity.is_some_and(|id| *id != before_id) {
            return Ok(None);
        }
        #[cfg(test)]
        media_hook();
        let parent = path.parent().ok_or(Error::AssetDenied)?;
        let scope = crate::scoped::ScopedDir::open(parent)?;
        let file = scope.open_file(Path::new(path.file_name().ok_or(Error::AssetDenied)?))?;
        let (device, inode, _) = crate::scoped::identity(&file)?;
        let opened_id = FileIdentity { device, inode };
        #[cfg(test)]
        media_hook();
        if opened_id != before_id
            || identity.is_some_and(|id| *id != opened_id)
            || crate::scoped::final_path(&file, path)? != path
            || paths::canonicalize(path).map_err(|e| io_error("realpath-target", path, e))? != path
            || file_identity(path)? != opened_id
        {
            return Ok(None);
        }
        Ok(Some(Self {
            file,
            identity: before_id,
        }))
    }
    pub fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>> {
        if length == 0
            || offset
                .checked_add(length)
                .is_none_or(|end| end > 9_007_199_254_740_991)
        {
            return Err(Error::Invalid("invalid media range".into()));
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| io_error("seek", "media", e))?;
        let mut bytes = Vec::new();
        self.file
            .by_ref()
            .take(length)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("read", "media", e))?;
        Ok(bytes)
    }
}
async fn favicon_asset(path: PathBuf) -> Result<Option<ResolvedAsset>> {
    if extension(&path.to_string_lossy()) != "icns" {
        return Ok(file_asset(path));
    }
    let Some(mut opened) = OpenMediaFile::open(&path, None)? else {
        return Ok(None);
    };
    if opened
        .file
        .metadata()
        .map_err(|e| io_error("stat", &path, e))?
        .len()
        > ICON_MAX_BYTES
    {
        return Ok(None);
    }
    let bytes = opened.read_range(0, ICON_MAX_BYTES)?;
    Ok(
        extract_icns_png(&bytes, 96).map(|bytes| ResolvedAsset::Bytes {
            bytes,
            mime_type: "image/png".into(),
        }),
    )
}
pub fn extract_icns_png(bytes: &[u8], min_size: u32) -> Option<Vec<u8>> {
    let be = |offset| -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    if bytes.get(..4)? != b"icns" {
        return None;
    }
    let end = (be(4)? as usize).min(bytes.len());
    let mut offset = 8;
    let mut best: Option<(&[u8], u32)> = None;
    while offset + 8 <= end {
        let length = be(offset + 4)? as usize;
        if length < 8 || length > end - offset {
            break;
        }
        let entry = &bytes[offset + 8..offset + length];
        offset += length;
        if !entry.starts_with(b"\x89PNG\r\n\x1a\n") || entry.len() < 24 {
            continue;
        }
        let width = u32::from_be_bytes(entry[16..20].try_into().ok()?);
        if best.is_none_or(|(_, w)| {
            if w < min_size {
                width > w
            } else {
                width >= min_size && width < w
            }
        }) {
            best = Some((entry, width));
        }
    }
    best.map(|(bytes, _)| bytes.to_vec())
}
pub async fn read_mcp_app_html(path: PathBuf) -> Result<Option<Vec<u8>>> {
    blocking(move || {
        let mut bytes = Vec::new();
        open_read(&path, false)?
            .take(5 * 1024 * 1024 + 128 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("read", path, e))?;
        Ok((bytes.len() <= 5 * 1024 * 1024 + 128 * 1024).then_some(bytes))
    })
    .await
}

#[cfg(test)]
thread_local! {
 static MEDIA_HOOKS:std::cell::RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>>=Default::default();
 static DRAFT_HOOK:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=Default::default();
}
#[cfg(test)]
fn media_hook() {
    MEDIA_HOOKS.with(|hooks| {
        if let Some(hook) = hooks.borrow_mut().pop_front() {
            hook();
        }
    });
}
#[cfg(test)]
fn draft_hook() {
    DRAFT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}
#[cfg(test)]
mod race_tests {
    use super::*;

    struct External;
    impl ExternalAssets for External {
        fn tool_image<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: u64,
        ) -> AssetFuture<'a, Option<(String, Vec<u8>)>> {
            Box::pin(async { Ok(None) })
        }
        fn native_icon<'a>(
            &'a self,
            _: &'a NativeAppIconReference,
        ) -> AssetFuture<'a, Option<PathBuf>> {
            Box::pin(async { Ok(None) })
        }
        fn validate_source_control<'a>(
            &'a self,
            _: &'a Path,
            _: &'a str,
            _: Option<&'a str>,
        ) -> AssetFuture<'a, Option<SourceControlMedia>> {
            Box::pin(async { Ok(None) })
        }
    }
    #[tokio::test]
    async fn draft_media_second_realpath_must_equal_the_first() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = paths::canonicalize(tmp.path()).expect("root");
        fs::create_dir(root.join("project")).expect("project");
        fs::create_dir(root.join("outside")).expect("outside");
        fs::write(root.join("project/sound.mp3"), "inside").expect("file");
        fs::write(root.join("outside/sound.mp3"), "outside").expect("file");
        let service = AssetAccess {
            signer: AssetSigner::new(vec![1; 32]).expect("signer"),
            attachments_dir: root.join("attachments"),
            home: root.clone(),
            internal: InternalPaths {
                state_dir: root.join("state"),
                secrets_dir: root.join("secrets"),
            },
            favicons: Arc::new(FaviconResolver::new(root.clone())),
            external: Arc::new(External),
        };
        let swapped = root.clone();
        DRAFT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(swapped.join("project"), swapped.join("original")).expect("rename");
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir(swapped.join("outside"), swapped.join("project"))
                    .expect("swap");
                #[cfg(unix)]
                std::os::unix::fs::symlink(swapped.join("outside"), swapped.join("project"))
                    .expect("swap");
            }))
        });
        let project = root.join("project");
        let result = service
            .issue_url(
                AssetResource::DraftWorkspaceFile {
                    cwd: Text(project.to_string_lossy().into()),
                    path: Text("sound.mp3".into()),
                },
                AssetContext {
                    workspace_root: Some(&project),
                    ..Default::default()
                },
                100,
            )
            .await;
        assert!(
            result.is_err(),
            "draft media granted after canonical path changed"
        );
    }
    #[test]
    fn two_path_swaps_cannot_substitute_a_different_opened_media_identity() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = paths::canonicalize(tmp.path()).expect("root");
        let path = root.join("video.mp4");
        let original = root.join("original.mp4");
        let substitute = root.join("substitute.mp4");
        fs::write(&path, "original").expect("file");
        fs::write(&substitute, "substitute").expect("file");
        let expected = file_identity(&path).expect("identity");
        let a = path.clone();
        let b = original.clone();
        let c = substitute.clone();
        MEDIA_HOOKS.with(|hooks| {
            hooks.borrow_mut().push_back(Box::new(move || {
                fs::rename(&a, &b).expect("stash");
                fs::rename(&c, &a).expect("swap");
            }))
        });
        let a = path.clone();
        let b = original.clone();
        let c = substitute.clone();
        MEDIA_HOOKS.with(|hooks| {
            hooks.borrow_mut().push_back(Box::new(move || {
                fs::rename(&a, &c).expect("stash");
                fs::rename(&b, &a).expect("restore");
            }))
        });
        let result = OpenMediaFile::open(&path, Some(&expected));
        assert!(
            result.expect("open").is_none(),
            "descriptor on substitute accepted with original signed identity"
        );
    }
}
