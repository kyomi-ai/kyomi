// SPDX-License-Identifier: AGPL-3.0-or-later

//! IndexedDB-backed local metadata cache for the sync engine (KYO-169).
//!
//! ## Why IndexedDB instead of rusqlite + sqlite-wasm-vfs?
//!
//! `rusqlite` carries `links = "sqlite3"` which conflicts with `sqlx`'s sqlite
//! feature at the Cargo workspace resolver level.  The resolver checks link key
//! conflicts across **all** target platforms, so the `cfg(target_arch = "wasm32")`
//! guard does not help.  `indexed_db_futures` provides the same persistent
//! key-value semantics with no link conflicts and a clean async Rust API.
//!
//! ## Schema
//!
//! Two IndexedDB object stores inside the `kyomi-sync` database (version 1):
//!
//! ### `entity_cache`
//!
//! Out-of-line keys encoded as `"{entity_type}\x00{workspace_id}\x00{entity_id}"`.
//! The `\x00` separator is below any printable character so prefix-range queries
//! work with a simple `Bound` key range.  Values are JSON objects
//! `{ entity_id, data, updated_at }` serialised via serde.
//!
//! ### `sync_cursors`
//!
//! Out-of-line keys: `workspace_id`.  Values: the opaque `last_sync_id` string.
//!
//! ## Thread safety
//!
//! `indexed_db_futures::Database` is `!Send`.  Callers must not share a `CacheDb`
//! across async boundaries — wrap in `send_wrapper::SendWrapper` when storing in
//! a Leptos context value.

use indexed_db_futures::{
    Build,
    BuildSerde,
    database::Database,
    prelude::QuerySource,
    transaction::TransactionMode,
    KeyRange,
};
use serde::{Deserialize, Serialize};

/// Name of the IndexedDB database.
const DB_NAME: &str = "kyomi-sync";

/// Object store for entity metadata.
const STORE_ENTITIES: &str = "entity_cache";

/// Object store for per-workspace sync cursors.
const STORE_CURSORS: &str = "sync_cursors";

/// Schema version — increment when adding new object stores or indexes.
const DB_VERSION: u8 = 1;

/// Handle to the open IndexedDB database.
///
/// All cache operations take `&CacheDb` and are async.  Open one handle per
/// logical task via [`init_cache_db`] and hold it for the lifetime of that task.
pub struct CacheDb {
    inner: Database,
}

/// Value record stored in the `entity_cache` object store.
#[derive(Serialize, Deserialize)]
struct EntityRecord {
    /// Matches the `entity_id` component of the IDB key — stored inline so
    /// callers that iterate all records of a type don't have to parse the key.
    entity_id: String,
    /// Serialised JSON blob for the entity (opaque to the cache layer).
    data: String,
    /// ISO-8601 timestamp of the last server-side update.
    updated_at: String,
}

// ── Key encoding ─────────────────────────────────────────────────────────────

/// Encode the composite key for an entity record.
///
/// Format: `"{entity_type}\x00{workspace_id}\x00{entity_id}"`
///
/// The NUL byte separator sorts below every printable Unicode code point, which
/// lets us compute prefix range bounds cheaply (see [`prefix_range`]).
fn entity_key(entity_type: &str, workspace_id: &str, entity_id: &str) -> String {
    format!("{entity_type}\x00{workspace_id}\x00{entity_id}")
}

/// Build a `KeyRange` that matches all keys for a given `(entity_type, workspace_id)` prefix.
///
/// The lower bound is `"{entity_type}\x00{workspace_id}\x00"` (inclusive).
/// The upper bound is `"{entity_type}\x00{workspace_id}\x01"` (exclusive) — `\x01`
/// is the next code point after `\x00`, so any entity_id value falls within.
fn prefix_range(entity_type: &str, workspace_id: &str) -> KeyRange<String> {
    let lower = format!("{entity_type}\x00{workspace_id}\x00");
    let upper = format!("{entity_type}\x00{workspace_id}\x01");
    // lower inclusive, upper exclusive
    KeyRange::Bound(lower, false, upper, true)
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Open (or create) the local cache database.
///
/// This is async because IndexedDB `open` is async.  Call once at sync engine
/// startup; hold the returned `CacheDb` for the duration.
///
/// # Errors
///
/// Returns [`CacheDbError`] if the browser's IndexedDB API is unavailable or if
/// the schema upgrade fails.
pub async fn init_cache_db(_workspace_id: &str) -> Result<CacheDb, CacheDbError> {
    let db = Database::open(DB_NAME)
        .with_version(DB_VERSION)
        .with_on_upgrade_needed(|_event, db| {
            // Create entity_cache store (out-of-line keys — we supply keys explicitly).
            if !db.object_store_names().any(|n| n == STORE_ENTITIES) {
                db.create_object_store(STORE_ENTITIES)
                    .build()
                    .map_err(|e| wasm_bindgen::JsValue::from(e.to_string()))?;
            }

            // Create sync_cursors store.
            if !db.object_store_names().any(|n| n == STORE_CURSORS) {
                db.create_object_store(STORE_CURSORS)
                    .build()
                    .map_err(|e| wasm_bindgen::JsValue::from(e.to_string()))?;
            }

            Ok(())
        })
        .await
        .map_err(CacheDbError::Open)?;

    Ok(CacheDb { inner: db })
}

// ── Read operations ───────────────────────────────────────────────────────────

/// Read all cached entities of `entity_type` for `workspace_id`.
///
/// Returns a `Vec` of `(entity_id, data_json, updated_at)` tuples in
/// key-sort order (which is `entity_id` alphabetical order).
pub async fn read_all(
    db: &CacheDb,
    entity_type: &str,
    workspace_id: &str,
) -> Result<Vec<(String, String, String)>, CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_ENTITIES)
        .with_mode(TransactionMode::Readonly)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_ENTITIES).map_err(CacheDbError::Transaction)?;

    let range = prefix_range(entity_type, workspace_id);
    let records: Vec<EntityRecord> = store
        .get_all::<EntityRecord>()
        .with_query(range)
        .serde()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    Ok(records
        .into_iter()
        .map(|r| (r.entity_id, r.data, r.updated_at))
        .collect())
}

