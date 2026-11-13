use super::transport::{GitHubHttp, error, graphql_data};
use crate::{FailureReason, Result};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::{Mutex, oneshot};

type Reply = oneshot::Sender<Result<Value>>;
pub(super) type Batches = Arc<Mutex<HashMap<String, Vec<(Value, Reply)>>>>;
impl GitHubHttp {
    pub(super) async fn batched_graphql(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        variables: Value,
    ) -> Result<Value> {
        let credential = self.credential(host, std::path::Path::new(cwd)).await?;
        let key = format!(
            "{}\0{}\0{}\0{}",
            host.to_lowercase(),
            credential.fingerprint,
            operation,
            super::transport::is_interactive()
        );
        let (sender, receiver) = oneshot::channel();
        let mut batches = self.batches.lock().await;
        let pending = batches.entry(key.clone()).or_default();
        let start = pending.is_empty();
        pending.push((variables, sender));
        if start {
            let http = self.pinned(host.into(), credential);
            let host = host.to_string();
            let cwd = cwd.to_string();
            let operation = operation.to_string();
            let interactive = super::transport::is_interactive();
            tokio::spawn(async move {
                let run = async {
                    if matches!(
                        operation.as_str(),
                        "PullRequestSummary" | "PullRequestWatchFingerprint"
                    ) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    } else {
                        tokio::task::yield_now().await;
                    }
                    let pending = http.batches.lock().await.remove(&key).unwrap_or_default();
                    let mut chunks = Vec::new();
                    let mut chunk = Vec::new();
                    for item in pending {
                        chunk.push(item);
                        if chunk.len() == 25 {
                            chunks.push(std::mem::take(&mut chunk));
                        }
                    }
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    }
                    stream::iter(chunks)
                        .for_each_concurrent(4, |chunk| async {
                            let inputs: Vec<_> = chunk.iter().map(|(v, _)| v.clone()).collect();
                            let answers =
                                http.graphql_batch(&host, &cwd, &operation, &inputs).await;
                            for ((_, sender), answer) in chunk.into_iter().zip(answers) {
                                let _ = sender.send(answer);
                            }
                        })
                        .await;
                };
                if interactive {
                    super::transport::with_interactive(run).await;
                } else {
                    run.await;
                }
            });
        }
        drop(batches);
        receiver.await.map_err(|_| {
            error(
                operation,
                FailureReason::Failed,
                "GitHub batch was cancelled.",
            )
        })?
    }
    pub(super) async fn graphql_batch(
        &self,
        host: &str,
        cwd: &str,
        operation: &str,
        inputs: &[Value],
    ) -> Vec<Result<Value>> {
        let Some(document) = super::queries::document(operation) else {
            return inputs
                .iter()
                .map(|_| {
                    Err(error(
                        operation,
                        FailureReason::Failed,
                        "Unknown GraphQL batch operation.",
                    ))
                })
                .collect();
        };
        if inputs.len() == 1 {
            return vec![
                self.graphql_document(host, cwd, operation, document, inputs[0].clone())
                    .await,
            ];
        }
        let (document, variables) = aliased_document(operation, document, inputs);
        let response = self
            .graphql_answer(host, cwd, "T3Batch", &document, variables)
            .await;
        inputs.iter().enumerate().map(|(index,_)| {
            let value=response.as_ref().map_err(Clone::clone)?;
            let alias=format!("t3_{index}");
            let mut errors=Vec::new();
            for error in value["errors"].as_array().into_iter().flatten() {
                if error["path"].as_array().is_none_or(|p|p.is_empty()) || error["path"][0]==alias {
                    let mut error=error.clone();
                    if error["path"].is_array() {error["path"][0]=json!("repository");}
                    errors.push(error);
                }
            }
            graphql_data(operation,json!({"data":{"repository":value["data"][&alias]},"errors":errors,"__t3CredentialSource":value["__t3CredentialSource"]}))
        }).collect()
    }
}
fn aliased_document(operation: &str, document: &str, inputs: &[Value]) -> (String, Value) {
    let start = document.find('{').expect("static query selection");
    let definitions = &document[document.find('(').expect("static query variables") + 1
        ..document[..start]
            .rfind(')')
            .expect("static query variables")];
    let mut depth = 0;
    let mut end = start;
    for (offset, byte) in document.as_bytes()[start..].iter().enumerate() {
        if *byte == b'{' {
            depth += 1;
        }
        if *byte == b'}' {
            depth -= 1;
            if depth == 0 {
                end = start + offset;
                break;
            }
        }
    }
    let body = &document[start + 1..end];
    let tail = &document[end + 1..];
    static VARIABLE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\$([A-Za-z_]\w*)").expect("variable regex"));
    static FRAGMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\bfragment\s+([A-Za-z_]\w*)").expect("fragment regex")
    });
    let fragments: Vec<_> = FRAGMENT
        .captures_iter(tail)
        .map(|c| c[1].to_string())
        .collect();
    let (mut declarations, mut fields, mut tails, mut variables) =
        (Vec::new(), Vec::new(), Vec::new(), json!({}));
    for (index, input) in inputs.iter().enumerate() {
        let replace = |text: &str| {
            VARIABLE
                .replace_all(text, |c: &regex::Captures<'_>| {
                    format!("${}_{index}", &c[1])
                })
                .to_string()
        };
        declarations.push(replace(definitions));
        let mut body = replace(body).replacen("repository", &format!("t3_{index}: repository"), 1);
        let mut tail = tail.to_string();
        for name in &fragments {
            body = body.replace(&format!("...{name}"), &format!("...{name}_{index}"));
            tail = tail
                .replace(
                    &format!("fragment {name} "),
                    &format!("fragment {name}_{index} "),
                )
                .replace(&format!("...{name}"), &format!("...{name}_{index}"));
        }
        fields.push(body);
        tails.push(tail);
        if let Some(object) = input.as_object() {
            for (key, value) in object {
                variables[format!("{key}_{index}")] = value.clone();
            }
        }
    }
    let _ = operation;
    (
        format!(
            "query T3Batch({}) {{\n{}\n}}\n{}",
            declarations.join(","),
            fields.join("\n"),
            tails.join("\n")
        ),
        variables,
    )
}
