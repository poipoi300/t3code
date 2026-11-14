//! IndexedDB storage. Both areas are object stores of one database so a conditional
//! transaction covers both; conditions are compared inside request callbacks, where the
//! transaction is still active, and writes are issued from the last callback.
use super::js::{self, FirstEvent, Listeners, remove_listeners};
use js_sys::{Array, Uint8Array};
use std::{cell::RefCell, rc::Rc};
use t3_client::platform::{
    PlatformError, StorageArea, StorageKey, StorageTransaction, StorageWrite,
};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    IdbDatabase, IdbFactory, IdbIndexParameters, IdbObjectStore, IdbOpenDbRequest, IdbRequest,
    IdbTransaction, IdbTransactionMode,
};

pub(crate) const ORDINARY: &str = "ordinary";
pub(crate) const SECURE: &str = "secure";
const NAMESPACE_INDEX: &str = "namespace";
const VERSION: u32 = 1;

fn store_name(area: StorageArea) -> &'static str {
    match area {
        StorageArea::Ordinary => ORDINARY,
        StorageArea::Secure => SECURE,
    }
}
fn storage(context: &str) -> impl Fn(JsValue) -> PlatformError + '_ {
    move |error| PlatformError::Storage(format!("{context}: {}", js::error_name(&error)))
}
fn encode(key: &StorageKey) -> Result<String, PlatformError> {
    key.encode_tuple()
        .map_err(|_| PlatformError::InvalidData("storage key".into()))
}

