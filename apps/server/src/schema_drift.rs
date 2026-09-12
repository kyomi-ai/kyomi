// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared handle publishing the most recent migration-drift check result,
//! so `/api/health` can report it without querying the database itself —
//! see KYO-716.
//!
//! `main.rs`'s periodic migration-drift watch task is the sole writer;
//! `health.rs`'s handler is a read-only reader. This is deliberate: the
//! health endpoint backs the `kyomi-api` deployment's startup, readiness,
//! *and* liveness probes (`kyomi-private/k8s/kyomi-api.yaml`), which are hit
//! on a schedule by all three — a per-probe database round trip is
//! unacceptable load and would couple liveness to database latency.

use std::sync::{Arc, RwLock};

/// Cheaply cloneable (inner `Arc`) handle onto the most recently observed
/// migration drift.
///
/// `Default` is the truthful initial value: it's seeded right after
/// [`kyomi_core::db::DbPool::connect`] returns successfully, at which point
/// drift is zero by definition — that call just ran `.run()` to completion,
/// so the binary embeds everything the database currently has recorded.
#[derive(Clone, Default)]
pub struct SchemaDriftStatus(Arc<RwLock<Vec<i64>>>);

impl SchemaDriftStatus {
    /// Record the versions found missing by the most recent check. Pass an
    /// empty `Vec` to record "current".
    pub fn set(&self, missing_versions: Vec<i64>) {
        match self.0.write() {
            Ok(mut guard) => *guard = missing_versions,
            // A previous holder panicked while holding the lock. The
            // written-to value is still perfectly usable data — recover it
            // rather than let one panic permanently blind the health
            // endpoint to schema drift for the rest of the process's life.
            Err(poisoned) => *poisoned.into_inner() = missing_versions,
        }
    }

    /// The versions missing as of the most recent check. Empty means
    /// "current" (or "not yet checked", which is also truthfully "current"
    /// — see the `Default` doc above).
    pub fn get(&self) -> Vec<i64> {
        match self.0.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reports_current() {
        let status = SchemaDriftStatus::default();
        assert_eq!(status.get(), Vec::<i64>::new());
    }

    #[test]
    fn set_then_get_round_trips() {
        let status = SchemaDriftStatus::default();
        status.set(vec![20260823000000, 20260829000000]);
        assert_eq!(status.get(), vec![20260823000000, 20260829000000]);
    }

    #[test]
    fn clone_shares_the_same_underlying_state() {
        let status = SchemaDriftStatus::default();
        let handle = status.clone();
        status.set(vec![1]);
        assert_eq!(handle.get(), vec![1], "clones must observe writes through any handle");
    }
}
