#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use t3_usage::{
    UsageProviderKind as P,
    reader::{list_transcripts, read_transcript},
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn codex_dates_every_baseline_and_rejects_reemitted_snapshots() {
    use t3_usage::codex::CodexScanState;
    let usage = |input, output| json!({"input_tokens":input,"output_tokens":output});
    let count = |second, last, total: Option<(u64, u64)>| {
        let mut info = json!({"last_token_usage":usage(last, last / 5)});
        if let Some((input, output)) = total {
            info["total_token_usage"] = usage(input, output);
        }
        json!({"type":"event_msg","timestamp":format!("2026-08-01T10:00:{second:02}.000Z"),"payload":{"type":"token_count","info":info}})
    };
    let anonymous = |second, input| json!({"type":"token_usage_record","timestamp":format!("2026-08-01T10:00:{second:02}.000Z"),"payload":{"usage":usage(input,input/5)}});
    let cases = [
        (
            vec![
                count(0, 100, Some((100, 20))),
                count(5, 20, None),
                count(10, 30, None),
                count(5, 20, Some((120, 24))),
            ],
            None,
            vec![120, 24, 36, 0],
            1,
        ),
        (
            vec![count(0, 100, None), count(10, 20, Some((60, 12)))],
            None,
            vec![120, 24],
            0,
        ),
        (
            vec![
                count(0, 100, Some((100, 20))),
                count(5, 20, Some((120, 24))),
                count(10, 30, Some((150, 30))),
                count(5, 20, Some((120, 24))),
                count(10, 30, Some((150, 30))),
            ],
            None,
            vec![120, 24, 36, 0, 0],
            1,
        ),
        (
            vec![
                count(0, 100, Some((100, 20))),
                count(0, 20, Some((120, 24))),
                count(0, 30, Some((150, 30))),
                count(0, 20, Some((120, 24))),
                count(0, 30, Some((150, 30))),
            ],
            None,
            vec![120, 24, 36, 0, 0],
            1,
        ),
        (
            vec![
                count(0, 100, Some((100, 20))),
                count(5, 20, Some((120, 24))),
                count(10, 30, Some((150, 30))),
                count(0, 100, Some((100, 20))),
                count(5, 20, Some((120, 24))),
            ],
            Some(3),
            vec![120, 24, 36, 0, 0],
            1,
        ),
        (
            vec![
                count(0, 100, Some((100, 20))),
                anonymous(5, 40),
                anonymous(6, 20),
                count(7, 20, Some((60, 12))),
                count(8, 10, Some((70, 14))),
            ],
            None,
            vec![120, 0, 0, 24, 12],
            1,
        ),
    ];
    for (index, (lines, undated_after, expected, malformed)) in cases.into_iter().enumerate() {
        let mut state = CodexScanState {
            model: "gpt-6-astra".into(),
            ..Default::default()
        };
        let actual: Vec<_> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                if undated_after == Some(i) {
                    state.latest_cumulative_at_ms = None;
                }
                state
                    .parse(line)
                    .iter()
                    .map(|r| t3_usage::total_tokens(&r.totals))
                    .sum::<u64>()
            })
            .collect();
        assert_eq!(actual, expected, "case {index}");
        assert_eq!(state.malformed_records, malformed, "case {index}");
    }
}
fn line(id: u64, output: u64) -> String {
    format!(
        "{}\n",
        json!({"type":"assistant","timestamp":"2026-08-01T10:00:00Z","requestId":format!("req_{id}"),"sessionId":"session-1","message":{"id":format!("msg_{id}"),"model":"claude-fable-5","usage":{"input_tokens":10,"output_tokens":output}}})
    )
}

