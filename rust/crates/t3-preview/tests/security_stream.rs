#![allow(clippy::unwrap_used)]
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{preview::*, preview_automation::*};
use t3_preview::{Error, HtmlService, discovery::*, html::*, stream::*, urls::*};

fn html_service(root: &std::path::Path) -> HtmlService {
    HtmlService::new(
        HtmlOptions {
            attachments_dir: root.join("attachments"),
            state_dir: root.join("state"),
            secrets_dir: root.join("secrets"),
            recovery: Arc::new(MemoryHtmlRecovery::default()),
        },
        None,
    )
}

#[test]
fn url_normalization_preserves_remote_access_and_refuses_non_web_protocols() {
    for (raw, expected) in [
        ("localhost:5173", "http://localhost:5173/"),
        (" 127.0.0.1:3000/x ", "http://127.0.0.1:3000/x"),
        ("[::1]:5173", "http://[::1]:5173/"),
        ("example.com/path", "https://example.com/path"),
        ("http://10.0.0.5:3000", "http://10.0.0.5:3000/"),
        (
            "https://user:password@example.com/x?q=1#p",
            "https://user:password@example.com/x?q=1#p",
        ),
    ] {
        assert_eq!(normalize_preview_url(raw).unwrap(), expected);
    }
    for raw in [
        "",
        "  ",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,secret",
        "about:blank",
        "blob:https://example.com/1",
        "chrome://settings",
        "ftp://host/a",
        "mailto:user@example.com",
        "ws://localhost:3000",
        "ssh://host",
        "http://[bad]/",
    ] {
        assert!(normalize_preview_url(raw).is_err(), "accepted {raw}");
    }
}

#[test]
fn invalid_preview_url_text_preserves_protocol_length_and_blank_tab_guidance() {
    for (raw, reason, protocol) in [
        ("", "empty", None),
        (
            " JavaScript:private ",
            "unsupported-protocol",
            Some("javascript:"),
        ),
        ("http://[", "parse", Some("http:")),
        (
            "gopher://example.com",
            "unsupported-protocol",
            Some("gopher:"),
        ),
    ] {
        let error = normalize_preview_url(raw).unwrap_err();
        let suffix = protocol.map(|p| format!(": {p}")).unwrap_or_default();
        assert_eq!(
            error.to_string(),
            format!(
                "Invalid preview URL ({reason}{suffix}; input length {}). Use an http(s) URL or a schemeless host such as localhost:5173. With preview_open, omit url when creating a new tab to open a blank tab.",
                raw.encode_utf16().count()
            )
        );
    }
}

#[test]
fn configured_discovery_urls_are_loopback_only_capped_and_rewrite_wildcards() {
    let urls = [
        "http://0.0.0.0:3000/docs#first",
        "https://127.0.0.1:4000/private",
        "http://[::1]:5000/docs",
        "http://localhost.evil:3000",
        "http://127.0.0.2:3000",
        "https://example.com",
        "file:///x",
        "http://localhost:3000/docs#first",
    ]
    .map(str::to_owned);
    assert_eq!(
        configured_urls(&urls),
        vec![
            "http://localhost:3000/docs#first",
            "https://127.0.0.1:4000/private",
            "http://[::1]:5000/docs"
        ]
    );
    let many = (0..40)
        .map(|n| format!("http://localhost:3000/{n}"))
        .collect::<Vec<_>>();
    assert_eq!(configured_urls(&many).len(), 32);
    let prefix = "http://0.0.0.0/";
    let long = format!("{prefix}{}", "a".repeat(2048 - prefix.len()));
    assert!(configured_urls(&[long]).is_empty());
}

#[test]
fn profile_paths_are_one_encoded_component_for_every_platform() {
    let root = PathBuf::from("profiles");
    for id in [
        "work/team",
        "../secret",
        "..",
        ".",
        "C:\\secret",
        "\\\\server\\share",
        "a\0b",
        "%2e%2e/slash",
        "équipe",
    ] {
        let path = profile_directory(&root, id);
        assert_eq!(path.parent(), Some(root.as_path()));
        assert_ne!(path, root.join(".."));
        assert_ne!(path, root.join("."));
    }
    assert_eq!(
        profile_directory(&root, "work/team"),
        root.join("work%2Fteam")
    );
    assert_eq!(profile_directory(&root, ".."), root.join("%2E%2E"));
}

