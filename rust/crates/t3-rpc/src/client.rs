use hashlink::LinkedHashMap;
use std::collections::VecDeque;

use serde_json::Value;

use crate::codec::{Cause, ClientMessage, Exit, Request, RequestId, ServerMessage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Unary,
    Stream,
}

#[derive(Clone, Copy, Debug)]
pub struct ClientOptions {
    pub ping_interval_ms: u64,
    pub ping_timeout_ms: u64,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            ping_interval_ms: 5000,
            ping_timeout_ms: 15000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("RPC connection closed")]
    Disconnected,
    #[error("RPC connection timed out")]
    PingTimeout,
    #[error("RPC call interrupted")]
    Interrupted,
    #[error("duplicate request id")]
    DuplicateId,
    #[error("numeric request ids exhausted")]
    IdExhausted,
    #[error("RPC connection defect: {0}")]
    Defect(Value),
    #[error("RPC protocol error: {0}")]
    Protocol(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CallEvent<A = Value, E = Value, P = Value> {
    Chunk { id: RequestId, values: Vec<A> },
    Exit { id: RequestId, exit: Exit<A, E> },
    Failed { id: RequestId, error: ClientError },
    ServerRequest(Request<P>),
}

pub struct Client<P = Value, A = Value, E = Value> {
    options: ClientOptions,
    next_id: u64,
    pending: LinkedHashMap<RequestId, CallKind>,
    outgoing: VecDeque<ClientMessage<P>>,
    events: VecDeque<CallEvent<A, E, P>>,
    last_seen: u64,
    next_ping: u64,
    awaiting_response: bool,
    closed: bool,
    eof: bool,
}

impl<P, A, E> Client<P, A, E> {
    pub fn new(now_ms: u64, options: ClientOptions) -> Self {
        Self {
            options,
            next_id: 0,
            pending: LinkedHashMap::new(),
            outgoing: VecDeque::new(),
            events: VecDeque::new(),
            last_seen: now_ms,
            next_ping: now_ms.saturating_add(options.ping_interval_ms),
            awaiting_response: false,
            closed: false,
            eof: false,
        }
    }

    pub fn start(
        &mut self,
        tag: impl Into<String>,
        payload: P,
        kind: CallKind,
    ) -> Result<RequestId, ClientError> {
        if self.next_id > 9_007_199_254_740_991 {
            return Err(ClientError::IdExhausted);
        }
        let id = self.next_id.into();
        self.start_request(Request::new(id, tag, payload), kind)
    }

    /// Supplies caller-owned headers, tracing, or string ids without interpreting payloads.
    pub fn start_request(
        &mut self,
        request: Request<P>,
        kind: CallKind,
    ) -> Result<RequestId, ClientError> {
        if self.closed || self.eof {
            return Err(ClientError::Disconnected);
        }
        if self.pending.contains_key(&request.id) {
            return Err(ClientError::DuplicateId);
        }
        if let RequestId::Number(n) = &request.id
            && let Some(n) = n.as_u64()
        {
            self.next_id = self.next_id.max(n.saturating_add(1));
        }
        let id = request.id.clone();
        if request.is_notification.is_none() {
            self.pending.insert(id.clone(), kind);
        }
        self.outgoing.push_back(ClientMessage::Request(request));
        Ok(id)
    }

    pub fn receive(&mut self, message: ServerMessage<A, E, P>, now_ms: u64) {
        if self.closed {
            return;
        }
        self.last_seen = now_ms;
        self.awaiting_response = false;
        match message {
            ServerMessage::Chunk { request_id, values } => {
                if self.pending.get(&request_id) == Some(&CallKind::Stream) && values.is_empty() {
                    self.fail_response(Some(request_id), Value::String("empty RPC chunk".into()));
                } else if self.pending.get(&request_id) == Some(&CallKind::Stream) {
                    self.events.push_back(CallEvent::Chunk {
                        id: request_id,
                        values,
                    });
                }
            }
            ServerMessage::Exit { request_id, exit } => {
                if matches!(&exit, Exit::Failure { cause } if cause.iter().any(|c| matches!(c, Cause::Unknown(_))))
                {
                    self.fail_response(Some(request_id), Value::String("invalid RPC cause".into()));
                    return;
                }
                if self.remove(&request_id) {
                    self.events.push_back(CallEvent::Exit {
                        id: request_id,
                        exit,
                    });
                }
            }
            ServerMessage::Defect { defect } => self.fail_pending(ClientError::Defect(defect)),
            ServerMessage::ClientProtocolError { error, request_ids } => {
                if let Some(ids) = request_ids {
                    for id in ids {
                        if self.remove(&id) {
                            self.events.push_back(CallEvent::Failed {
                                id,
                                error: ClientError::Protocol(error.clone()),
                            });
                        }
                    }
                } else {
                    self.fail_pending(ClientError::Protocol(error));
                }
            }
            ServerMessage::Request(request) => {
                self.events.push_back(CallEvent::ServerRequest(request))
            }
            ServerMessage::Unknown(value) => {
                if matches!(
                    value.get("_tag").and_then(Value::as_str),
                    Some("Exit" | "Chunk")
                ) {
                    let id = value
                        .get("requestId")
                        .cloned()
                        .and_then(|id| serde_json::from_value(id).ok());
                    self.fail_response(id, value);
                }
            }
            ServerMessage::Pong => {}
        }
    }

    fn fail_response(&mut self, id: Option<RequestId>, error: Value) {
        let Some(id) = id else {
            self.fail_pending(ClientError::Protocol(error));
            return;
        };
        if self.remove(&id) {
            self.outgoing.push_back(ClientMessage::Interrupt {
                request_id: id.clone(),
            });
            self.events.push_back(CallEvent::Failed {
                id,
                error: ClientError::Protocol(error),
            });
        }
    }

    /// ACK only after the caller has accepted a whole chunk into its bounded buffer.
    pub fn acknowledge(&mut self, id: RequestId) {
        if !self.closed && self.pending.contains_key(&id) {
            self.outgoing
                .push_back(ClientMessage::Ack { request_id: id });
        }
    }

    pub fn interrupt(&mut self, id: &RequestId) {
        if self.remove(id) {
            self.outgoing.push_back(ClientMessage::Interrupt {
                request_id: id.clone(),
            });
            self.events.push_back(CallEvent::Failed {
                id: id.clone(),
                error: ClientError::Interrupted,
            });
        }
    }

    pub fn eof(&mut self) {
        if !self.closed && !self.eof {
            self.eof = true;
            self.outgoing.push_back(ClientMessage::Eof);
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        if self.closed || now_ms < self.next_ping {
            return;
        }
        self.next_ping = now_ms.saturating_add(self.options.ping_interval_ms);
        if self.awaiting_response
            && now_ms.saturating_sub(self.last_seen) > self.options.ping_timeout_ms
        {
            self.disconnect(ClientError::PingTimeout);
        } else {
            self.awaiting_response = true;
            self.outgoing.push_back(ClientMessage::Ping);
        }
    }

    pub fn disconnect(&mut self, reason: ClientError) {
        self.closed = true;
        self.outgoing.clear();
        self.fail_pending(reason);
    }

    fn remove(&mut self, id: &RequestId) -> bool {
        self.pending.remove(id).is_some()
    }

    fn fail_pending(&mut self, error: ClientError) {
        while let Some((id, _)) = self.pending.pop_front() {
            self.events.push_back(CallEvent::Failed {
                id,
                error: error.clone(),
            });
        }
    }

    pub fn pop_outgoing(&mut self) -> Option<ClientMessage<P>> {
        self.outgoing.pop_front()
    }
    pub fn pop_event(&mut self) -> Option<CallEvent<A, E, P>> {
        self.events.pop_front()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl<P, A, E> Default for Client<P, A, E> {
    fn default() -> Self {
        Self::new(0, ClientOptions::default())
    }
}
