//! The guest HTTP shape shared by every host: request building, SSE decoding, rejection mapping
//! and the transport over a scripted backend.
#![allow(clippy::unwrap_used)]

use futures::{executor::block_on, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use t3_client::guest::{
    http::{
        GuestChunks, GuestHttp, GuestMethod, GuestRequest, GuestResponse, GuestStreamResponse,
        HttpGuestTransport, SseDecoder, rejection, requests,
    },
    transport::{GuestError, GuestFailure, GuestFuture, GuestTransport},
};

#[test]
fn requests_use_the_guest_routes_and_keep_the_invite_out_of_every_url() {
    let redeem = requests::redeem("share_1.secret", "Alex");
    assert_eq!(redeem.method, GuestMethod::Post);
    assert_eq!(redeem.path, "/api/guest/redeem");
    let body: Value = serde_json::from_str(redeem.json.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"credential": "share_1.secret", "name": "Alex"})
    );

    let session = requests::session("a b&c");
    assert_eq!(session.method, GuestMethod::Get);
    assert_eq!(session.path, "/api/guest/session?share=a+b%26c");
    assert_eq!(session.json, None);
    assert_eq!(requests::stream("s1").path, "/api/guest/stream?share=s1");
    let submit = requests::submit("s1", "hi \"there\"");
    assert_eq!(submit.path, "/api/guest/messages?share=s1");
    let body: Value = serde_json::from_str(submit.json.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"shareId": "s1", "text": "hi \"there\""}));
    for request in [&redeem, &session, &submit, &requests::stream("s1")] {
        assert!(!request.path.contains("secret"), "{}", request.path);
    }
}

#[test]
fn rejections_follow_the_server_body_and_five_hundreds_retry() {
    let body = br#"{"reason":"used","message":"This guest link was already used."}"#;
    assert_eq!(
        rejection(409, body),
        GuestError::Rejected {
            status: 409,
            reason: GuestFailure::Used,
            message: "This guest link was already used.".into()
        }
    );
    // Unreadable 4xx bodies are still terminal, with the TS fallback copy.
    assert_eq!(
        rejection(403, b"<html>"),
        GuestError::Rejected {
            status: 403,
            reason: GuestFailure::Unknown,
            message: "Something went wrong. Try again.".into()
        }
    );
    assert!(matches!(
        rejection(502, b"bad gateway"),
        GuestError::Network(_)
    ));
    assert!(matches!(rejection(500, body), GuestError::Network(_)));
}

fn decode(chunks: &[&[u8]]) -> Vec<Value> {
    let mut decoder = SseDecoder::default();
    chunks
        .iter()
        .flat_map(|chunk| decoder.push(chunk))
        .collect()
}

#[test]
fn sse_frames_survive_arbitrary_chunking_and_comments() {
    let wire = b": ping\n\ndata: {\"type\":\"a\"}\n\r\ndata:{\"type\":\"b\"}\r\n\r\nevent: x\ndata: {\"type\":\ndata: 1}\n\n";
    let whole = decode(&[wire]);
    assert_eq!(
        whole,
        vec![
            json!({"type": "a"}),
            json!({"type": "b"}),
            json!({"type": 1})
        ]
    );
    // One byte at a time, which also splits "\r\n" and the multi-byte character below.
    let bytes: Vec<&[u8]> = wire.chunks(1).collect();
    assert_eq!(decode(&bytes), whole);

    let text = "data: {\"text\":\"h\u{e9}llo \u{1f600}\"}\n\n".as_bytes();
    let bytes: Vec<&[u8]> = text.chunks(1).collect();
    assert_eq!(
        decode(&bytes),
        vec![json!({"text": "h\u{e9}llo \u{1f600}"})]
    );
}

#[test]
fn sse_ignores_unparseable_frames_and_a_byte_order_mark_and_unterminated_tail() {
    let got = decode(&[b"\xef\xbb\xbfdata: not json\n\ndata: {\"ok\":true}\n\ndata: {\"lost\":1}"]);
    assert_eq!(got, vec![json!({"ok": true})]);
}

enum Reply {
    Plain(u16, &'static str),
    Stream(u16, Vec<Result<Vec<u8>, GuestError>>),
}

struct Backend {
    replies: RefCell<VecDeque<Reply>>,
    seen: RefCell<Vec<GuestRequest>>,
    dropped: Rc<RefCell<bool>>,
    slept: RefCell<Vec<u64>>,
}

struct Chunks {
    chunks: VecDeque<Result<Vec<u8>, GuestError>>,
    dropped: Rc<RefCell<bool>>,
}
impl GuestChunks for Chunks {
    fn next(&mut self) -> GuestFuture<'_, Option<Vec<u8>>> {
        let next = self.chunks.pop_front();
        Box::pin(async move { next.transpose() })
    }
}
impl Drop for Chunks {
    fn drop(&mut self) {
        *self.dropped.borrow_mut() = true;
    }
}

