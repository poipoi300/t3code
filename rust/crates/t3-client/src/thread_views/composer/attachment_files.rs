//! Local attachment identity and preview ownership, independent of host file access.
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

const DIRECTORY: &str = "t3-composer-attachments";
pub(super) fn media_kind(attachment: &serde_json::Value) -> Option<&'static str> {
    let mime = attachment["mimeType"]
        .as_str()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if matches!(
        mime.as_str(),
        "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    ) {
        return Some("image");
    }
    let generic = matches!(
        mime.as_str(),
        "" | "application/octet-stream"
            | "binary/octet-stream"
            | "application/unknown"
            | "video/mp2t"
            | "video/vnd.dlna.mpeg-tts"
    );
    if mime.starts_with("video/") && !generic {
        return Some("video");
    }
    if !generic {
        return None;
    }
    let name = attachment["name"].as_str().unwrap_or_default();
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if matches!(extension.trim(), "gif" | "jpeg" | "jpg" | "png" | "webp") {
        Some("image")
    } else if matches!(
        extension.as_str(),
        "avi" | "m4v" | "mkv" | "mov" | "mp4" | "ogv" | "webm"
    ) {
        Some("video")
    } else {
        None
    }
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
fn generated_name(name: &str) -> bool {
    name.get(..36).is_some_and(uuid) && name.as_bytes().get(36) == Some(&b'-')
}
fn decode(value: &str) -> Option<String> {
    let mut bytes = vec![];
    let mut index = 0;
    while index < value.len() {
        if value.as_bytes()[index] == b'%' {
            bytes.push(u8::from_str_radix(value.get(index + 1..index + 3)?, 16).ok()?);
            index += 3;
        } else {
            bytes.push(value.as_bytes()[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).ok()
}
fn encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
fn file_path(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file"
        || url.host_str().is_some_and(|host| !host.is_empty())
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = decode(url.path())?;
    if path.contains(['\\', '\0']) || path.split('/').any(|part| part == "..") {
        return None;
    }
    Some(if path.starts_with("/private/var/") {
        path[8..].into()
    } else {
        path
    })
}
fn location(uri: &str) -> Option<(String, String)> {
    let path = file_path(uri)?;
    let (document, name) = path.rsplit_once(&format!("/{DIRECTORY}/"))?;
    if name.is_empty() || name == "." || name.contains('/') {
        return None;
    }
    Some((document.into(), name.into()))
}
fn container_prefix(document: &str) -> Option<&str> {
    let root = document.get(..document.len().checked_sub(10)?)?;
    if !document
        .get(document.len() - 10..)?
        .eq_ignore_ascii_case("/Documents")
    {
        return None;
    }
    let (prefix, id) = root.rsplit_once('/')?;
    let suffix = "/Containers/Data/Application";
    if !uuid(id)
        || !prefix
            .get(prefix.len().checked_sub(suffix.len())?..)?
            .eq_ignore_ascii_case(suffix)
    {
        return None;
    }
    root.get(..root.len() - id.len())
}
pub fn reference_key(uri: &str) -> String {
    let Some((document, name)) = location(uri) else {
        return uri.into();
    };
    let document = if generated_name(&name) {
        container_prefix(&document).map_or_else(
            || document.clone(),
            |prefix| format!("{prefix}<app>/Documents"),
        )
    } else {
        document
    };
    format!("file://{document}/{DIRECTORY}/{}", encode(&name))
}
pub fn resolve_owned_uri(uri: &str, document_directory: &str) -> Option<String> {
    let (document, name) = location(uri)?;
    let target = file_path(document_directory)?
        .trim_end_matches('/')
        .to_string();
    if target.is_empty() {
        return None;
    }
    if document != target
        && (!generated_name(&name)
            || container_prefix(&target).is_none()
            || container_prefix(&target) != container_prefix(&document))
    {
        return None;
    }
    let mut resolved = url::Url::parse(document_directory).ok()?;
    resolved.set_path(&format!(
        "{}/{DIRECTORY}/{}",
        resolved.path().trim_end_matches('/'),
        encode(&name)
    ));
    Some(resolved.into())
}

#[derive(Clone, Default)]
pub struct AttachmentRetention {
    files: Rc<RefCell<BTreeMap<String, usize>>>,
}
pub struct RetainedFile {
    owner: AttachmentRetention,
    key: String,
    on_last_release: Option<Box<dyn FnOnce()>>,
}
impl AttachmentRetention {
    pub fn retain(&self, uri: &str, on_last_release: impl FnOnce() + 'static) -> RetainedFile {
        let key = reference_key(uri);
        *self.files.borrow_mut().entry(key.clone()).or_default() += 1;
        RetainedFile {
            owner: self.clone(),
            key,
            on_last_release: Some(Box::new(on_last_release)),
        }
    }
    pub fn is_retained(&self, uri: &str) -> bool {
        self.files.borrow().contains_key(&reference_key(uri))
    }
}
impl Drop for RetainedFile {
    fn drop(&mut self) {
        let last = {
            let mut files = self.owner.files.borrow_mut();
            match files.get_mut(&self.key) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                _ => {
                    files.remove(&self.key);
                    true
                }
            }
        };
        if last && let Some(callback) = self.on_last_release.take() {
            callback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIRST: &str = "file:///private/var/mobile/Containers/Data/Application/11111111-1111-1111-1111-111111111111/Documents/";
    const SECOND: &str = "file:///var/mobile/Containers/Data/Application/22222222-2222-2222-2222-222222222222/Documents/";
    const NAME: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa-file name.txt";
    #[test]
    fn generated_files_survive_ios_container_moves_and_encoding_changes() {
        let first = format!("{FIRST}{DIRECTORY}/{NAME}");
        let second = format!("{SECOND}{DIRECTORY}/{}", NAME.replace(' ', "%20"));
        assert_eq!(reference_key(&first), reference_key(&second));
        assert_eq!(resolve_owned_uri(&first, SECOND), Some(second));
        let ungenerated = format!("{FIRST}{DIRECTORY}/user.txt");
        assert!(resolve_owned_uri(&ungenerated, SECOND).is_none());
        assert_ne!(
            reference_key(&ungenerated),
            reference_key(&format!("{SECOND}{DIRECTORY}/user.txt"))
        );
    }
    #[test]
    fn foreign_or_invalid_file_locations_remain_distinct() {
        for uri in [
            "https://host/t3-composer-attachments/file.txt",
            "file://other/a/t3-composer-attachments/x",
            "file:///a/t3-composer-attachments/x?query",
            "file:///a/t3-composer-attachments/x#fragment",
            "file:///a/t3-composer-attachments/%2e%2e%2fx",
            "file:///a/t3-composer-attachments/%5cx",
            "file:///a/t3-composer-attachments/%00x",
        ] {
            assert_eq!(reference_key(uri), uri);
            assert!(resolve_owned_uri(uri, FIRST).is_none());
        }
    }
    #[test]
    fn previews_retain_aliases_until_the_last_owner_releases() {
        let retention = AttachmentRetention::default();
        let releases = Rc::new(RefCell::new(vec![]));
        let a = format!("{FIRST}{DIRECTORY}/{NAME}");
        let b = format!("{SECOND}{DIRECTORY}/{NAME}");
        let first = retention.retain(&a, {
            let releases = releases.clone();
            move || releases.borrow_mut().push(1)
        });
        let second = retention.retain(&b, {
            let releases = releases.clone();
            let retention = retention.clone();
            let a = a.clone();
            move || {
                assert!(!retention.is_retained(&a));
                releases.borrow_mut().push(2);
            }
        });
        drop(first);
        assert!(retention.is_retained(&a));
        assert!(releases.borrow().is_empty());
        drop(second);
        assert!(!retention.is_retained(&a));
        assert_eq!(*releases.borrow(), vec![2]);
    }
}
