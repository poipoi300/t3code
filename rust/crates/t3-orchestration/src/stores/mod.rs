pub mod event_store;
pub mod outbox;
pub mod positions;
pub mod projects;
pub mod receipts;

use crate::error::Result;
use t3_persistence::{
    Database,
    rusqlite::{Connection, Transaction, TransactionBehavior},
};

#[derive(Clone)]
pub struct StoreDatabase(pub Database);
impl StoreDatabase {
    pub async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        // One deferred read transaction, so every statement in `f` reads the same snapshot.
        self.0
            .reader(move |connection| {
                Ok((|| {
                    let tx = connection.unchecked_transaction()?;
                    let value = f(&tx)?;
                    tx.commit()?;
                    Ok(value)
                })())
            })
            .await?
    }
    pub async fn transaction<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.0
            .writer(move |connection| {
                Ok((|| {
                    let tx =
                        connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let result = f(&tx)?;
                    tx.commit()?;
                    Ok(result)
                })())
            })
            .await?
    }
}
