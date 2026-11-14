use serde_json::json;
use t3_client::{
    device::DeviceHubAccess,
    preview::{gesture::*, open::*, *},
};

#[test]
fn explicit_runtime_opens_start_at_saved_zoom_and_appearance() {
    let defaults = serde_json::from_value(
        json!({"viewport":{"_tag":"fill"},"profileId":"work","zoomFactor":0.8,"appearance":"dark"}),
    )
    .expect("defaults");
    for entry in [
        EntryPoint::Session,
        EntryPoint::File,
        EntryPoint::TerminalLink,
    ] {
        let input = serde_json::from_value(
            json!({"threadId":"thread","url":"https://t3.chat/","runtime":"server"}),
        )
        .expect("open");
        let output = serde_json::to_value(apply_defaults(input, &defaults, entry)).expect("wire");
        assert_eq!(output["zoomFactor"], 0.8);
        assert_eq!(output["colorScheme"], "dark");
        assert_eq!(output["profileId"], "work");
    }
    for runtime in [None, Some("desktop")] {
        let mut input = json!({"threadId":"thread"});
        if let Some(runtime) = runtime {
            input["runtime"] = json!(runtime);
        }
        let output = serde_json::to_value(apply_defaults(
            serde_json::from_value(input).expect("open"),
            &defaults,
            EntryPoint::Session,
        ))
        .expect("wire");
        assert!(output.get("runtime").is_none());
        assert!(output.get("zoomFactor").is_none());
        assert!(output.get("colorScheme").is_none());
    }
}

#[test]
fn interrupted_pointer_ends_and_resizes_hold_the_starting_shape() {
    let container = Size {
        width: 1440.,
        height: 900.,
    };
    let frame = Frame {
        x: 996.,
        y: 433.,
        width: 268.,
        height: 179.,
    };
    let mut gesture = Gesture {
        pointer_id: 1,
        pointer_type: "mouse".into(),
        pointer: Position { x: 996., y: 433. },
        frame,
        direction: Some(Direction::Northwest),
        source: Size {
            width: 1200.,
            height: 800.,
        },
        moved: true,
    };
    let pointer = |id, buttons| Pointer {
        pointer_id: id,
        buttons,
        position: Position { x: 936., y: 393. },
    };
    assert_eq!(
        step(&gesture, pointer(1, 1), container),
        Step::Resize(Frame {
            x: 936.,
            y: 393.,
            width: 328.,
            height: 219.
        })
    );
    assert_eq!(step(&gesture, pointer(1, 0), container), Step::End);
    assert_eq!(step(&gesture, pointer(1, 2), container), Step::End);
    assert_eq!(step(&gesture, pointer(2, 0), container), Step::Ignore);
    assert_eq!(
        settle_resize(
            frame,
            Direction::Northwest,
            gesture.source,
            Size {
                width: 1200.,
                height: 1000.
            },
            container
        ),
        Some(Frame {
            x: 1018.,
            y: 407.,
            width: 246.,
            height: 205.
        })
    );
    assert_eq!(
        settle_resize(
            frame,
            Direction::Northwest,
            gesture.source,
            Size {
                width: 600.,
                height: 400.
            },
            container
        ),
        None
    );
    gesture.moved = false;
    gesture.direction = None;
    assert_eq!(
        step(
            &gesture,
            Pointer {
                pointer_id: 1,
                buttons: 1,
                position: Position { x: 1001., y: 433. }
            },
            container
        ),
        Step::Ignore
    );
    assert_eq!(
        step(
            &gesture,
            Pointer {
                pointer_id: 1,
                buttons: 1,
                position: Position { x: 1002., y: 433. }
            },
            container
        ),
        Step::Move(Position { x: 1002., y: 433. })
    );
}

#[test]
fn windows_paths_outside_the_workspace_keep_their_absolute_label_and_position() {
    let root = r"C:\Users\mike\dev-stuff\t3code";
    for (path, expected) in [
        (
            r"C:\Users\mike\.claude\settings.json:3",
            "C:/Users/mike/.claude/settings.json:3",
        ),
        ("D:/data/report.csv", "D:/data/report.csv"),
        ("src/app.ts", "t3code/src/app.ts"),
        (
            r"c:\users\MIKE\dev-stuff\T3CODE\src\app.ts:3:4",
            "t3code/src/app.ts:3:4",
        ),
    ] {
        assert_eq!(
            file_path::format_workspace_relative_path(path, Some(root)),
            expected
        );
    }
}

#[test]
fn stop_loading_preserves_the_stream_and_requires_viewer_control() {
    let mut session = PreviewSession::new(PreviewTarget {
        access: DeviceHubAccess {
            http_base: "https://host/api/preview-stream".into(),
            ws_base: "wss://host/api/preview-stream".into(),
            query: Default::default(),
            credentials: false,
        },
        thread_id: "thread".into(),
        tab_id: "tab".into(),
        max_width: 800.,
        max_height: 600.,
        interactive: true,
    });
    assert!(session.stop_loading().is_none());
    session.opened(1);
    for (allowed, owner) in [(false, "you"), (true, "agent"), (true, "you")] {
        session.text(1,&json!({"type":"control","canOperate":allowed,"controller":owner,"generation":1,"dialog":null}).to_string());
        let state = session.state.clone();
        let generation = session.generation();
        assert_eq!(
            session.stop_loading(),
            (allowed && owner == "you").then(|| PreviewEvent::Send(json!({"type":"stopLoading"})))
        );
        assert_eq!(session.state, state);
        assert_eq!(session.generation(), generation);
    }
    assert_eq!(
        session.binary(1, vec![1]),
        vec![
            PreviewEvent::Send(json!({"type":"ack"})),
            PreviewEvent::Frame(vec![1])
        ]
    );
}