#[test]
fn viewport_presets_orientation_and_area_are_enforced() {
    let input: PreviewAutomationResizeInput = serde_json::from_value(
        json!({"mode":"preset","preset":"iphone-12-pro","orientation":"landscape"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(resolve_viewport(input).unwrap()).unwrap(),
        json!({"_tag":"preset","width":844,"height":390,"presetId":"iphone-12-pro"})
    );
    for (w, h) in [(239, 800), (1280, 0), (3841, 800), (3840, 3840)] {
        assert!(
            validate_viewport(&PreviewViewportSetting::Freeform {
                width: w,
                height: h
            })
            .is_err()
        );
    }
    assert!(
        validate_viewport(&PreviewViewportSetting::Freeform {
            width: 3840,
            height: 2160
        })
        .is_ok()
    );
}

#[test]
fn viewport_height_rejection_scans_styles_and_comments_without_scanning_data_uris() {
    for html in [
        "<style>main{min-height:100vh}</style>",
        "<style media=screen>main{height:calc(100dvh - 2rem)}",
        "<div style='top:.5svh'></div>",
        "<div style=height:100vh></div>",
    ] {
        assert_eq!(
            reject_viewport_height(html).unwrap_err().tag(),
            "HtmlRenderViewportHeightError"
        );
    }
    for html in [
        "<style>main{width:100vw;height:320px}</style><img src='data:image/png;base64,AB1vh+2vh/3vhC'>",
        "<p>Avoid 100vh</p><!-- <div style='height:100vh'> --><script>const unit='100vh';</script>",
        "<style>main{--word:a100vh;height:100vhat}</style>",
    ] {
        reject_viewport_height(html).unwrap();
    }
    reject_viewport_height(&format!("<style>{}vx</style>", "1".repeat(200000))).unwrap();
}

#[test]
fn bootstrap_is_in_live_head_before_content_and_outside_inert_text() {
    for source in [
        "<html><head><title>X</title></head><body/></html>",
        "<html><body/></html>",
        "<!doctype html><p>X</p>",
        "<!-- <head> --><template><head></head></template><p>X</p>",
        "<script>const value='<head>'</script><p>X</p>",
    ] {
        let result = inject_bootstrap(source);
        assert!(result.contains("<style id=\"t3-theme\">"));
        assert!(
            result.contains("<body/>")
                || result.contains("<p>X</p>")
                || result.contains("<title>X</title>")
        );
        assert!(
            result.find("<style id=\"t3-theme\">").unwrap()
                < result.find("<title>X</title>").unwrap_or(usize::MAX)
        );
    }
    let result = inject_bootstrap("<head><meta charset='utf-8'><meta name=viewport></head>");
    assert_eq!(result.matches("<meta charset").count(), 1);
    assert_eq!(result.matches("<meta name").count(), 1);
    let inert = "<script>const text='</style><head>fake';</script><head>";
    let result = inject_bootstrap(&format!("{inert}<title>live</title></head>"));
    assert!(result.starts_with(inert));
    assert!(result.find("<style id=\"t3-theme\">").unwrap() < result.find("<title>live").unwrap());
}

#[test]
fn svg_roots_and_image_magic_refuse_renamed_secrets_and_doctype_smuggling() {
    let good = [
        "<svg/>",
        "<?xml version='1.0'?><?xml-stylesheet href='x'?><!DOCTYPE svg [<!ENTITY fill 'red'>]><svg />",
        "<!--comment--><svg\n/> ",
    ];
    for image in good {
        assert!(is_image_bytes(image.as_bytes()), "{image}");
    }
    for secret in [
        "API_KEY=abc123",
        "<!doctype html><body><svg/>SECRET",
        "<!DOCTYPE config [<!ENTITY b '\"]><svg/>\"'>]><config>secret</config>",
        "<?p?><?p?><config><svg/></config>",
        "<!DOCTYPE svg [<!ENTITY a 'x><svg/>",
        "<SVG/>SECRET",
        "<svgé/>SECRET",
        "<svgx/>SECRET",
    ] {
        assert!(!is_image_bytes(secret.as_bytes()), "{secret}");
    }
    for magic in [
        &b"\x89PNG"[..],
        &b"\xff\xd8\xff"[..],
        &b"GIF8"[..],
        &b"RIFF1234WEBP"[..],
        &b"1234ftypavif"[..],
    ] {
        assert!(is_image_bytes(magic));
    }
}

#[tokio::test]
async fn local_images_are_inlined_once_per_reference_and_urls_are_untouched() {
    let root = tempfile::tempdir().unwrap();
    let png = root.path().join("shot.png");
    let svg = root.path().join("logo.svg");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    std::fs::write(&svg, b"<svg/>").unwrap();
    let source = format!(
        "<html><head><title>Images</title></head><img src=\"{}\"><style>main{{background:url({})}}</style><script>const image='{}';</script><img src='https://example.com/a.png'><img src='./b.png'>",
        png.display(),
        svg.display(),
        png.display()
    );
    let prepared = html_service(root.path()).prepare(&source).await.unwrap();
    assert_eq!(
        prepared
            .matches("data:image/png;base64,iVBORw0KGgo=")
            .count(),
        2
    );
    assert!(prepared.contains("url(data:image/svg+xml;base64,PHN2Zy8+)"));
    assert!(!prepared.contains(&root.path().to_string_lossy().to_string()));
    assert!(prepared.contains("https://example.com/a.png"));
    assert!(prepared.contains("./b.png"));
}

#[tokio::test]
async fn every_unreadable_or_renamed_local_image_is_reported_without_its_bytes() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing.jpg");
    let secret = root.path().join("secret.png");
    let folder = root.path().join("folder.webp");
    std::fs::write(&secret, "API_KEY=abc123").unwrap();
    std::fs::create_dir(&folder).unwrap();
    let source = format!(
        "<img src=\"{}\"><img src='{}'><img src=\"{}\"><img src=\"{}\">",
        missing.display(),
        secret.display(),
        folder.display(),
        secret.display()
    );
    let error = html_service(root.path())
        .prepare(&source)
        .await
        .unwrap_err();
    let Error::Operation {
        tag,
        detail: Some(detail),
        ..
    } = error
    else {
        panic!()
    };
    assert_eq!(tag, "HtmlRenderImagesNotFoundError");
    assert_eq!(detail["paths"], json!([missing, secret, folder]));
    assert!(!detail.to_string().contains("abc123"));
}

#[tokio::test]
async fn page_file_reads_refuse_hardlinks_secrets_wrong_extensions_and_relative_paths() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let secrets = root.path().join("secrets");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&secrets).unwrap();
    let page = root.path().join("page.html");
    std::fs::write(&page, "<!doctype html><p>page</p>").unwrap();
    assert!(
        read_page_file(&page, &state, &secrets)
            .unwrap()
            .contains("page")
    );
    let linked = root.path().join("linked.html");
    std::fs::hard_link(&page, &linked).unwrap();
    let renamed = root.path().join("renamed.html");
    std::fs::write(&renamed, "PRIVATE KEY").unwrap();
    let key = root.path().join("id_rsa");
    std::fs::write(&key, "<!doctype html><p>no</p>").unwrap();
    let folder = root.path().join("folder.html");
    std::fs::create_dir(&folder).unwrap();
    let internal = secrets.join("page.html");
    std::fs::write(&internal, "<html>secret</html>").unwrap();
    let sqlite = state.join("state.sqlite.html");
    std::fs::write(&sqlite, "<html>database</html>").unwrap();
    for path in [
        linked,
        page,
        renamed,
        key,
        folder,
        internal,
        sqlite,
        PathBuf::from("page.html"),
        PathBuf::from("//attacker/share/page.html"),
    ] {
        assert_eq!(
            read_page_file(&path, &state, &secrets).unwrap_err().tag(),
            "HtmlRenderFileError",
            "{}",
            path.display()
        );
    }
    let huge = root.path().join("huge.html");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len((MAX_PAGE_BYTES + 1) as u64)
        .unwrap();
    assert_eq!(
        read_page_file(&huge, &state, &secrets).unwrap_err().tag(),
        "HtmlRenderFileTooLargeError"
    );
}

