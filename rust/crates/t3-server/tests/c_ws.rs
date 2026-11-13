#![cfg(feature = "local-services")]
#![allow(clippy::unwrap_used)]
mod c_support;
use c_support::*;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use t3_rpc::CallKind;
use tokio_tungstenite::tungstenite::{
    Error, Message, client::IntoClientRequest, protocol::frame::coding::CloseCode,
};

const MAX_MESSAGE: usize = 100 * 1024 * 1024;

fn http_status(error: Error) -> u16 {
    match error {
        Error::Http(response) => response.status().as_u16(),
        other => panic!("expected an HTTP rejection, got {other:?}"),
    }
}

async fn connect_with(
    fx: &Fixture,
    query: &str,
    header: Option<(&'static str, String)>,
) -> Result<Socket, Error> {
    let mut request = fx.ws_url(query).into_client_request().unwrap();
    if let Some((name, value)) = header {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

#[tokio::test]
async fn the_protocol_version_is_checked_before_any_credential() {
    let fx = Fixture::start().await;
    // No credential of any kind: a wrong or missing version still answers 426, never 401.
    for query in [
        "",
        "?orchestrationProtocol=",
        "?orchestrationProtocol=1",
        "?orchestrationProtocol=2x",
        "?orchestrationProtocol=1&orchestrationProtocol=2",
    ] {
        let response = fx
            .http
            .get(format!("{}/ws{query}", fx.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 426, "{query}");
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(
            body["code"], "orchestration_protocol_incompatible",
            "{query}"
        );
        assert_eq!(body["orchestrationProtocolVersion"], 2, "{query}");
    }
    // The first occurrence of the parameter decides, as URLSearchParams.get does.
    let url = format!(
        "{}/ws?orchestrationProtocol=2&orchestrationProtocol=1",
        fx.base
    );
    assert_eq!(fx.http.get(&url).send().await.unwrap().status(), 401);
    // The right version without a credential is unauthenticated.
    let url = format!("{}/ws?orchestrationProtocol=2", fx.base);
    assert_eq!(fx.http.get(&url).send().await.unwrap().status(), 401);
    // An authenticated request that is not a WebSocket upgrade is refused, not served.
    let response = fx
        .http
        .get(&url)
        .header("cookie", &fx.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 426);
    fx.shutdown().await;
}

#[tokio::test]
async fn cookie_bearer_and_ticket_credentials_connect_and_anything_else_is_rejected() {
    let fx = Fixture::start().await;
    let token = fx.cookie.split_once('=').unwrap().1.to_string();
    for header in [
        ("cookie", fx.cookie.clone()),
        ("authorization", format!("Bearer {token}")),
    ] {
        let mut socket = connect_with(&fx, "", Some(header.clone())).await.unwrap();
        let mut client = t3_rpc::Client::new(0, Default::default());
        unary_ok(&mut socket, &mut client, "server.probe", json!({})).await;
        // The same credential, repeated after the first socket closed, still works.
        socket.close(None).await.unwrap();
    }
    let ticket = fx.ticket(&fx.cookie, "rpc").await;
    let mut socket = connect_with(&fx, &format!("&wsTicket={ticket}"), None)
        .await
        .unwrap();
    let mut client = t3_rpc::Client::new(0, Default::default());
    unary_ok(&mut socket, &mut client, "server.probe", json!({})).await;

    for (query, header) in [
        ("", None),
        ("&wsTicket=not-a-ticket", None),
        (
            "",
            Some(("authorization", "Bearer not-a-token".to_string())),
        ),
        ("", Some(("authorization", "Bearer ".to_string()))),
        ("", Some(("cookie", "t3_session=forged".to_string()))),
    ] {
        let error = connect_with(&fx, query, header.clone()).await.unwrap_err();
        assert_eq!(http_status(error), 401, "{query} {header:?}");
    }
    // A media-audience ticket does not open an RPC socket.
    let media = fx.ticket(&fx.cookie, "media").await;
    let error = connect_with(&fx, &format!("&wsTicket={media}"), None)
        .await
        .unwrap_err();
    assert_eq!(http_status(error), 401);
    fx.shutdown().await;
}

#[tokio::test]
async fn malformed_and_unknown_requests_fail_only_themselves() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;

    socket
        .send(Message::Text("{not json".into()))
        .await
        .unwrap();
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame[0]["_tag"], "Defect");

    socket
        .send(Message::Text(
            r#"{"_tag":"Request","id":"7","tag":"no.such.method","payload":{},"headers":[]}"#
                .into(),
        ))
        .await
        .unwrap();
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame[0]["_tag"], "Exit");
    assert_eq!(frame[0]["requestId"], "7");
    assert_eq!(frame[0]["exit"]["cause"][0]["_tag"], "Die");

    // A known method whose payload does not decode is a defect, never a handler call.
    let cause = first_cause(
        unary(
            &mut socket,
            &mut client,
            "server.updateSettings",
            json!({"patch": 5}),
        )
        .await,
    );
    assert_eq!(cause["_tag"], "Die");
    assert_eq!(cause["defect"]["_tag"], "InvalidPayload");

    // An interrupt for a call the server never saw is ignored.
    socket
        .send(Message::Text(
            r#"{"_tag":"Interrupt","requestId":"404"}"#.into(),
        ))
        .await
        .unwrap();
    unary_ok(&mut socket, &mut client, "server.probe", json!({})).await;

    // Application-level ping gets an application-level pong.
    socket
        .send(Message::Text(r#"{"_tag":"Ping"}"#.into()))
        .await
        .unwrap();
    assert_eq!(next_frame(&mut socket).await[0]["_tag"], "Pong");

    // Binary frames carrying UTF-8 JSON, and arrays of envelopes, are accepted.
    let batch = r#"[{"_tag":"Request","id":"9","tag":"server.probe","payload":{},"headers":[]},{"_tag":"Request","id":"10","tag":"server.probe","payload":{},"headers":[]}]"#;
    socket
        .send(Message::Binary(batch.as_bytes().to_vec().into()))
        .await
        .unwrap();
    let mut answered = Vec::new();
    while answered.len() < 2 {
        for envelope in next_frame(&mut socket).await {
            assert_eq!(envelope["_tag"], "Exit");
            assert_eq!(envelope["exit"]["_tag"], "Success");
            answered.push(envelope["requestId"].clone());
        }
    }
    answered.sort_by_key(|id| id.as_str().unwrap().to_owned());
    assert_eq!(answered, [json!("10"), json!("9")]);
    fx.shutdown().await;
}

/// Reads whole messages the way the client's frame joiner does and returns each with the
/// number of wire frames it took and the largest frame in bytes.
async fn read_messages(socket: &mut Socket, count: usize) -> Vec<(Value, usize, usize)> {
    let mut messages = Vec::new();
    let (mut pending, mut frames, mut largest) = (String::new(), 0, 0);
    while messages.len() < count {
        let text = match socket.next().await.unwrap().unwrap() {
            Message::Text(t) => t.to_string(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected {other:?}"),
        };
        frames += 1;
        largest = largest.max(text.len());
        if let Some(part) = text.strip_prefix('\u{1e}') {
            pending.push_str(part);
            continue;
        }
        let whole = std::mem::take(&mut pending) + &text;
        let message = serde_json::from_str(&whole)
            .unwrap_or_else(|e| panic!("frames interleaved or truncated: {e}"));
        messages.push((message, frames, largest));
        (frames, largest) = (0, 0);
    }
    messages
}

#[tokio::test]
async fn large_responses_are_split_into_contiguous_parts_only_when_negotiated() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    // Enough custom keybindings that the configuration is far beyond one 48 KiB part.
    let when = "terminalFocus && !terminalOpen && editorFocus && !modalOpen && sidebarFocus";
    let mut count = 0;
    for modifiers in [
        "ctrl",
        "alt",
        "shift",
        "ctrl+alt",
        "ctrl+shift",
        "alt+shift",
        "ctrl+alt+shift",
        "meta",
    ] {
        for letter in 'a'..='z' {
            if count == 250 {
                break;
            }
            count += 1;
            unary_ok(
                &mut socket,
                &mut client,
                "server.upsertKeybinding",
                json!({"key": format!("{modifiers}+{letter}"), "command": "chat.new", "when": when}),
            )
            .await;
        }
    }
    let request = |id: &str| {
        Message::Text(
            json!({"_tag":"Request","id":id,"tag":"server.getConfig","payload":{},"headers":[]})
                .to_string()
                .into(),
        )
    };

    // Negotiated: two concurrent large responses arrive as 48 KiB parts, never interleaved.
    let (mut split, _) = fx.connect(&fx.cookie, "&frameParts=1").await;
    split.send(request("1")).await.unwrap();
    split.send(request("2")).await.unwrap();
    for (message, frames, largest) in read_messages(&mut split, 2).await {
        assert_eq!(message["exit"]["_tag"], "Success");
        assert!(frames > 1, "a large message must be split into parts");
        assert!(largest <= 48 * 1024, "a part was {largest} bytes");
        let keybindings = message["exit"]["value"]["keybindings"].as_array().unwrap();
        assert!(keybindings.len() >= 250);
    }

    // Not negotiated: the same response is one frame.
    let (mut whole, _) = fx.connect(&fx.cookie, "").await;
    whole.send(request("1")).await.unwrap();
    let (message, frames, largest) = read_messages(&mut whole, 1).await.remove(0);
    assert_eq!(message["exit"]["_tag"], "Success");
    assert_eq!(frames, 1);
    assert!(
        largest > 48 * 1024,
        "the fixture is too small to need splitting: {largest}"
    );
    fx.shutdown().await;
}

#[tokio::test]
async fn an_interrupted_stream_ends_and_the_socket_keeps_serving() {
    let fx = Fixture::start().await;
    let (mut socket, mut client) = fx.connect(&fx.cookie, "").await;
    let id = client
        .start("subscribeServerConfig", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(chunk(&mut socket, &mut client).await["type"], "snapshot");
    client.interrupt(&id);
    // The client completes the interrupted call locally, at once.
    assert!(matches!(
        client.pop_event(),
        Some(t3_rpc::CallEvent::Failed { .. })
    ));
    unary_ok(&mut socket, &mut client, "server.probe", json!({})).await;
    // The stream is gone: a keybinding change reaches no subscriber on this socket. Any chunk
    // for it would surface as an unexpected event in the calls below.
    unary_ok(
        &mut socket,
        &mut client,
        "server.upsertKeybinding",
        json!({"key": "ctrl+alt+shift+f11", "command": "chat.new"}),
    )
    .await;
    unary_ok(&mut socket, &mut client, "server.probe", json!({})).await;
    while let Some(event) = client.pop_event() {
        assert!(
            !matches!(event, t3_rpc::CallEvent::Chunk { .. }),
            "a chunk arrived for the interrupted stream: {event:?}"
        );
    }
    fx.shutdown().await;
}

#[tokio::test]
async fn closing_a_socket_marks_its_session_disconnected_and_revocation_sends_a_close_frame() {
    let fx = Fixture::start().await;
    let (mut admin, mut admin_client) = fx.connect(&fx.cookie, "").await;
    admin_client
        .start("subscribeAuthAccess", json!({}), CallKind::Stream)
        .unwrap();
    assert_eq!(
        chunk(&mut admin, &mut admin_client).await["type"],
        "snapshot"
    );

    // A second client connects, then vanishes without a closing handshake.
    let other_cookie = fx.scoped_cookie(&["orchestration:read"]).await;
    let (other, _) = fx.connect(&other_cookie, "").await;
    let other_id = loop {
        let change = chunk(&mut admin, &mut admin_client).await;
        if change["type"] == "clientUpserted"
            && change["payload"]["connected"] == true
            && change["payload"]["current"] == false
        {
            break change["payload"]["sessionId"].clone();
        }
    };
    drop(other);
    loop {
        let change = chunk(&mut admin, &mut admin_client).await;
        if change["type"] == "clientUpserted"
            && change["payload"]["sessionId"] == other_id
            && change["payload"]["connected"] == false
        {
            break;
        }
    }

    // A third client is revoked while connected and receives a close frame.
    let third_cookie = fx.scoped_cookie(&["orchestration:read"]).await;
    let (mut third, _) = fx.connect(&third_cookie, "").await;
    let third_id = loop {
        let change = chunk(&mut admin, &mut admin_client).await;
        if change["type"] == "clientUpserted"
            && change["payload"]["connected"] == true
            && change["payload"]["current"] == false
        {
            break change["payload"]["sessionId"].clone();
        }
    };
    let revoke = fx
        .http
        .post(format!("{}/api/auth/clients/revoke", fx.base))
        .header("cookie", &fx.cookie)
        .json(&json!({"sessionId": third_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 200);
    loop {
        match third.next().await {
            Some(Ok(Message::Close(_))) => break,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("expected a close frame, got {other:?}"),
        }
    }
    fx.shutdown().await;
}

#[tokio::test]
async fn a_single_message_over_the_limit_is_refused_and_the_socket_closed() {
    let fx = Fixture::start().await;
    let (mut socket, _) = fx.connect_unlimited("").await;
    // Whether this send completes depends on how early the server stops reading.
    let _ = socket
        .send(Message::Text("a".repeat(MAX_MESSAGE + 1).into()))
        .await;
    // The transport rejects the message from its frame header, while the peer is still
    // sending, so the peer sees the 1009 close frame or, when the operating system resets the
    // connection first, a reset. It is never served: no reply frame comes back.
    loop {
        match socket.next().await {
            Some(Ok(Message::Close(frame))) => {
                if let Some(frame) = frame {
                    assert_eq!(frame.code, CloseCode::Size);
                }
                break;
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Err(_)) | None => break,
            other => panic!("an oversized message was answered: {other:?}"),
        }
    }
    fx.shutdown().await;
}

#[tokio::test]
async fn frame_parts_that_assemble_beyond_the_limit_close_the_socket_with_1009() {
    let fx = Fixture::start().await;
    let (mut socket, _) = fx.connect_unlimited("&frameParts=1").await;
    let part = format!("\u{1e}{}", "a".repeat(48 * 1024 - 3));
    let parts = MAX_MESSAGE / (48 * 1024 - 3) + 2;
    for _ in 0..parts {
        if socket
            .send(Message::Text(part.clone().into()))
            .await
            .is_err()
        {
            break;
        }
    }
    loop {
        match socket.next().await {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(frame.code, CloseCode::Size);
                break;
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("expected a 1009 close frame, got {other:?}"),
        }
    }
    fx.shutdown().await;
}
