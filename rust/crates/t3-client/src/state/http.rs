use super::{
    decoding,
    sync::{
        HistoryQuery, HistoryView, ShellLoadResult, ShellSnapshotOptions, SnapshotSource,
        ThreadLoadResult,
    },
};
use crate::{
    ClientError,
    authorization::AuthorizationError,
    permissions::EnvironmentHttpClient,
    platform::{HttpResponse, Platform, PlatformError},
    types::ThreadId,
};
use futures::{FutureExt, future::LocalBoxFuture};
use std::rc::Rc;
use url::Url;

const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

/// Orchestration snapshots over HTTP, through the environment's authenticated client: every
/// request is authorized when it is sent, so a renewed DPoP token or relay base applies to the
/// next load and a rejected one is renewed once (`EnvironmentHttpClient::send`).
pub struct HttpSnapshotSource<P> {
    pub platform: Rc<P>,
    pub client: EnvironmentHttpClient,
}

fn snapshot_url(
    base: &str,
    thread: Option<&ThreadId>,
    suffix: &str,
    query: &[(&str, String)],
) -> Result<String, AuthorizationError> {
    let mut url = Url::parse(base).map_err(|_| AuthorizationError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AuthorizationError::InvalidUrl);
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_path("/api/orchestration");
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| AuthorizationError::InvalidUrl)?;
        if let Some(thread) = thread {
            path.push("threads").push(&thread.0);
        }
        path.push(suffix);
    }
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
    }
    Ok(url.into())
}

impl<P: Platform + 'static> HttpSnapshotSource<P> {
    async fn get(
        &self,
        thread: Option<&ThreadId>,
        suffix: &str,
        query: &[(&str, String)],
        timeout: u64,
    ) -> Result<HttpResponse, ClientError> {
        self.client
            .send(
                "GET",
                |base| snapshot_url(base, thread, suffix, query),
                timeout,
                MAX_SNAPSHOT_BYTES,
            )
            .await
            .map_err(|error| match error {
                AuthorizationError::CredentialRejected
                | AuthorizationError::CredentialMissing
                | AuthorizationError::SessionChanged => ClientError::PermissionDenied {
                    method: "snapshot HTTP".into(),
                },
                AuthorizationError::Timeout => {
                    PlatformError::Network("snapshot request timed out".into()).into()
                }
                AuthorizationError::InvalidUrl => {
                    ClientError::InvalidPayload("snapshot HTTP origin".into())
                }
                AuthorizationError::InvalidResponse => PlatformError::TooLarge.into(),
                _ => PlatformError::Network("snapshot request failed".into()).into(),
            })
    }

    pub async fn transcript(
        &self,
        thread: &ThreadId,
    ) -> Result<t3_contracts::orchestration_v2::OrchestrationV2ThreadTranscript, ClientError> {
        let response = self.get(Some(thread), "transcript", &[], 60_000).await?;
        if response.status != 200 {
            return Err(PlatformError::Network(format!(
                "thread transcript HTTP {}",
                response.status
            ))
            .into());
        }
        decoding::transcript(serde_json::from_slice(&response.body)?)
    }
}

impl<P: Platform + 'static> SnapshotSource for HttpSnapshotSource<P> {
    fn shell(
        &self,
        options: ShellSnapshotOptions,
    ) -> LocalBoxFuture<'_, Result<ShellLoadResult, ClientError>> {
        async move {
            let mut query = Vec::new();
            if options.omit_idle_subagents { query.push(("omitIdleSubagents", "1".to_owned())); }
            if options.omit_older_settled { query.push(("omitOlderSettled", "1".to_owned())); }
            if let Some(sequence) = options.resume_from { query.push(("resumeFrom", sequence.to_string())); }
            let response = self.get(None, "shell", &query, 20_000).await?;
            if response.status != 200 { return Ok(ShellLoadResult::Unavailable); }
            let raw: serde_json::Value = serde_json::from_slice(&response.body)?;
            if serde_json::from_value::<t3_contracts::orchestration_v2::OrchestrationV2ShellResumable>(raw.clone()).is_ok() { return Ok(ShellLoadResult::Resumable); }
            Ok(ShellLoadResult::Present(decoding::deferred_shell(raw, self.platform.clone())?))
        }.boxed_local()
    }

    fn thread(
        &self,
        thread: ThreadId,
    ) -> LocalBoxFuture<'_, Result<ThreadLoadResult, ClientError>> {
        async move {
            let query = [
                "compactTurnItems",
                "dedupeBodies",
                "compactIds",
                "deferWork",
            ]
            .map(|key| (key, "1".to_owned()));
            let response = match self.get(Some(&thread), "bounded", &query, 6_000).await {
                Ok(response) => response,
                Err(_) => return Ok(ThreadLoadResult::Unavailable),
            };
            if response.status == 404
                && serde_json::from_slice::<
                    t3_contracts::environment_http::EnvironmentResourceNotFoundError,
                >(&response.body)
                .is_ok()
            {
                return Ok(ThreadLoadResult::Missing);
            }
            if response.status != 200 {
                return Ok(ThreadLoadResult::Unavailable);
            }
            match serde_json::from_slice(&response.body)
                .map_err(ClientError::from)
                .and_then(decoding::bounded_snapshot)
            {
                Ok(snapshot)
                    if snapshot.projection.thread.id
                        == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(thread) =>
                {
                    Ok(ThreadLoadResult::Present(Box::new(snapshot)))
                }
                _ => Ok(ThreadLoadResult::Unavailable),
            }
        }
        .boxed_local()
    }

    fn history(
        &self,
        request: HistoryQuery,
    ) -> LocalBoxFuture<
        '_,
        Result<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage, ClientError>,
    > {
        async move {
            let mut query = vec![
                ("cursor", request.cursor.clone()),
                ("compactIds", "1".to_owned()),
            ];
            if let Some(through) = &request.through_entry {
                query.push(("throughEntryId", through.clone()));
            } else {
                query.push(("deferWork", "1".to_owned()));
            }
            if let Some(view) = request.view {
                query.push((
                    "view",
                    match view {
                        HistoryView::Conversation => "conversation",
                        HistoryView::Activity => "activity",
                        HistoryView::Work => "work",
                    }
                    .to_owned(),
                ));
            }
            let response = self
                .get(Some(&request.thread), "history", &query, 6_000)
                .await?;
            if response.status != 200 {
                return Err(PlatformError::Network(format!(
                    "thread history HTTP {}",
                    response.status
                ))
                .into());
            }
            decoding::history_page(serde_json::from_slice(&response.body)?)
        }
        .boxed_local()
    }
}