#[tokio::test]
async fn html_publish_keeps_source_snapshot_and_recovery_is_consumed_once() {
    let root = tempfile::tempdir().unwrap();
    let recovery = Arc::new(MemoryHtmlRecovery::default());
    let service = HtmlService::new(
        HtmlOptions {
            attachments_dir: root.path().join("attachments"),
            state_dir: root.path().join("state"),
            secrets_dir: root.path().join("secrets"),
            recovery: recovery.clone(),
        },
        None,
    );
    let source = root.path().join("report.html");
    std::fs::write(&source, "<!doctype html><p>first draft</p>").unwrap();
    let input = PublishedSource {
        source: HtmlSource::Path {
            path: source.clone(),
        },
        title: "  Report  ".into(),
        height: 9000.0,
    };
    let reference = service
        .publish("thread-html".into(), input.clone())
        .await
        .unwrap();
    assert_eq!(reference.title, "Report");
    assert_eq!(reference.height, 2000);
    assert!(reference.attachment_id.ends_with("-html"));
    assert!(reference.heights.is_none());
    std::fs::write(source, "<!doctype html><p>second draft</p>").unwrap();
    let saved = std::fs::read_to_string(
        root.path()
            .join("attachments")
            .join(format!("{}.html", reference.attachment_id)),
    )
    .unwrap();
    assert!(saved.contains("first draft"));
    assert!(!saved.contains("second draft"));
    assert_eq!(
        recovery.take(&"thread-html".into(), &input),
        Some(reference)
    );
    assert!(recovery.take(&"thread-html".into(), &input).is_none());
}

