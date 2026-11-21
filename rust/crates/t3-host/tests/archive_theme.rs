use std::{
    io::{Cursor, Write},
    path::Path,
};
use t3_host::{environment_theme::*, zip_archive::*};
use zip::{ZipWriter, write::SimpleFileOptions};
fn zipped(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
fn limits() -> ArchiveLimits {
    ArchiveLimits {
        compressed_bytes: 1024 * 1024,
        entries: 10,
        entry_bytes: 1024,
        expanded_bytes: 2048,
    }
}
#[test]
fn archives_reject_traversal_absolute_windows_names_and_symlinks() {
    for name in [
        "../outside",
        "/root",
        "a/../../b",
        "a\\b",
        "C:/drive",
        "a\0b",
    ] {
        let bytes = zipped(&[(name, b"data")]);
        assert!(
            Archive::new(Cursor::new(bytes), limits())
                .and_then(|mut a| a.read_entry(0))
                .is_err(),
            "{name:?}"
        );
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_symlink("link", "outside", SimpleFileOptions::default())
        .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(
        Archive::new(Cursor::new(bytes), limits())
            .unwrap()
            .read_entry(0)
            .is_err()
    );
}
#[test]
fn archive_bounds_cover_count_member_total_compressed_size_and_crc() {
    let bytes = zipped(&[("a", b"1234"), ("b", b"5678")]);
    let mut bound = limits();
    bound.entries = 1;
    assert!(Archive::new(Cursor::new(&bytes), bound).is_err());
    let mut bound = limits();
    bound.entry_bytes = 3;
    assert!(
        Archive::new(Cursor::new(&bytes), bound)
            .unwrap()
            .read_entry(0)
            .is_err()
    );
    let mut bound = limits();
    bound.expanded_bytes = 7;
    let mut archive = Archive::new(Cursor::new(&bytes), bound).unwrap();
    assert_eq!(archive.read_entry(0).unwrap().bytes, b"1234");
    assert!(archive.read_entry(1).is_err());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("archive.zip");
    std::fs::write(&path, &bytes).unwrap();
    let mut bound = limits();
    bound.compressed_bytes = bytes.len() as u64 - 1;
    assert!(Archive::open(&path, bound).is_err());
    assert!(Archive::new(Cursor::new(&bytes), bound).is_err());
    let mut duplicate = bytes.clone();
    let second = duplicate
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"PK\x01\x02")
        .nth(1)
        .unwrap()
        .0;
    duplicate[second + 46] = b'a';
    assert!(matches!(
        Archive::new(Cursor::new(duplicate), limits()),
        Err(t3_host::Error::Refused("archive repeats a member"))
    ));
    let mut corrupt = bytes;
    let offset = corrupt.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    corrupt[offset + 16] ^= 1;
    assert!(
        Archive::new(Cursor::new(corrupt), limits())
            .and_then(|mut a| a.read_entry(0))
            .is_err()
    );
}
#[test]
fn archive_declared_size_mismatch_and_truncated_central_directory_fail() {
    let mut bytes = zipped(&[("a", b"1234")]);
    let offset = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    bytes[offset + 24..offset + 28].copy_from_slice(&99_u32.to_le_bytes());
    assert!(
        Archive::new(Cursor::new(bytes), limits())
            .and_then(|mut a| a.read_entry(0))
            .is_err()
    );
    assert!(Archive::new(Cursor::new(b"PK\x03\x04garbage"), limits()).is_err());
}
fn theme() -> serde_json::Value {
    serde_json::json!({"name":"Custom","appearance":"dark","canvas":"#123","accent":"#abcdef"})
}
fn write(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
#[test]
fn themes_reject_reserved_ids_invalid_shapes_and_per_file_size() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("custom.json"), &theme());
    for id in ["t3-chat", "t3-code", "system", "Upper", "bad_id"] {
        write(&temp.path().join(format!("{id}.json")), &theme());
    }
    let mut invalid = theme();
    invalid["canvas"] = "red".into();
    write(&temp.path().join("bad-color.json"), &invalid);
    let mut invalid = theme();
    invalid["name"] = " ".into();
    write(&temp.path().join("bad-name.json"), &invalid);
    for key in ["light", "dark"] {
        let mut invalid = theme();
        invalid["variants"] = serde_json::json!({key:null});
        write(&temp.path().join(format!("null-{key}.json")), &invalid);
    }
    write(
        &temp.path().join("empty.json"),
        &serde_json::json!({"name":"No palette","appearance":"dark","variants":{"dark":{"canvas":"#123"}}}),
    );
    std::fs::write(
        temp.path().join("huge.json"),
        vec![b' '; MAX_THEME_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let themes = read_published_themes(temp.path());
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].id, "custom");
}
#[test]
fn malformed_themes_count_towards_work_bound_and_total_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..MAX_THEME_FILES {
        std::fs::write(temp.path().join(format!("a{i:02}.json")), "bad").unwrap();
    }
    write(&temp.path().join("z-valid.json"), &theme());
    assert!(read_published_themes(temp.path()).is_empty());
    let temp = tempfile::tempdir().unwrap();
    let mut value = theme();
    let colors: serde_json::Map<String, serde_json::Value> = (0..400)
        .map(|i| {
            (
                format!("role{i}"),
                serde_json::Value::String("x".repeat(60)),
            )
        })
        .collect();
    value["colors"] = colors.into();
    for i in 0..10 {
        write(&temp.path().join(format!("theme-{i}.json")), &value);
    }
    let themes = read_published_themes(temp.path());
    assert!(!themes.is_empty());
    assert!(themes.len() < 10);
}
#[test]
fn theme_reader_rejects_symlinked_files_and_accepts_bom() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("actual.json");
    std::fs::write(&file, format!("\u{feff}{}", theme())).unwrap();
    assert!(
        read_theme_file_guarded(&file, MAX_THEME_FILE_BYTES)
            .unwrap()
            .starts_with('{')
    );
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&file, temp.path().join("linked.json")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&file, temp.path().join("linked.json")).unwrap();
    assert!(
        read_theme_file_guarded(&temp.path().join("linked.json"), MAX_THEME_FILE_BYTES).is_none()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn reading_themes_does_not_schedule_another_scan() {
    use notify::{EventKind, RecursiveMode, Watcher, event::AccessKind};
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("workshop.json");
    write(&file, &theme());
    let (opens, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let mut observer = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if let Ok(event) = e
            && matches!(event.kind, EventKind::Access(AccessKind::Open(_)))
            && event.paths.contains(&file)
        {
            let _ = opens.send(());
        }
    })
    .unwrap();
    observer
        .watch(temp.path(), RecursiveMode::NonRecursive)
        .unwrap();
    let service = EnvironmentThemes::new(temp.path().to_owned());
    let _watch = service.start().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), observed.recv())
            .await
            .expect("initial scan"),
        Some(())
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), observed.recv())
            .await
            .is_err(),
        "the initial read triggered another scan without a write"
    );
}
