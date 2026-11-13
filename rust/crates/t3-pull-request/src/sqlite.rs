use crate::{
    Error, Result,
    linked::{Records, ViewedMark, ViewedScope},
};
use async_trait::async_trait;
use t3_contracts::{
    pull_request::PullRequestLinkedThreadsResult,
    pull_request_review_guide::PullRequestReviewGuide, thread_pull_request::ThreadPullRequestKey,
};
use t3_persistence::{
    Database,
    rusqlite::{OptionalExtension, params},
};

pub struct SqliteRecords {
    database: Database,
}
impl SqliteRecords {
    pub fn new(database: Database) -> Self {
        Self { database }
    }
}
fn database_error(operation: &str) -> Error {
    Error::invalid(
        operation,
        "This environment could not read or write its pull request records.",
    )
}
#[async_trait]
impl Records for SqliteRecords {
    async fn viewed(&self, scope: &ViewedScope) -> Result<(Vec<ViewedMark>, bool)> {
        let number = i64::try_from(scope.number).map_err(|_| database_error("pullRequests"))?;
        let scope = scope.clone();
        let mut rows=self.database.reader(move|connection|{
            let mut query=connection.prepare("SELECT path, revision FROM pull_request_files_viewed WHERE provider=?1 AND host=?2 AND repository=?3 AND number=?4 AND viewer=?5 ORDER BY path LIMIT 501")?;
            Ok(query.query_map(params![scope.provider,scope.host,scope.repository,number,scope.viewer],|r|Ok(ViewedMark{path:r.get(0)?,revision:r.get(1)?}))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).await.map_err(|_|database_error("filesViewed"))?;
        let truncated = rows.len() > 500;
        rows.truncate(500);
        Ok((rows, truncated))
    }
    async fn set_viewed(
        &self,
        scope: &ViewedScope,
        marks: Vec<(ViewedMark, bool)>,
        viewed_at: String,
    ) -> Result<()> {
        let number = i64::try_from(scope.number).map_err(|_| database_error("pullRequests"))?;
        let scope = scope.clone();
        self.database.writer(move|connection|{let transaction=connection.transaction()?;for (mark,viewed) in marks{
            if viewed{transaction.execute("INSERT INTO pull_request_files_viewed (provider,host,repository,number,viewer,path,revision,viewed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(provider,host,repository,number,viewer,path) DO UPDATE SET revision=excluded.revision,viewed_at=excluded.viewed_at",params![scope.provider,scope.host,scope.repository,number,scope.viewer,mark.path,mark.revision,viewed_at])?;}
            else{transaction.execute("DELETE FROM pull_request_files_viewed WHERE provider=?1 AND host=?2 AND repository=?3 AND number=?4 AND viewer=?5 AND path=?6",params![scope.provider,scope.host,scope.repository,number,scope.viewer,mark.path])?;}
        }transaction.commit()?;Ok(())}).await.map_err(|_|database_error("setFilesViewed"))
    }
    async fn guide(&self, key: &ThreadPullRequestKey) -> Result<Option<PullRequestReviewGuide>> {
        let number = i64::try_from(key.number).map_err(|_| database_error("pullRequests"))?;
        let key = key.clone();
        let raw:Option<String>=self.database.reader(move|c|Ok(c.query_row("SELECT guide_json FROM pull_request_review_guides WHERE host=?1 AND repository=?2 AND number=?3",params![key.host.as_str(),key.repository.as_str(),number],|r|r.get(0)).optional()?)).await.map_err(|_|database_error("reviewGuide"))?;
        raw.map(|s| {
            serde_json::from_str(&s)
                .map_err(|_| Error::invalid("reviewGuide", "The stored review guide is invalid."))
        })
        .transpose()
    }
    async fn set_guide(
        &self,
        key: &ThreadPullRequestKey,
        guide: PullRequestReviewGuide,
    ) -> Result<()> {
        let number = i64::try_from(key.number).map_err(|_| database_error("pullRequests"))?;
        let key = key.clone();
        let raw = serde_json::to_string(&guide)
            .map_err(|_| Error::invalid("generateReviewGuide", "Invalid review guide."))?;
        self.database.writer(move|c|{c.execute("INSERT INTO pull_request_review_guides (host,repository,number,guide_json) VALUES (?1,?2,?3,?4) ON CONFLICT(host,repository,number) DO UPDATE SET guide_json=excluded.guide_json",params![key.host.as_str(),key.repository.as_str(),number,raw])?;Ok(())}).await.map_err(|_|database_error("generateReviewGuide"))
    }
    async fn linked_threads(
        &self,
        key: &ThreadPullRequestKey,
    ) -> Result<PullRequestLinkedThreadsResult> {
        let number = i64::try_from(key.number).map_err(|_| database_error("pullRequests"))?;
        let key = key.clone();
        let rows=self.database.reader(move|connection|{
            let mut query=connection.prepare("SELECT t.thread_id,t.project_id,t.title,t.archived_at,(json_extract(t.payload_json,'$.settledAt') IS NOT NULL OR json_extract(t.payload_json,'$.settledOverride') IS 'settled') AS settled,json_extract(link.value,'$.host'),json_extract(link.value,'$.repository') FROM orchestration_v2_projection_threads AS t JOIN json_each(t.payload_json,'$.pullRequests') AS link WHERE json_extract(link.value,'$.number')=?1 AND json_extract(link.value,'$.source') != 'stack-dismissed' AND t.deleted_at IS NULL AND json_extract(t.payload_json,'$.lineage.sideChat') IS NOT 1 ORDER BY t.updated_at DESC,t.thread_id ASC")?;
            let values=query.query_map([number],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,bool>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(values.into_iter().filter(|row|row.5.eq_ignore_ascii_case(key.host.as_str())&&row.6.eq_ignore_ascii_case(key.repository.as_str())).map(|row|serde_json::json!({"id":row.0,"projectId":row.1,"title":row.2,"archivedAt":row.3,"settled":row.4})).collect::<Vec<_>>())
        }).await.map_err(|_|database_error("linkedThreads"))?;
        crate::convert(serde_json::json!({"threads":rows}))
    }
}