#[test]
fn appended_records_resume_and_unterminated_tail_is_replayed_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jsonl");
    let first_line = line(1, 5);
    let tail = line(2, 7);
    fs::write(&path, first_line.clone() + tail.trim_end()).unwrap();
    let first = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.tail_records.len(), 1);
    assert_eq!(first.position.resume_offset, first_line.len() as u64);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("\n{}", line(3, 11)).as_bytes())
        .unwrap();
    let second = read_transcript(&path, &P::Claude, Some(&first.position)).unwrap();
    assert!(second.resumed);
    assert!(second.tail_records.is_empty());
    assert_eq!(
        second
            .records
            .iter()
            .map(|r| r.totals.output_tokens)
            .collect::<Vec<_>>(),
        [7, 11]
    );
    let full = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!([first.records, second.records].concat(), full.records);
    assert_eq!(second.position, full.position);
}
#[test]
fn codex_resume_carries_state_and_suppresses_duplicates_across_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jsonl");
    let content = fs::read_to_string(fixture("codex-legacy.jsonl")).unwrap();
    let cut = content.rfind('\n').unwrap() + 1;
    fs::write(&path, &content[..cut]).unwrap();
    let first = read_transcript(&path, &P::Codex, None).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&content.as_bytes()[cut..])
        .unwrap();
    let resumed = read_transcript(&path, &P::Codex, Some(&first.position)).unwrap();
    let full = read_transcript(&path, &P::Codex, None).unwrap();
    assert!(resumed.resumed);
    assert_eq!([first.records, resumed.records].concat(), full.records);
    assert_eq!(resumed.tail_records, full.tail_records);
    assert_eq!(resumed.position, full.position);
    assert_eq!(resumed.malformed_records, 1);
}
#[test]
fn rewrites_and_truncation_reject_saved_guard() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jsonl");
    fs::write(&path, line(1, 5) + &line(2, 7)).unwrap();
    let first = read_transcript(&path, &P::Claude, None).unwrap();
    for replacement in [line(4, 13) + &line(5, 17), line(3, 11)] {
        fs::write(&path, &replacement).unwrap();
        let parsed = read_transcript(&path, &P::Claude, Some(&first.position)).unwrap();
        assert!(!parsed.resumed);
        assert_eq!(
            parsed.records,
            read_transcript(&path, &P::Claude, None).unwrap().records
        );
    }
}
#[test]
fn compressed_rollouts_always_start_from_zero_and_corruption_is_a_read_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jsonl.zst");
    let content = fs::read(fixture("codex-legacy.jsonl")).unwrap();
    fs::write(&path, zstd::encode_all(content.as_slice(), 0).unwrap()).unwrap();
    let first = read_transcript(&path, &P::Codex, None).unwrap();
    let second = read_transcript(&path, &P::Codex, Some(&first.position)).unwrap();
    assert!(!second.resumed);
    assert_eq!(second.position.resume_offset, 0);
    assert_eq!(first.records, second.records);
    fs::write(&path, "not a zstd frame").unwrap();
    assert!(read_transcript(&path, &P::Claude, None).is_none());
    assert!(read_transcript(&dir.path().join("missing"), &P::Claude, None).is_none());
}
#[test]
fn large_lines_skip_payloads_and_keep_usage_after_chunk_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.jsonl");
    let mut writer = fs::File::create(&path).unwrap();
    writer.write_all(b"{\"padding\":\"").unwrap();
    for _ in 0..64 {
        writer
            .write_all("日本語 é".repeat(8192).as_bytes())
            .unwrap();
    }
    let sample = line(1, 42);
    writer.write_all(b"\",").unwrap();
    writer.write_all(&sample.as_bytes()[1..]).unwrap();
    writer.write_all(line(2, 7).as_bytes()).unwrap();
    drop(writer);
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| r.totals.output_tokens)
            .collect::<Vec<_>>(),
        [42, 7]
    );
    assert_eq!(
        parsed.position.resume_offset,
        fs::metadata(&path).unwrap().len()
    );
}
#[test]
fn crlf_and_utf8_preserve_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf8.jsonl");
    let s = line(1, 42)
        .replace("\n", "\r\n")
        .replace("\"type\"", "\"日本語\":\"é\",\"type\"");
    fs::write(&path, &s).unwrap();
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.position.resume_offset, s.len() as u64);
}
#[test]
fn discovery_has_stable_depth_first_name_order_and_filters_grok_logs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for index in 0..40 {
        let child = root.join(format!("project-{index}/sessions"));
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("a.jsonl"), "").unwrap();
        fs::write(child.parent().unwrap().join("updates.jsonl"), "").unwrap();
    }
    fs::write(root.join("ignored.zst"), "").unwrap();
    fs::write(root.join("partial.jsonl.zst.tmp"), "").unwrap();
    let first = list_transcripts(root, 0.0, None);
    assert_eq!(first.failed_paths, 0);
    assert_eq!(first.files.len(), 80);
    assert_eq!(first.walked, list_transcripts(root, 0.0, None).walked);
    assert_eq!(
        list_transcripts(root, 0.0, Some("updates.jsonl"))
            .files
            .len(),
        40
    );
}

#[test]
fn hostile_selected_nesting_is_skipped_without_losing_the_next_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.jsonl");
    let sample = line(1, 42);
    let hostile = sample.replace(
        "\"input_tokens\":10",
        &format!(
            "\"padding\":{}0{},\"input_tokens\":10",
            "[".repeat(200_000),
            "]".repeat(200_000)
        ),
    );
    fs::write(&path, hostile + &line(2, 7)).unwrap();
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| r.totals.output_tokens)
            .collect::<Vec<_>>(),
        [7]
    );
}

