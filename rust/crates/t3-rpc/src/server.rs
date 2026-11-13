use hashlink::LinkedHashMap;
use std::collections::{HashMap, VecDeque};

use serde::Serialize;
use serde_json::Value;

use crate::{CallKind, ClientMessage, Exit, Request, RequestId, ServerMessage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPolicy {
    Single,
    Count8,
    Terminal,
}

impl WindowPolicy {
    pub fn for_method(tag: &str) -> Self {
        match tag {
            "terminal.attach" | "terminal.observe" => Self::Terminal,
            "subscribeServerConfig"
            | "orchestration.subscribeShell"
            | "orchestration.subscribeThread" => Self::Count8,
            _ => Self::Single,
        }
    }
}

#[derive(Debug)]
pub enum ServerAction<P = Value> {
    Dispatch(Request<P>),
    Cancel(RequestId),
    Ended,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServerError {
    #[error("request is no longer active")]
    Inactive,
    #[error("stream is waiting for an acknowledgement")]
    Backpressure,
    #[error("a stream chunk must be nonempty")]
    EmptyChunk,
    #[error("request is not a stream")]
    NotStream,
    #[error("terminal chunk could not be encoded: {0}")]
    Encoding(String),
}

struct Active {
    dispatch_action: u64,
    kind: CallKind,
    policy: WindowPolicy,
    sizes: VecDeque<usize>,
    waiting: bool,
    notification: bool,
    discard: bool,
    acknowledged: bool,
}

impl Active {
    fn full(&self) -> bool {
        self.sizes.len() >= 8 || self.sizes.iter().sum::<usize>() >= 64 * 1024
    }

    fn silent(&self) -> bool {
        self.notification || self.acknowledged
    }
}

/// One connection. Requests run concurrently without a global concurrency ceiling.
pub struct Server<P = Value, A = Value, E = Value> {
    #[cfg(test)]
    pub(crate) poll_checks: std::cell::Cell<usize>,
    methods: HashMap<String, (CallKind, WindowPolicy)>,
    active: LinkedHashMap<RequestId, Active>,
    outgoing: VecDeque<ServerMessage<A, E, P>>,
    actions: LinkedHashMap<u64, ServerAction<P>>,
    next_action: u64,
    notification_count: u64,
    upgrade_headers: Vec<(String, String)>,
    eof: bool,
    closed: bool,
    ended: bool,
}

impl<P, A, E> Server<P, A, E> {
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            poll_checks: std::cell::Cell::new(0),
            methods: HashMap::new(),
            active: LinkedHashMap::new(),
            outgoing: VecDeque::new(),
            actions: LinkedHashMap::new(),
            next_action: 0,
            notification_count: 0,
            upgrade_headers: Vec::new(),
            eof: false,
            closed: false,
            ended: false,
        }
    }

    pub fn register(&mut self, tag: impl Into<String>, kind: CallKind) {
        let tag = tag.into();
        let policy = WindowPolicy::for_method(&tag);
        self.register_with_window(tag, kind, policy);
    }

    pub fn set_upgrade_headers(&mut self, headers: Vec<(String, String)>) {
        self.upgrade_headers = headers;
    }

    pub fn register_with_window(
        &mut self,
        tag: impl Into<String>,
        kind: CallKind,
        policy: WindowPolicy,
    ) {
        self.methods.insert(tag.into(), (kind, policy));
    }

    pub fn receive(&mut self, message: ClientMessage<P>) {
        self.receive_with_interruptor(message, None);
    }

    /// Fiber ids identify the local interrupting execution context, not the remote caller.
    pub fn receive_with_interruptor(&mut self, message: ClientMessage<P>, fiber_id: Option<i64>) {
        if self.closed {
            return;
        }
        match message {
            ClientMessage::Request(mut request) => {
                if !self.upgrade_headers.is_empty() {
                    let mut headers = self.upgrade_headers.clone();
                    headers.append(&mut request.headers);
                    request.headers = headers;
                }
                let notification = request.is_notification.is_some();
                if notification {
                    request.id = RequestId::String(format!(
                        "@effect/rpc/notification/{}",
                        self.notification_count
                    ));
                    self.notification_count += 1;
                }
                if self.active.contains_key(&request.id) {
                    return;
                }
                let Some(&(kind, policy)) = self.methods.get(&request.tag) else {
                    if !notification {
                        self.outgoing.push_back(ServerMessage::Exit {
                            request_id: request.id,
                            exit: Exit::die(format!("Unknown request tag: {}", request.tag)),
                        });
                    }
                    return;
                };
                if self.eof {
                    return;
                }
                self.active.insert(
                    request.id.clone(),
                    Active {
                        dispatch_action: self.next_action,
                        kind,
                        policy,
                        sizes: VecDeque::new(),
                        waiting: false,
                        notification,
                        discard: !notification && request.discard.is_some(),
                        acknowledged: false,
                    },
                );
                self.push_action(ServerAction::Dispatch(request));
            }
            ClientMessage::Ack { request_id } => {
                if let Some(active) = self.active.get_mut(&request_id) {
                    if active.policy == WindowPolicy::Single {
                        active.waiting = false;
                    } else {
                        let was_full = active.full();
                        active.sizes.pop_front();
                        if was_full && !active.full() {
                            active.waiting = false;
                        }
                    }
                }
            }
            ClientMessage::Interrupt { request_id } => {
                let Some(active) = self.active.remove(&request_id) else {
                    return;
                };
                let silent = active.silent();
                self.actions.remove(&active.dispatch_action);
                self.push_action(ServerAction::Cancel(request_id.clone()));
                if !silent {
                    self.outgoing.push_back(ServerMessage::Exit {
                        request_id,
                        exit: Exit::interrupt(fiber_id),
                    });
                }
                self.check_ended();
            }
            ClientMessage::Ping => self.outgoing.push_back(ServerMessage::Pong),
            ClientMessage::Eof => {
                self.eof = true;
                self.check_ended();
            }
            ClientMessage::Unknown(value) => {
                if value.get("_tag").and_then(Value::as_str) == Some("Request") {
                    if value.get("isNotification") == Some(&Value::Bool(true)) {
                        return;
                    }
                    if let Some(id) = value
                        .get("id")
                        .cloned()
                        .and_then(|id| serde_json::from_value(id).ok())
                    {
                        let tag = value.get("tag").and_then(Value::as_str).unwrap_or("");
                        let reason = if !self.methods.contains_key(tag) {
                            format!("Unknown request tag: {tag}")
                        } else {
                            "Invalid request payload or headers".into()
                        };
                        self.outgoing.push_back(ServerMessage::Exit {
                            request_id: id,
                            exit: Exit::die(reason),
                        });
                    } else {
                        self.outgoing.push_back(ServerMessage::Defect {
                            defect: Value::String(format!(
                                "Invalid request id: {}",
                                value.get("id").map_or("undefined".into(), Value::to_string)
                            )),
                        });
                    }
                    return;
                }
                let tag = value
                    .get("_tag")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined");
                self.outgoing.push_back(ServerMessage::Defect {
                    defect: Value::String(format!("Unknown request tag: {tag}")),
                });
            }
        }
    }

    /// Call after construction succeeds and middleware has accepted the request.
    pub fn started(&mut self, id: &RequestId) -> Result<(), ServerError> {
        let active = self.active.get_mut(id).ok_or(ServerError::Inactive)?;
        if active.discard && !active.acknowledged {
            active.acknowledged = true;
            self.outgoing.push_back(ServerMessage::Exit {
                request_id: id.clone(),
                exit: Exit::void(),
            });
        }
        Ok(())
    }

    pub fn can_poll(&self, id: &RequestId) -> bool {
        #[cfg(test)]
        self.poll_checks.set(self.poll_checks.get() + 1);
        self.active.get(id).is_some_and(|active| !active.waiting)
    }

    pub fn send_chunk(&mut self, id: &RequestId, values: Vec<A>) -> Result<(), ServerError>
    where
        A: Serialize,
    {
        if values.is_empty() {
            return Err(ServerError::EmptyChunk);
        }
        let active = self.active.get_mut(id).ok_or(ServerError::Inactive)?;
        if active.kind != CallKind::Stream {
            return Err(ServerError::NotStream);
        }
        if active.waiting {
            return Err(ServerError::Backpressure);
        }
        if active.silent() {
            return Ok(());
        }
        match active.policy {
            WindowPolicy::Single => active.waiting = true,
            WindowPolicy::Count8 => {
                active.sizes.push_back(0);
                active.waiting = active.full();
            }
            WindowPolicy::Terminal => {
                let wire = serde_json::to_value(&values)
                    .map_err(|e| ServerError::Encoding(e.to_string()))?;
                let bytes = 128
                    + wire
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(terminal_event_size)
                        .sum::<usize>();
                active.sizes.push_back(bytes);
                active.waiting = active.full();
            }
        }
        self.outgoing.push_back(ServerMessage::Chunk {
            request_id: id.clone(),
            values,
        });
        Ok(())
    }

    pub fn finish(&mut self, id: &RequestId, exit: Exit<A, E>) -> Result<(), ServerError> {
        let active = self.active.get(id).ok_or(ServerError::Inactive)?;
        if active.waiting {
            return Err(ServerError::Backpressure);
        }
        if !active.silent() {
            self.outgoing.push_back(ServerMessage::Exit {
                request_id: id.clone(),
                exit,
            });
        }
        self.active.remove(id);
        self.check_ended();
        Ok(())
    }

    /// A handler or encoder defect terminates the call even while its stream awaits an ACK.
    pub fn fail_defect(
        &mut self,
        id: &RequestId,
        defect: impl Into<Value>,
    ) -> Result<(), ServerError> {
        self.active
            .get_mut(id)
            .ok_or(ServerError::Inactive)?
            .waiting = false;
        self.finish(id, Exit::die(defect))
    }

    fn check_ended(&mut self) {
        if self.eof && self.active.is_empty() && !self.ended {
            self.ended = self.closed;
            self.eof = self.closed;
            self.push_action(ServerAction::Ended);
        }
    }

    pub fn disconnect(&mut self) {
        self.closed = true;
        self.outgoing.clear();
        self.actions.clear();
        while let Some((id, _)) = self.active.pop_front() {
            self.push_action(ServerAction::Cancel(id));
        }
        self.eof = true;
        self.check_ended();
    }

    pub fn pop_outgoing(&mut self) -> Option<ServerMessage<A, E, P>> {
        self.outgoing.pop_front()
    }
    pub fn pop_action(&mut self) -> Option<ServerAction<P>> {
        self.actions.pop_front().map(|(_, action)| action)
    }

    fn push_action(&mut self, action: ServerAction<P>) {
        self.actions.insert(self.next_action, action);
        self.next_action = self.next_action.wrapping_add(1);
    }
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl<P, A, E> Default for Server<P, A, E> {
    fn default() -> Self {
        Self::new()
    }
}

fn text_size(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_str).map_or(0, str::len)
}

fn terminal_event_size(event: &Value) -> usize {
    let ids = if event.get("threadId").is_some() {
        text_size(event, "threadId") + text_size(event, "terminalId")
    } else {
        0
    };
    let variable = match event.get("type").and_then(Value::as_str) {
        Some("output") => text_size(event, "data"),
        Some("error") => text_size(event, "message"),
        Some("activity") => text_size(event, "label"),
        _ => event.get("snapshot").map_or(0, |snapshot| {
            [
                "threadId",
                "terminalId",
                "history",
                "cwd",
                "worktreePath",
                "label",
            ]
            .into_iter()
            .map(|key| text_size(snapshot, key))
            .sum()
        }),
    };
    128 + ids + variable
}
