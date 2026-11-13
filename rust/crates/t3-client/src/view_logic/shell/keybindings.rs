//! Key events and focus are host-supplied data; wire rules retain the contracts representation.
use crate::view_logic::JsTrim;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::keybindings::{
    KeybindingCommand, KeybindingRule, KeybindingShortcut, KeybindingWhenNode,
    MAX_KEYBINDINGS_COUNT, MAX_WHEN_EXPRESSION_DEPTH, ResolvedKeybindingRule,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub meta: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: String,
    pub code: String,
    pub modifiers: Modifiers,
    pub event_type: Option<String>,
    pub repeat: bool,
    pub alt_graph: bool,
    pub editable_focus: bool,
}

impl KeyEvent {
    pub fn is_keydown(&self) -> bool {
        self.event_type.as_deref().is_none_or(|v| v == "keydown")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchContext(pub BTreeMap<String, bool>);
impl MatchContext {
    pub fn new(desktop: bool) -> Self {
        Self(BTreeMap::from([
            ("isDesktop".into(), desktop),
            ("isWeb".into(), !desktop),
        ]))
    }
    pub fn get(&self, key: &str) -> bool {
        self.0.get(key).copied().unwrap_or(false)
    }
    pub fn with(mut self, key: &str, value: bool) -> Self {
        self.0.insert(key.into(), value);
        self
    }
}

pub fn is_mac_platform(platform: &str) -> bool {
    let platform = platform.to_lowercase();
    ["mac", "iphone", "ipad", "ipod"]
        .iter()
        .any(|s| platform.contains(s))
}
pub fn normalize_event_key(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "esc" => "escape".into(),
        value => value.into(),
    }
}
fn physical_key(code: &str) -> Option<&str> {
    match code {
        "Backquote" => Some("`"),
        "Backslash" => Some("\\"),
        "BracketLeft" => Some("["),
        "BracketRight" => Some("]"),
        "Comma" => Some(","),
        "Equal" => Some("="),
        "Minus" => Some("-"),
        "Period" => Some("."),
        "Quote" => Some("'"),
        "Semicolon" => Some(";"),
        "Slash" => Some("/"),
        value
            if value.len() == 6
                && value.starts_with("Digit")
                && value.as_bytes()[5].is_ascii_digit() =>
        {
            Some(&value[5..])
        }
        _ => None,
    }
}
fn latin_letter(value: &str) -> bool {
    value.len() == 1 && value.as_bytes()[0].is_ascii_lowercase()
}
pub fn shortcut_key_from_event(event: &KeyEvent) -> String {
    let layout = normalize_event_key(&event.key);
    if latin_letter(&layout)
        || (layout == "+"
            && !event.modifiers.shift
            && !event.modifiers.alt
            && !event.code.starts_with("Digit"))
    {
        return layout;
    }
    physical_key(&event.code)
        .map(String::from)
        .unwrap_or(layout)
}
pub fn resolve_event_keys(event: &KeyEvent) -> BTreeSet<String> {
    let layout = normalize_event_key(&event.key);
    let mut keys = BTreeSet::from([layout.clone(), shortcut_key_from_event(event)]);
    if !latin_letter(&layout)
        && event.code.len() == 4
        && event.code.starts_with("Key")
        && event.code.as_bytes()[3].is_ascii_uppercase()
    {
        keys.insert(event.code[3..].to_ascii_lowercase());
    }
    keys
}
pub fn shortcut_modifiers(shortcut: &KeybindingShortcut, platform: &str) -> Modifiers {
    let mac = is_mac_platform(platform);
    Modifiers {
        meta: shortcut.meta_key || (shortcut.mod_key && mac),
        ctrl: shortcut.ctrl_key || (shortcut.mod_key && !mac),
        shift: shortcut.shift_key,
        alt: shortcut.alt_key,
    }
}
pub fn matches_shortcut(event: &KeyEvent, shortcut: &KeybindingShortcut, platform: &str) -> bool {
    if !is_mac_platform(platform)
        && event.alt_graph
        && !(event.key.len() == 1 && event.key.as_bytes()[0].is_ascii_alphanumeric())
    {
        return false;
    }
    event.modifiers == shortcut_modifiers(shortcut, platform)
        && resolve_event_keys(event).contains(&shortcut.key)
}
pub fn evaluate_when(node: &KeybindingWhenNode, context: &MatchContext) -> bool {
    match node {
        KeybindingWhenNode::Identifier { name } => match name.as_str() {
            "true" => true,
            "false" => false,
            _ => context.get(name),
        },
        KeybindingWhenNode::Not { node } => !evaluate_when(node, context),
        KeybindingWhenNode::And { left, right } => {
            evaluate_when(left, context) && evaluate_when(right, context)
        }
        KeybindingWhenNode::Or { left, right } => {
            evaluate_when(left, context) || evaluate_when(right, context)
        }
    }
}
fn enabled(binding: &ResolvedKeybindingRule, context: &MatchContext) -> bool {
    binding
        .when_ast
        .as_ref()
        .and_then(Option::as_ref)
        .is_none_or(|node| evaluate_when(node, context))
}
fn contextual_shortcut(
    binding: &ResolvedKeybindingRule,
    context: &MatchContext,
) -> KeybindingShortcut {
    let mut shortcut = binding.shortcut.clone();
    if context.get("composerRequiresModifier")
        && matches!(
            binding.command.0.as_str(),
            "composer.sendAlternate" | "composer.sendBackground"
        )
        && shortcut.key == "enter"
        && shortcut.mod_key
        && !shortcut.meta_key
        && !shortcut.ctrl_key
        && !shortcut.alt_key
        && !shortcut.shift_key
    {
        shortcut.shift_key = true;
    }
    shortcut
}
pub fn shortcut_conflict_key(shortcut: &KeybindingShortcut, platform: &str) -> String {
    let m = shortcut_modifiers(shortcut, platform);
    format!(
        "{}|{}|{}|{}|{}",
        shortcut.key,
        if m.meta { "meta" } else { "" },
        if m.ctrl { "ctrl" } else { "" },
        if m.shift { "shift" } else { "" },
        if m.alt { "alt" } else { "" }
    )
}
pub fn effective_shortcuts_for_command(
    bindings: &[ResolvedKeybindingRule],
    command: &str,
    platform: &str,
    context: &MatchContext,
) -> Vec<KeybindingShortcut> {
    let mut claimed = BTreeSet::new();
    bindings
        .iter()
        .rev()
        .filter(|b| enabled(b, context))
        .filter_map(|b| {
            let shortcut = contextual_shortcut(b, context);
            (claimed.insert(shortcut_conflict_key(&shortcut, platform)) && b.command.0 == command)
                .then_some(shortcut)
        })
        .collect()
}
pub fn resolve_shortcut_command<'a>(
    event: &KeyEvent,
    bindings: &'a [ResolvedKeybindingRule],
    platform: &str,
    context: &MatchContext,
) -> Option<&'a KeybindingCommand> {
    let mut context = context.clone();
    context
        .0
        .entry("editableFocus".into())
        .or_insert(event.editable_focus);
    bindings
        .iter()
        .rev()
        .find(|b| {
            enabled(b, &context)
                && matches_shortcut(event, &contextual_shortcut(b, &context), platform)
        })
        .map(|b| &b.command)
}
pub fn resolve_chat_shortcut_command<'a>(
    event: &KeyEvent,
    primary: &'a [ResolvedKeybindingRule],
    active: &'a [ResolvedKeybindingRule],
    platform: &str,
    context: &MatchContext,
) -> Option<&'a KeybindingCommand> {
    if let Some(command) = resolve_shortcut_command(event, primary, platform, context)
        .filter(|c| !c.0.starts_with("script."))
    {
        return Some(command);
    }
    resolve_shortcut_command(event, active, platform, context)
        .filter(|c| c.0.starts_with("script."))
}
pub fn format_shortcut_key_label(key: &str) -> String {
    match key {
        " " => "Space".into(),
        "escape" => "Esc".into(),
        "arrowup" => "Up".into(),
        "arrowdown" => "Down".into(),
        "arrowleft" => "Left".into(),
        "arrowright" => "Right".into(),
        value => {
            let mut chars = value.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}
pub fn format_shortcut_label(shortcut: &KeybindingShortcut, platform: &str) -> String {
    let m = shortcut_modifiers(shortcut, platform);
    let label = format_shortcut_key_label(&shortcut.key);
    if is_mac_platform(platform) {
        return format!(
            "{}{}{}{}{}",
            if m.ctrl { "⌃" } else { "" },
            if m.alt { "⌥" } else { "" },
            if m.shift { "⇧" } else { "" },
            if m.meta { "⌘" } else { "" },
            label
        );
    }
    let mut parts = Vec::new();
    for (held, text) in [
        (m.ctrl, "Ctrl"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
        (m.meta, "Meta"),
    ] {
        if held {
            parts.push(text);
        }
    }
    parts.push(&label);
    parts.join("+")
}
pub fn shortcut_label_for_command(
    bindings: &[ResolvedKeybindingRule],
    command: Option<&str>,
    platform: &str,
    context: &MatchContext,
) -> Option<String> {
    effective_shortcuts_for_command(bindings, command?, platform, context)
        .first()
        .map(|s| format_shortcut_label(s, platform))
}
pub fn thread_jump_command_for_index(index: usize) -> Option<String> {
    (index < 9).then(|| format!("thread.jump.{}", index + 1))
}
pub fn model_picker_jump_command_for_index(index: usize) -> Option<String> {
    (index < 9).then(|| format!("modelPicker.jump.{}", index + 1))
}
fn jump_index(command: &str, prefix: &str) -> Option<usize> {
    let number = command.strip_prefix(prefix)?;
    (number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'))
        .then(|| usize::from(number.as_bytes()[0] - b'1'))
}
pub fn thread_jump_index_from_command(command: &str) -> Option<usize> {
    jump_index(command, "thread.jump.")
}
pub fn model_picker_jump_index_from_command(command: &str) -> Option<usize> {
    jump_index(command, "modelPicker.jump.")
}
pub fn thread_traversal_direction(command: &str) -> Option<&'static str> {
    match command {
        "thread.previous" => Some("previous"),
        "thread.next" => Some("next"),
        _ => None,
    }
}
pub fn is_thread_navigation_shortcut(
    event: &KeyEvent,
    bindings: &[ResolvedKeybindingRule],
    platform: &str,
    context: &MatchContext,
) -> bool {
    resolve_shortcut_command(event, bindings, platform, context).is_some_and(|c| {
        thread_jump_index_from_command(&c.0).is_some() || thread_traversal_direction(&c.0).is_some()
    })
}
pub fn should_show_thread_jump_hints(
    modifiers: Modifiers,
    bindings: &[ResolvedKeybindingRule],
    platform: &str,
    context: &MatchContext,
) -> bool {
    !context.get("terminalFocus")
        && (0..9).any(|i| {
            effective_shortcuts_for_command(
                bindings,
                &format!("thread.jump.{}", i + 1),
                platform,
                context,
            )
            .first()
            .is_some_and(|s| shortcut_modifiers(s, platform) == modifiers)
        })
}
pub fn is_command_shortcut(
    event: &KeyEvent,
    bindings: &[ResolvedKeybindingRule],
    command: &str,
    platform: &str,
    context: &MatchContext,
) -> bool {
    (command != "editor.openFavorite" || !event.repeat)
        && resolve_shortcut_command(event, bindings, platform, context)
            .is_some_and(|c| c.0 == command)
}
pub fn is_rich_text_formatting_shortcut(event: &KeyEvent) -> bool {
    let m = event.modifiers;
    (m.meta || m.ctrl)
        && !m.alt
        && !m.shift
        && resolve_event_keys(event)
            .iter()
            .any(|k| matches!(k.as_str(), "b" | "i" | "e"))
}
pub fn is_rich_text_bold_shortcut(event: &KeyEvent, selection_collapsed: Option<bool>) -> bool {
    event.is_keydown()
        && selection_collapsed == Some(false)
        && is_rich_text_formatting_shortcut(event)
        && resolve_event_keys(event).contains("b")
}
pub fn is_terminal_clear_shortcut(event: &KeyEvent, platform: &str) -> bool {
    let m = event.modifiers;
    event.is_keydown()
        && !m.alt
        && !m.shift
        && ((event.key.to_lowercase() == "l" && m.ctrl && !m.meta)
            || (is_mac_platform(platform) && event.key.to_lowercase() == "k" && m.meta && !m.ctrl))
}
pub fn terminal_delete_shortcut_data(event: &KeyEvent, platform: &str) -> Option<&'static str> {
    let m = event.modifiers;
    (event.is_keydown()
        && is_mac_platform(platform)
        && normalize_event_key(&event.key) == "backspace"
        && m.meta
        && !m.ctrl
        && !m.alt
        && !m.shift)
        .then_some("\u{15}")
}
pub fn terminal_navigation_shortcut_data(event: &KeyEvent, platform: &str) -> Option<&'static str> {
    if !event.is_keydown() || event.modifiers.shift {
        return None;
    }
    let left = match normalize_event_key(&event.key).as_str() {
        "arrowleft" => true,
        "arrowright" => false,
        _ => return None,
    };
    let m = event.modifiers;
    if m.alt && !m.meta && !m.ctrl {
        return Some(if left { "\u{1b}b" } else { "\u{1b}f" });
    }
    (is_mac_platform(platform) && m.meta && !m.alt && !m.ctrl).then_some(if left {
        "\u{1}"
    } else {
        "\u{5}"
    })
}

pub fn parse_shortcut(value: &str) -> Option<KeybindingShortcut> {
    let lower = value.to_lowercase();
    let mut tokens: Vec<_> = lower.split('+').map(JsTrim::js_trim).collect();
    let trailing = tokens.last() == Some(&"");
    while tokens.last() == Some(&"") {
        tokens.pop();
    }
    if trailing {
        tokens.push("+");
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
            "" => return None,
            "cmd" | "meta" => shortcut.meta_key = true,
            "ctrl" | "control" => shortcut.ctrl_key = true,
            "shift" => shortcut.shift_key = true,
            "alt" | "option" => shortcut.alt_key = true,
            "mod" => shortcut.mod_key = true,
            value => {
                if !shortcut.key.is_empty() {
                    return None;
                }
                shortcut.key = match value {
                    "space" => " ",
                    "esc" => "escape",
                    value => value,
                }
                .into();
            }
        }
    }
    (!shortcut.key.is_empty()).then_some(shortcut)
}