#[test]
fn hostile_wide_json_is_skipped_before_materializing_selected_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.jsonl");
    let hostile = line(1, 42).replace(
        "\"input_tokens\":10",
        &format!("\"padding\":[{}0],\"input_tokens\":10", "0,".repeat(20_000)),
    );
    fs::write(&path, hostile + &line(2, 7)).unwrap();
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| r.totals.output_tokens)
            .collect::<Vec<_>>(),
        [7]
    );
}

#[test]
fn hostile_line_and_key_lengths_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.jsonl");
    let mut writer = fs::File::create(&path).unwrap();
    writer.write_all(b"{\"").unwrap();
    let chunk = vec![b'x'; 1024 * 1024];
    for _ in 0..300 {
        writer.write_all(&chunk).unwrap();
    }
    writer.write_all(b"\":0}\n").unwrap();
    writer.write_all(line(1, 42).as_bytes()).unwrap();
    drop(writer);
    assert!(read_transcript(&path, &P::Claude, None).is_none());

    let mut writer = fs::File::create(&path).unwrap();
    writer.write_all(b"{\"").unwrap();
    for _ in 0..20 {
        writer.write_all(&chunk).unwrap();
    }
    writer.write_all(b"\":0}\n").unwrap();
    writer.write_all(line(2, 7).as_bytes()).unwrap();
    drop(writer);
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].totals.output_tokens, 7);
    assert_eq!(
        parsed.position.resume_offset,
        fs::metadata(&path).unwrap().len()
    );
}

#[test]
fn compression_bomb_is_a_read_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bomb.jsonl.zst");
    let mut encoder =
        zstd::stream::write::Encoder::new(fs::File::create(&path).unwrap(), 0).unwrap();
    let chunk = vec![b' '; 1024 * 1024];
    for _ in 0..64 {
        encoder.write_all(&chunk).unwrap();
    }
    encoder.write_all(line(1, 42).as_bytes()).unwrap();
    encoder.finish().unwrap();
    assert!(read_transcript(&path, &P::Claude, None).is_none());
}

#[test]
fn invalid_utf8_matches_nodes_replacement_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.jsonl");
    let sample = line(1, 42).replace("claude-fable-5", "claude-X");
    let mut bytes = sample.into_bytes();
    *bytes.iter_mut().find(|b| **b == b'X').unwrap() = 255;
    fs::write(&path, bytes).unwrap();
    let parsed = read_transcript(&path, &P::Claude, None).unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].model, "claude-\u{fffd}");
}

#[test]
fn hostile_grok_counters_cannot_overflow_the_reducer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.jsonl");
    fs::write(&path, json!({"timestamp":1785580000000u64,"params":{"sessionId":"fixture","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"a":{"inputTokens":1e99,"cachedReadTokens":1e99,"cacheCreationTokens":1e99,"outputTokens":1e99},"b":{"inputTokens":1e99,"outputTokens":1e99}},"costUsdTicks":100}}}}).to_string() + "\n").unwrap();
    let parsed = read_transcript(&path, &P::Grok, None).unwrap();
    assert_eq!(parsed.records.len(), 2);
    assert_eq!(parsed.records[0].totals.uncached_input_tokens, 0);
}

#[test]
fn discovery_and_direct_reads_reject_links() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.jsonl");
    fs::write(&target, line(1, 42)).unwrap();
    let link = dir.path().join("link.jsonl");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(list_transcripts(dir.path(), 0.0, None).files.is_empty());
    assert!(read_transcript(&link, &P::Claude, None).is_none());
}

#[cfg(unix)]
#[test]
fn discovery_and_direct_reads_reject_fifos_without_opening_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pipe.jsonl");
    nix::unistd::mkfifo(
        &path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    assert!(list_transcripts(dir.path(), 0.0, None).files.is_empty());
    assert!(read_transcript(&path, &P::Claude, None).is_none());
}

#[test]
fn metadata_milliseconds_match_the_node_timestamp_table_exactly() {
    use std::time::{Duration, UNIX_EPOCH};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("time.jsonl");
    let file = fs::File::create(&path).unwrap();
    let cases: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("node-mtimes.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let sec = case["sec"].as_i64().unwrap();
        let nsec = case["nsec"].as_u64().unwrap();
        let at = if sec >= 0 {
            UNIX_EPOCH + Duration::new(sec as u64, nsec as u32)
        } else {
            UNIX_EPOCH - Duration::from_secs(sec.unsigned_abs()) + Duration::from_nanos(nsec)
        };
        file.set_modified(at).unwrap();
        let actual = list_transcripts(dir.path(), f64::NEG_INFINITY, None).files[0].mtime_ms;
        assert_eq!(
            actual.to_bits(),
            case["milliseconds"].as_f64().unwrap().to_bits(),
            "sec={sec} nsec={nsec}"
        );
    }
}
