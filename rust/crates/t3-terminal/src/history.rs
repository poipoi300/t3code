use crate::{DEFAULT_TERMINAL_ID, TerminalError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

pub fn retained_terminal_history(value: &str, max_bytes: usize) -> String {
    let mut start = value.len().saturating_sub(max_bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_owned()
}
#[derive(Debug)]
pub struct BoundedTerminalHistory {
    chunks: VecDeque<String>,
    bytes: usize,
    breaks: usize,
    max_lines: usize,
    max_bytes: usize,
}
impl BoundedTerminalHistory {
    pub fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            breaks: 0,
            max_lines,
            max_bytes,
        }
    }
    pub fn append(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if self.max_bytes == 0 || self.max_lines == 0 {
            self.clear();
            if self.max_bytes > 0 && value.ends_with('\n') {
                self.chunks.push_back("\n".into());
                self.bytes = 1;
                self.breaks = 1;
            }
            return;
        }
        self.bytes += value.len();
        self.breaks += value.bytes().filter(|&b| b == b'\n').count();
        let mut offset = 0;
        while offset < value.len() {
            let mut end = (offset + 16 * 1024).min(value.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            let chunk = &value[offset..end];
            if let Some(previous) = self.chunks.back_mut()
                && previous.len() + chunk.len() <= 16 * 1024
            {
                previous.push_str(chunk);
            } else {
                self.chunks.push_back(chunk.into());
            }
            offset = end;
        }
        let trailing = self.chunks.back().is_some_and(|s| s.ends_with('\n'));
        let mut lines = self.breaks + usize::from(!trailing);
        while lines > self.max_lines {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            if let Some(pos) = front.find('\n') {
                front.drain(..pos + 1);
                self.bytes -= pos + 1;
                self.breaks -= 1;
                lines -= 1;
            } else {
                self.bytes -= front.len();
                front.clear();
            }
            if front.is_empty() {
                self.chunks.pop_front();
            }
        }
        while self.bytes > self.max_bytes {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            let mut drop = (self.bytes - self.max_bytes).min(front.len());
            while !front.is_char_boundary(drop) {
                drop += 1;
            }
            self.breaks -= front[..drop].bytes().filter(|&b| b == b'\n').count();
            front.drain(..drop);
            self.bytes -= drop;
            if front.is_empty() {
                self.chunks.pop_front();
            }
        }
    }
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
        self.breaks = 0;
    }
    pub fn value(&self) -> String {
        self.chunks.iter().map(String::as_str).collect()
    }
}