#[derive(Debug, Clone, PartialEq)]
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
    let mut chars = expression.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if crate::view_logic::is_js_whitespace(c) => {}
            '!' => tokens.push(Token::Not),
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            '&' if chars.next() == Some('&') => tokens.push(Token::And),
            '|' if chars.next() == Some('|') => tokens.push(Token::Or),
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut name = c.to_string();
                while let Some(&c) = chars
                    .peek()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                {
                    chars.next();
                    name.push(c);
                }
                tokens.push(Token::Identifier(name));
            }
            _ => return None,
        }
    }
    struct Parser {
        tokens: Vec<Token>,
        index: usize,
    }
    impl Parser {
        fn unary(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            if depth > MAX_WHEN_EXPRESSION_DEPTH {
                return None;
            }
            let mut count = 0;
            while self.tokens.get(self.index) == Some(&Token::Not) {
                self.index += 1;
                count += 1;
                if count > MAX_WHEN_EXPRESSION_DEPTH {
                    return None;
                }
            }
            let mut node = match self.tokens.get(self.index)?.clone() {
                Token::Identifier(name) => {
                    self.index += 1;
                    KeybindingWhenNode::Identifier { name }
                }
                Token::Open => {
                    self.index += 1;
                    let node = self.or(depth + 1)?;
                    if self.tokens.get(self.index) != Some(&Token::Close) {
                        return None;
                    }
                    self.index += 1;
                    node
                }
                _ => return None,
            };
            for _ in 0..count {
                node = KeybindingWhenNode::Not {
                    node: Box::new(node),
                };
            }
            Some(node)
        }
        fn and(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            let mut left = self.unary(depth)?;
            while self.tokens.get(self.index) == Some(&Token::And) {
                self.index += 1;
                left = KeybindingWhenNode::And {
                    left: Box::new(left),
                    right: Box::new(self.unary(depth)?),
                };
            }
            Some(left)
        }
        fn or(&mut self, depth: usize) -> Option<KeybindingWhenNode> {
            let mut left = self.and(depth)?;
            while self.tokens.get(self.index) == Some(&Token::Or) {
                self.index += 1;
                left = KeybindingWhenNode::Or {
                    left: Box::new(left),
                    right: Box::new(self.and(depth)?),
                };
            }
            Some(left)
        }
    }
    let mut parser = Parser { tokens, index: 0 };
    let node = parser.or(0)?;
    (parser.index == parser.tokens.len()).then_some(node)
}
pub fn compile_rule(rule: &KeybindingRule) -> Option<ResolvedKeybindingRule> {
    let when_ast = match &rule.when {
        None => None,
        Some(Some(value)) => Some(Some(parse_when(&value.0)?)),
        Some(None) => return None,
    };
    Some(ResolvedKeybindingRule {
        command: rule.command.clone(),
        shortcut: parse_shortcut(&rule.key.0)?,
        when_ast,
    })
}
pub fn compile_config(config: &[KeybindingRule]) -> Vec<ResolvedKeybindingRule> {
    let mut compiled: Vec<_> = config.iter().filter_map(compile_rule).collect();
    compiled.drain(..compiled.len().saturating_sub(MAX_KEYBINDINGS_COUNT));
    compiled
}
pub fn merge_with_defaults(custom: &[ResolvedKeybindingRule]) -> Vec<ResolvedKeybindingRule> {
    let mut merged: Vec<_> = default_keybindings()
        .into_iter()
        .filter(|b| !custom.iter().any(|c| c.command == b.command))
        .chain(custom.iter().cloned())
        .collect();
    merged.drain(..merged.len().saturating_sub(MAX_KEYBINDINGS_COUNT));
    merged
}

