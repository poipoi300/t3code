//! Port of packages/client-runtime/src/secretRequest.ts and the web's `answerSecretRequest`
//! command (apps/web/src/state/server.ts, `secrets.answerRequest`).
//!
//! Reference answers (`SecretSourceSelect`, `secretRequestSources`) are behind the web build's
//! `T3CODE_FEATURE_SECRETS` flag, off by default, so the card offers no sources and they are not
//! ported here.

use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::Value;
use t3_contracts::base_schemas::{ThreadId, TurnItemId};
use t3_contracts::orchestration_v2::{
    OrchestrationV2SecretRequestDelivery, OrchestrationV2SecretRequestStatus,
    ProjectedTurnItemVisibility, TurnItemSecretRequest,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::rpc::WsSecretsAnswerRequestRpc;
use t3_contracts::secret_request::{SecretRequestAnswer, SecretRequestAnswerInput};

use crate::composition::ClientRuntime;
use crate::platform::Platform;
use crate::rpc::{RpcCallError, RpcEndpoint};
use crate::types::EnvironmentId;

pub const SECRET_REQUEST_DEFAULT_PLACEHOLDER: &str = "Paste the secret";
pub const SECRET_REQUEST_FAILURE: &str = "Could not answer the request. Try again.";

/// A `file` request lets the agent's commands read the value, so its card must not promise more.
pub fn secret_request_privacy_note(item: &TurnItemSecretRequest) -> &'static str {
    match item.delivery.clone().flatten() {
        Some(OrchestrationV2SecretRequestDelivery::File) => {
            "Kept out of chat, in a private file this agent's commands can read for 24 hours"
        }
        None => "Stored securely, never shown to the agent",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretRequestOutcome {
    Saved,
    Declined,
    Ended,
}

/// What the card shows. Labels are client English keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretRequestDisplay {
    Pending,
    PendingElsewhere {
        label: &'static str,
    },
    Answered {
        outcome: SecretRequestOutcome,
        label: &'static str,
    },
}

/// `visibility` is the projected row's: a request inherited from another thread (a fork) can
/// only be answered where it was asked.
pub fn secret_request_display(
    status: &OrchestrationV2SecretRequestStatus,
    visibility: &ProjectedTurnItemVisibility,
) -> SecretRequestDisplay {
    use OrchestrationV2SecretRequestStatus as S;
    match status {
        S::Pending if *visibility == ProjectedTurnItemVisibility::Local => {
            SecretRequestDisplay::Pending
        }
        S::Pending => SecretRequestDisplay::PendingElsewhere {
            label: "Waiting for an answer in the original thread",
        },
        S::Saved => SecretRequestDisplay::Answered {
            outcome: SecretRequestOutcome::Saved,
            label: "Saved securely and kept private",
        },
        S::Declined => SecretRequestDisplay::Answered {
            outcome: SecretRequestOutcome::Declined,
            label: "Declined",
        },
        S::Cancelled => SecretRequestDisplay::Answered {
            outcome: SecretRequestOutcome::Ended,
            label: "Request ended",
        },
    }
}

/// Port of contracts' `secretRequestPageUrl`: only http(s) with a host.
fn page_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty()))
        .then_some(url)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRequestLink {
    pub href: String,
    pub host: String,
}

/// Only http(s), so an agent cannot make the card open another scheme.
pub fn secret_request_link(url: Option<&str>, reason: &str) -> Option<SecretRequestLink> {
    let raw = match url {
        Some(url) => url.to_owned(),
        None => reason_link(reason)?,
    };
    let url = page_url(&raw)?;
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    };
    Some(SecretRequestLink {
        href: url.as_str().to_owned(),
        host,
    })
}

