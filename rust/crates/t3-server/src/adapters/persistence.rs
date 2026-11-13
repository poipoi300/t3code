pub use t3_persistence::Database;

/// Opens and migrates the V2 database off the async runtime. The caller must already hold
/// `ServerOwner`, which owns the migration lock for the whole server lifetime.
pub async fn open(path: std::path::PathBuf) -> crate::Result<Database> {
    let database = tokio::task::spawn_blocking(move || Database::open_v2(path, Default::default()))
        .await?
        .map_err(service_error)?;
    for warning in warnings(&database) {
        tracing::warn!(%warning, "database opened in a degraded mode");
    }
    Ok(database)
}

/// Reader and checkpoint fallbacks the database chose at open; empty when fully healthy.
pub fn warnings(database: &Database) -> Vec<String> {
    database
        .reader_warning()
        .map(str::to_owned)
        .into_iter()
        .chain(database.checkpoint_warning())
        .collect()
}

pub fn service_error(error: impl std::fmt::Display) -> crate::Error {
    crate::Error::Service(error.to_string())
}