include!("keybinding_defaults.rs");

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifierState {
    pub modifiers: Modifiers,
    pub ignore_editable: bool,
}
impl ModifierState {
    pub fn keyboard(&mut self, event: &KeyEvent) {
        if self.ignore_editable && event.editable_focus {
            self.reset();
            return;
        }
        let down = event.event_type.as_deref() == Some("keydown");
        match event.key.as_str() {
            "Meta" | "OS" | "Command" => self.modifiers.meta = down,
            "Control" => self.modifiers.ctrl = down,
            "Alt" | "Option" => self.modifiers.alt = down,
            "Shift" => self.modifiers.shift = down,
            _ => {
                self.modifiers.meta &= event.modifiers.meta;
                self.modifiers.ctrl &= event.modifiers.ctrl;
                self.modifiers.alt &= event.modifiers.alt;
                self.modifiers.shift &= event.modifiers.shift;
            }
        }
    }
    pub fn focus(&mut self, editable: bool) {
        if self.ignore_editable && editable {
            self.reset();
        }
    }
    /// The host calls this on paste, blur, and removal of the final subscriber.
    pub fn reset(&mut self) {
        self.modifiers = Modifiers::default();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateToast {
    Success,
    InvalidConfiguration(String),
}
#[derive(Debug, Clone, Default)]
pub struct UpdateToastController {
    last: Option<(UpdateToast, i64)>,
}
impl UpdateToastController {
    pub fn handle(
        &mut self,
        event: Option<&t3_contracts::server::ServerConfigStreamEvent>,
        now_ms: i64,
    ) -> Option<UpdateToast> {
        use t3_contracts::server::ServerConfigStreamEvent;
        let ServerConfigStreamEvent::KeybindingsUpdated(event) = event? else {
            return None;
        };
        use t3_contracts::server::ServerConfigIssue;
        let decision = event
            .payload
            .issues
            .0
            .first()
            .map(|issue| {
                UpdateToast::InvalidConfiguration(match issue {
                    ServerConfigIssue::MalformedConfig(issue) => issue.message.0.clone(),
                    ServerConfigIssue::InvalidEntry(issue) => issue.message.0.clone(),
                })
            })
            .unwrap_or(UpdateToast::Success);
        if self.last.as_ref().is_some_and(|(last, at)| {
            last == &decision && i128::from(now_ms) - i128::from(*at) < 2_000
        }) {
            return None;
        }
        self.last = Some((decision.clone(), now_ms));
        Some(decision)
    }
}
