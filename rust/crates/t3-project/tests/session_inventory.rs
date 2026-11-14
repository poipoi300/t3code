//! The native session inventory (`AgentSessionScanner.ts` discovery, reads and Git identities)
//! over temporary provider homes; no real provider home is read. Ported from
//! `AgentSessionScanner.test.ts`, plus hostile store contents.
#![cfg(feature = "session-inventory")]
#![allow(clippy::unwrap_used)]
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use t3_contracts::agent_sessions::{AgentSessionImportSource, AgentSessionSource};
use t3_project::{
    Result,
    inventory::{InventoryHost, InventoryPaths, NativeSessionInventory},
    sessions::{RecentSession, SessionInventory},
};

const NOW: &str = "2026-08-24T12:00:00.000Z";
const DAY: i64 = 24 * 60 * 60 * 1000;

fn ms(iso: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(iso).unwrap().timestamp_millis()
}

struct Host {
    settings: Value,
    imported: Vec<(String, AgentSessionImportSource)>,
}
#[async_trait]
impl InventoryHost for Host {
    async fn settings(&self) -> Result<Value> {
        Ok(self.settings.clone())
    }
    async fn imported_transcripts(&self) -> Result<Vec<(String, AgentSessionImportSource)>> {
        Ok(self.imported.clone())
    }
    fn now_ms(&self) -> i64 {
        ms(NOW)
    }
}

