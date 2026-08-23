// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure schema-hash wipe decision for the sync engine cache (KYO-479).
//!
//! Extracted out of [`crate::cache::db`] so the *decision* — not just the
//! wiring around it — is unit-testable on the host target. `cache::db` is
//! `#[cfg(target_arch = "wasm32")]`-gated (it binds to IndexedDB via
//! `indexed_db_futures`/`web_sys`) and cannot be compiled or exercised by
//! `cargo test` on a normal host target. This module has no such dependency,
//! so it is compiled and tested unconditionally.

/// Decide whether the local cache must be wiped before this session proceeds.
///
/// `stored` is the `schemaHash` value last written to IDB's `_meta` store —
/// `None` means the profile has no schemaHash record at all. `current` is
/// the schema hash compiled into this build (`cache::db::SCHEMA_HASH`).
///
/// A missing hash is treated as **unknown provenance**, not as "safe to
/// keep": a browser profile whose last successful sync predates the commit
/// that started writing `schemaHash` has entity data and a sync cursor but
/// no stored hash. Bootstrap is one-shot (gated purely on `idb_cursor ==
/// 0`), so such a profile would otherwise never re-bootstrap and any entity
/// mutated before `sync_log` tracking began would stay permanently invisible
/// on that client — the KYO-479 regression. Treating `None` as "wipe" closes
/// that gap: the client re-bootstraps exactly as a genuinely fresh profile
/// would (which also has `stored == None`, and for which a wipe is a no-op —
/// see `cache::db::init_cache_db`).
pub fn cache_needs_wipe(stored: Option<&str>, current: &str) -> bool {
    match stored {
        Some(hash) => hash != current,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Some(current)` — hash matches, no wipe needed.
    #[test]
    fn matching_hash_does_not_wipe() {
        assert!(!cache_needs_wipe(Some("2026-07-25-v7-watch-privacy-filter"), "2026-07-25-v7-watch-privacy-filter"));
    }

    /// `Some(different)` — a real schema bump, must wipe.
    #[test]
    fn different_hash_wipes() {
        assert!(cache_needs_wipe(Some("2026-06-01-v6-old-shape"), "2026-07-25-v7-watch-privacy-filter"));
    }

    /// `None` with a profile that predates schemaHash tracking — the
    /// KYO-479 regression. Must wipe, not silently trust stale data.
    #[test]
    fn missing_hash_wipes() {
        assert!(cache_needs_wipe(None, "2026-07-25-v7-watch-privacy-filter"));
    }

    /// `None` on a genuinely fresh/empty profile also resolves to "wipe",
    /// but `wipe_all_data` on empty IDB stores is a no-op (see
    /// `cache::db::init_cache_db` doc comment) — this is the same case as
    /// the row above, the predicate cannot and need not distinguish them.
    #[test]
    fn fresh_profile_also_reports_wipe_but_it_is_a_harmless_noop() {
        assert!(cache_needs_wipe(None, "2026-07-25-v7-watch-privacy-filter"));
    }
}
