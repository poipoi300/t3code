use t3_contracts::resource_telemetry::*;
use t3_telemetry::{Error, desktop::DesktopTelemetryControl};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test]
async fn readiness_is_announced_as_a_complete_control_line_without_waiting_for_http_or_samples() {
    let (writer, reader) = tokio::io::duplex(1);
    let control = DesktopTelemetryControl::new(writer);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let (sent, received) =
        tokio::join!(control.announce_server_ready(), reader.read_line(&mut line));
    sent.expect("ready sent");
    assert!(received.expect("ready line") > 0);
    assert_eq!(line, "{\"version\":1,\"type\":\"serverReady\"}\n");
    let message: DesktopTelemetryControlMessage =
        serde_json::from_str(&line).expect("canonical message");
    assert!(matches!(
        message,
        DesktopTelemetryControlMessage::DesktopTelemetryServerReady(_)
    ));
    DesktopTelemetryControl::default()
        .announce_server_ready()
        .await
        .expect("web/test no-op");
}

#[tokio::test]
async fn concurrent_controls_keep_frames_intact_and_a_closed_pipe_is_reported() {
    let (writer, reader) = tokio::io::duplex(1);
    let control = DesktopTelemetryControl::new(writer);
    let demand = DesktopTelemetryControlMessage::DesktopTelemetrySetDiagnosticsDemand(Box::new(
        DesktopTelemetrySetDiagnosticsDemand {
            version: WireLiteral,
            r#type: DesktopTelemetrySetDiagnosticsDemandType::SetDiagnosticsDemand,
            enabled: true,
        },
    ));
    let read = async move {
        let mut reader = BufReader::new(reader);
        let mut types = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("control line");
            let value: serde_json::Value = serde_json::from_str(&line).expect("intact JSON");
            types.insert(value["type"].as_str().expect("type").to_owned());
        }
        types
    };
    let (ready, demand, types) =
        tokio::join!(control.announce_server_ready(), control.send(&demand), read);
    ready.expect("ready");
    demand.expect("demand");
    assert_eq!(
        types,
        ["serverReady".to_owned(), "setDiagnosticsDemand".to_owned()]
            .into_iter()
            .collect()
    );
    let (writer, reader) = tokio::io::duplex(1);
    drop(reader);
    assert!(matches!(
        DesktopTelemetryControl::new(writer)
            .announce_server_ready()
            .await,
        Err(Error::Io(_))
    ));
}