/// Temporary homes laid out as `AgentSessionScanner.test.ts` does.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
    pi: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let root = PathBuf::from(
            root.to_string_lossy()
                .strip_prefix(r"\\?\")
                .map_or_else(|| root.to_string_lossy().into_owned(), str::to_owned),
        );
        let (claude, codex, pi) = (root.join("claude"), root.join("codex"), root.join("pi-agent"));
        for dir in [&claude, &codex, &pi, &root.join("home"), &root.join("base")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self {
            _temp: temp,
            root,
            claude,
            codex,
            pi,
        }
    }
    fn workspace(&self, name: &str) -> PathBuf {
        let path = self.root.join("work").join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn settings(&self) -> Value {
        json!({"providerInstances": {
            "claudeAgent": {"driver": "claudeAgent", "config": {"homePath": self.claude}},
            "codex": {"driver": "codex", "config": {"homePath": self.codex}},
            "pi": {"driver": "pi", "enabled": true, "config": {}, "environment": [
                {"name": "PI_CODING_AGENT_DIR", "value": self.pi, "sensitive": false},
                {"name": "PI_CODING_AGENT_SESSION_DIR", "value": "", "sensitive": false}
            ]}
        }})
    }
    fn inventory(&self, settings: Value, imported: Vec<(String, AgentSessionImportSource)>) -> Arc<NativeSessionInventory> {
        Arc::new(NativeSessionInventory::new(
            InventoryPaths {
                home: self.root.join("home"),
                temporary_root: self.root.join("tmp-root"),
                base_dir: self.root.join("base"),
                worktrees_dir: self.root.join("base").join("worktrees"),
            },
            Default::default(),
            Arc::new(Host { settings, imported }),
        ))
    }
}

fn write(path: &Path, contents: impl AsRef<[u8]>, mtime_ms: i64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms as u64))
        .unwrap();
}
fn lines(records: &[Value]) -> String {
    records.iter().map(|r| format!("{r}\n")).collect()
}
fn claude_line(cwd: &Path) -> String {
    lines(&[json!({"type": "user", "cwd": cwd, "sessionId": "s1"}), json!({"type": "assistant"})])
}
fn codex_line(cwd: &Path) -> String {
    lines(&[json!({"timestamp": "2026-01-01T00:00:00.000Z", "type": "session_meta", "payload": {"id": "r1", "cwd": cwd}})])
}
fn claude_transcript(cwd: &Path, session: &str) -> String {
    lines(&[
        json!({"type": "user", "cwd": cwd, "sessionId": session, "timestamp": "2026-08-23T12:00:00.000Z", "message": {"role": "user", "content": "Fix the project"}}),
        json!({"type": "assistant", "sessionId": session, "timestamp": "2026-08-23T12:01:00.000Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "Done"}]}}),
    ])
}
fn codex_transcript(cwd: &Path, session: &str) -> String {
    lines(&[
        json!({"type": "session_meta", "payload": {"id": session, "cwd": cwd}}),
        json!({"type": "event_msg", "timestamp": "2026-08-24T10:00:00.000Z", "payload": {"type": "user_message", "message": "Review this code"}}),
        json!({"type": "response_item", "timestamp": "2026-08-24T10:01:00.000Z", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Looks good"}]}}),
    ])
}
fn rollout(codex: &Path, day: &str, name: &str) -> PathBuf {
    let mut parts = day.split('-');
    let (y, m, d) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
    codex.join("sessions").join(y).join(m).join(d).join(name)
}

fn same(a: &str, b: &Path) -> bool {
    let key = |p: &str| p.replace('\\', "/").to_lowercase();
    key(a) == key(&b.to_string_lossy())
}

/// `(workspace root, source, thread count, last active)` per candidate.
async fn candidates(inventory: &Arc<NativeSessionInventory>) -> Vec<(String, AgentSessionSource, u64, String)> {
    let mut found: Vec<_> = inventory
        .candidates()
        .await
        .unwrap()
        .candidates
        .into_iter()
        .map(|c| (c.workspace_root, c.source, c.thread_count, c.last_active_at.unwrap().0))
        .collect();
    found.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));
    found
}
async fn recent(inventory: &Arc<NativeSessionInventory>, root: &Path, completed: &[AgentSessionImportSource]) -> Vec<RecentSession> {
    inventory
        .recent_threads(&root.to_string_lossy(), completed)
        .await
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
        .await
}
fn importable(outcomes: &[RecentSession]) -> Vec<&t3_project::transcript::SessionThread> {
    outcomes
        .iter()
        .filter_map(|o| match o {
            RecentSession::Importable { thread, .. } => Some(thread.as_ref()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn reads_claude_project_cwds_from_transcripts_newest_first() {
    let f = Fixture::new();
    let (older, newer) = (f.workspace("older"), f.workspace("newer"));
    write(&f.claude.join("projects/-slug-older/a.jsonl"), claude_line(&older), ms("2026-01-01T00:00:00.000Z"));
    write(&f.claude.join("projects/-slug-older/b.jsonl"), claude_line(&older), ms("2026-01-02T00:00:00.000Z"));
    write(&f.claude.join("projects/-slug-newer/c.jsonl"), claude_line(&newer), ms("2026-03-01T00:00:00.000Z"));
    let found = candidates(&f.inventory(f.settings(), vec![])).await;
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(same(&found[0].0, &newer));
    assert_eq!((found[0].2, found[0].3.as_str()), (1, "2026-03-01T00:00:00.000Z"));
    assert!(same(&found[1].0, &older));
    assert_eq!((found[1].2, found[1].3.as_str()), (2, "2026-01-02T00:00:00.000Z"));
    assert!(found.iter().all(|c| c.1 == AgentSessionSource::ClaudeAgent));
}

#[tokio::test]
async fn both_agents_report_the_same_cwd_and_instance_environment_names_the_home() {
    let f = Fixture::new();
    let workspace = f.workspace("shared");
    let custom = f.root.join("custom-codex");
    write(&f.claude.join("projects/-slug/a.jsonl"), claude_line(&workspace), ms("2026-01-01T00:00:00.000Z"));
    write(&rollout(&custom, "2026-04-01", "rollout-2026-04-01T09-00-00-aaa.jsonl"), codex_line(&workspace), ms("2026-04-01T09:00:00.000Z"));
    let mut settings = f.settings();
    // An empty `homePath` with CODEX_HOME set in the instance's environment.
    settings["providerInstances"]["codex"] = json!({"driver": "codex", "config": {"homePath": ""},
        "environment": [{"name": "CODEX_HOME", "value": custom, "sensitive": false}]});
    let found = candidates(&f.inventory(settings, vec![])).await;
    let sources: Vec<_> = found.iter().map(|c| c.1.clone()).collect();
    assert_eq!(sources, [AgentSessionSource::Codex, AgentSessionSource::ClaudeAgent]);
    assert!(found.iter().all(|c| same(&c.0, &workspace)));
}

#[tokio::test]
async fn disabled_instances_missing_directories_and_excluded_roots_are_not_offered() {
    let f = Fixture::new();
    let kept = f.workspace("kept");
    let gone = f.root.join("work").join("gone");
    let excluded = [f.root.join("home"), f.root.join("home").join("Downloads").join("x"), f.root.join("base").join("worktrees").join("w1"), f.root.join("elsewhere").join(".t3").join("worktrees").join("w2")];
    write(&f.claude.join("projects/a/kept.jsonl"), claude_line(&kept), ms("2026-01-01T00:00:00.000Z"));
    write(&f.claude.join("projects/a/gone.jsonl"), claude_line(&gone), ms("2026-01-01T00:00:00.000Z"));
    for (index, dir) in excluded.iter().enumerate() {
        std::fs::create_dir_all(dir).unwrap();
        write(&f.claude.join(format!("projects/b/{index}.jsonl")), claude_line(dir), ms("2026-01-01T00:00:00.000Z"));
    }
    write(&rollout(&f.codex, "2026-01-01", "rollout-x.jsonl"), codex_line(&kept), ms("2026-01-01T00:00:00.000Z"));
    let mut settings = f.settings();
    settings["providerInstances"]["codex"]["enabled"] = json!(false);
    let found = candidates(&f.inventory(settings, vec![])).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(same(&found[0].0, &kept));
    assert_eq!(found[0].1, AgentSessionSource::ClaudeAgent);
}

#[tokio::test]
async fn linked_worktrees_fold_into_their_checkout_with_its_origin_and_branch() {
    let f = Fixture::new();
    let main = f.workspace("main");
    let worktree = f.workspace("feature-wt");
    let git = main.join(".git");
    std::fs::create_dir_all(git.join("worktrees").join("feature-wt")).unwrap();
    std::fs::write(git.join("config"), "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n").unwrap();
    std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", git.join("worktrees").join("feature-wt").display())).unwrap();
    std::fs::write(git.join("worktrees").join("feature-wt").join("commondir"), "../..\n").unwrap();
    std::fs::write(git.join("worktrees").join("feature-wt").join("HEAD"), "ref: refs/heads/feature/login\n").unwrap();
    write(&f.claude.join("projects/wt/session.jsonl"), claude_transcript(&worktree, "11111111-1111-4111-8111-111111111111"), ms(NOW) - DAY);
    let inventory = f.inventory(f.settings(), vec![]);
    let candidates = inventory.candidates().await.unwrap().candidates;
    assert_eq!(candidates.len(), 1);
    assert!(same(&candidates[0].workspace_root, &main), "{}", candidates[0].workspace_root);
    let git = candidates[0].git.clone().unwrap();
    assert_eq!(git.remote_key.as_deref(), Some("github.com/acme/widgets"));
    assert_eq!(git.repository.as_deref(), Some("acme/widgets"));
    // Importing the checkout brings the worktree's session with its path and branch.
    let outcomes = recent(&inventory, &main, &[]).await;
    match outcomes.as_slice() {
        [RecentSession::Importable { worktree: Some(found), .. }] => {
            assert!(same(&found.path, &worktree));
            assert_eq!(found.branch.as_deref(), Some("feature/login"));
        }
        other => panic!("{}", other.len()),
    }
}

#[tokio::test]
async fn imports_recent_claude_codex_pi_and_compressed_sessions_for_the_selected_project_only() {
    let f = Fixture::new();
    let (workspace, other) = (f.workspace("selected"), f.workspace("other"));
    let now = ms(NOW);
    write(&f.claude.join("projects/-selected/claude-recent.jsonl"), claude_transcript(&workspace, "claude-recent"), now - DAY);
    write(&f.claude.join("projects/-selected/claude-old.jsonl"), claude_transcript(&workspace, "claude-old"), now - 31 * DAY);
    write(&f.claude.join("projects/-other/claude-other.jsonl"), claude_transcript(&other, "claude-other"), now - DAY);
    write(&rollout(&f.codex, "2026-08-24", "rollout-codex-recent.jsonl"), codex_transcript(&workspace, "codex-recent"), now - 60 * 60 * 1000);
    let zst = zstd::encode_all(codex_transcript(&workspace, "codex-zst").as_bytes(), 3).unwrap();
    write(&rollout(&f.codex, "2026-08-23", "rollout-codex-zst.jsonl.zst"), zst, now - 3 * DAY);
    let pi = |id: &str, parent: Option<&str>, message: Value| json!({"type": "message", "id": id, "parentId": parent, "timestamp": "2026-08-24T11:00:00.000Z", "message": message});
    write(
        &f.pi.join("sessions/--selected--/2026-08-24_pi.jsonl"),
        lines(&[
            json!({"type": "session", "version": 3, "id": "pi-recent", "cwd": workspace}),
            pi("a1", None, json!({"role": "user", "content": "Plan the change"})),
            pi("a2", Some("a1"), json!({"role": "assistant", "content": [{"type": "text", "text": "Abandoned answer"}]})),
            pi("a3", Some("a2"), json!({"role": "user", "content": "Abandoned follow-up"})),
            pi("b1", Some("a1"), json!({"role": "assistant", "content": [{"type": "thinking", "thinking": "Hidden"}, {"type": "toolCall", "id": "c", "name": "bash", "arguments": {}}]})),
            pi("bn", Some("b1"), json!({"role": "toolResult", "content": null})),
            pi("b2", Some("bn"), json!({"role": "toolResult", "content": [{"type": "text", "text": "Tool output"}]})),
            pi("b3", Some("b2"), json!({"role": "assistant", "content": [{"type": "text", "text": "Active answer"}]})),
            json!({"type": "session_info", "id": "b4", "parentId": "b3", "name": "Pi refactor"}),
        ]),
        now - 30 * 60 * 1000,
    );
    let threads = recent(&f.inventory(f.settings(), vec![]), &workspace, &[]).await;
    let threads = importable(&threads);
    let ids: Vec<_> = threads.iter().map(|t| t.provider_session_id.as_str()).collect();
    assert_eq!(ids, ["pi-recent", "codex-recent", "claude-recent", "codex-zst"]);
    let texts: Vec<Vec<&str>> = threads.iter().map(|t| t.messages.iter().map(|m| m.text.as_str()).collect()).collect();
    assert_eq!(texts[0], ["Plan the change", "Active answer"]);
    assert_eq!(texts[1], ["Review this code", "Looks good"]);
    assert_eq!(texts[2], ["Fix the project", "Done"]);
    assert_eq!(threads[0].title, "Pi refactor");
}

#[tokio::test]
async fn codex_titles_subagents_duplicates_and_completed_identity() {
    let f = Fixture::new();
    let workspace = f.workspace("codex");
    let now = ms(NOW);
    let id = "019a0000-0000-7000-8000-000000000001";
    let named = rollout(&f.codex, "2026-08-24", &format!("rollout-2026-08-24T10-00-00-{id}.jsonl"));
    write(&named, codex_transcript(&workspace, id), now - 2 * 60 * 60 * 1000);
    // A copy of the same session in another file is reported once.
    write(&rollout(&f.codex, "2026-08-23", "rollout-copy.jsonl"), codex_transcript(&workspace, id), now - 5 * 60 * 60 * 1000);
    // A subagent rollout is neither discovered nor imported.
    write(
        &rollout(&f.codex, "2026-08-24", "rollout-subagent.jsonl"),
        lines(&[json!({"type": "session_meta", "payload": {"id": "sub", "cwd": workspace, "source": {"subagent": {"parent": id}}}})]),
        now - 60 * 60 * 1000,
    );
    write(&f.codex.join("session_index.jsonl"), lines(&[json!({"id": id, "thread_name": "Old name"}), json!({"id": id, "thread_name": "Saved name"})]), now);
    let inventory = f.inventory(f.settings(), vec![]);
    let found = candidates(&inventory).await;
    assert_eq!(found.iter().map(|c| c.2).sum::<u64>(), 2, "{found:?}");
    let outcomes = recent(&inventory, &workspace, &[]).await;
    let threads = importable(&outcomes);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].title, "Saved name");
    let duplicates = outcomes.iter().filter(|o| matches!(o, RecentSession::Duplicate(_))).count();
    assert_eq!(duplicates, 1);
    let RecentSession::Importable { source, .. } = &outcomes[0] else { panic!("importable first") };
    // An unchanged imported file is already imported; a changed one is read again.
    let again = recent(&inventory, &workspace, std::slice::from_ref(source)).await;
    assert!(matches!(again[0], RecentSession::AlreadyImported(_)));
    write(&named, format!("{}\n", codex_transcript(&workspace, id)), now - 2 * 60 * 60 * 1000);
    let changed = recent(&inventory, &workspace, std::slice::from_ref(source)).await;
    assert!(matches!(changed[0], RecentSession::Importable { .. }));
}

// --- Hostile store contents ------------------------------------------------------------------

#[tokio::test]
async fn oversized_and_deeply_nested_transcripts_are_skipped_without_failing_the_scan() {
    let f = Fixture::new();
    let workspace = f.workspace("hostile");
    let now = ms(NOW);
    // 40 MiB of selected text: more than the 32 MiB history budget.
    let huge = "x".repeat(40 * 1024 * 1024);
    write(
        &f.claude.join("projects/h/oversized.jsonl"),
        lines(&[
            json!({"type": "user", "cwd": workspace, "sessionId": "big", "message": {"role": "user", "content": "first"}}),
            json!({"type": "assistant", "sessionId": "big", "message": {"role": "assistant", "content": huge}}),
        ]),
        now - DAY,
    );
    // A line nested 100,000 levels deep inside a selected field.
    let deep = format!(
        "{}\n{{\"type\":\"assistant\",\"message\":{{\"content\":{}{}}}}}\n",
        json!({"type": "user", "cwd": workspace, "sessionId": "deep", "message": {"role": "user", "content": "first"}}),
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    write(&f.claude.join("projects/h/deep.jsonl"), deep, now - DAY - 1);
    write(&f.claude.join("projects/h/fine.jsonl"), claude_transcript(&workspace, "22222222-2222-4222-8222-222222222222"), now - 2 * DAY);
    let inventory = f.inventory(f.settings(), vec![]);
    let found = candidates(&inventory).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, 3);
    let outcomes = recent(&inventory, &workspace, &[]).await;
    let ids: Vec<_> = importable(&outcomes).iter().map(|t| t.provider_session_id.clone()).collect();
    assert_eq!(ids, ["22222222-2222-4222-8222-222222222222"]);
    assert_eq!(outcomes.iter().filter(|o| matches!(o, RecentSession::Skipped)).count(), 2);
}

#[tokio::test]
async fn links_out_of_the_store_are_not_followed() {
    let f = Fixture::new();
    let workspace = f.workspace("linked");
    let outside = f.root.join("outside");
    write(&outside.join("project").join("leak.jsonl"), claude_line(&workspace), ms("2026-01-01T00:00:00.000Z"));
    #[cfg(windows)]
    {
        std::fs::create_dir_all(f.claude.join("projects")).unwrap();
        std::os::windows::fs::symlink_dir(outside.join("project"), f.claude.join("projects").join("linked-dir")).unwrap();
        std::fs::create_dir_all(f.claude.join("projects").join("real")).unwrap();
        std::os::windows::fs::symlink_file(outside.join("project").join("leak.jsonl"), f.claude.join("projects").join("real").join("linked.jsonl")).unwrap();
    }
    #[cfg(unix)]
    {
        std::fs::create_dir_all(f.claude.join("projects").join("real")).unwrap();
        std::os::unix::fs::symlink(outside.join("project"), f.claude.join("projects").join("linked-dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("project").join("leak.jsonl"), f.claude.join("projects").join("real").join("linked.jsonl")).unwrap();
    }
    assert!(candidates(&f.inventory(f.settings(), vec![])).await.is_empty());
}

#[tokio::test]
async fn a_session_naming_a_network_cwd_is_never_resolved() {
    let f = Fixture::new();
    for (index, cwd) in [r"\\attacker.invalid\share\repo", "//attacker.invalid/share/repo", r"/\attacker.invalid\share", r"\\?\UNC\attacker.invalid\share"].iter().enumerate() {
        write(
            &f.claude.join(format!("projects/n/{index}.jsonl")),
            lines(&[json!({"type": "user", "cwd": cwd, "sessionId": "n", "message": {"role": "user", "content": "hi"}})]),
            ms(NOW) - DAY,
        );
    }
    let inventory = f.inventory(f.settings(), vec![]);
    assert!(inventory.candidates().await.unwrap().candidates.is_empty());
    let outcomes = recent(&inventory, Path::new(r"\\attacker.invalid\share\repo"), &[]).await;
    assert!(outcomes.is_empty());
}
