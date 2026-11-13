use super::{GitHub, repository::encode, transport::now};
use crate::{
    ForgeError,
    forge::{ChangeRequestEvents, PullRequestInput},
};
use futures_util::{Stream, StreamExt};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use t3_process::CancellationToken;
use tokio::sync::mpsc;

const EVENT_TYPES: &[&str] = &[
    "com.github.pull_request.review.submitted.v1",
    "com.github.pull_request.head.changed.v1",
    "com.github.pull_request.state.changed.v1",
    "com.github.pull_request.merge_conflict.v1",
    "com.github.pull_request.check_suite.status.changed.v1",
];
struct Events {
    receiver: mpsc::Receiver<()>,
    cancel: CancellationToken,
}
impl Stream for Events {
    type Item = ();
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
        self.receiver.poll_recv(cx)
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Reconnect,
    Renew,
    Personal,
    Stop,
}

#[derive(Debug, PartialEq, Eq)]
enum Frame {
    Changed,
    End(Ending),
}
async fn send_change(sender: &mpsc::Sender<()>, cancel: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        sent = sender.send(()) => sent.is_ok(),
    }
}
#[derive(Default)]
struct Frames {
    bytes: Vec<u8>,
    event: String,
    data: Vec<String>,
    data_bytes: usize,
    started: bool,
}
impl Frames {
    fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.bytes.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(end) = self
            .bytes
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
        {
            if self.bytes[end] == b'\r' && end + 1 == self.bytes.len() {
                break;
            }
            let width = if self.bytes[end] == b'\r' && self.bytes.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            let line = String::from_utf8_lossy(&self.bytes[..end]).to_string();
            let line = if !self.started {
                line.trim_start_matches('\u{feff}')
            } else {
                &line
            };
            self.started = true;
            if line.is_empty() {
                if !self.data.is_empty() {
                    frames.push(if self.event == "github.stream.end" {
                        Frame::End(
                            if serde_json::from_str::<Value>(&self.data.join("\n"))
                                .ok()
                                .is_some_and(|v| v["outcome"] == "subscription_ended")
                            {
                                Ending::Renew
                            } else {
                                Ending::Reconnect
                            },
                        )
                    } else {
                        Frame::Changed
                    });
                }
                self.event.clear();
                self.data.clear();
                self.data_bytes = 0;
            } else if !line.starts_with(':') {
                let (name, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match name {
                    "event" => self.event = value.into(),
                    "data" => {
                        self.data_bytes += value.len();
                        self.data.push(value.into());
                    }
                    _ => {}
                }
            }
            self.bytes.drain(..end + width);
        }
        frames
    }
    fn size(&self) -> usize {
        self.bytes.len() + self.data_bytes + self.event.len()
    }
}
impl GitHub {
    pub(super) fn events(&self, input: PullRequestInput) -> ChangeRequestEvents {
        let activity = self.event_tasks.token();
        if self.event_stop.is_cancelled() {
            return Box::pin(futures_util::stream::empty());
        }
        let (sender, receiver) = mpsc::channel(16);
        let cancel = self.event_stop.child_token();
        let this = self.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            let _activity = activity;
            this.run_events(input, sender, token).await;
        });
        Box::pin(Events { receiver, cancel })
    }
    async fn event_connection(
        &self,
        input: &PullRequestInput,
        url: &url::Url,
        personal: bool,
        sender: &mpsc::Sender<()>,
        cancel: &CancellationToken,
    ) -> (Ending, usize) {
        let credential = if personal {
            self.http
                .personal_credential(&input.repo.host, std::path::Path::new(&input.repo.cwd))
                .await
        } else {
            self.http
                .credential(&input.repo.host, std::path::Path::new(&input.repo.cwd))
                .await
        };
        let Ok(credential) = credential else {
            return (Ending::Reconnect, 0);
        };
        let send = self
            .http
            .client()
            .get(self.http.fixture_url(url))
            .header("accept", "text/event-stream")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "t3code")
            .bearer_auth(&credential.token)
            .send();
        let response = tokio::select! { _ = cancel.cancelled() => return (Ending::Stop,0), response = tokio::time::timeout(Duration::from_secs(45),send) => response };
        let Ok(Ok(response)) = response else {
            return (Ending::Reconnect, 0);
        };
        if matches!(response.status().as_u16(), 403 | 404) {
            return (
                if credential.source == "app" && !personal {
                    Ending::Personal
                } else {
                    Ending::Stop
                },
                0,
            );
        }
        if response.status().as_u16() != 200 {
            return (Ending::Reconnect, 0);
        }
        if !send_change(sender, cancel).await {
            return (Ending::Stop, 0);
        }
        let mut changes = 1;
        let mut chunks = response.bytes_stream();
        let mut frames = Frames::default();
        loop {
            let chunk = tokio::select! { _ = cancel.cancelled() => return (Ending::Stop,changes), chunk = tokio::time::timeout(Duration::from_secs(45),chunks.next()) => chunk };
            let Ok(Some(Ok(chunk))) = chunk else {
                return (Ending::Reconnect, changes);
            };
            for frame in frames.push(&chunk) {
                match frame {
                    Frame::Changed => {
                        if !send_change(sender, cancel).await {
                            return (Ending::Stop, changes);
                        }
                        changes += 1;
                    }
                    Frame::End(ending) => return (ending, changes),
                }
            }
            if frames.size() > 1024 * 1024 {
                return (Ending::Reconnect, changes);
            }
        }
    }
    async fn run_events(
        &self,
        mut input: PullRequestInput,
        sender: mpsc::Sender<()>,
        cancel: CancellationToken,
    ) {
        input.repo.host = input.repo.host.trim().to_lowercase();
        if input.repo.host != "github.com" {
            return;
        }
        let Some(app) = &self.app_sign_in else {
            return;
        };
        if !matches!(app.token(&input.repo.host).await, Ok(Some(_))) {
            return;
        }
        if super::adapter::repo_parts(&input.repo.repository).is_err() || input.number == 0 {
            return;
        }
        let resource = format!(
            "https://{}/{}/pull/{}",
            input.repo.host, input.repo.repository, input.number
        );
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let pauses = self.event_pauses.lock().await;
            if pauses
                .get(&input.repo.host)
                .into_iter()
                .chain(pauses.get(&resource))
                .any(|at| *at > now())
            {
                return;
            }
            drop(pauses);
            let subscribe = self.rest_json(
                &input.repo.host,
                &input.repo.cwd,
                "subscribePullRequestEvents",
                Method::POST,
                "event-subscriptions",
                Some(json!({"resource_url":resource,"event_types":EVENT_TYPES})),
            );
            let response =
                tokio::select! { _ = cancel.cancelled() => return, result = subscribe => result };
            let subscription = match response {
                Ok(value) => value,
                Err(ForgeError::Provider {
                    status: Some(status),
                    ..
                }) => {
                    let pause = match status {
                        404 => Some((resource.clone(), 3_600_000)),
                        403 => Some((input.repo.host.clone(), 3_600_000)),
                        422 => Some((input.repo.host.clone(), 600_000)),
                        _ => None,
                    };
                    if let Some((key, duration)) = pause {
                        self.event_pauses
                            .lock()
                            .await
                            .insert(key, now().saturating_add(duration));
                    }
                    return;
                }
                Err(_) => return,
            };
            let id = match &subscription["id"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return,
            };
            let url = subscription["events_url"]
                .as_str()
                .and_then(|s| url::Url::parse(s).ok())
                .filter(|url| {
                    url.origin().ascii_serialization() == "https://api.github.com"
                        && url.username().is_empty()
                        && url.password().is_none()
                });
            let Some(url) = url else {
                return;
            };
            let (mut personal, mut failures, mut renew) = (false, 0u32, false);
            loop {
                let opened = tokio::time::Instant::now();
                let (ending, changes) = self
                    .event_connection(&input, &url, personal, &sender, &cancel)
                    .await;
                failures = if changes > 1 || opened.elapsed() >= Duration::from_secs(120) {
                    0
                } else {
                    failures + 1
                };
                match ending {
                    Ending::Stop => break,
                    Ending::Renew => {
                        renew = true;
                        break;
                    }
                    Ending::Personal if !personal => {
                        personal = true;
                        continue;
                    }
                    Ending::Personal => break,
                    Ending::Reconnect if failures > 6 => break,
                    Ending::Reconnect => {
                        if !backoff(failures, &cancel).await {
                            break;
                        }
                    }
                }
            }
            let _ = self
                .http
                .rest(
                    &input.repo.host,
                    &input.repo.cwd,
                    "unsubscribePullRequestEvents",
                    Method::DELETE,
                    &format!("event-subscriptions/{}", encode(&id)),
                    None,
                )
                .await;
            if !renew || !backoff(0, &cancel).await {
                return;
            }
        }
    }
}
async fn backoff(failures: u32, cancel: &CancellationToken) -> bool {
    let max = 1000u64.saturating_mul(1u64 << failures.min(16)).min(60_000);
    let jitter = now() % 501;
    let delay = Duration::from_millis(max / 2 + max * jitter / 1000);
    tokio::select! { _ = cancel.cancelled() => false, _ = tokio::time::sleep(delay) => true }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shutdown_releases_a_full_change_queue() {
        let (sender, mut receiver) = mpsc::channel(1);
        sender.send(()).await.unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(!send_change(&sender, &cancel).await);
        assert_eq!(receiver.recv().await, Some(()));
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn sse_decodes_fragmented_utf8_mixed_newlines_and_stream_end() {
        let recording = "\u{feff}event: com.github.pull_request.head.changed.v1\r\ndata: {\"title\":\"é\"}\r\n\r\n: heartbeat\n\nevent: github.stream.end\ndata: {\"outcome\":\"subscription_ended\",\"reason\":\"expired\"}\n\n";
        let mut decoder = Frames::default();
        let mut frames = Vec::new();
        for byte in recording.as_bytes() {
            frames.extend(decoder.push(&[*byte]));
        }
        assert_eq!(frames, vec![Frame::Changed, Frame::End(Ending::Renew)]);
        assert_eq!(decoder.size(), 0);
    }
}