#[test]
fn stream_frames_are_bounded_and_control_cannot_be_buried() {
    let queue = ViewerQueue::default();
    let mut dropped = vec![];
    queue.push(control_message(true, "agent", 0, json!(null)));
    queue.push(ViewerOutput::Message(
        json!({"type":"viewport","width":1280,"height":800}),
    ));
    for n in 1..=70 {
        dropped.extend(queue.push(ViewerOutput::Frame {
            data: vec![n as u8],
            frame_id: n,
        }));
    }
    assert_eq!(dropped, (63..=70).collect::<Vec<_>>());
    dropped = queue.push(control_message(true, "you", 1, json!(null)));
    assert_eq!(dropped, (1..=62).collect::<Vec<_>>());
    queue.push(ViewerOutput::Message(
        json!({"type":"viewport","width":390,"height":844}),
    ));
    queue.push(ViewerOutput::Reconnect);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {let one=queue.recv().await.unwrap();let two=queue.recv().await.unwrap();assert!(matches!(one,ViewerOutput::Message(ref v)if v["type"]=="control"&&v["controller"]=="you"));assert!(matches!(two,ViewerOutput::Message(ref v)if v["type"]=="viewport"));});
}

#[test]
fn wire_flow_sends_only_latest_pending_frame_and_acknowledges_replaced_frames() {
    let mut flow = FrameFlow::default();
    assert_eq!(flow.offer(vec![1], 1), (Some(vec![1]), None));
    assert_eq!(flow.offer(vec![2], 2), (None, None));
    assert_eq!(flow.offer(vec![3], 3), (None, Some(2)));
    assert_eq!(flow.offer(vec![4], 4), (None, Some(3)));
    assert_eq!(flow.ack(), (Some(vec![4]), Some(1)));
    assert_eq!(flow.ack(), (None, Some(4)));
    assert_eq!(flow.ack(), (None, None));
    assert!(check_socket_buffer(MAX_SOCKET_BUFFER_BYTES - 14, 0).is_ok());
    assert!(check_socket_buffer(MAX_SOCKET_BUFFER_BYTES - 14, 1).is_err());
    assert!(check_socket_buffer(usize::MAX, usize::MAX).is_err());
}