impl GuestHttp for Backend {
    fn send(&self, request: GuestRequest) -> GuestFuture<'_, GuestResponse> {
        self.seen.borrow_mut().push(request);
        let reply = self.replies.borrow_mut().pop_front();
        Box::pin(async move {
            match reply {
                Some(Reply::Plain(status, body)) => Ok(GuestResponse {
                    status,
                    body: body.as_bytes().to_vec(),
                }),
                _ => Err(GuestError::Network("offline".into())),
            }
        })
    }
    fn open(&self, request: GuestRequest) -> GuestFuture<'_, GuestStreamResponse> {
        self.seen.borrow_mut().push(request);
        let reply = self.replies.borrow_mut().pop_front();
        let dropped = self.dropped.clone();
        Box::pin(async move {
            match reply {
                Some(Reply::Stream(status, chunks)) => Ok(GuestStreamResponse {
                    status,
                    chunks: Box::new(Chunks {
                        chunks: chunks.into(),
                        dropped,
                    }),
                }),
                Some(Reply::Plain(status, body)) => Ok(GuestStreamResponse {
                    status,
                    chunks: Box::new(Chunks {
                        chunks: VecDeque::from([Ok(body.as_bytes().to_vec())]),
                        dropped,
                    }),
                }),
                None => Err(GuestError::Network("offline".into())),
            }
        })
    }
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        self.slept.borrow_mut().push(millis);
        Box::pin(async {})
    }
}

fn transport(replies: Vec<Reply>) -> (HttpGuestTransport<Rc<Backend>>, Rc<Backend>) {
    let backend = Rc::new(Backend {
        replies: RefCell::new(replies.into()),
        seen: RefCell::default(),
        dropped: Rc::default(),
        slept: RefCell::default(),
    });
    (HttpGuestTransport::new(backend.clone()), backend)
}

#[test]
fn transport_maps_statuses_and_delivers_stream_frames_incrementally() {
    let (transport, backend) = transport(vec![
        Reply::Plain(204, ""),
        Reply::Plain(403, r#"{"reason":"inactive","message":"gone"}"#),
        Reply::Plain(204, ""),
        Reply::Plain(502, "oops"),
        Reply::Stream(
            200,
            vec![
                Ok(b": ping\n\ndata: {\"type\":\"view\"}\n".to_vec()),
                Ok(b"\ndata: {\"type\":\"ended\"}\n\n".to_vec()),
                Err(GuestError::Network("reset".into())),
            ],
        ),
        Reply::Plain(403, r#"{"reason":"invalid","message":"no"}"#),
    ]);
    block_on(async {
        assert_eq!(transport.redeem("c.s", "Alex").await, Ok(()));
        assert!(matches!(
            transport.session("s1").await,
            Err(GuestError::Rejected {
                reason: GuestFailure::Inactive,
                ..
            })
        ));
        assert_eq!(transport.submit("s1", "hello").await, Ok(()));
        assert!(matches!(
            transport.submit("s1", "hello").await,
            Err(GuestError::Network(_))
        ));
        let mut stream = transport.open_stream("s1").await.unwrap();
        assert_eq!(stream.next().await, Ok(Some(json!({"type": "view"}))));
        assert_eq!(stream.next().await, Ok(Some(json!({"type": "ended"}))));
        // A dropped connection is a network failure, not a clean close.
        assert!(matches!(stream.next().await, Err(GuestError::Network(_))));
        assert!(!*backend.dropped.borrow());
        drop(stream);
        assert!(
            *backend.dropped.borrow(),
            "dropping releases the connection"
        );
        // A terminal rejection arrives as the open error, with the body read for its reason.
        assert!(matches!(
            transport.open_stream("s1").await,
            Err(GuestError::Rejected {
                reason: GuestFailure::Invalid,
                ..
            })
        ));
        transport.sleep(750).await;
    });
    assert_eq!(*backend.slept.borrow(), vec![750]);
    assert_eq!(backend.seen.borrow().len(), 6);
}

#[test]
fn a_clean_end_of_stream_is_none() {
    let (transport, _) = transport(vec![Reply::Stream(
        200,
        vec![Ok(b"data: {\"a\":1}\n\n".to_vec())],
    )]);
    block_on(async {
        let mut stream = transport.open_stream("s1").await.unwrap();
        assert_eq!(stream.next().await, Ok(Some(json!({"a": 1}))));
        assert_eq!(stream.next().await, Ok(None));
    });
}
