//! Shared presentation decisions. Host operations and clocks are supplied by the caller.
pub mod auto_settle;
#[path = "view_logic/chat_view.rs"]
pub mod chat_view;
pub mod clock;
#[path = "view_logic/context_references.rs"]
pub mod context_references;
#[path = "view_logic/errors.rs"]
pub mod errors;
#[path = "view_logic/onboarding.rs"]
pub mod onboarding;
#[path = "view_logic/session.rs"]
pub mod session;
#[path = "view_logic/shell.rs"]
pub mod shell;

pub(crate) fn is_js_whitespace(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

pub trait JsTrim {
    fn js_trim(&self) -> &str;
    fn js_trim_start(&self) -> &str;
    fn js_trim_end(&self) -> &str;
}

impl JsTrim for str {
    fn js_trim(&self) -> &str {
        self.trim_matches(is_js_whitespace)
    }
    fn js_trim_start(&self) -> &str {
        self.trim_start_matches(is_js_whitespace)
    }
    fn js_trim_end(&self) -> &str {
        self.trim_end_matches(is_js_whitespace)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}
