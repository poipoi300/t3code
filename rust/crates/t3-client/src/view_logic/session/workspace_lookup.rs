use super::links::{is_absolute, is_windows_absolute, resolve_path_link_target};
use crate::view_logic::JsTrim;
use std::{borrow::Borrow, future::Future};
use t3_contracts::orchestration_v2::OrchestrationV2ProjectedTurnItem;

const FILESYSTEM_METADATA_BATCH_LIMIT: usize = 64;
const AGENT_DIRECTORY_SEARCH_LIMIT: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntryCandidate {
    pub path: String,
    pub is_file: bool,
}

fn cd_operands(script: &str) -> Vec<String> {
    fn end_command(words: &mut Vec<String>, operands: &mut Vec<String>) {
        if let Some((name, rest)) = words.split_first() {
            if name == "cd" {
                if let Some(operand) = rest.iter().find(|word| {
                    word.as_str() != "--"
                        && !word
                            .strip_prefix('-')
                            .is_some_and(|s| s.chars().all(|c| "LPe@".contains(c)))
                }) {
                    operands.push(operand.clone());
                }
            } else if ["sh", "bash", "zsh", "dash"]
                .contains(&name.rsplit('/').next().unwrap_or_default())
            {
                let index = rest
                    .iter()
                    .position(|word| {
                        word.strip_prefix('-')
                            .and_then(|s| s.strip_suffix('c'))
                            .is_some_and(|s| s.bytes().all(|c| c.is_ascii_lowercase()))
                    })
                    .map_or(0, |i| i + 1);
                if let Some(inner) = rest.get(index)
                    && inner.len() >= 2
                    && matches!(inner.as_bytes()[0], b'\'' | b'"')
                    && inner.as_bytes().last() == inner.as_bytes().first()
                {
                    operands.extend(cd_operands(&inner[1..inner.len() - 1]));
                }
            }
        }
        words.clear();
    }
    let mut words: Vec<String> = vec![];
    let mut operands = vec![];
    let mut at = 0;
    let mut word_open = false;
    while at < script.len() {
        let source = &script[at..];
        let Some(ch) = source.chars().next() else {
            break;
        };
        if crate::view_logic::is_js_whitespace(ch) && !matches!(ch, ' ' | '\t' | '\n') {
            at += ch.len_utf8();
            continue;
        }
        let previous = script[..at].chars().next_back();
        let comment = ch == '#'
            && previous
                .is_none_or(|c| crate::view_logic::is_js_whitespace(c) || ";&|()".contains(c));
        let length = if comment {
            source.find('\n').unwrap_or(source.len())
        } else if matches!(ch, '\'' | '"') {
            source[1..].find(ch).map_or(1, |i| i + 2)
        } else if ";&|()\n".contains(ch) {
            1
        } else if matches!(ch, ' ' | '\t') {
            source
                .bytes()
                .take_while(|c| matches!(c, b' ' | b'\t'))
                .count()
        } else {
            source
                .find(|c: char| crate::view_logic::is_js_whitespace(c) || ";&|()\"'".contains(c))
                .unwrap_or(source.len())
                .max(ch.len_utf8())
        };
        let token = &source[..length];
        at += length;
        if !matches!(ch, '\'' | '"' | '#') && token.contains("<<") {
            break;
        }
        if ";&|()\n".contains(ch) {
            end_command(&mut words, &mut operands);
            word_open = false;
        } else if matches!(ch, ' ' | '\t') || (!word_open && ch == '#') {
            word_open = false;
        } else if word_open {
            if let Some(word) = words.last_mut() {
                word.push_str(token);
            }
        } else {
            words.push(token.into());
            word_open = true;
        }
    }
    end_command(&mut words, &mut operands);
    operands
}

fn literal_directory(operand: &str) -> Option<&str> {
    let path = if operand.starts_with(['\'', '"']) {
        operand.get(1..operand.len().checked_sub(1)?)?
    } else {
        operand
    };
    (is_absolute(path)
        && !path.contains(['$', '`', '*', '?', '[', ']', '{', '}', '"', '\''])
        && (!path.contains('\\') || is_windows_absolute(path)))
    .then_some(path)
}

pub fn agent_working_directories<T: Borrow<OrchestrationV2ProjectedTurnItem>>(
    items: &[T],
    source_thread: &str,
    source_item: &str,
) -> Vec<String> {
    use t3_contracts::{
        orchestration_v2::OrchestrationV2TurnItem, orchestration_v2_id_table::IdOrIndex,
    };
    let Some(reply) = items.iter().position(|row| {
        let row = row.borrow();
        matches!((&row.source_thread_id, &row.source_item_id), (IdOrIndex::Id(thread), IdOrIndex::Id(item)) if thread.0 == source_thread && item.0 == source_item)
    }) else {
        return vec![];
    };
    let mut directories = vec![];
    for row in items[..reply].iter().rev() {
        let OrchestrationV2TurnItem::CommandExecution(command) = &row.borrow().item else {
            continue;
        };
        for operand in cd_operands(&command.input).iter().rev() {
            if let Some(path) = literal_directory(operand)
                && !directories.iter().any(|directory| directory == path)
            {
                directories.push(path.to_string());
            }
            if directories.len() == FILESYSTEM_METADATA_BATCH_LIMIT {
                return directories;
            }
        }
    }
    directories
}

