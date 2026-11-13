use crate::{
    Result, files, invalid,
    watch::{self, WatchHandle},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use t3_contracts::keybindings::{
    KeybindingRule, KeybindingShortcut, KeybindingWhenNode, MAX_KEYBINDINGS_COUNT,
    MAX_WHEN_EXPRESSION_DEPTH, ResolvedKeybindingRule, ResolvedKeybindingsConfig,
};

pub use t3_contracts::server::ServerConfigIssue as ConfigIssue;
fn malformed(message: String) -> ConfigIssue {
    use t3_contracts::{
        base_schemas::TrimmedNonEmptyString,
        server::{KeybindingsMalformedConfigIssue, KeybindingsMalformedConfigIssueKind},
    };
    ConfigIssue::MalformedConfig(KeybindingsMalformedConfigIssue {
        kind: KeybindingsMalformedConfigIssueKind::KeybindingsMalformedConfig,
        message: TrimmedNonEmptyString(message),
    })
}
fn invalid_entry(index: usize, message: String) -> ConfigIssue {
    use t3_contracts::{
        auth::EncodedNumber,
        base_schemas::TrimmedNonEmptyString,
        server::{KeybindingsInvalidEntryIssue, KeybindingsInvalidEntryIssueKind},
    };
    ConfigIssue::InvalidEntry(KeybindingsInvalidEntryIssue {
        kind: KeybindingsInvalidEntryIssueKind::KeybindingsInvalidEntry,
        message: TrimmedNonEmptyString(message),
        index: EncodedNumber::Finite((index as u64).into()),
    })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeybindingsState {
    pub keybindings: ResolvedKeybindingsConfig,
    pub issues: Vec<ConfigIssue>,
}

pub fn default_bindings() -> Vec<KeybindingRule> {
    crate::defaults::keybindings()
}

pub fn parse_shortcut(value: &str) -> Option<KeybindingShortcut> {
    let lowercase = value.to_lowercase();
    let mut tokens: Vec<_> = lowercase.split('+').map(str::trim).collect();
    let mut trailing = false;
    while tokens.last() == Some(&"") {
        tokens.pop();
        trailing = true;
    }
    if trailing {
        tokens.push("+");
    }
    if tokens.contains(&"") {
        return None;
    }
    let mut shortcut = KeybindingShortcut {
        key: String::new(),
        meta_key: false,
        ctrl_key: false,
        shift_key: false,
        alt_key: false,
        mod_key: false,
    };
    for token in tokens {
        match token {
            "cmd" | "meta" => shortcut.meta_key = true,
            "ctrl" | "control" => shortcut.ctrl_key = true,
            "shift" => shortcut.shift_key = true,
            "alt" | "option" => shortcut.alt_key = true,
            "mod" => shortcut.mod_key = true,
            _ if !shortcut.key.is_empty() => return None,
            "space" => shortcut.key = " ".into(),
            "esc" => shortcut.key = "escape".into(),
            _ => shortcut.key = token.into(),
        }
    }
    if shortcut.key.is_empty() {
        None
    } else {
        Some(shortcut)
    }
}
pub fn encode_shortcut(s: &KeybindingShortcut) -> String {
    let mut parts = Vec::new();
    for (enabled, name) in [
        (s.mod_key, "mod"),
        (s.meta_key, "meta"),
        (s.ctrl_key, "ctrl"),
        (s.alt_key, "alt"),
        (s.shift_key, "shift"),
    ] {
        if enabled {
            parts.push(name);
        }
    }
    parts.push(if s.key == " " { "space" } else { &s.key });
    parts.join("+")
}
#[derive(Debug, PartialEq)]
enum Token {
    Identifier(String),
    Not,
    And,
    Or,
    Open,
    Close,
}
pub fn parse_when(expression: &str) -> Option<KeybindingWhenNode> {
    let mut tokens = Vec::new();
    let chars: Vec<_> = expression.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let token = match c {
            '!' => Token::Not,
            '(' => Token::Open,
            ')' => Token::Close,
            '&' if chars.get(i + 1) == Some(&'&') => {
                i += 1;
                Token::And
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                i += 1;
                Token::Or
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                i += 1;
                while chars
                    .get(i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                {
                    i += 1;
                }
                tokens.push(Token::Identifier(chars[start..i].iter().collect()));
                continue;
            }
            _ => return None,
        };
        tokens.push(token);
        i += 1;
    }
    struct Parser {
        tokens: Vec<Token>,
        i: usize,
    }
    impl Parser {
        fn primary(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            if depth > MAX_WHEN_EXPRESSION_DEPTH {
                return None;
            }
            match self.tokens.get(self.i)? {
                Token::Identifier(name) => {
                    let name = name.clone();
                    self.i += 1;
                    Some(KeybindingWhenNode::Identifier { name })
                }
                Token::Open => {
                    self.i += 1;
                    let node = self.or(depth + 1)?;
                    if self.tokens.get(self.i) != Some(&Token::Close) {
                        return None;
                    }
                    self.i += 1;
                    Some(node)
                }
                _ => None,
            }
        }
        fn unary(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            let mut count = 0;
            while self.tokens.get(self.i) == Some(&Token::Not) {
                self.i += 1;
                count += 1;
                if count > MAX_WHEN_EXPRESSION_DEPTH {
                    return None;
                }
            }
            let mut node = self.primary(depth)?;
            for _ in 0..count {
                node = KeybindingWhenNode::Not {
                    node: Box::new(node),
                };
            }
            Some(node)
        }
        fn and(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            let mut left = self.unary(depth)?;
            while self.tokens.get(self.i) == Some(&Token::And) {
                self.i += 1;
                left = KeybindingWhenNode::And {
                    left: Box::new(left),
                    right: Box::new(self.unary(depth)?),
                };
            }
            Some(left)
        }
        fn or(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            let mut left = self.and(depth)?;
            while self.tokens.get(self.i) == Some(&Token::Or) {
                self.i += 1;
                left = KeybindingWhenNode::Or {
                    left: Box::new(left),
                    right: Box::new(self.and(depth)?),
                };
            }
            Some(left)
        }
    }
    let mut p = Parser { tokens, i: 0 };
    let node = p.or(0)?;
    if p.i == p.tokens.len() {
        Some(node)
    } else {
        None
    }
}
pub fn encode_when(node: &KeybindingWhenNode) -> String {
    match node {
        KeybindingWhenNode::Identifier { name } => name.clone(),
        KeybindingWhenNode::Not { node } => format!("!({})", encode_when(node)),
        KeybindingWhenNode::And { left, right } => {
            format!("({} && {})", encode_when(left), encode_when(right))
        }
        KeybindingWhenNode::Or { left, right } => {
            format!("({} || {})", encode_when(left), encode_when(right))
        }
    }
}
pub fn compile_rule(rule: &KeybindingRule) -> Option<ResolvedKeybindingRule> {
    let shortcut = parse_shortcut(&rule.key.0)?;
    let when_ast = match &rule.when {
        Some(Some(s)) => Some(Some(parse_when(&s.0)?)),
        Some(None) => return None,
        None => None,
    };
    Some(ResolvedKeybindingRule {
        command: rule.command.clone(),
        shortcut,
        when_ast,
    })
}
fn same(left: &KeybindingRule, right: &KeybindingRule) -> bool {
    compile_rule(left) == compile_rule(right)
}
fn context(left: &KeybindingRule, right: &KeybindingRule) -> bool {
    parse_shortcut(&left.key.0) == parse_shortcut(&right.key.0) && left.when == right.when
}
fn decode_rule(value: &Value) -> Option<KeybindingRule> {
    let rule: KeybindingRule = serde_json::from_value(value.clone()).ok()?;
    compile_rule(&rule)?;
    Some(rule)
}
fn cap<T>(mut items: Vec<T>) -> Vec<T> {
    if items.len() > MAX_KEYBINDINGS_COUNT {
        items.drain(..items.len() - MAX_KEYBINDINGS_COUNT);
    }
    items
}
fn compile_merge(custom: Vec<KeybindingRule>) -> ResolvedKeybindingsConfig {
    let custom = cap(custom
        .into_iter()
        .filter_map(|r| compile_rule(&r))
        .collect());
    let mut all: Vec<_> = default_bindings()
        .iter()
        .filter(|r| !custom.iter().any(|c| c.command == r.command))
        .filter_map(compile_rule)
        .collect();
    all.extend(custom);
    ResolvedKeybindingsConfig(cap(all))
}

pub struct KeybindingsService {
    path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
    cache: tokio::sync::OnceCell<tokio::sync::watch::Sender<KeybindingsState>>,
    changes: crate::events::Changes<KeybindingsState>,
}
impl KeybindingsService {
    pub fn new(path: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            path: path.into(),
            write_lock: tokio::sync::Mutex::new(()),
            cache: tokio::sync::OnceCell::new(),
            changes: crate::events::Changes::new(),
        })
    }
    pub fn subscribe_changes(&self) -> tokio::sync::mpsc::UnboundedReceiver<KeybindingsState> {
        self.changes.subscribe()
    }
    fn raw(&self) -> Result<Vec<Value>> {
        let Some(raw) = files::read(&self.path)? else {
            return Ok(Vec::new());
        };
        files::lenient_json(&raw)?
            .as_array()
            .cloned()
            .ok_or_else(|| invalid("keybindings", "expected JSON array"))
    }
    fn writable(&self) -> Result<Vec<Value>> {
        self.raw()?
            .into_iter()
            .map(|entry| match decode_rule(&entry) {
                Some(rule) => Ok(serde_json::to_value(rule)?),
                None => Ok(entry),
            })
            .collect()
    }
    fn load(&self) -> Result<KeybindingsState> {
        Ok(self.load_custom()?.0)
    }
    fn load_custom(&self) -> Result<(KeybindingsState, Vec<KeybindingRule>)> {
        let raw = match self.raw() {
            Ok(raw) => raw,
            Err(crate::Error::Io {
                operation,
                path,
                source,
            }) => {
                return Err(crate::Error::Io {
                    operation,
                    path,
                    source,
                });
            }
            Err(error) => {
                return Ok((
                    KeybindingsState {
                        keybindings: compile_merge(Vec::new()),
                        issues: vec![malformed(format!("expected JSON array ({error})"))],
                    },
                    Vec::new(),
                ));
            }
        };
        let mut rules = Vec::new();
        let mut issues = Vec::new();
        for (index, value) in raw.iter().enumerate() {
            if let Some(rule) = decode_rule(value) {
                rules.push(rule);
            } else {
                issues.push(invalid_entry(index, "Invalid keybinding rule".into()));
            }
        }
        Ok((
            KeybindingsState {
                keybindings: compile_merge(rules.clone()),
                issues,
            },
            rules,
        ))
    }
    async fn cache(&self) -> Result<&tokio::sync::watch::Sender<KeybindingsState>> {
        self.cache
            .get_or_try_init(async || Ok(tokio::sync::watch::channel(self.load()?).0))
            .await
    }
    pub async fn snapshot(&self) -> Result<KeybindingsState> {
        Ok(self.cache().await?.borrow().clone())
    }
    pub async fn reload(&self) -> Result<KeybindingsState> {
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let next = self.load()?;
        cache.send_replace(next.clone());
        self.changes.send(next.clone());
        Ok(next)
    }
    pub async fn start(self: &Arc<Self>) -> Result<WatchHandle> {
        let weak = Arc::downgrade(self);
        let watcher = watch::watch(&self.path, false, move || {
            let weak = weak.clone();
            async move {
                if let Some(service) = weak.upgrade() {
                    service.reload().await?;
                }
                Ok(())
            }
        })?;
        self.sync_defaults().await?;
        self.snapshot().await?;
        Ok(watcher)
    }
    pub async fn sync_defaults(&self) -> Result<()> {
        self.sync_defaults_inner(|| {}).await
    }
    async fn sync_defaults_inner(&self, after_load: impl FnOnce()) -> Result<()> {
        const ID: &str = "composer.sendBackground:mod+enter";
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let marker = self.path.with_file_name("keybindings-migrations");
        let mut applied: Vec<String> = files::read(&marker)
            .ok()
            .flatten()
            .unwrap_or_default()
            .split('\n')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let mut seen = std::collections::HashSet::new();
        applied.retain(|id| seen.insert(id.clone()));
        let pending = !applied.iter().any(|s| s == ID);
        let defaults = default_bindings();
        if !self
            .path
            .try_exists()
            .map_err(|e| files::io("inspect", &self.path, e))?
        {
            files::write_json(&self.path, &defaults, true).await?;
            if pending {
                applied.push(ID.into());
                files::atomic_write(&marker, format!("{}\n", applied.join("\n")).as_bytes())
                    .await?;
            }
            cache.send_replace(self.load()?);
            return Ok(());
        }
        let (loaded, custom) = self.load_custom()?;
        after_load();
        if !loaded.issues.is_empty() {
            cache.send_replace(self.load()?);
            return Ok(());
        }
        let mut missing = Vec::new();
        for default in &defaults {
            if custom.iter().any(|c| c.command == default.command) {
                continue;
            }
            if custom.iter().any(|c| context(c, default)) {
                tracing::warn!(key = %default.key.0, command = %default.command.0, "skipping default keybinding due to shortcut conflict");
                continue;
            }
            missing.push(default.clone());
        }
        let late = defaults
            .iter()
            .find(|r| r.command.0 == "composer.sendBackground" && r.key.0 == "mod+enter")
            .cloned();
        let alongside = defaults
            .iter()
            .find(|r| r.command.0 == "composer.sendBackground" && r.key.0 == "mod+alt+enter");
        if pending
            && alongside.is_some_and(|a| custom.iter().any(|c| same(c, a)))
            && late
                .as_ref()
                .is_some_and(|r| !custom.iter().any(|c| context(c, r)))
            && let Some(late) = &late
        {
            missing.push(late.clone());
        }
        let slots = MAX_KEYBINDINGS_COUNT.saturating_sub(custom.len());
        let late_skipped = pending
            && late
                .as_ref()
                .is_some_and(|late| missing.iter().skip(slots).any(|r| same(r, late)));
        missing.truncate(slots);
        if !missing.is_empty() {
            missing.extend(custom);
            files::write_json(&self.path, &missing, true).await?;
        }
        if pending && !late_skipped {
            applied.push(ID.into());
            files::atomic_write(&marker, format!("{}\n", applied.join("\n")).as_bytes()).await?;
        }
        cache.send_replace(self.load()?);
        Ok(())
    }
    pub async fn upsert(
        &self,
        rule: KeybindingRule,
        replace: Option<KeybindingRule>,
    ) -> Result<KeybindingsState> {
        if compile_rule(&rule).is_none() {
            return Err(invalid("keybindings", "Invalid keybinding rule"));
        }
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let mut raw = self.writable()?;
        raw.retain(|v| {
            decode_rule(v).is_none_or(|r| {
                !same(&r, &rule) && replace.as_ref().is_none_or(|target| !same(&r, target))
            })
        });
        raw.push(serde_json::to_value(rule)?);
        if raw.len() > MAX_KEYBINDINGS_COUNT {
            tracing::warn!(
                max_entries = MAX_KEYBINDINGS_COUNT,
                "truncating keybindings config to max entries"
            );
        }
        files::write_json(&self.path, &cap(raw), true).await?;
        let next = self.load()?;
        cache.send_replace(next.clone());
        self.changes.send(next.clone());
        Ok(next)
    }
    pub async fn remove(&self, rule: &KeybindingRule) -> Result<KeybindingsState> {
        let _write = self.write_lock.lock().await;
        let cache = self.cache().await?;
        let mut raw = self.writable()?;
        raw.retain(|v| decode_rule(v).is_none_or(|r| !same(&r, rule)));
        files::write_json(&self.path, &raw, true).await?;
        let next = self.load()?;
        cache.send_replace(next.clone());
        self.changes.send(next.clone());
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn defaults_use_one_validated_disk_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("keybindings.json");
        std::fs::write(&path, r#"[{"key":"alt+a","command":"chat.new"}]"#).unwrap();
        let service = KeybindingsService::new(&path);
        service.sync_defaults_inner(|| {
            std::fs::write(&path, r#"[{"key":"alt+b","command":"chat.new"},{"key":"alt+c","command":"future.command"}]"#).unwrap();
        }).await.unwrap();
        let saved = service.raw().unwrap();
        assert!(saved.iter().any(|v| v["key"] == "alt+a"));
        assert!(!saved.iter().any(|v| v["key"] == "alt+b"));
    }
}
