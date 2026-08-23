// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client-side persistent cache and reactive in-memory store for the sync
//! engine (KYO-169).
//!
//! ### `store` — reactive in-memory store (all targets)
//!
//! [`store::SyncStore`] is the single source of truth for list pages. It is
//! available on both SSR and WASM targets so page components that read from it
//! compile on both. On SSR the store is empty; pages show a loading state until
//! `initialized()` becomes `true` on the client after IDB hydration.
//!
//! ### `db` — IndexedDB persistence (WASM only)
//!
//! On WASM targets the persistent cache is backed by two IndexedDB object
//! stores (via `indexed_db_futures`) that survive page reloads without
//! round-tripping to the server on startup.  See [`db`] for the full schema
//! and API.
//!
//! ### `schema_hash` — wipe decision (all targets)
//!
//! [`schema_hash::cache_needs_wipe`] is the pure predicate `db::init_cache_db`
//! uses to decide whether to wipe the cache on open. It lives in its own
//! ungated module — with no `indexed_db_futures`/`web_sys` dependency — so it
//! can be unit-tested on the host target even though `db` itself cannot
//! (KYO-479).

pub mod schema_hash;
pub mod store;

#[cfg(target_arch = "wasm32")]
pub mod db;

#[cfg(target_arch = "wasm32")]
pub mod sync_engine;

#[cfg(target_arch = "wasm32")]
pub use db::*;