pub fn pick_workspace_link_match(
    relative: &str,
    cwd: &str,
    entries: &[WorkspaceEntryCandidate],
) -> Option<String> {
    let relative = relative.js_trim();
    let files = entries
        .iter()
        .filter(|entry| entry.is_file)
        .collect::<Vec<_>>();
    if !relative.contains(['/', '\\']) && !matches!(relative, "" | "." | "..") {
        let basename = |entry: &&WorkspaceEntryCandidate| {
            entry
                .path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        if let Some(exact) = files.iter().find(|entry| basename(entry) == relative) {
            return Some(exact.path.clone());
        }
        let matches = files
            .iter()
            .filter(|entry| basename(entry).to_lowercase() == relative.to_lowercase())
            .collect::<Vec<_>>();
        return (matches.len() == 1).then(|| matches[0].path.clone());
    }
    let normalized = relative.replace('\\', "/");
    if !is_absolute(relative) {
        let segments = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let root = cwd
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let same = |a: &str, b: &str| {
            if is_windows_absolute(cwd) {
                a.to_lowercase() == b.to_lowercase()
            } else {
                a == b
            }
        };
        for count in (1..=root.len().min(segments.len().saturating_sub(1))).rev() {
            if root[root.len() - count..]
                .iter()
                .zip(&segments)
                .all(|(a, b)| same(a, b))
            {
                let stripped = segments[count..].join("/");
                if let Some(entry) = files
                    .iter()
                    .find(|entry| same(&entry.path.replace('\\', "/"), &stripped))
                {
                    return Some(entry.path.clone());
                }
                break;
            }
        }
    }
    let matches = files
        .into_iter()
        .filter(|entry| {
            let path = entry.path.replace('\\', "/");
            path == normalized || path.ends_with(&format!("/{normalized}"))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0].path.clone())
}

pub async fn find_agent_directory_match<
    E,
    F: Future<Output = Result<Vec<WorkspaceEntryCandidate>, E>>,
>(
    relative: &str,
    directories: &[String],
    mut search: impl FnMut(String) -> F,
) -> Result<Option<String>, E> {
    for directory in directories.iter().take(AGENT_DIRECTORY_SEARCH_LIMIT) {
        let entries = search(directory.clone()).await?;
        if let Some(path) = pick_workspace_link_match(relative, directory, &entries) {
            return Ok(Some(resolve_path_link_target(&path, directory)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn directories(commands: &[&str]) -> Vec<String> {
        let rows = commands.iter().chain([&"the reply", &"cd /work/later"]).enumerate().map(|(i, input)| serde_json::from_value(json!({
            "sourceThreadId":"thread", "sourceItemId":format!("item-{i}"), "visibility":"local","position":i,
            "item":{"id":format!("item-{i}"),"threadId":"thread","ordinal":i,"type":"command_execution","input":input,"status":"completed","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-01-01T00:00:00Z","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null}
        })).unwrap()).collect::<Vec<OrchestrationV2ProjectedTurnItem>>();
        let reply = format!("item-{}", commands.len());
        let result = agent_working_directories(&rows, "thread", &reply);
        let shared = rows.into_iter().map(std::rc::Rc::new).collect::<Vec<_>>();
        assert_eq!(agent_working_directories(&shared, "thread", &reply), result);
        result
    }
    #[test]
    fn upstream_latest_literal_cd_destinations_before_reply() {
        assert_eq!(
            directories(&[
                "cd /work/repo-c && cd /work/repo-b && git diff",
                "git status",
                "/bin/zsh -lc \"cd '/work/my repo'; ls\"",
                "cd -P /work/repo-a # not this; cd /work/commented",
                "cd /work/repo-c\npwd",
            ]),
            [
                "/work/repo-c",
                "/work/repo-a",
                "/work/my repo",
                "/work/repo-b"
            ]
        );
        assert_eq!(
            directories(&[
                "cd docs && cd ../other && cd ~/repo && cd $ROOT/x && cd /work/*/src",
                "abcd /work/a; echo cd /work/b; git -C /work/c status; cd /work/d",
                "echo \"cd /work/g\"; echo 'x; cd /work/h' && grep -c \"cd /work/i\" notes",
                "echo bash -c 'cd /work/j'; cd -",
                "cat <<EOF > notes\ncd /work/k\nEOF",
                "cd /work/e\\ f && cd `pwd`/x"
            ]),
            ["/work/d"]
        );
    }
    #[test]
    fn latest_directory_match_then_next_and_four_root_limit() {
        futures::executor::block_on(async {
            let dirs = ["/work/repo-b/server", "/work/repo-b", "/work/repo-c"].map(String::from);
            let result =
                find_agent_directory_match::<(), _>("config.test.yaml", &dirs, |root| async move {
                    Ok(vec![WorkspaceEntryCandidate {
                        path: if root == "/work/repo-b" {
                            "server/config.test.yaml".into()
                        } else {
                            "config.test.yaml".into()
                        },
                        is_file: true,
                    }])
                })
                .await
                .unwrap();
            assert_eq!(
                result.as_deref(),
                Some("/work/repo-b/server/config.test.yaml")
            );
            let result =
                find_agent_directory_match::<(), _>("config.test.yaml", &dirs, |root| async move {
                    Ok(if root == "/work/repo-c" {
                        vec![WorkspaceEntryCandidate {
                            path: "etc/config.test.yaml".into(),
                            is_file: true,
                        }]
                    } else {
                        vec![]
                    })
                })
                .await
                .unwrap();
            assert_eq!(result.as_deref(), Some("/work/repo-c/etc/config.test.yaml"));
            let mut calls = 0;
            let dirs = (0..6).map(|i| format!("/root-{i}")).collect::<Vec<_>>();
            assert_eq!(
                find_agent_directory_match::<(), _>("config.test.yaml", &dirs, |_| {
                    calls += 1;
                    async { Ok(vec![]) }
                })
                .await
                .unwrap(),
                None
            );
            assert_eq!(calls, 4);
        });
    }
}