// ── Write operations ──────────────────────────────────────────────────────────

/// Insert or update a single entity in the cache.
///
/// Uses IndexedDB `put` semantics (upsert — overwrites if the key already exists).
pub async fn upsert(
    db: &CacheDb,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
    data: &str,
    updated_at: &str,
) -> Result<(), CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_ENTITIES)
        .with_mode(TransactionMode::Readwrite)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_ENTITIES).map_err(CacheDbError::Transaction)?;

    let key = entity_key(entity_type, workspace_id, entity_id);
    let record = EntityRecord {
        entity_id: entity_id.to_owned(),
        data: data.to_owned(),
        updated_at: updated_at.to_owned(),
    };

    store
        .put(record)
        .with_key(key.as_str())
        .serde()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

/// Delete a single entity from the cache.
pub async fn delete(
    db: &CacheDb,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
) -> Result<(), CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_ENTITIES)
        .with_mode(TransactionMode::Readwrite)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_ENTITIES).map_err(CacheDbError::Transaction)?;

    let key = entity_key(entity_type, workspace_id, entity_id);
    store
        .delete(KeyRange::Only(key))
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

/// Delete all cached entities of `entity_type` for `workspace_id`.
///
/// Typically called when the sync engine performs a full-refresh reset
/// (`last_sync_id = null`).
pub async fn delete_all_of_type(
    db: &CacheDb,
    entity_type: &str,
    workspace_id: &str,
) -> Result<(), CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_ENTITIES)
        .with_mode(TransactionMode::Readwrite)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_ENTITIES).map_err(CacheDbError::Transaction)?;

    let range = prefix_range(entity_type, workspace_id);
    store
        .delete(range)
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

// ── Sync cursor ───────────────────────────────────────────────────────────────

/// Return the last successfully processed sync ID for `workspace_id`.
///
/// Returns `None` if no sync has completed yet (first load or after a reset),
/// signalling the sync engine to fetch the full initial dataset.
pub async fn get_last_sync_id(
    db: &CacheDb,
    workspace_id: &str,
) -> Result<Option<String>, CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_CURSORS)
        .with_mode(TransactionMode::Readonly)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_CURSORS).map_err(CacheDbError::Transaction)?;

    let value: Option<String> = store
        .get::<String, _, _>(KeyRange::Only(workspace_id.to_owned()))
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    Ok(value)
}

/// Persist the last successfully processed sync ID for `workspace_id`.
///
/// The cursor is read on the next startup so the sync engine can request only
/// events that occurred after the last known state (delta sync), avoiding a
/// full re-fetch.
pub async fn set_last_sync_id(
    db: &CacheDb,
    workspace_id: &str,
    sync_id: &str,
) -> Result<(), CacheDbError> {
    let tx = db
        .inner
        .transaction(STORE_CURSORS)
        .with_mode(TransactionMode::Readwrite)
        .build()
        .map_err(CacheDbError::Transaction)?;

    let store = tx.object_store(STORE_CURSORS).map_err(CacheDbError::Transaction)?;

    store
        .put(sync_id.to_owned())
        .with_key(workspace_id.to_owned())
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur during cache database operations.
#[derive(Debug)]
pub enum CacheDbError {
    /// The IndexedDB database could not be opened or upgraded.
    Open(indexed_db_futures::error::OpenDbError),
    /// An IndexedDB transaction (or request within one) failed.
    Transaction(indexed_db_futures::error::Error),
    /// Serde serialisation/deserialisation of a record failed.
    Serde(indexed_db_futures::error::Error),
}

impl std::fmt::Display for CacheDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheDbError::Open(e) => write!(f, "IndexedDB open failed: {e}"),
            CacheDbError::Transaction(e) => write!(f, "IndexedDB transaction failed: {e}"),
            CacheDbError::Serde(e) => write!(f, "IndexedDB serde error: {e}"),
        }
    }
}

impl std::error::Error for CacheDbError {}
