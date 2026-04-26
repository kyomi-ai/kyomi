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
    BuildPrimitive,
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
    entity_id: String,
    data: String,
    updated_at: String,
}

// ── Key encoding ─────────────────────────────────────────────────────────────

fn entity_key(entity_type: &str, workspace_id: &str, entity_id: &str) -> String {
    format!("{entity_type}\x00{workspace_id}\x00{entity_id}")
}

fn prefix_range(entity_type: &str, workspace_id: &str) -> KeyRange<String> {
    let lower = format!("{entity_type}\x00{workspace_id}\x00");
    let upper = format!("{entity_type}\x00{workspace_id}\x01");
    KeyRange::Bound(lower, false, upper, true)
}

// ── Initialisation ────────────────────────────────────────────────────────────

pub async fn init_cache_db(_workspace_id: &str) -> Result<CacheDb, CacheDbError> {
    let db = Database::open(DB_NAME)
        .with_version(DB_VERSION)
        .with_on_upgrade_needed(|_event, db| {
            if !db.object_store_names().any(|n| n == STORE_ENTITIES) {
                db.create_object_store(STORE_ENTITIES)
                    .build()
                    .map_err(|e| wasm_bindgen::JsValue::from(e.to_string()))?;
            }

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
    let records = store
        .get_all::<EntityRecord>()
        .with_query::<String, KeyRange<String>>(range)
        .serde()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    Ok(records
        .filter_map(|r| r.ok())
        .map(|r| (r.entity_id, r.data, r.updated_at))
        .collect())
}

// ── Write operations ──────────────────────────────────────────────────────────

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
        .with_key(key)
        .serde()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

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
        .delete::<String, KeyRange<String>>(KeyRange::Only(key))
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

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
        .delete::<String, KeyRange<String>>(range)
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

// ── Sync cursor ───────────────────────────────────────────────────────────────

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
        .get::<String, String, KeyRange<String>>(KeyRange::Only(workspace_id.to_owned()))
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    Ok(value)
}

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
        .put::<String>(sync_id.to_owned())
        .with_key::<String>(workspace_id.to_owned())
        .primitive()
        .map_err(CacheDbError::Serde)?
        .await
        .map_err(CacheDbError::Transaction)?;

    tx.commit().await.map_err(CacheDbError::Transaction)?;

    Ok(())
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CacheDbError {
    Open(indexed_db_futures::error::OpenDbError),
    Transaction(indexed_db_futures::error::Error),
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
