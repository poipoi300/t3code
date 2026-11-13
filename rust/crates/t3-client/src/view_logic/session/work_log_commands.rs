use crate::view_logic::{JsTrim, is_js_whitespace};
use serde::Serialize;
include!("work_log_command_tables.rs");

#[derive(Debug, Clone)]
pub(super) struct Word {
    pub text: String,
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
}
fn program(value: &str) -> Option<&str> {
    let value = value.js_trim();
    let name = value.rsplit(['/', '\\']).next()?;
    if name.is_empty()
        || name.starts_with(|c| "<>(){}[];|&$`#!%@:".contains(c))
        || name.ends_with(|c| "){]}`".contains(c))
        || (name.chars().any(is_js_whitespace) && !value.contains(['/', '\\']))
    {
        return None;
    }
    if let Some((scheme, rest)) = value.split_once(':')
        && !rest.starts_with(['/', '\\'])
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    {
        return None;
    }
    Some(name)
}
fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let name = name.strip_suffix('+').unwrap_or(name);
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    })
}
pub(super) fn tokenize(input: &str) -> Option<Vec<Word>> {
    let chars: Vec<_> = input.char_indices().collect();
    let mut words = vec![];
    let (mut quote, mut escaped, mut backticks, mut depth, mut braces, mut started) =
        (None, false, false, 0_usize, 0_usize, false);
    let (mut text, mut starts, mut ends) = (String::new(), vec![], vec![]);
    let mut pending_start = 0;
    let mut utf16_offset = 0;
    for (index, (_, ch)) in chars.iter().copied().enumerate() {
        let offset = utf16_offset;
        let end = offset + ch.len_utf16();
        utf16_offset = end;
        let windows = quote.is_none()
            && (text.starts_with(|c: char| c.is_ascii_alphabetic())
                && text.as_bytes().get(1) == Some(&b':')
                || text.starts_with(".\\")
                || text.starts_with("..\\"));
        let mut emit = |ch: char, start: usize, end: usize| {
            text.push(ch);
            for unit in 0..ch.len_utf16() {
                starts.push(
                    start
                        + if end - start == ch.len_utf16() {
                            unit
                        } else {
                            0
                        },
                );
                ends.push(if end - start == ch.len_utf16() {
                    start + unit + 1
                } else {
                    end
                });
            }
        };
        if escaped {
            emit(ch, pending_start, end);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            let next = chars.get(index + 1).map(|(_, c)| *c);
            if (quote == Some('"') || windows)
                && next.is_some_and(|c| !matches!(c, '"' | '\\' | '$' | '`' | '\n'))
            {
                emit(ch, offset, end);
                started = true;
                continue;
            }
            escaped = true;
            pending_start = offset;
            started = true;
            continue;
        }
        if backticks {
            emit(ch, offset, end);
            if ch == '`' {
                backticks = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                emit(ch, offset, end);
            }
            started = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            started = true;
            continue;
        }
        if ch == '`' {
            backticks = true;
            emit(ch, offset, end);
            started = true;
            continue;
        }
        if ch == '(' {
            depth += 1;
        }
        if ch == ')' && depth > 0 {
            depth -= 1;
        }
        if ch == '{'
            && (braces > 0
                || chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|(_, c)| *c == '$'))
        {
            braces += 1;
        }
        if ch == '}' && braces > 0 {
            braces -= 1;
        }
        if is_js_whitespace(ch) && depth == 0 && braces == 0 {
            if started {
                words.push(Word {
                    text: std::mem::take(&mut text),
                    starts: std::mem::take(&mut starts),
                    ends: std::mem::take(&mut ends),
                });
                started = false;
            }
            continue;
        }
        emit(ch, offset, end);
        started = true;
    }
    if quote.is_some() || escaped || backticks || depth > 0 || braces > 0 {
        return None;
    }
    if started {
        words.push(Word { text, starts, ends });
    }
    Some(words)
}
#[derive(Debug)]
pub(super) struct CommandSplit {
    pub first: String,
    pub rest: Option<String>,
    pub separator: String,
}
pub(super) fn split_first(input: &str) -> CommandSplit {
    let chars: Vec<_> = input.char_indices().collect();
    let mut quote = None;
    let mut escape = false;
    let mut backticks = false;
    let mut depth = 0_usize;
    let mut braces = 0_usize;
    let mut comment = false;
    let mut clean = String::new();
    let mut heredocs: Vec<(String, bool)> = vec![];
    let mut first_separator = None;
    let mut skip_until = 0;
    for (index, (byte, ch)) in chars.iter().copied().enumerate() {
        if byte < skip_until {
            continue;
        }
        if comment {
            if ch != '\n' {
                continue;
            }
            comment = false;
        }
        if escape {
            clean.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            clean.push(ch);
            escape = true;
            continue;
        }
        if backticks {
            clean.push(ch);
            if ch == '`' {
                backticks = false;
            }
            continue;
        }
        if let Some(q) = quote {
            clean.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            clean.push(ch);
            continue;
        }
        if ch == '`' {
            backticks = true;
            clean.push(ch);
            continue;
        }
        if ch == '#'
            && (index == 0
                || chars
                    .get(index - 1)
                    .is_some_and(|(_, c)| is_js_whitespace(*c) || ";&|(".contains(*c)))
        {
            comment = true;
            continue;
        }
        if ch == '(' {
            depth += 1;
        }
        if ch == ')' && depth > 0 {
            depth -= 1;
        }
        if ch == '{'
            && (braces > 0
                || chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|(_, c)| *c == '$'))
        {
            braces += 1;
        }
        if ch == '}' && braces > 0 {
            braces -= 1;
        }
        if depth > 0 || braces > 0 {
            clean.push(ch);
            continue;
        }
        if input[byte..].starts_with("<<") && !input[byte..].starts_with("<<<") {
            let strip = input[byte..].starts_with("<<-");
            let offset = byte + if strip { 3 } else { 2 };
            let tail = input[offset..].js_trim_start();
            let delimiter = tail.split(is_js_whitespace).next().unwrap_or_default();
            if let Some(words) = tokenize(delimiter)
                && let Some(word) = words.first()
            {
                heredocs.push((word.text.clone(), strip));
            }
        }
        let double = input[byte..].starts_with("&&")
            || input[byte..].starts_with("||")
            || input[byte..].starts_with("|&");
        let redirection = ch == '&'
            && (input[byte + 1..].starts_with('>')
                || chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|(_, c)| matches!(c, '>' | '<')));
        if ";&|\n".contains(ch) && !redirection {
            let length = if double { 2 } else { 1 };
            if !heredocs.is_empty() {
                if ch != '\n' {
                    first_separator.get_or_insert((clean.len(), byte, length));
                    clean.push(ch);
                    continue;
                }
                let mut remaining = &input[byte + 1..];
                for (delimiter, strip) in heredocs {
                    let mut found = false;
                    while !remaining.is_empty() {
                        let (line, rest) = remaining.split_once('\n').unwrap_or((remaining, ""));
                        remaining = rest;
                        let line = line.strip_suffix('\r').unwrap_or(line);
                        if (if strip {
                            line.trim_start_matches('\t')
                        } else {
                            line
                        }) == delimiter
                        {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        remaining = "";
                        break;
                    }
                }
                let (first, between, separator) = first_separator.map_or(
                    (clean.clone(), "".to_string(), "\n".to_string()),
                    |(clean_index, position, length)| {
                        (
                            clean[..clean_index].into(),
                            input[position + length..byte].js_trim().into(),
                            input[position..position + length].into(),
                        )
                    },
                );
                let rest = [between.as_str(), remaining.js_trim()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                return CommandSplit {
                    first: first.js_trim_start().into(),
                    rest: (!rest.is_empty()).then_some(rest),
                    separator,
                };
            }
            return CommandSplit {
                first: clean.js_trim_start().into(),
                rest: (!input[byte + length..].js_trim().is_empty())
                    .then(|| input[byte + length..].js_trim().into()),
                separator: input[byte..byte + length].into(),
            };
        }
        clean.push(ch);
        if double {
            skip_until = byte + 2;
        }
    }
    CommandSplit {
        first: clean.js_trim().into(),
        rest: None,
        separator: String::new(),
    }
}
fn redirection(words: &[Word], index: usize) -> Option<usize> {
    let token = &words.get(index)?.text;
    if token.starts_with("<(") || token.starts_with(">(") {
        return None;
    }
    let operator = token.find(['<', '>'])?;
    let prefix = &token[..operator];
    if !prefix.is_empty()
        && !prefix.bytes().all(|c| c.is_ascii_digit())
        && prefix != "&"
        && prefix != "*"
        && !(prefix.starts_with('{') && prefix.ends_with('}'))
    {
        return None;
    }
    let rest = token[operator..].trim_start_matches(['<', '>', '-', '|', '&']);
    Some(index + if rest.is_empty() { 2 } else { 1 })
}
pub(super) fn shell_script_index(words: &[Word], start: usize) -> Option<usize> {
    let mut i = start;
    while let Some(word) = words.get(i) {
        let flag = word.text.as_str();
        if flag == "--" || !flag.starts_with('-') {
            return None;
        }
        if ["-o", "-O", "--rcfile", "--init-file"].contains(&flag) {
            i += 2;
            continue;
        }
        if flag == "--command"
            || (flag.starts_with('-')
                && flag[1..].bytes().all(|c| c.is_ascii_alphabetic())
                && flag.contains('c'))
        {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}
fn parse_program(command: &str, depth: usize, exec: bool, remaining: usize) -> Option<String> {
    if depth >= 8 || remaining == 0 {
        return None;
    }
    let split = split_first(command);
    let first = split.first.js_trim();
    let next = |exec| {
        split
            .rest
            .as_ref()
            .and_then(|rest| parse_program(rest, depth, exec, remaining - 1))
    };
    if let Some(tail) = first
        .strip_prefix('&')
        .filter(|s| s.starts_with(is_js_whitespace))
    {
        let words = tokenize(tail.js_trim())?;
        return program(&words.first()?.text).map(str::to_string);
    }
    if first.starts_with("[[") || first.contains("() {") || first.contains("(){") {
        return None;
    }
    let words = tokenize(first)?;
    let mut i = 0;
    let mut wrapper = None;
    let mut execution = exec;
    if first.starts_with('$') && first.contains('=') {
        let (name, value) = first.split_once('=')?;
        if name.to_ascii_lowercase().starts_with("$env:") {
            return next(false);
        }
        let value = value.js_trim();
        if value.starts_with(['\'', '"', '[', '@'])
            || value.starts_with(|c: char| c.is_ascii_digit())
        {
            return next(false);
        }
        return parse_program(value, depth + 1, false, remaining).or_else(|| next(false));
    }
    while let Some(word) = words.get(i) {
        let token = word.text.as_str();
        if token.is_empty() {
            return None;
        }
        if let Some(after) = redirection(&words, i) {
            if after > words.len() {
                return None;
            }
            i = after;
            continue;
        }
        if assignment(token) {
            i += 1;
            continue;
        }
        if !execution && token == ":" {
            return next(execution);
        }
        let name = program(token)?;
        let unqualified = token == name;
        if matches!(name, "env" | "sudo") {
            wrapper = Some(name);
            execution = true;
            i += 1;
            continue;
        }
        if let Some(wrapper) = wrapper
            && token.starts_with('-')
        {
            if token == "--" {
                i += 1;
                continue;
            }
            let (value_options, flags): (&[&str], &[&str]) = if wrapper == "env" {
                (
                    &["-C", "--chdir", "-S", "--split-string", "-u", "--unset"],
                    &[
                        "-0",
                        "--null",
                        "-i",
                        "--ignore-environment",
                        "--debug",
                        "-v",
                    ],
                )
            } else {
                (
                    &[
                        "-C",
                        "--close-from",
                        "-D",
                        "--chdir",
                        "-g",
                        "--group",
                        "-u",
                        "--user",
                    ],
                    &[
                        "-A",
                        "--askpass",
                        "-b",
                        "--background",
                        "-E",
                        "-H",
                        "-i",
                        "-n",
                        "-S",
                    ],
                )
            };
            if wrapper == "env" && (token == "-S" || token == "--split-string") {
                return parse_program(&words.get(i + 1)?.text, depth + 1, true, remaining);
            }
            if let Some(value) = token
                .strip_prefix("--split-string=")
                .filter(|_| wrapper == "env")
            {
                return parse_program(value, depth + 1, true, remaining);
            }
            if value_options.contains(&token) {
                words.get(i + 1)?;
                i += 2;
                continue;
            }
            if flags.contains(&token)
                || token
                    .split_once('=')
                    .is_some_and(|(key, _)| value_options.contains(&key))
            {
                i += 1;
                continue;
            }
            return None;
        }
        let lower = name.to_ascii_lowercase();
        let bare = lower.strip_suffix(".exe").unwrap_or(&lower);
        if SHELL_PROGRAMS.contains(&bare)
            && let Some(script) = shell_script_index(&words, i + 1)
        {
            return parse_program(&words.get(script)?.text, depth + 1, false, remaining);
        }
        if matches!(bare, "pwsh" | "powershell" | "cmd") {
            let mut j = i + 1;
            while let Some(arg) = words.get(j) {
                let flag = arg.text.to_ascii_lowercase();
                if matches!(flag.as_str(), "/c" | "/k" | "-c" | "-command") {
                    let payload = &words.get(j + 1)?.text;
                    let payload = if let Some(rest) = &split.rest {
                        format!("{payload} {} {rest}", split.separator)
                    } else {
                        payload.clone()
                    };
                    return parse_program(&payload, depth + 1, false, remaining);
                }
                if matches!(flag.as_str(), "-f" | "-file") {
                    return program(&words.get(j + 1)?.text).map(str::to_string);
                }
                if matches!(flag.as_str(), "-encodedcommand" | "-enc" | "-e") {
                    return None;
                }
                if POWERSHELL_OPTIONS_WITH_VALUE.contains(&flag.as_str()) {
                    j += 2;
                    continue;
                }
                if POWERSHELL_FLAGS.contains(&flag.as_str()) {
                    j += 1;
                    continue;
                }
                if !flag.starts_with('-') && bare != "cmd" {
                    return program(&arg.text).map(str::to_string);
                }
                j += 1;
            }
        }
        if lower == "start-process" {
            let mut j = i + 1;
            while let Some(arg) = words.get(j) {
                let flag = arg.text.to_ascii_lowercase();
                if flag == "-filepath" {
                    return program(&words.get(j + 1)?.text).map(str::to_string);
                }
                if START_PROCESS_OPTIONS_WITH_VALUE.contains(&flag.as_str()) {
                    j += 2;
                    continue;
                }
                if START_PROCESS_FLAGS.contains(&flag.as_str()) {
                    j += 1;
                    continue;
                }
                return program(&arg.text).map(str::to_string);
            }
        }
        if unqualified && matches!(name, "command" | "exec" | "builtin") {
            let mut j = i + 1;
            while let Some(arg) = words.get(j).filter(|w| w.text.starts_with('-')) {
                let flag = arg.text.as_str();
                if flag == "--" {
                    j += 1;
                    break;
                }
                if (name == "command" && flag == "-p")
                    || (name == "exec" && (flag == "-c" || flag == "-l"))
                {
                    j += 1;
                    continue;
                }
                if name == "exec" && flag == "-a" {
                    words.get(j + 1)?;
                    j += 2;
                    continue;
                }
                return None;
            }
            let target = &words.get(j)?.text;
            if assignment(target) {
                return None;
            }
            if redirection(&words, j).is_some_and(|n| n >= words.len()) {
                return None;
            }
            let command = words[j..]
                .iter()
                .map(|w| format!("'{}'", w.text.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ");
            return parse_program(&command, depth + 1, name == "exec", remaining).or_else(|| {
                if name != "exec"
                    && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&target.as_str())
                    && !TERMINAL_SHELL_PROGRAMS.contains(&target.as_str())
                {
                    next(false)
                } else {
                    None
                }
            });
        }
        if unqualified {
            let target = match name {
                "bundle" if words.get(i + 1).is_some_and(|w| w.text == "exec") => Some(i + 2),
                "nohup" => {
                    Some(i + 1 + usize::from(words.get(i + 1).is_some_and(|w| w.text == "--")))
                }
                "script" if words.get(i + 1).is_some_and(|w| w.text.starts_with('-')) => {
                    Some(i + 3)
                }
                "arch" => Some(
                    i + if words.get(i + 1).is_some_and(|w| w.text == "-arch") {
                        3
                    } else {
                        2
                    },
                ),
                "timeout" | "gtimeout"
                    if words.get(i + 1).is_some_and(|w| {
                        w.text
                            .trim_end_matches(['s', 'm', 'h', 'd'])
                            .parse::<f64>()
                            .is_ok()
                    }) =>
                {
                    Some(i + 2)
                }
                _ => None,
            };
            if let Some(target) = target.filter(|t| *t < words.len()) {
                let command = words[target..]
                    .iter()
                    .map(|w| format!("'{}'", w.text.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(name) = parse_program(&command, depth + 1, true, remaining) {
                    return Some(name);
                }
            }
            if !execution && matches!(name, "nocorrect" | "noglob" | "time") {
                i += 1;
                if name == "time" && words.get(i).is_some_and(|w| w.text == "-p") {
                    i += 1;
                }
                continue;
            }
            if NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&name) {
                if (!execution
                    || (wrapper == Some("sudo") && ["[", "[[", "test", "true"].contains(&name)))
                    && (!TERMINAL_SHELL_PROGRAMS.contains(&name)
                        || (name == "false" && split.separator != "&&"))
                {
                    return next(false);
                }
                return None;
            }
            if !execution && POWERSHELL_SETUP_PROGRAMS.contains(&lower.as_str()) {
                return next(false);
            }
        }
        return Some(name.into());
    }
    if wrapper.is_none() {
        next(execution)
    } else {
        None
    }
}
pub fn command_program_name(command: &str) -> Option<String> {
    parse_program(command, 0, false, 64)
}
pub fn with_visible_control_characters(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' => {
                char::from_u32(0x2400 + c as u32).unwrap_or(c)
            }
            '\u{7f}' => '␡',
            _ => c,
        })
        .collect()
}
pub fn command_display_text(command: &str) -> String {
    let trimmed = command.js_trim();
    let split = split_first(trimmed);
    if split.rest.is_none()
        && let Some(words) = tokenize(trimmed)
        && let Some(name) = words.first().and_then(|w| program(&w.text))
        && SHELL_PROGRAMS.contains(&name.strip_suffix(".exe").unwrap_or(name))
        && let Some(script) = shell_script_index(&words, 1)
        && script + 1 == words.len()
    {
        return with_visible_control_characters(words[script].text.js_trim());
    }
    with_visible_control_characters(trimmed)
}
pub fn command_highlight_language(command: &str) -> &'static str {
    let trimmed = command.js_trim();
    if trimmed.starts_with('&') {
        return "powershell";
    }
    if tokenize(trimmed)
        .and_then(|w| w.first().and_then(|w| program(&w.text).map(str::to_string)))
        .is_some_and(|p| {
            matches!(
                p.to_ascii_lowercase().trim_end_matches(".exe"),
                "powershell" | "pwsh"
            )
        })
    {
        return "powershell";
    }
    let mut rest = Some(trimmed.to_string());
    for _ in 0..64 {
        let Some(next) = rest.take() else {
            break;
        };
        let split = split_first(&next);
        let first = split.first.js_trim();
        let word = first.split(is_js_whitespace).next().unwrap_or_default();
        if ["$env:", "$global:", "$local:", "$script:"]
            .iter()
            .any(|p| first.to_ascii_lowercase().starts_with(p))
            || (first.starts_with('$') && first.contains(" ="))
            || word.split_once('-').is_some_and(|(verb, noun)| {
                verb.starts_with(|c: char| c.is_ascii_uppercase())
                    && noun.starts_with(|c: char| c.is_ascii_uppercase())
            })
        {
            return "powershell";
        }
        rest = split.rest;
    }
    "shellscript"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmbeddedScript {
    pub language: String,
    pub text: String,
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
}

type Interpreter = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
    bool,
    bool,
);
fn interpreter(name: &str) -> Option<Interpreter> {
    let sql_values = &[
        "-d",
        "-h",
        "-u",
        "-U",
        "--dbname",
        "--host",
        "--user",
        "--username",
        "-separator",
        "-nullvalue",
        "-init",
    ][..];
    Some(match name {
        "sh" | "bash" | "zsh" | "dash" | "ash" | "ksh" => (
            "shellscript",
            &["--command"],
            &["-o", "-O", "--rcfile", "--init-file"],
            &[],
            true,
            false,
        ),
        "fish" => ("fish", &["-c", "--command"], &[], &[], true, false),
        "python" | "python2" | "python3" | "py" => {
            ("python", &["-c"], &["-W", "-X"], &["-m"], true, false)
        }
        "node" | "nodejs" => (
            "javascript",
            &["-e", "--eval", "-p", "--print"],
            &[
                "-r",
                "--require",
                "--import",
                "--loader",
                "-C",
                "--conditions",
            ],
            &[],
            true,
            false,
        ),
        "bun" => (
            "typescript",
            &["-e", "--eval", "-p", "--print"],
            &[],
            &[],
            false,
            false,
        ),
        "deno" => (
            "typescript",
            &[],
            &["-c", "--config", "--import-map", "--ext"],
            &[],
            false,
            false,
        ),
        "ruby" => ("ruby", &["-e"], &["-r", "-I", "-C"], &[], true, false),
        "perl" => ("perl", &["-e", "-E"], &[], &[], true, false),
        "osascript" => ("applescript", &["-e"], &["-l", "-s"], &[], true, false),
        "pwsh" | "powershell" => (
            "powershell",
            &["-c", "-command"],
            &["-executionpolicy", "-workingdirectory", "-windowstyle"],
            &["-f", "-file", "-encodedcommand"],
            false,
            false,
        ),
        "psql" => (
            "sql",
            &["-c", "--command"],
            sql_values,
            &["-f", "--file"],
            true,
            true,
        ),
        "mysql" | "mariadb" => ("sql", &["-e", "--execute"], sql_values, &[], true, true),
        "sqlite3" => ("sql", &["-cmd"], sql_values, &[], true, true),
        "duckdb" => ("sql", &["-c", "-cmd"], sql_values, &["-f"], true, true),
        _ => return None,
    })
}
fn language_name(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if name.starts_with("python") && name[6..].chars().all(|c| c.is_ascii_digit() || c == '.') {
        "python".into()
    } else {
        name.into()
    }
}
fn file_language(file: &str) -> Option<&'static str> {
    match file.rsplit('.').next()?.to_lowercase().as_str() {
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "py" => Some("python"),
        "rs" => Some("rust"),
        "rb" => Some("ruby"),
        "sh" => Some("shellscript"),
        "sql" => Some("sql"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "html" => Some("html"),
        "css" => Some("css"),
        "md" => Some("markdown"),
        "xml" => Some("xml"),
        "diff" | "patch" => Some("diff"),
        _ => None,
    }
}
fn delimiter_language(delimiter: &str) -> Option<&'static str> {
    match delimiter.to_lowercase().as_str() {
        "py" | "python" => Some("python"),
        "js" | "javascript" => Some("javascript"),
        "ts" | "typescript" => Some("typescript"),
        "sh" | "bash" | "shell" => Some("shellscript"),
        "rb" | "ruby" => Some("ruby"),
        "yml" | "yaml" => Some("yaml"),
        "md" | "markdown" => Some("markdown"),
        "sql" => Some("sql"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "html" => Some("html"),
        "css" => Some("css"),
        "xml" => Some("xml"),
        "diff" | "patch" => Some("diff"),
        _ => None,
    }
}
pub fn embedded_scripts(command: &str) -> Vec<EmbeddedScript> {
    let units = command.encode_utf16().count();
    let source = EmbeddedScript {
        language: "shellscript".into(),
        text: command.into(),
        starts: (0..units).collect(),
        ends: (1..=units).collect(),
    };
    scripts_in(&source, 0)
}
fn scripts_in(source: &EmbeddedScript, depth: usize) -> Vec<EmbeddedScript> {
    if depth >= 4 {
        return vec![];
    }
    let mut quote = None;
    let mut escaped = false;
    let mut backticks = false;
    let mut nesting = 0_usize;
    let mut comment = false;
    let mut ranges = vec![];
    let (mut start, mut start_unit, mut unit) = (0, 0, 0);
    for (index, ch) in source.text.char_indices() {
        let end_unit = unit + ch.len_utf16();
        let current_unit = unit;
        unit = end_unit;
        if comment {
            if ch != '\n' {
                continue;
            }
            comment = false;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if backticks {
            if ch == '`' {
                backticks = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            continue;
        }
        if ch == '`' {
            backticks = true;
            continue;
        }
        if ch == '#'
            && (index == 0
                || source.text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|c| is_js_whitespace(c) || ";&|(".contains(c)))
        {
            comment = true;
            continue;
        }
        if ch == '(' {
            nesting += 1;
        } else if ch == ')' {
            nesting = nesting.saturating_sub(1);
        }
        if nesting > 0 {
            continue;
        }
        if source.text[index..].starts_with("<<") {
            return scripts_in_command(source, depth);
        }
        if ";|&\n".contains(ch) {
            let redirection = ch == '&'
                && (source.text[..index].ends_with(['<', '>'])
                    || source.text[index + 1..].starts_with('>'));
            if redirection {
                continue;
            }
            if start < index {
                ranges.push((start, index, start_unit, current_unit));
            }
            start = index + ch.len_utf8();
            start_unit = end_unit;
        }
    }
    if quote.is_some() || escaped || backticks || nesting > 0 {
        return vec![];
    }
    if ranges.is_empty() {
        let trimmed = source.text.js_trim();
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            let start = source.text.len() - source.text.js_trim_start().len() + 1;
            let end = start + trimmed.len() - 2;
            let start_unit = source.text[..start].encode_utf16().count();
            let end_unit = start_unit + source.text[start..end].encode_utf16().count();
            let inner = EmbeddedScript {
                language: source.language.clone(),
                text: source.text[start..end].into(),
                starts: source.starts[start_unit..end_unit].to_vec(),
                ends: source.ends[start_unit..end_unit].to_vec(),
            };
            return scripts_in(&inner, depth + 1);
        }
        return scripts_in_command(source, depth);
    }
    if start < source.text.len() {
        ranges.push((start, source.text.len(), start_unit, unit));
    }
    ranges
        .into_iter()
        .flat_map(|(start, end, start_unit, end_unit)| {
            let segment = EmbeddedScript {
                language: source.language.clone(),
                text: source.text[start..end].into(),
                starts: source.starts[start_unit..end_unit].to_vec(),
                ends: source.ends[start_unit..end_unit].to_vec(),
            };
            scripts_in(&segment, depth)
        })
        .collect()
}
fn scripts_in_command(source: &EmbeddedScript, depth: usize) -> Vec<EmbeddedScript> {
    if depth >= 4 {
        return vec![];
    }
    let command = &source.text;
    let mut quote = None;
    let mut escaped = false;
    let heredoc = command.char_indices().find_map(|(i, ch)| {
        if escaped {
            escaped = false;
            return None;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            return None;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            return None;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            return None;
        }
        (command[i..].starts_with("<<") && !command[i..].starts_with("<<<")).then_some(i)
    });
    let header = if heredoc.is_some() {
        command.split('\n').next().unwrap_or(command)
    } else {
        command.as_str()
    };
    let Some(words) = tokenize(header) else {
        return vec![];
    };
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let name = word.text.as_str();
        if [
            "if", "then", "else", "elif", "do", "while", "until", "{", "}",
        ]
        .contains(&name)
        {
            index += 1;
            continue;
        }
        if assignment(name) || name == "--" {
            index += 1;
            continue;
        }
        let options: Option<&[&str]> = match name {
            "env" => Some(&["-u", "--unset", "-C", "--chdir"]),
            "sudo" => Some(&["-u", "--user", "-g", "--group", "-C", "-D", "--chdir"]),
            "nice" => Some(&["-n", "--adjustment"]),
            "exec" => Some(&["-a"]),
            "nohup" | "command" | "builtin" => Some(&[]),
            _ => None,
        };
        let Some(options) = options else {
            break;
        };
        index += 1;
        while let Some(arg) = words.get(index).filter(|w| w.text.starts_with('-')) {
            let key = arg.text.split('=').next().unwrap_or_default();
            index += if options.contains(&key) && !arg.text.contains('=') {
                2
            } else {
                1
            };
        }
    }
    let Some(program) = words.get(index) else {
        return vec![];
    };
    let name = language_name(&program.text);
    let config = interpreter(&name);
    let mut found = vec![];
    let mut stdin = false;
    let mut add = |language: &str, word: &Word, offset: usize| {
        let text = word.text.chars().skip(offset).collect::<String>();
        if text.js_trim().is_empty() {
            return;
        }
        let starts = word
            .starts
            .iter()
            .skip(offset)
            .filter_map(|i| source.starts.get(*i).copied())
            .collect();
        let ends = word
            .ends
            .iter()
            .skip(offset)
            .filter_map(|i| i.checked_sub(1).and_then(|i| source.ends.get(i)).copied())
            .collect();
        found.push(EmbeddedScript {
            language: language.into(),
            text,
            starts,
            ends,
        });
    };
    if let Some((language, inline, values, files, reads_stdin, repeat)) = config {
        let mut j = index + 1;
        let mut positionals = vec![];
        let mut has_script = false;
        let mut file = false;
        while let Some(arg) = words.get(j) {
            if arg.text.starts_with(['<', '>']) {
                break;
            }
            let flag = if language == "powershell" {
                arg.text.to_lowercase()
            } else {
                arg.text.clone()
            };
            let (key, value) = if flag.starts_with("--") {
                flag.split_once('=')
                    .map_or((flag.as_str(), None), |(k, v)| (k, Some(v)))
            } else {
                (flag.as_str(), None)
            };
            if key.starts_with('-') && key != "-" {
                if files.contains(&key) {
                    file = true;
                    j += if value.is_some() { 1 } else { 2 };
                    if !repeat {
                        break;
                    }
                    continue;
                }
                let shell = language == "shellscript"
                    && key.starts_with('-')
                    && key[1..].chars().all(|c| c.is_ascii_lowercase())
                    && key.contains('c');
                if inline.contains(&key) || shell {
                    if value.is_some() {
                        add(language, arg, arg.text.find('=').map_or(0, |i| i + 1));
                        j += 1;
                    } else {
                        if let Some(word) = words.get(j + 1) {
                            add(language, word, 0);
                        }
                        j += 2;
                    }
                    has_script = true;
                    if !repeat {
                        break;
                    }
                    continue;
                }
                j += if values.contains(&key) && value.is_none() {
                    2
                } else {
                    1
                };
                continue;
            }
            positionals.push(arg);
            j += 1;
            if reads_stdin && !repeat && name != "deno" {
                break;
            }
        }
        if name == "deno" {
            if positionals.first().is_some_and(|w| w.text == "eval") {
                if let Some(word) = positionals.get(1) {
                    add(language, word, 0);
                    has_script = true;
                }
            } else {
                file = true;
            }
        }
        if matches!(name.as_str(), "sqlite3" | "duckdb") {
            for word in positionals.iter().skip(1) {
                add(language, word, 0);
                has_script = true;
            }
        }
        if !repeat && name != "deno" {
            file |= positionals.first().is_some_and(|w| w.text != "-");
        }
        stdin = reads_stdin && !has_script && !file;
        if stdin && let Some(i) = command.find("<<<") {
            let start = i + 3;
            let tail = &command[start..];
            if let Some(words) = tokenize(tail)
                && let Some(word) = words.first()
            {
                let offset = command[..start].encode_utf16().count();
                let word = Word {
                    text: word.text.clone(),
                    starts: word.starts.iter().map(|i| i + offset).collect(),
                    ends: word.ends.iter().map(|i| i + offset).collect(),
                };
                add(language, &word, 0);
            }
        }
    }
    if let Some(operator) = heredoc
        && let Some(header_end) = command.find('\n')
    {
        let strip = command[operator..].starts_with("<<-");
        let start = operator + if strip { 3 } else { 2 };
        let raw = header[start..]
            .js_trim_start()
            .split(is_js_whitespace)
            .next()
            .unwrap_or_default();
        let delimiter = tokenize(raw)
            .and_then(|w| w.first().map(|w| w.text.clone()))
            .unwrap_or_default();
        let body_start = header_end + 1;
        let mut body_end = command.len();
        let mut cursor = body_start;
        let delimiter = if header.ends_with('\r') {
            format!("{delimiter}\r")
        } else {
            delimiter
        };
        for line in command[body_start..].split_inclusive('\n') {
            let comparable = line.strip_suffix('\n').unwrap_or(line);
            if (if strip {
                comparable.trim_start_matches('\t')
            } else {
                comparable
            }) == delimiter
            {
                body_end = cursor;
                break;
            }
            cursor += line.len();
        }
        let written = if matches!(name.as_str(), "cat" | "tee") {
            let filename = if name == "tee" {
                words
                    .get(
                        index
                            + 1
                            + usize::from(words.get(index + 1).is_some_and(|w| w.text == "-a")),
                    )
                    .map(|w| w.text.as_str())
            } else {
                header
                    .find('>')
                    .filter(|i| *i == 0 || !header.as_bytes()[i - 1].is_ascii_digit())
                    .and_then(|i| {
                        header[i + 1..]
                            .js_trim_start()
                            .split(is_js_whitespace)
                            .next()
                    })
            };
            filename.and_then(file_language)
        } else {
            None
        };
        let language = if stdin { config.map(|c| c.0) } else { written }
            .or_else(|| delimiter_language(&delimiter));
        if let Some(language) = language {
            let text = command[body_start..body_end].to_string();
            let offset = command[..body_start].encode_utf16().count();
            let count = text.encode_utf16().count();
            if !text.js_trim().is_empty() {
                found.push(EmbeddedScript {
                    language: language.into(),
                    text,
                    starts: source.starts[offset..offset + count].to_vec(),
                    ends: source.ends[offset..offset + count].to_vec(),
                });
            }
        }
    }
    if found.is_empty()
        && let Some(start) = command.find("$(")
    {
        let tail = &command[start + 2..];
        if let Some(end) = tail.rfind(')') {
            let text = tail[..end].to_string();
            let offset = command[..start + 2].encode_utf16().count();
            let count = text.encode_utf16().count();
            let nested = EmbeddedScript {
                language: "shellscript".into(),
                text,
                starts: source.starts[offset..offset + count].to_vec(),
                ends: source.ends[offset..offset + count].to_vec(),
            };
            found.extend(scripts_in(&nested, depth + 1));
        }
    }
    let mut result = vec![];
    for script in found {
        let nested = if script.language == "shellscript" {
            scripts_in(&script, depth + 1)
        } else {
            vec![]
        };
        result.push(script);
        result.extend(nested);
    }
    result
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    pub content: String,
    pub offset: usize,
    pub color: Option<String>,
    pub font_style: Option<u32>,
}
pub fn with_embedded_scripts<E>(
    code: &str,
    lines: Vec<Vec<SyntaxToken>>,
    embedded: &[EmbeddedScript],
    mut tokenize: impl FnMut(&str, &str) -> Result<Vec<Vec<SyntaxToken>>, E>,
) -> Vec<Vec<SyntaxToken>> {
    if embedded.is_empty() {
        return lines;
    }
    let mut styles = vec![(None, None); code.encode_utf16().count()];
    for token in lines.iter().flatten() {
        for i in token.offset
            ..token
                .offset
                .saturating_add(token.content.encode_utf16().count())
                .min(styles.len())
        {
            styles[i] = (token.color.clone(), token.font_style);
        }
    }
    for script in embedded {
        let Ok(tokens) = tokenize(&script.text, &script.language) else {
            continue;
        };
        for token in tokens.iter().flatten().filter(|t| !t.content.is_empty()) {
            let (Some(start), Some(end)) = (
                script.starts.get(token.offset),
                script
                    .ends
                    .get(token.offset + token.content.encode_utf16().count() - 1),
            ) else {
                continue;
            };
            for i in *start..(*end).min(styles.len()) {
                styles[i] = (token.color.clone(), token.font_style);
            }
        }
    }
    let mut result = vec![];
    let mut offset = 0;
    let mut source_lines = code.split('\n').peekable();
    while let Some(raw_line) = source_lines.next() {
        let has_newline = source_lines.peek().is_some();
        let line = if has_newline {
            raw_line.strip_suffix('\r').unwrap_or(raw_line)
        } else {
            raw_line
        };
        let mut tokens: Vec<SyntaxToken> = vec![];
        for ch in line.chars() {
            let style = styles.get(offset).cloned().unwrap_or_default();
            if let Some(last) = tokens
                .last_mut()
                .filter(|t| t.color == style.0 && t.font_style == style.1)
            {
                last.content.push(ch);
            } else {
                tokens.push(SyntaxToken {
                    content: ch.to_string(),
                    offset,
                    color: style.0,
                    font_style: style.1,
                });
            }
            offset += ch.len_utf16();
        }
        result.push(tokens);
        offset += usize::from(raw_line.len() != line.len()) + usize::from(has_newline);
    }
    result
}
