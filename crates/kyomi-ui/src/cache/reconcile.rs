// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure per-entity-type count reconciliation decision for the sync engine
//! cache (KYO-480).
//!
//! Once a client has a cursor, `cache::sync_engine` only ever replays
//! `sync_log` rows newer than it (a delta) — nothing ever compares the local
//! set against the server's authoritative set. A bug in the schema-hash
//! guard (KYO-479) or an entity whose only mutation predates `sync_log`
//! coverage can therefore leave a client's cache permanently, silently
//! diverged. This module is the *detection and decision* half of the fix:
//! given the server's per-entity-type counts (now carried on every
//! `sync_complete`, see `kyomi_types::sync::SyncResponse::SyncComplete`) and
//! the client's own local counts, decide which entity types have diverged
//! and are due a repair right now.
//!
//! Extracted into its own ungated module — no `indexed_db_futures`/`web_sys`
//! dependency — so it is compiled and unit-tested on the host target, the
//! same split `cache::schema_hash` established for the KYO-479 wipe
//! decision. `cache::sync_engine` (wasm32-only) is the thin glue: it reads
//! local counts off the real `SyncStore`, calls [`diverged_types`] and
//! [`RepairGuard::admit`], and — for whatever this module says needs
//! repair — wipes that entity type locally (`SyncStore` + IndexedDB) and
//! sends a `sync_bootstrap` request. The *repopulation* itself is not new
//! code: it is the same `sync_action`/`sync_complete` pipeline every first
//! visit, schema-hash wipe, and `sync_reset` already relies on (KYO-169,
//! KYO-479) — this module's job ends at "what must be wiped and re-fetched."

use std::collections::{HashMap, HashSet};

use kyomi_types::sync::entity_types;

// ── Divergence detection ────────────────────────────────────────────────────

/// Compare local per-entity-type counts against the server's authoritative
/// counts and return the entity types that disagree.
///
/// Only entity types in [`entity_types::RECONCILED`] are ever considered —
/// `workspace_settings` and the Tier 2 detail caches have no per-type count
/// in the protocol at all (see that constant's doc comment).
///
/// Catches **both** directions: the local set missing rows the server has
/// (`local < server`, the KYO-479 failure mode — entities that can never
/// arrive via delta), and the local set holding rows the server no longer
/// has, i.e. stale extras (`local > server`).
///
/// An entity type **absent** from `server_counts` is skipped, not treated as
/// "expected zero" — the server omits a type from the map when its count
/// query failed this cycle (`compute_sync_counts` in
/// `apps/server/src/routes/websocket.rs`), and a transient DB error must
/// never be read as "you have zero of these, wipe them all."
pub fn diverged_types(
    local_counts: &HashMap<String, i64>,
    server_counts: &HashMap<String, i64>,
) -> Vec<String> {
    entity_types::RECONCILED
        .iter()
        .filter_map(|&entity_type| {
            let server_n = server_counts.get(entity_type)?;
            let local_n = local_counts.get(entity_type).copied().unwrap_or(0);
            (local_n != *server_n).then(|| entity_type.to_string())
        })
        .collect()
}

// ── Anti-thrash guard ───────────────────────────────────────────────────────

/// Tracks which entity types have already had a repair triggered since the
/// last time the WebSocket transitioned to `Connected` (KYO-480 anti-thrash
/// guard).
///
/// Scope is deliberately **per connection**, not permanent: if a mismatch
/// survives a repair — the repair's own bootstrap request never completes
/// because the connection drops mid-stream, or (in principle) a server-side
/// bug makes the count query and the list query permanently disagree for
/// some type — this guard blocks further repair attempts for that type
/// until the next reconnect resets it. That bounds each connection to at
/// most one full-bootstrap repair per entity type, rather than an
/// unbounded loop re-triggering on every subsequent `sync_complete`.
///
/// Because a reconnect calls [`reset`](Self::reset), "a failed repair
/// retries on the next sync" falls out of this for free: a repair that
/// failed hard enough to matter took the connection down with it, and the
/// very next connection gets a clean slate to try again. A repair that
/// merely detects the *same* mismatch again on a *later* `sync_complete`
/// within the *same* still-open connection is intentionally **not**
/// retried — that is precisely the thrash this guard exists to prevent.
#[derive(Debug, Default, Clone)]
pub struct RepairGuard {
    attempted: HashSet<String>,
}