#[derive(Default, Debug)]
pub struct HistorySanitizer {
    pending: String,
}
impl HistorySanitizer {
    pub fn clear(&mut self) {
        self.pending.clear();
    }
    pub fn push(&mut self, data: &str) -> String {
        if self.pending.is_empty() && !data.contains('\x1b') {
            return data.into();
        }
        let pending_scan_start = if self.pending.ends_with('\x1b') {
            self.pending.len() - 1
        } else {
            self.pending.len()
        };
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(data);
        let bytes = input.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != 27 {
                let end = input[i..]
                    .find('\x1b')
                    .map(|n| i + n)
                    .unwrap_or(bytes.len());
                out.push_str(&input[i..end]);
                i = end;
                continue;
            }
            let start = i;
            let Some(&kind) = bytes.get(i + 1) else {
                self.pending = input[i..].into();
                break;
            };
            if kind == b'[' {
                let mut c = i + 2;
                let mut body = String::new();
                let mut controls = String::new();
                let mut complete = false;
                while c < bytes.len() {
                    let ch = match input[c..].chars().next() {
                        Some(ch) => ch,
                        None => break,
                    };
                    if matches!(ch, '\x18' | '\x1a') || ('@'..='~').contains(&ch) {
                        let strip = match ch {
                            'n' => true,
                            'R' => body.chars().all(|c| c.is_ascii_digit() || ";?".contains(c)),
                            'c' => body
                                .chars()
                                .all(|c| c.is_ascii_digit() || ">=;?".contains(c)),
                            'p' | 'y' => body.strip_suffix('$').is_some_and(|s| {
                                s.chars().all(|c| c.is_ascii_digit() || ";?".contains(c))
                            }),
                            'q' => body
                                .strip_prefix('>')
                                .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit() || c == ';')),
                            'u' => body.starts_with('?'),
                            _ => false,
                        };
                        c += ch.len_utf8();
                        complete = true;
                        if strip {
                            out.push_str(&controls);
                        } else {
                            out.push_str(&input[start..c]);
                        }
                        break;
                    }
                    if ch < '\x10' {
                        controls.push(ch);
                    } else if ch >= ' ' && ch != '\x7f' || ch == '\x1b' {
                        body.push(ch);
                    }
                    c += ch.len_utf8();
                }
                if !complete {
                    self.pending = input[start..].into();
                    break;
                }
                i = c;
            } else if matches!(kind, b']' | b'P' | b'^' | b'_') {
                let mut c = (i + 2).max(pending_scan_start);
                let mut end = None;
                while c < bytes.len() {
                    let ch = match input[c..].chars().next() {
                        Some(ch) => ch,
                        None => break,
                    };
                    if matches!(ch, '\x18' | '\x1a')
                        || (kind == b']' && ch == '\x07')
                        || (kind != b']' && ch == '\u{9c}')
                    {
                        end = Some(c + ch.len_utf8());
                        break;
                    }
                    if ch == '\x1b' {
                        if let Some(next) = bytes.get(c + 1) {
                            end = Some(if *next == b'\\' { c + 2 } else { c });
                        }
                        break;
                    }
                    c += ch.len_utf8();
                }
                let Some(end) = end else {
                    self.pending = input[start..].into();
                    break;
                };
                let content = &input[start + 2..end];
                let strip = if kind == b']' {
                    ["10;", "11;", "12;"].iter().any(|p| {
                        content
                            .strip_prefix(p)
                            .is_some_and(|s| s.starts_with('?') || s.starts_with("rgb:"))
                    })
                } else if kind == b'P' {
                    let content = content.strip_prefix(['0', '1']).unwrap_or(content);
                    ["$q", "$r", "+q", "+r"]
                        .iter()
                        .any(|s| content.starts_with(s))
                } else {
                    false
                };
                if !strip {
                    out.push_str(&input[start..end]);
                }
                i = end;
            } else {
                let mut c = i + 1;
                while c < bytes.len() && (0x20..=0x2f).contains(&bytes[c]) {
                    c += 1;
                }
                if c == bytes.len() {
                    self.pending = input[start..].into();
                    break;
                }
                let end = if bytes[c] == 27 {
                    c
                } else if (0x30..=0x7e).contains(&bytes[c]) {
                    c + 1
                } else {
                    i + 1 + input[i + 1..].chars().next().map_or(0, char::len_utf8)
                };
                if kind != b'Z' {
                    out.push_str(&input[start..end]);
                }
                i = end;
            }
        }
        out
    }
}

const INPUT_RESET: &str = "\x1b[?1049h\x1b[<100u\x1b[=0;1u\x1b[?1049l\x1b[?1;9;66;67;1000;1002;1003;1004;1005;1006;1015;2004l\x1b[?25;1035;1036h\x1b[<100u\x1b[=0;1u\x1b[>4m";
pub(crate) fn neutralize(history: &str, lines: usize, bytes: usize) -> String {
    let reset = if history.contains('\x1b') {
        INPUT_RESET.len() + "\x1b7\x1b[?1049l".len()
    } else {
        0
    };
    let mut bounded = BoundedTerminalHistory::new(
        lines.saturating_sub(usize::from(reset > 0 && history.ends_with('\n'))),
        bytes.saturating_sub(reset),
    );
    bounded.append(history);
    let mut history = bounded.value();
    if !history.contains('\x1b') {
        return history;
    }
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new("\x1b(?:\\[\\?([0-9;]+)([hl])|(c))").expect("constant expression")
    });
    let re = &*RE;
    let mut alternate = None;
    for cap in re.captures_iter(&history) {
        if cap.get(3).is_some() {
            alternate = None;
        }
        if let Some(params) = cap.get(1) {
            for p in params.as_str().split(';') {
                if matches!(p, "47" | "1047" | "1049") {
                    if &cap[2] == "h" {
                        alternate = alternate.or(Some(p.to_owned()));
                    } else {
                        alternate = None;
                    }
                }
            }
        }
    }
    if let Some(mode) = alternate {
        history.push_str(&format!("\x1b7\x1b[?{mode}l"));
    }
    history.push_str(INPUT_RESET);
    history
}

