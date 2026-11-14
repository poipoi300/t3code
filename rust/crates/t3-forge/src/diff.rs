use std::collections::BTreeMap;
#[derive(Default)]
struct Entry {
    old: Option<String>,
    new: Option<String>,
    deleted: bool,
    revision: Option<String>,
    body: bool,
}
fn unquote(path: &str) -> String {
    if !path.starts_with('"') || !path.ends_with('"') {
        return path.into();
    }
    let mut bytes = path.as_bytes()[1..path.len() - 1]
        .iter()
        .copied()
        .peekable();
    let mut out = vec![];
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            out.push(byte);
            continue;
        }
        let Some(byte) = bytes.next() else {
            break;
        };
        let code = match byte {
            b't' => b'\t',
            b'n' => b'\n',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'0'..=b'7' => {
                let mut code = (byte - b'0') as u16;
                for _ in 0..2 {
                    if let Some(next) = bytes.peek().copied().filter(|n| (b'0'..=b'7').contains(n))
                    {
                        bytes.next();
                        code = code * 8 + (next - b'0') as u16;
                    } else {
                        break;
                    }
                }
                code as u8
            }
            other => other,
        };
        out.push(code);
    }
    String::from_utf8_lossy(&out).into()
}
fn quoted_end(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (at, byte) in s.bytes().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Some(at);
        }
    }
    None
}
fn side(value: &str, prefix: &str) -> Option<String> {
    let token = value.split('\t').next().unwrap_or(value);
    if token == "/dev/null" {
        return None;
    }
    let path = unquote(token);
    Some(path.strip_prefix(prefix).unwrap_or(&path).into())
}
fn header_side(value: &str, prefix: &str) -> Option<String> {
    unquote(value).strip_prefix(prefix).map(str::to_string)
}
fn header_paths(s: &str) -> (Option<String>, Option<String>) {
    if s.starts_with('"') {
        let Some(end) = quoted_end(s) else {
            return (None, None);
        };
        if s.as_bytes().get(end + 1) != Some(&b' ') {
            return (None, None);
        }
        (
            header_side(&s[..=end], "a/"),
            header_side(&s[end + 2..], "b/"),
        )
    } else if s.ends_with('"') {
        let Some(at) = s
            .find('"')
            .filter(|at| *at > 0 && s.as_bytes().get(at - 1) == Some(&b' '))
        else {
            return (None, None);
        };
        (header_side(&s[..at - 1], "a/"), header_side(&s[at..], "b/"))
    } else {
        let splits: Vec<_> = s.match_indices(" b/").map(|(at, _)| at).collect();
        let at = splits
            .iter()
            .copied()
            .find(|at| s.starts_with("a/") && s[2..*at] == s[*at + 3..])
            .or_else(|| (splits.len() == 1).then(|| splits[0]));
        match at {
            Some(at) if s.starts_with("a/") => (Some(s[2..at].into()), Some(s[at + 3..].into())),
            _ => (None, None),
        }
    }
}
pub(crate) fn revisions(patch: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut entry = None::<Entry>;
    let close = |entry: &mut Option<Entry>, result: &mut BTreeMap<String, String>| {
        if let Some(e) = entry.take() {
            let path = if e.deleted { e.old } else { e.new.or(e.old) };
            if let (Some(path), Some(rev)) = (path, e.revision)
                && !path.is_empty()
            {
                result.insert(path, rev);
            }
        }
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            close(&mut entry, &mut result);
            let (old, new) = header_paths(rest);
            entry = Some(Entry {
                old,
                new,
                ..Default::default()
            });
            continue;
        }
        let Some(e) = entry.as_mut().filter(|e| !e.body) else {
            continue;
        };
        if line.starts_with("@@") {
            e.body = true;
        } else if let Some(rest) = line.strip_prefix("index ") {
            e.revision = rest
                .split_once("..")
                .and_then(|(_, head)| head.split(' ').next())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        } else if line.starts_with("deleted file mode") {
            e.deleted = true;
        } else if let Some(rest) = line.strip_prefix("rename from ") {
            e.old = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            e.new = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("--- ") {
            e.old = side(rest, "a/");
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            e.new = side(rest, "b/");
            if e.new.is_none() {
                e.deleted = true;
            }
        }
    }
    close(&mut entry, &mut result);
    result
}
#[cfg(test)]
mod tests {
    #[test]
    fn revisions_ignore_hunk_content_and_handle_quoted_renames() {
        let patch = "diff --git a/old path b/new path\nindex abc..def 100644\nrename from old path\nrename to new path\n--- a/old path\n+++ b/new path\n@@ -1 +1 @@\n--- a/forged\n+++ b/forged\ndiff --git \"a/tab\\tname\" \"b/tab\\tname\"\nindex 123..0000000\ndeleted file mode 100644\n--- \"a/tab\\tname\"\n+++ /dev/null\n";
        assert_eq!(
            super::revisions(patch),
            std::collections::BTreeMap::from([
                ("new path".into(), "def".into()),
                ("tab\tname".into(), "0000000".into())
            ])
        );
    }
}