impl RepairGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// From `diverged` (the output of [`diverged_types`]), return the subset
    /// that should actually be repaired right now: diverged **and** not
    /// already attempted this connection.
    ///
    /// Marks every returned entity type as attempted immediately —
    /// optimistically, before the repair is known to succeed — so that a
    /// second mismatch report for the same type arriving before the first
    /// repair's bootstrap response lands does not fire a second, concurrent
    /// repair for it.
    pub fn admit(&mut self, diverged: Vec<String>) -> Vec<String> {
        diverged
            .into_iter()
            .filter(|entity_type| self.attempted.insert(entity_type.clone()))
            .collect()
    }

    /// Reset on every transition to `Connected`. A new connection is a
    /// fresh chance to repair — including for a type whose previous repair
    /// attempt never completed because the connection itself dropped.
    pub fn reset(&mut self) {
        self.attempted.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    // ── diverged_types ───────────────────────────────────────────────────

    #[test]
    fn matching_counts_diverge_on_nothing() {
        let local = counts(&[
            (entity_types::DASHBOARD, 3),
            (entity_types::KNOWLEDGE, 1),
            (entity_types::CHAT_SESSION, 5),
            (entity_types::WATCH, 2),
        ]);
        let server = local.clone();
        assert!(diverged_types(&local, &server).is_empty());
    }

    /// The KYO-479 failure mode: local is missing rows the server has.
    #[test]
    fn missing_entities_direction_is_detected() {
        let local = counts(&[(entity_types::WATCH, 1)]);
        let server = counts(&[(entity_types::WATCH, 3)]);
        assert_eq!(diverged_types(&local, &server), vec![entity_types::WATCH]);
    }

    /// Stale extras: local holds rows the server no longer has.
    #[test]
    fn stale_extras_direction_is_detected() {
        let local = counts(&[(entity_types::DASHBOARD, 5)]);
        let server = counts(&[(entity_types::DASHBOARD, 2)]);
        assert_eq!(
            diverged_types(&local, &server),
            vec![entity_types::DASHBOARD]
        );
    }

    #[test]
    fn multiple_diverged_types_are_all_reported() {
        let local = counts(&[
            (entity_types::DASHBOARD, 5),
            (entity_types::KNOWLEDGE, 1),
            (entity_types::CHAT_SESSION, 5),
            (entity_types::WATCH, 1),
        ]);
        let server = counts(&[
            (entity_types::DASHBOARD, 2), // stale extra
            (entity_types::KNOWLEDGE, 1), // matches
            (entity_types::CHAT_SESSION, 5), // matches
            (entity_types::WATCH, 3),     // missing
        ]);
        let mut diverged = diverged_types(&local, &server);
        diverged.sort();
        let mut expected = vec![
            entity_types::DASHBOARD.to_string(),
            entity_types::WATCH.to_string(),
        ];
        expected.sort();
        assert_eq!(diverged, expected);
    }

    /// A type absent from `server_counts` (its count query failed
    /// server-side this cycle) must not be treated as "expected zero" —
    /// skip it entirely rather than flagging a false divergence.
    #[test]
    fn entity_type_missing_from_server_counts_is_not_flagged() {
        let local = counts(&[(entity_types::WATCH, 4)]);
        let server: HashMap<String, i64> = HashMap::new(); // watch count query failed
        assert!(diverged_types(&local, &server).is_empty());
    }

    /// `local_counts` never has to enumerate every type explicitly — a type
    /// absent from `local_counts` but present in `server_counts` reads as
    /// local count zero, i.e. a real (missing-everything) divergence.
    #[test]
    fn entity_type_missing_from_local_counts_reads_as_zero() {
        let local: HashMap<String, i64> = HashMap::new();
        let server = counts(&[(entity_types::WATCH, 2)]);
        assert_eq!(diverged_types(&local, &server), vec![entity_types::WATCH]);
    }

    // ── RepairGuard ──────────────────────────────────────────────────────

    #[test]
    fn first_mismatch_is_admitted() {
        let mut guard = RepairGuard::new();
        let admitted = guard.admit(vec![entity_types::WATCH.to_string()]);
        assert_eq!(admitted, vec![entity_types::WATCH.to_string()]);
    }

    /// Anti-thrash: the same entity type mismatching again within the same
    /// connection (guard never reset) is not re-admitted.
    #[test]
    fn repeated_mismatch_within_same_connection_is_not_readmitted() {
        let mut guard = RepairGuard::new();
        assert_eq!(
            guard.admit(vec![entity_types::WATCH.to_string()]),
            vec![entity_types::WATCH.to_string()]
        );
        // Same type diverges again on the very next sync_complete — blocked.
        assert!(guard.admit(vec![entity_types::WATCH.to_string()]).is_empty());
        assert!(guard.admit(vec![entity_types::WATCH.to_string()]).is_empty());
    }

    /// Retry-on-failure: a reconnect (guard reset) gives a type that was
    /// already attempted this connection a fresh chance.
    #[test]
    fn reset_on_reconnect_allows_retry() {
        let mut guard = RepairGuard::new();
        assert_eq!(
            guard.admit(vec![entity_types::WATCH.to_string()]),
            vec![entity_types::WATCH.to_string()]
        );
        assert!(guard.admit(vec![entity_types::WATCH.to_string()]).is_empty());

        // The repair's bootstrap request never completed — the connection
        // dropped and came back. A fresh connection resets the guard.
        guard.reset();

        assert_eq!(
            guard.admit(vec![entity_types::WATCH.to_string()]),
            vec![entity_types::WATCH.to_string()]
        );
    }

    /// A distinct entity type diverging is independent of one already
    /// attempted — the guard is keyed per type, not a single global flag.
    #[test]
    fn guard_is_per_entity_type_not_global() {
        let mut guard = RepairGuard::new();
        assert_eq!(
            guard.admit(vec![entity_types::WATCH.to_string()]),
            vec![entity_types::WATCH.to_string()]
        );
        // dashboard diverges later in the same connection — independently admitted.
        assert_eq!(
            guard.admit(vec![entity_types::DASHBOARD.to_string()]),
            vec![entity_types::DASHBOARD.to_string()]
        );
        // watch is still blocked.
        assert!(guard.admit(vec![entity_types::WATCH.to_string()]).is_empty());
    }

    /// A single `admit` call mixing an already-attempted type with a new
    /// one only returns the new one.
    #[test]
    fn admit_filters_a_mixed_batch() {
        let mut guard = RepairGuard::new();
        guard.admit(vec![entity_types::WATCH.to_string()]);
        let admitted = guard.admit(vec![
            entity_types::WATCH.to_string(),
            entity_types::DASHBOARD.to_string(),
        ]);
        assert_eq!(admitted, vec![entity_types::DASHBOARD.to_string()]);
    }
}