#[derive(Clone, Debug)]
pub struct HistoryStore {
    pub directory: PathBuf,
    pub max_lines: usize,
    pub max_bytes: usize,
}
impl HistoryStore {
    pub fn new(directory: impl Into<PathBuf>, max_lines: usize, max_bytes: usize) -> Self {
        Self {
            directory: directory.into(),
            max_lines,
            max_bytes,
        }
    }
    fn encoded(&self, thread: &str, terminal: &str) -> PathBuf {
        self.directory.join(filename(
            &format!("terminal_{}", encode(thread)),
            if terminal == DEFAULT_TERMINAL_ID {
                None
            } else {
                Some(encode(terminal))
            },
        ))
    }
    pub fn path(&self, thread: &str, terminal: &str) -> PathBuf {
        let encoded = self.encoded(thread, terminal);
        if encoded.file_name().is_some_and(|s| s.len() <= 143) {
            return encoded;
        }
        self.directory.join(filename(
            &format!("terminal_sha256.{}", hash(thread)),
            if terminal == DEFAULT_TERMINAL_ID {
                None
            } else {
                Some(hash(terminal))
            },
        ))
    }
    fn paths(&self, thread: &str, terminal: &str) -> Vec<PathBuf> {
        let mut paths = vec![self.path(thread, terminal), self.encoded(thread, terminal)];
        if terminal == DEFAULT_TERMINAL_ID {
            paths.push(self.directory.join(format!("{}.log", legacy(thread))));
        }
        paths.dedup();
        paths
    }
    pub fn read(&self, thread: &str, terminal: &str) -> Result<String, TerminalError> {
        for path in self.paths(thread, terminal) {
            if !self
                .exists(&path)
                .map_err(|e| error("read", thread, terminal, e))?
            {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(error("read", thread, terminal, source)),
            };
            let result = (|| {
                let size = file.metadata()?.len();
                let offset = size.saturating_sub(self.max_bytes as u64);
                file.seek(SeekFrom::Start(offset))?;
                let mut data = Vec::new();
                file.take(size - offset).read_to_end(&mut data)?;
                let mut start = 0;
                if offset > 0 {
                    while start < data.len() && data[start] & 0xc0 == 0x80 {
                        start += 1;
                    }
                }
                Ok::<_, std::io::Error>((
                    String::from_utf8_lossy(&data[start..]).into_owned(),
                    offset > 0,
                ))
            })();
            let (raw, truncated) =
                result.map_err(|source| error("read", thread, terminal, source))?;
            let mut history = BoundedTerminalHistory::new(self.max_lines, self.max_bytes);
            history.append(&raw);
            let value = history.value();
            if path != self.path(thread, terminal) {
                self.replace(thread, terminal, &value)
                    .map_err(|e| error("migrate", thread, terminal, e))?;
                let _ = fs::remove_file(path);
            } else if truncated || value != raw {
                let _ = self.replace(thread, terminal, &value);
            }
            return Ok(value);
        }
        Ok(String::new())
    }
    fn exists(&self, path: &std::path::Path) -> std::io::Result<bool> {
        if path.file_name().is_some_and(|n| n.len() > 143) {
            match fs::read_dir(&self.directory) {
                Ok(entries) => {
                    for entry in entries {
                        if entry?.file_name() == path.file_name().unwrap_or_default() {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            }
        } else {
            path.try_exists()
        }
    }
    fn replace(&self, thread: &str, terminal: &str, data: &str) -> std::io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let mut file = tempfile::Builder::new()
            .prefix(".history-")
            .tempfile_in(&self.directory)?;
        file.write_all(data.as_bytes())?;
        file.persist(self.path(thread, terminal))
            .map_err(|e| e.error)?;
        Ok(())
    }
    pub fn persist(
        &self,
        thread: &str,
        terminal: &str,
        history: &str,
    ) -> Result<(), TerminalError> {
        (|| {
            fs::create_dir_all(&self.directory)?;
            fs::write(self.path(thread, terminal), history)
        })()
        .map_err(|e| error("truncate", thread, terminal, e))
    }
    pub(crate) fn append(
        &self,
        thread: &str,
        terminal: &str,
        data: &str,
        history: impl FnOnce() -> String,
    ) -> Result<(), TerminalError> {
        let path = self.path(thread, terminal);
        if fs::metadata(&path).map_or(true, |m| {
            m.len().saturating_add(data.len() as u64) > self.max_bytes.saturating_mul(2) as u64
        }) {
            return self.persist(thread, terminal, &history());
        }
        let result = OpenOptions::new()
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(data.as_bytes()));
        if result.is_err() {
            return self.persist(thread, terminal, &history());
        }
        Ok(())
    }
    pub fn delete(&self, thread: &str, terminal: Option<&str>) -> Result<(), TerminalError> {
        let paths = if let Some(t) = terminal {
            self.paths(thread, t)
        } else {
            match fs::read_dir(&self.directory) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| {
                        let name = p.file_name().unwrap_or_default().to_string_lossy();
                        name == format!("{}.log", legacy(thread))
                            || [
                                format!("terminal_{}", encode(thread)),
                                format!("terminal_sha256.{}", hash(thread)),
                            ]
                            .iter()
                            .any(|part| {
                                name == format!("{part}.log")
                                    || name.starts_with(&format!("{part}_"))
                            })
                    })
                    .collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
                Err(e) => return Err(error("truncate", thread, "*", e)),
            }
        };
        for path in paths {
            if !self
                .exists(&path)
                .map_err(|e| error("truncate", thread, terminal.unwrap_or("*"), e))?
            {
                continue;
            }
            if let Err(e) = fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(error("truncate", thread, terminal.unwrap_or("*"), e));
            }
        }
        Ok(())
    }
}
fn encode(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(s)
}
fn hash(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}
fn filename(thread: &str, terminal: Option<String>) -> String {
    format!(
        "{thread}{}.log",
        terminal.map(|s| format!("_{s}")).unwrap_or_default()
    )
}
fn legacy(s: &str) -> String {
    s.encode_utf16()
        .map(|c| {
            if let Some(c) = char::from_u32(u32::from(c))
                && (c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                c
            } else {
                '_'
            }
        })
        .collect()
}
fn error(
    operation: &'static str,
    thread: &str,
    terminal: &str,
    source: std::io::Error,
) -> TerminalError {
    TerminalError::History {
        operation,
        thread_id: thread.into(),
        terminal_id: terminal.into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn append_only_materializes_history_for_compaction_or_recovery() {
        let temp = tempfile::tempdir().expect("temp");
        let store = HistoryStore::new(temp.path(), 5000, 64);
        store.persist("t", "term-1", "first").expect("persist");
        store
            .append("t", "term-1", "tail", || {
                panic!("append must not concatenate history")
            })
            .expect("append");
        assert_eq!(
            fs::read_to_string(store.path("t", "term-1")).expect("read"),
            "firsttail"
        );
        store
            .append("t", "term-1", &"x".repeat(128), || "retained".into())
            .expect("compact");
        assert_eq!(
            fs::read_to_string(store.path("t", "term-1")).expect("read"),
            "retained"
        );
    }
}