fn js_whitespace(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// `https?://` at `at`, ignoring case; the byte length of the scheme and `://`.
fn scheme_at(text: &str, at: usize) -> Option<usize> {
    let rest = text.get(at..)?;
    let lower = |n: usize| rest.get(..n).map(str::to_ascii_lowercase);
    if lower(8).as_deref() == Some("https://") {
        Some(8)
    } else if lower(7).as_deref() == Some("http://") {
        Some(7)
    } else {
        None
    }
}

/// The longest run from `start` of chars that `keep` accepts; at least one is required.
fn run_from(text: &str, start: usize, keep: impl Fn(char) -> bool) -> Option<usize> {
    let end = text[start..]
        .char_indices()
        .find(|(_, ch)| !keep(*ch))
        .map_or(text.len(), |(offset, _)| start + offset);
    (end > start).then_some(end)
}

/// One alternative of `REASON_LINK` at byte `at`: the matched range's end and the link.
enum Found {
    Exact(String),
    Bare(String),
}

fn match_at(text: &str, at: usize) -> Option<(usize, Found)> {
    let first = text[at..].chars().next()?;
    // `(https?://[^`\s]+)`
    if first == '`'
        && let Some(scheme) = scheme_at(text, at + 1)
        && let Some(end) = run_from(text, at + 1 + scheme, |ch| ch != '`' && !js_whitespace(ch))
        && text[end..].starts_with('`')
    {
        return Some((end + 1, Found::Exact(text[at + 1..end].to_owned())));
    }
    // <(https?://[^>\s]+)>
    if first == '<'
        && let Some(scheme) = scheme_at(text, at + 1)
        && let Some(end) = run_from(text, at + 1 + scheme, |ch| ch != '>' && !js_whitespace(ch))
        && text[end..].starts_with('>')
    {
        return Some((end + 1, Found::Exact(text[at + 1..end].to_owned())));
    }
    // \]\((https?://(?:[^()\s]|\([^()\s]*\))+)\)
    if text[at..].starts_with("](")
        && let Some(scheme) = scheme_at(text, at + 2)
    {
        let start = at + 2;
        let mut end = start + scheme;
        let mut units = 0;
        while let Some(ch) = text[end..].chars().next() {
            if ch == '(' {
                let close = text[end + 1..]
                    .char_indices()
                    .find(|(_, c)| matches!(c, '(' | ')') || js_whitespace(*c));
                match close {
                    Some((offset, ')')) => {
                        end += 1 + offset + 1;
                        units += 1;
                    }
                    _ => break,
                }
            } else if ch == ')' || js_whitespace(ch) {
                break;
            } else {
                end += ch.len_utf8();
                units += 1;
            }
        }
        if units > 0 && text[end..].starts_with(')') {
            return Some((end + 1, Found::Exact(text[start..end].to_owned())));
        }
    }
    // (https?:\/\/[^\s<>"'`[\]]+)
    if let Some(scheme) = scheme_at(text, at)
        && let Some(end) = run_from(text, at + scheme, |ch| {
            !js_whitespace(ch) && !matches!(ch, '<' | '>' | '"' | '\'' | '`' | '[' | ']')
        })
    {
        return Some((end, Found::Bare(text[at..end].to_owned())));
    }
    None
}

/// Drops trailing sentence punctuation; a `)` stays when the link opened a matching `(`.
fn trim_punctuation(link: &str) -> &str {
    let mut end = link.len();
    while let Some(last) = link[..end].chars().next_back() {
        if !".,;:!?)".contains(last) {
            break;
        }
        let kept = &link[..end];
        if last == ')' && kept.matches('(').count() >= kept.matches(')').count() {
            break;
        }
        end -= last.len_utf8();
    }
    &link[..end]
}

/// The trailing run of `*` or `_` (at most three), as `(\*{1,3}|_{1,3})$` finds it.
fn trailing_run(link: &str) -> Option<&str> {
    let last = link.chars().next_back().filter(|ch| matches!(ch, '*' | '_'))?;
    let count = link.chars().rev().take_while(|ch| *ch == last).count().min(3);
    Some(&link[link.len() - count..])
}

/// Drops a closing `*` or `_` run only when the same run opened emphasis before the link and is
/// still open.
fn trim_emphasis(link: &str, before: &str) -> String {
    let Some(run) = trailing_run(link) else {
        return link.to_owned();
    };
    // (?:^|[\s(*_])RUN(?=\S|$), last non-overlapping match.
    let followed = |at: usize| {
        before[at..]
            .chars()
            .next()
            .is_none_or(|ch| !js_whitespace(ch))
    };
    let mut opener_end = None;
    let mut at = 0;
    while at <= before.len() {
        let matched = if at == 0 && before.starts_with(run) && followed(run.len()) {
            Some(run.len())
        } else {
            before[at..].chars().next().and_then(|ch| {
                let after = at + ch.len_utf8();
                (matches!(ch, '(' | '*' | '_') || js_whitespace(ch))
                    .then_some(after)
                    .filter(|after| {
                        before[*after..].starts_with(run) && followed(after + run.len())
                    })
                    .map(|after| after + run.len())
            })
        };
        match matched {
            Some(end) => {
                opener_end = Some(end);
                at = end;
            }
            None => match before[at..].chars().next() {
                Some(ch) => at += ch.len_utf8(),
                None => break,
            },
        }
    }
    let Some(opener_end) = opener_end else {
        return link.to_owned();
    };
    // \S RUN (?:[\s.,;:!?)]|$) anywhere after the opener.
    let rest = &before[opener_end..];
    let closed = rest.char_indices().any(|(offset, ch)| {
        if js_whitespace(ch) {
            return false;
        }
        let after = offset + ch.len_utf8();
        rest[after..].starts_with(run)
            && rest[after + run.len()..]
                .chars()
                .next()
                .is_none_or(|next| js_whitespace(next) || ".,;:!?)".contains(next))
    });
    if closed {
        link.to_owned()
    } else {
        trim_punctuation(&link[..link.len() - run.len()]).to_owned()
    }
}

/// The one distinct http(s) link in `reason`, for agents that put it there instead of in `url`.
fn reason_link(reason: &str) -> Option<String> {
    let mut links: Vec<String> = Vec::new();
    let mut at = 0;
    while at < reason.len() {
        if let Some((end, found)) = match_at(reason, at) {
            let link = match found {
                Found::Exact(link) => link,
                Found::Bare(bare) => {
                    let before = &reason[..at];
                    let mut link = trim_punctuation(&bare).to_owned();
                    loop {
                        let next = trim_emphasis(&link, before);
                        if next == link {
                            break;
                        }
                        link = next;
                    }
                    link
                }
            };
            // Two spellings of one page (`https://a.dev` and `https://a.dev/`) count once.
            let key = page_url(&link).map_or(link, |url| url.as_str().to_owned());
            if !links.contains(&key) {
                links.push(key);
            }
            at = end.max(at + 1);
        } else {
            at += reason[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    (links.len() == 1).then(|| links.remove(0))
}

/// The answers this build's card offers (references are feature-flagged off; see the module doc).
#[derive(Clone, PartialEq, Eq)]
pub enum SecretRequestReply {
    Save { secret: String },
    Decline,
}

pub fn secret_request_answer_input(
    item: &TurnItemSecretRequest,
    reply: SecretRequestReply,
) -> Option<SecretRequestAnswerInput> {
    let (IdOrIndex::Id(turn_item_id), IdOrIndex::Id(thread_id)) = (&item.id, &item.thread_id)
    else {
        return None;
    };
    let answer = match reply {
        SecretRequestReply::Decline => SecretRequestAnswer::Decline,
        SecretRequestReply::Save { secret } => {
            let secret = t3_contracts::wire::trim_js(&secret);
            if secret.is_empty() {
                return None;
            }
            SecretRequestAnswer::Save {
                secret: secret.to_owned(),
            }
        }
    };
    Some(SecretRequestAnswerInput {
        thread_id: ThreadId(thread_id.0.clone()),
        turn_item_id: TurnItemId(turn_item_id.0.clone()),
        answer,
    })
}

/// contracts' `SecretRequestError.message`, by reason.
fn failure_reason_message(error: &Value) -> Option<String> {
    let reason = error.get("reason")?.as_str()?;
    if reason == "reference_unreadable"
        && let Some(provider) = error.get("provider").and_then(Value::as_str)
    {
        return Some(format!(
            "Could not read the secret from the secret provider \"{provider}\". Unlock it and try again, or enter the value."
        ));
    }
    Some(
        match reason {
            "load_failed" => "Could not load the secret request.",
            "not_found" => "This secret request no longer exists.",
            "already_answered" => "This secret request was already answered.",
            "agent_stopped" => "The agent that asked has stopped, so this secret can't be used.",
            "store_failed" => "Could not store the secret.",
            "reference_unreadable" => "Could not read the secret from the secret provider.",
            "record_failed" => "Saved the secret, but could not update the request.",
            "invalid_ref" => "That secretRef is not valid.",
            "read_failed" => "Could not read the secret.",
            "ref_unavailable" => {
                "That secretRef was already used or does not exist. Ask the user again with request_secret."
            }
            "ref_expired" => "That secretRef expired. Ask the user again with request_secret.",
            "consume_failed" => "Could not use that secretRef. Try again.",
            "not_a_file_request" => {
                "The user did not save this secret for shell use. Ask again with request_secret and delivery: \"file\"."
            }
            _ => return None,
        }
        .to_owned(),
    )
}

/// Only known server errors pass their message through; anything else (transport or encoding
/// failures) gets the generic copy, so the typed value can never surface in the UI.
pub fn secret_request_failure_message(error: &RpcCallError) -> String {
    let RpcCallError::Rejected(errors) = error else {
        return SECRET_REQUEST_FAILURE.to_owned();
    };
    errors
        .first()
        .and_then(|error| match error.get("_tag").and_then(Value::as_str) {
            Some("SecretRequestError") => failure_reason_message(error),
            Some("EnvironmentAuthorizationError") => error
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .map(str::to_owned),
            _ => None,
        })
        .unwrap_or_else(|| SECRET_REQUEST_FAILURE.to_owned())
}

/// Why an answer did not land. `Interrupted` shows nothing, as `isAtomCommandInterrupted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretAnswerError {
    Interrupted,
    Failed(String),
}

/// `serverEnvironment.answerSecretRequest`, over the environment's live session.
#[derive(Clone)]
pub struct SecretRequests<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
}

impl<P: Platform + Clone + 'static> SecretRequests<P> {
    pub fn new(runtime: ClientRuntime<P>) -> Self {
        Self { runtime }
    }

    pub fn answer(
        &self,
        environment: EnvironmentId,
        input: SecretRequestAnswerInput,
    ) -> LocalBoxFuture<'static, Result<(), SecretAnswerError>> {
        let session = self
            .runtime
            .client(&environment)
            .and_then(|client| client.session());
        async move {
            let Some(session) = session else {
                return Err(SecretAnswerError::Failed(SECRET_REQUEST_FAILURE.to_owned()));
            };
            match session.command::<WsSecretsAnswerRequestRpc>(input).await {
                Ok(()) => Ok(()),
                Err(RpcCallError::Interrupted) => Err(SecretAnswerError::Interrupted),
                Err(error) => Err(SecretAnswerError::Failed(secret_request_failure_message(
                    &error,
                ))),
            }
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_offers_the_form_for_a_request_inherited_from_another_thread() {
        use OrchestrationV2SecretRequestStatus as S;
        assert_eq!(
            secret_request_display(&S::Pending, &ProjectedTurnItemVisibility::Inherited),
            SecretRequestDisplay::PendingElsewhere {
                label: "Waiting for an answer in the original thread"
            }
        );
        assert!(matches!(
            secret_request_display(&S::Saved, &ProjectedTurnItemVisibility::Inherited),
            SecretRequestDisplay::Answered {
                outcome: SecretRequestOutcome::Saved,
                ..
            }
        ));
        assert_eq!(
            secret_request_display(&S::Pending, &ProjectedTurnItemVisibility::Local),
            SecretRequestDisplay::Pending
        );
    }

    #[test]
    fn finds_the_one_link_in_a_reason() {
        for (reason, href) in [
            (
                "Open this link and sign in: https://a.dev/x?s=1. Then paste the code.",
                Some("https://a.dev/x?s=1"),
            ),
            (
                "Open [https://a.dev/x?s=1](https://a.dev/x?s=1) and sign in.",
                Some("https://a.dev/x?s=1"),
            ),
            (
                "Sign in [here](https://a.dev/login?state=abc(def)).",
                Some("https://a.dev/login?state=abc(def)"),
            ),
            (
                "Sign in at https://a.dev/login?state=abc(def).",
                Some("https://a.dev/login?state=abc(def)"),
            ),
            ("(Sign in at https://a.dev/login.)", Some("https://a.dev/login")),
            ("Open **https://a.dev/x?s=1** and sign in.", Some("https://a.dev/x?s=1")),
            (
                "Open https://a.dev/x?state=u7o_ and sign in.",
                Some("https://a.dev/x?state=u7o_"),
            ),
            ("**Sign in at https://a.dev/device**", Some("https://a.dev/device")),
            ("_Open https://a.dev/device_.", Some("https://a.dev/device")),
            (
                "_https://a.dev/x?state=u7o_&client_id=cli_",
                Some("https://a.dev/x?state=u7o_&client_id=cli"),
            ),
            (
                "Use client_id acme at https://a.dev/x?state=u7o_",
                Some("https://a.dev/x?state=u7o_"),
            ),
            ("_Sign in_ at https://a.dev/x?state=u7o_", Some("https://a.dev/x?state=u7o_")),
            (
                "_Required_: Open https://a.dev/x?state=u7o_",
                Some("https://a.dev/x?state=u7o_"),
            ),
            ("(**https://a.dev/login**)", Some("https://a.dev/login")),
            ("Open ***https://a.dev/device***", Some("https://a.dev/device")),
            ("Open **_https://a.dev/device_**", Some("https://a.dev/device")),
            (
                "Open [https://a.dev/x?s=u7o_](https://a.dev/x?s=u7o_) and sign in.",
                Some("https://a.dev/x?s=u7o_"),
            ),
            ("Open `https://a.dev/x?s=1.` and sign in.", Some("https://a.dev/x?s=1.")),
            (
                "Open https://a.dev/x?s=1, then if needed https://a.dev/x?s=1 again.",
                Some("https://a.dev/x?s=1"),
            ),
            ("Open https://a.dev or https://b.dev.", None),
            ("Open https://a.dev (also written https://a.dev/).", Some("https://a.dev/")),
            ("Paste the code the CLI printed.", None),
        ] {
            assert_eq!(
                secret_request_link(None, reason).map(|link| link.href),
                href.map(str::to_owned),
                "{reason}"
            );
        }
    }

    #[test]
    fn prefers_the_agents_url_over_a_link_in_the_reason() {
        assert_eq!(
            secret_request_link(Some("https://auth.acme.dev/a"), "See https://docs.acme.dev")
                .map(|link| link.host),
            Some("auth.acme.dev".to_owned())
        );
        assert_eq!(secret_request_link(Some("javascript:alert(1)"), ""), None);
    }

    #[test]
    fn passes_through_server_errors_but_hides_anything_that_could_echo_the_payload() {
        let tagged = RpcCallError::Rejected(vec![serde_json::json!({
            "_tag": "SecretRequestError", "reason": "already_answered"
        })]);
        assert_eq!(
            secret_request_failure_message(&tagged),
            "This secret request was already answered."
        );
        let other = RpcCallError::Rejected(vec![serde_json::json!({
            "_tag": "SchemaError", "message": "Expected string, got \"whsec_1\""
        })]);
        assert_eq!(secret_request_failure_message(&other), SECRET_REQUEST_FAILURE);
        assert_eq!(
            secret_request_failure_message(&RpcCallError::Unknown),
            SECRET_REQUEST_FAILURE
        );
    }

    fn item(delivery: Option<&str>) -> TurnItemSecretRequest {
        let mut value = serde_json::json!({
            "id":"item-1","threadId":"thread-1","type":"secret_request","runId":null,"nodeId":null,
            "providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,
            "ordinal":0,"status":"waiting","title":null,"startedAt":null,"completedAt":null,
            "updatedAt":"2026-11-14T00:00:00.000Z","label":"API key","reason":"Needed",
            "secretStatus":"pending"
        });
        if let Some(delivery) = delivery {
            value["delivery"] = serde_json::json!(delivery);
        }
        serde_json::from_value(value).expect("secret request item")
    }

    #[test]
    fn does_not_promise_to_hide_a_value_that_the_agents_commands_can_read() {
        assert_eq!(
            secret_request_privacy_note(&item(None)),
            "Stored securely, never shown to the agent"
        );
        assert!(!secret_request_privacy_note(&item(Some("file"))).contains("never shown"));
    }

    #[test]
    fn answers_trim_the_secret_and_refuse_an_empty_one() {
        let item = item(None);
        let input = secret_request_answer_input(
            &item,
            SecretRequestReply::Save {
                secret: "  sk-1  ".into(),
            },
        )
        .expect("save");
        assert_eq!(
            input.answer,
            SecretRequestAnswer::Save {
                secret: "sk-1".into()
            }
        );
        assert_eq!(input.turn_item_id.0, "item-1");
        assert!(
            secret_request_answer_input(&item, SecretRequestReply::Save { secret: " ".into() })
                .is_none()
        );
        assert_eq!(
            secret_request_answer_input(&item, SecretRequestReply::Decline)
                .map(|input| input.answer),
            Some(SecretRequestAnswer::Decline)
        );
    }
}