async fn request(request: &IdbRequest, context: &str) -> Result<JsValue, PlatformError> {
    let target = request.clone();
    let done = FirstEvent::new(
        request,
        vec![
            ("success", Box::new(|_: &web_sys::Event| true)),
            ("error", Box::new(|_: &web_sys::Event| false)),
        ],
    )?;
    if done.await? {
        target.result().map_err(storage(context))
    } else {
        Err(PlatformError::Storage(format!(
            "{context}: {}",
            target
                .error()
                .ok()
                .flatten()
                .map_or_else(|| "Error".into(), |e| e.name())
        )))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Complete,
    Failed,
}
fn ending(transaction: &IdbTransaction) -> Result<FirstEvent<Ending>, PlatformError> {
    FirstEvent::new(
        transaction,
        vec![
            ("complete", Box::new(|_: &web_sys::Event| Ending::Complete)),
            ("error", Box::new(|_: &web_sys::Event| Ending::Failed)),
            ("abort", Box::new(|_: &web_sys::Event| Ending::Failed)),
        ],
    )
}

pub(crate) async fn open(factory: &IdbFactory, name: &str) -> Result<IdbDatabase, PlatformError> {
    let opening = factory
        .open_with_u32(name, VERSION)
        .map_err(storage("open database"))?;
    let upgrade_target = opening.clone();
    let mut listeners: Listeners = vec![js::listen(&opening, "upgradeneeded", move |_| {
        let Ok(result) = upgrade_target.result() else {
            return;
        };
        let database: IdbDatabase = result.unchecked_into();
        for name in [ORDINARY, SECURE] {
            if database.object_store_names().contains(name) {
                continue;
            }
            if let Ok(store) = database.create_object_store(name) {
                let parameters = IdbIndexParameters::new();
                parameters.set_unique(false);
                let _ = store.create_index_with_str_and_optional_parameters(
                    NAMESPACE_INDEX,
                    "namespace",
                    &parameters,
                );
            }
        }
    })?];
    let result = request(&opening, "open database").await;
    remove_listeners(&mut listeners);
    Ok(result?.unchecked_into())
}

/// The TypeScript client's DPoP key database: version 1, one plain store.
pub(crate) async fn open_dpop(
    factory: &IdbFactory,
    name: &str,
    store: &'static str,
) -> Result<IdbDatabase, PlatformError> {
    let opening = factory
        .open_with_u32(name, 1)
        .map_err(storage("open key database"))?;
    let upgrade_target = opening.clone();
    let mut listeners: Listeners = vec![js::listen(&opening, "upgradeneeded", move |_| {
        if let Ok(result) = upgrade_target.result() {
            let database: IdbDatabase = result.unchecked_into();
            if !database.object_store_names().contains(store) {
                let _ = database.create_object_store(store);
            }
        }
    })?];
    let result = request(&opening, "open key database").await;
    remove_listeners(&mut listeners);
    Ok(result?.unchecked_into())
}

/// Stores `value` only if `key` is absent; `false` when the add did not commit (another writer
/// created the key first, or the write failed), so the caller reads back what is stored.
pub(crate) async fn add_value(
    database: &IdbDatabase,
    store: &str,
    key: &str,
    value: &JsValue,
) -> Result<bool, PlatformError> {
    let transaction = transaction(database, &[store], IdbTransactionMode::Readwrite)?;
    let done = ending(&transaction)?;
    transaction
        .object_store(store)
        .and_then(|store| store.add_with_key(value, &JsValue::from_str(key)))
        .map_err(storage("add key"))?;
    Ok(matches!(done.await?, Ending::Complete))
}

/// Opens a database only if it already exists; never creates or upgrades someone else's store.
pub(crate) async fn open_existing(
    factory: &IdbFactory,
    name: &str,
) -> Result<Option<IdbDatabase>, PlatformError> {
    let opening: IdbOpenDbRequest = factory.open(name).map_err(storage("open database"))?;
    let created = Rc::new(RefCell::new(false));
    let upgrade_target = opening.clone();
    let flag = created.clone();
    let mut listeners: Listeners = vec![js::listen(&opening, "upgradeneeded", move |_| {
        *flag.borrow_mut() = true;
        if let Some(transaction) = upgrade_target.transaction() {
            let _ = transaction.abort();
        }
    })?];
    let result = request(&opening, "open legacy database").await;
    remove_listeners(&mut listeners);
    if *created.borrow() {
        return Ok(None);
    }
    Ok(Some(result?.unchecked_into()))
}

fn transaction(
    database: &IdbDatabase,
    stores: &[&str],
    mode: IdbTransactionMode,
) -> Result<IdbTransaction, PlatformError> {
    let names = stores
        .iter()
        .map(|s| JsValue::from_str(s))
        .collect::<Array>();
    database
        .transaction_with_str_sequence_and_mode(&names, mode)
        .map_err(storage("begin transaction"))
}

fn value_bytes(record: &JsValue) -> Option<Vec<u8>> {
    if record.is_undefined() || record.is_null() {
        return None;
    }
    js::get(record, "value")
        .dyn_into::<Uint8Array>()
        .ok()
        .map(|bytes| bytes.to_vec())
}

fn apply(store: &IdbObjectStore, write: &StorageWrite) -> Result<(), PlatformError> {
    let key = JsValue::from_str(&encode(&write.key)?);
    match &write.value {
        Some(value) => {
            let record = js::object(&[
                ("namespace", JsValue::from_str(&write.key.namespace)),
                ("value", Uint8Array::from(value.as_slice()).into()),
            ])?;
            store
                .put_with_key(&record, &key)
                .map_err(storage("write record"))?;
        }
        None => {
            store.delete(&key).map_err(storage("delete record"))?;
        }
    }
    Ok(())
}

pub(crate) async fn get(
    database: &IdbDatabase,
    area: StorageArea,
    key: &StorageKey,
) -> Result<Option<Vec<u8>>, PlatformError> {
    let transaction = transaction(database, &[store_name(area)], IdbTransactionMode::Readonly)?;
    let store = transaction
        .object_store(store_name(area))
        .map_err(storage("open store"))?;
    let pending = store
        .get(&JsValue::from_str(&encode(key)?))
        .map_err(storage("read record"))?;
    Ok(value_bytes(&request(&pending, "read record").await?))
}

pub(crate) async fn write(
    database: &IdbDatabase,
    area: StorageArea,
    writes: &[StorageWrite],
) -> Result<(), PlatformError> {
    let transaction = transaction(database, &[store_name(area)], IdbTransactionMode::Readwrite)?;
    let done = ending(&transaction)?;
    let store = transaction
        .object_store(store_name(area))
        .map_err(storage("open store"))?;
    for write in writes {
        if let Err(error) = apply(&store, write) {
            let _ = transaction.abort();
            return Err(error);
        }
    }
    match done.await? {
        Ending::Complete => Ok(()),
        Ending::Failed => Err(PlatformError::Storage("write transaction failed".into())),
    }
}

pub(crate) async fn list(
    database: &IdbDatabase,
    area: StorageArea,
    namespace: &str,
) -> Result<Vec<StorageKey>, PlatformError> {
    let transaction = transaction(database, &[store_name(area)], IdbTransactionMode::Readonly)?;
    let index = transaction
        .object_store(store_name(area))
        .and_then(|store| store.index(NAMESPACE_INDEX))
        .map_err(storage("open index"))?;
    let pending = index
        .get_all_keys_with_key(&JsValue::from_str(namespace))
        .map_err(storage("list records"))?;
    let keys: Array = request(&pending, "list records").await?.unchecked_into();
    let mut keys = keys
        .iter()
        .map(|raw| {
            raw.as_string()
                .and_then(|raw| StorageKey::decode_tuple(&raw).ok())
                .ok_or_else(|| PlatformError::InvalidData("stored key".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort();
    Ok(keys)
}

struct Conditional {
    remaining: usize,
    failed: bool,
    request: Option<StorageTransaction>,
    error: Option<PlatformError>,
}

fn apply_all(
    transaction: &IdbTransaction,
    request: &StorageTransaction,
) -> Result<(), PlatformError> {
    let ordinary = transaction
        .object_store(ORDINARY)
        .map_err(storage("open store"))?;
    let secure = transaction
        .object_store(SECURE)
        .map_err(storage("open store"))?;
    for write in &request.ordinary {
        apply(&ordinary, write)?;
    }
    for write in &request.secure {
        apply(&secure, write)?;
    }
    Ok(())
}

/// `Ok(false)`: a condition failed and the transaction was aborted with nothing written.
pub(crate) async fn transact(
    database: &IdbDatabase,
    request: StorageTransaction,
) -> Result<bool, PlatformError> {
    let transaction = transaction(database, &[ORDINARY, SECURE], IdbTransactionMode::Readwrite)?;
    let done = ending(&transaction)?;
    let state = Rc::new(RefCell::new(Conditional {
        remaining: request.conditions.len(),
        failed: false,
        request: None,
        error: None,
    }));
    let mut listeners: Listeners = Vec::new();
    let conditions = request.conditions.clone();
    if conditions.is_empty() {
        if let Err(error) = apply_all(&transaction, &request) {
            let _ = transaction.abort();
            return Err(error);
        }
    } else {
        state.borrow_mut().request = Some(request);
        for condition in conditions {
            let store = transaction
                .object_store(store_name(condition.area))
                .map_err(storage("open store"))?;
            let pending = store
                .get(&JsValue::from_str(&encode(&condition.key)?))
                .map_err(storage("read condition"))?;
            let state = state.clone();
            let target = pending.clone();
            let active = transaction.clone();
            listeners.push(js::listen(&pending, "success", move |_| {
                let mut state = state.borrow_mut();
                let current = target.result().ok().as_ref().and_then(value_bytes);
                if current != condition.expected {
                    state.failed = true;
                }
                state.remaining -= 1;
                if state.remaining > 0 {
                    return;
                }
                let outcome = if state.failed {
                    Ok(())
                } else {
                    match state.request.take() {
                        Some(request) => apply_all(&active, &request),
                        None => Ok(()),
                    }
                };
                if let Err(error) = outcome {
                    state.error = Some(error);
                }
                if state.failed || state.error.is_some() {
                    let _ = active.abort();
                }
            })?);
        }
    }
    let ended = done.await;
    remove_listeners(&mut listeners);
    let mut state = state.borrow_mut();
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    match ended? {
        Ending::Complete => Ok(true),
        Ending::Failed if state.failed => Ok(false),
        Ending::Failed => Err(PlatformError::Storage("transaction failed".into())),
    }
}

/// Raw values of a legacy store, read-only. Strings become UTF-8, byte arrays stay bytes,
/// anything else is its JSON text.
pub(crate) fn legacy_bytes(value: &JsValue) -> Result<Option<Vec<u8>>, PlatformError> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    if let Some(text) = value.as_string() {
        return Ok(Some(text.into_bytes()));
    }
    if let Some(bytes) = value.dyn_ref::<Uint8Array>() {
        return Ok(Some(bytes.to_vec()));
    }
    if let Some(buffer) = value.dyn_ref::<js_sys::ArrayBuffer>() {
        return Ok(Some(Uint8Array::new(buffer).to_vec()));
    }
    js_sys::JSON::stringify(value)
        .ok()
        .and_then(|text| text.as_string())
        .map(|text| Some(text.into_bytes()))
        .ok_or_else(|| PlatformError::InvalidData("legacy value is not serializable".into()))
}

pub(crate) async fn legacy_keys(
    database: &IdbDatabase,
    store: &str,
) -> Result<Vec<String>, PlatformError> {
    if !database.object_store_names().contains(store) {
        return Ok(Vec::new());
    }
    let transaction = transaction(database, &[store], IdbTransactionMode::Readonly)?;
    let pending = transaction
        .object_store(store)
        .and_then(|store| store.get_all_keys())
        .map_err(storage("list legacy records"))?;
    let keys: Array = request(&pending, "list legacy records")
        .await?
        .unchecked_into();
    Ok(keys.iter().filter_map(|key| key.as_string()).collect())
}

pub(crate) async fn legacy_value(
    database: &IdbDatabase,
    store: &str,
    key: &str,
) -> Result<Option<JsValue>, PlatformError> {
    if !database.object_store_names().contains(store) {
        return Ok(None);
    }
    let transaction = transaction(database, &[store], IdbTransactionMode::Readonly)?;
    let pending = transaction
        .object_store(store)
        .and_then(|store| store.get(&JsValue::from_str(key)))
        .map_err(storage("read legacy record"))?;
    let value = request(&pending, "read legacy record").await?;
    Ok((!value.is_undefined()).then_some(value))
}
