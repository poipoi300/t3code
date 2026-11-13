use std::sync::LazyLock;
use t3_contracts::project_clone::ProjectCloneStage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCloneProgressLine {
    pub stage: ProjectCloneStage,
    pub percent: Option<u8>,
    pub detail: Option<String>,
}
pub fn parse_git_clone_progress_line(line: &str) -> Option<GitCloneProgressLine> {
    use ProjectCloneStage::*;
    let line = line.trim();
    let stage = [
        ("remote: Enumerating objects", Counting),
        ("remote: Counting objects", Counting),
        ("remote: Compressing objects", Counting),
        ("Receiving objects", Receiving),
        ("Resolving deltas", Resolving),
        ("Updating files", Checkout),
        ("Checking out files", Checkout),
    ]
    .into_iter()
    .find(|(prefix, _)| line.starts_with(prefix))?
    .1;
    let mut result = GitCloneProgressLine {
        stage,
        percent: None,
        detail: None,
    };
    static PERCENT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?s):\s+(\d+)%\s+\((\d+)/(\d+)\)(.*)$")
            .expect("clone progress expression")
    });
    let Some(matched) = PERCENT.captures(line) else {
        return Some(result);
    };
    let tail = matched[4].trim_end();
    let tail = tail.strip_suffix(", done.").unwrap_or(tail).trim_end();
    if !tail.is_empty() && !tail.starts_with(',') {
        return Some(result);
    }
    result.percent = matched[1]
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite())
        .map(|p| p.clamp(0.0, 100.0) as u8);
    let detail = tail.strip_prefix(',').unwrap_or("").trim();
    if !detail.is_empty() && detail != "done." {
        result.detail = Some(crate::remote::transport_safe_error_value(detail));
    }
    Some(result)
}
#[derive(Default)]
pub(crate) struct Lines {
    bytes: Vec<u8>,
    discard: bool,
}
impl Lines {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<GitCloneProgressLine> {
        let mut result = Vec::new();
        for byte in bytes {
            if matches!(byte, b'\r' | b'\n') {
                if !self.discard
                    && let Some(line) =
                        parse_git_clone_progress_line(&String::from_utf8_lossy(&self.bytes))
                {
                    result.push(line);
                }
                self.bytes.clear();
                self.discard = false;
            } else if self.bytes.len() < 8192 && !self.discard {
                self.bytes.push(*byte);
            } else {
                self.bytes.clear();
                self.discard = true;
            }
        }
        result
    }
}