#[test]
fn process_listener_parsers_filter_nonlocal_and_established_sockets() {
    let lsof = "p42\ncnode\nn*:5173\nn127.0.0.1:3000\nn10.0.0.2:4000\np99\nctest\nn[::1]:8000\n";
    assert_eq!(
        parse_lsof(lsof)
            .iter()
            .map(|l| (l.port, l.pid))
            .collect::<Vec<_>>(),
        vec![(5173, Some(42)), (3000, Some(42)), (8000, Some(99))]
    );
    let windows = "TCP 127.0.0.1:5173 0.0.0.0:0 LISTENING 42\nTCP [::]:3000 [::]:0 ÉCOUTE 99\nTCP 127.0.0.1:4000 127.0.0.1:5000 ESTABLISHED 42\nTCP 10.0.0.2:9000 0.0.0.0:0 LISTENING 42";
    assert_eq!(
        parse_windows_listeners(windows)
            .iter()
            .map(|l| l.port)
            .collect::<Vec<_>>(),
        vec![5173, 3000]
    );
    let proc = "sl local_address rem_address st tx rx tr tm retrnsmt uid timeout inode\n0: 0100007F:223D 00000000:0000 0A 0 0 0 0 0 5001\n1: 0101007F:1388 00000000:0000 0A 0 0 0 0 0 5002\n2: 00000000:0016 00000000:0000 01 0 0 0 0 0 5003";
    assert_eq!(parse_proc_tcp(proc), vec![(8765, "5001".into())]);
    assert_eq!(
        parse_proc_environment("PATH=x\0T3CODE_THREAD_ID=thread-agent\0"),
        Some("thread-agent".into())
    );
}

#[test]
fn stop_refuses_unknown_or_shared_groups_and_preserves_terminal_job_boundary() {
    let table = parse_process_table(
        "10 1 10\n20 10 10\n100 10 10\n200 100 200\n210 200 200\n300 200 300\n400 300 400\n401 400 400\n500 100 500\n600 500 600\n",
    );
    assert_eq!(stoppable_process_group(&table, 401, 100), Some(400));
    assert_eq!(stoppable_process_group(&table, 600, 100), Some(600));
    for pid in [20, 210, 500, 999] {
        assert_eq!(stoppable_process_group(&table, pid, 100), None);
    }
    let mut server:DiscoveredLocalServer=serde_json::from_value(json!({"host":"localhost","port":3000,"url":"http://localhost:3000","pid":42,"processName":"node","terminal":null})).unwrap();
    let input = PreviewStopLocalServerInput {
        pid: 42,
        port: 3000,
    };
    assert!(authorize_stop(&[server.clone()], &input).is_err());
    server.agent = Some(Some(DiscoveredLocalServerAgent {
        thread_id: "t".into(),
    }));
    assert!(authorize_stop(&[server.clone()], &input).is_ok());
    assert!(
        authorize_stop(
            &[server],
            &PreviewStopLocalServerInput {
                pid: 43,
                port: 3000
            }
        )
        .is_err()
    );
}

#[test]
fn configured_probe_accepts_navigation_documents_without_following_redirects() {
    for status in [301, 302, 303, 307, 308] {
        assert!(probe_is_web(status, None, Some("https://remote.test")));
        assert!(!probe_is_web(status, None, None));
    }
    assert!(probe_is_web(
        200,
        Some("application/xhtml+xml; charset=utf-8"),
        None
    ));
    assert!(probe_is_web(200, Some("text/html"), None));
    for status in [204, 205, 304, 404, 503] {
        assert!(!probe_is_web(status, Some("text/html"), Some("/elsewhere")));
    }
    assert!(!probe_is_web(200, Some("application/json"), None));
}
