// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crate-wide serialization for tests that mutate process environment variables.
//!
//! `kyomi-auth` is edition 2024, where `std::env::set_var` / `remove_var` are
//! `unsafe`: concurrent `setenv`/`getenv` from two threads is undefined
//! behavior — glibc may reallocate the `environ` array out from under a
//! concurrent reader — **regardless of which keys are involved**. A lock
//! scoped to one variable (or one module) does not make this sound, because
//! it does nothing to exclude a mutator of a *different* variable running on
//! another thread at the same instant; `cargo test` runs the whole binary's
//! tests in parallel, so any two `#[test]`s that mutate the environment race
//! unless they share one mutex.
//!
//! [`EnvVarGuard`] is that one mutex, shared by every env-mutating test in
//! this crate. Acquire it, make the mutations the test needs via [`set`] /
//! [`remove`], and let it drop — the prior value of every key it touched is
//! restored automatically, including on panic (a poisoned lock is still
//! usable; see [`EnvVarGuard::acquire`]).
//!
//! # What this guard does *not* guarantee
//!
//! `std::env::set_var`'s Safety section requires no other thread be
//! concurrently *writing or reading* the environment through anything
//! outside `std::env`. This guard discharges only the writer half: every
//! mutator in this crate takes the same lock, so no two mutations race. It
//! cannot discharge the reader half — any other test in the same
//! parallel-by-default test binary may call `std::env::var` (directly, or
//! indirectly via libc/DNS) while a guard here holds the lock. That is a
//! real, currently-unmet part of the safety contract, accepted deliberately
//! rather than forcing `--test-threads=1` on the whole binary or giving up
//! env-dependent tests.
//!
//! [`set`]: EnvVarGuard::set
//! [`remove`]: EnvVarGuard::remove

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII guard holding the single crate-wide env-mutation mutex.
///
/// Build one with [`EnvVarGuard::acquire`], make mutations with [`set`] /
/// [`remove`] (both consume and return `Self`, so calls chain), and drop it
/// when the test body is done. Every key touched via `set`/`remove` is
/// restored to the value it held the *first* time this guard touched it.
///
/// [`set`]: EnvVarGuard::set
/// [`remove`]: EnvVarGuard::remove
#[must_use]
pub(crate) struct EnvVarGuard {
    _lock: MutexGuard<'static, ()>,
    /// Prior value of each touched key, captured on first touch only.
    saved: HashMap<String, Option<String>>,
}

impl EnvVarGuard {
    /// Acquire the shared lock, blocking until any other in-flight
    /// `EnvVarGuard` — in this module or any other — has been dropped.
    ///
    /// A poisoned lock (an earlier guard's holder panicked mid-test) is
    /// still safe to acquire: the data behind it is `()`, so there is
    /// nothing to leave inconsistent. Recovering it, rather than panicking
    /// again here, is what stops one failing test from poisoning every
    /// later env-mutating test in the binary.
    pub(crate) fn acquire() -> Self {
        let guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        Self {
            _lock: guard,
            saved: HashMap::new(),
        }
    }

    /// Set `key` to `value` for the lifetime of this guard.
    pub(crate) fn set(mut self, key: &str, value: &str) -> Self {
        self.record_prior(key);
        // SAFETY: `_lock` is held for this guard's entire lifetime, and
        // every env-mutating test in this crate acquires the same lock
        // before mutating, so no other *mutator* races this call. It does
        // NOT rule out a concurrent *reader* elsewhere in the binary —
        // `set_var`'s contract requires that too, and this guard leaves it
        // unmet; see the module doc's "What this guard does not guarantee".
        unsafe { std::env::set_var(key, value) };
        self
    }

    /// Remove `key` for the lifetime of this guard.
    pub(crate) fn remove(mut self, key: &str) -> Self {
        self.record_prior(key);
        // SAFETY: see `set` above — excludes other mutators via the same
        // mutex; the concurrent-reader gap noted there applies here too.
        unsafe { std::env::remove_var(key) };
        self
    }

    /// Capture `key`'s current value the first time this guard touches it,
    /// so `Drop` can restore exactly what was there before this test ran.
    fn record_prior(&mut self, key: &str) {
        self.saved
            .entry(key.to_string())
            .or_insert_with(|| std::env::var(key).ok());
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain() {
            // SAFETY: see `set` above — same crate-wide mutex, held through
            // the end of this `drop`, so restoration is exclusive against
            // other mutators; the reader gap noted there still applies.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(&key, v),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set `key` to `value` under the shared lock, without going through
    /// [`EnvVarGuard`] — used by these tests to establish a baseline *before*
    /// the guard under test exists. The lock is acquired and dropped
    /// immediately: `Mutex` is not reentrant, so it must be released before
    /// the test's own `EnvVarGuard::acquire()` call, and a brief window
    /// between the two is harmless — soundness requires every mutation to
    /// happen under the lock, not that the whole test be one critical
    /// section.
    fn set_baseline(key: &str, value: &str) {
        let guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `guard` (the same crate-wide mutex `EnvVarGuard` uses) is
        // held for this call, so no other *mutator* races it. Like
        // `EnvVarGuard::set`, this does not rule out a concurrent reader
        // elsewhere in the binary — see the module doc.
        unsafe { std::env::set_var(key, value) };
        drop(guard);
    }

    /// Remove `key` under the shared lock, without going through
    /// [`EnvVarGuard`]. See [`set_baseline`] for why this must still take
    /// the lock even though it's setup/teardown, not the guard under test.
    fn remove_baseline(key: &str) {
        let guard = lock().lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: see `set_baseline` above — same mutator-only guarantee.
        unsafe { std::env::remove_var(key) };
        drop(guard);
    }

    #[test]
    fn restores_prior_value_on_drop() {
        set_baseline("KYOMI_TEST_ENV_GUARD_PRIOR", "before");

        {
            let _g = EnvVarGuard::acquire().set("KYOMI_TEST_ENV_GUARD_PRIOR", "during");
            assert_eq!(
                std::env::var("KYOMI_TEST_ENV_GUARD_PRIOR").as_deref(),
                Ok("during")
            );
        }

        assert_eq!(
            std::env::var("KYOMI_TEST_ENV_GUARD_PRIOR").as_deref(),
            Ok("before")
        );
        remove_baseline("KYOMI_TEST_ENV_GUARD_PRIOR");
    }

    #[test]
    fn restores_unset_key_by_removing_it() {
        remove_baseline("KYOMI_TEST_ENV_GUARD_UNSET");

        {
            let _g = EnvVarGuard::acquire().set("KYOMI_TEST_ENV_GUARD_UNSET", "temp");
            assert!(std::env::var("KYOMI_TEST_ENV_GUARD_UNSET").is_ok());
        }

        assert!(std::env::var("KYOMI_TEST_ENV_GUARD_UNSET").is_err());
    }

    #[test]
    fn first_touch_wins_when_a_key_is_set_twice() {
        set_baseline("KYOMI_TEST_ENV_GUARD_DOUBLE", "original");

        {
            let _g = EnvVarGuard::acquire()
                .set("KYOMI_TEST_ENV_GUARD_DOUBLE", "first")
                .set("KYOMI_TEST_ENV_GUARD_DOUBLE", "second");
            assert_eq!(
                std::env::var("KYOMI_TEST_ENV_GUARD_DOUBLE").as_deref(),
                Ok("second")
            );
        }

        assert_eq!(
            std::env::var("KYOMI_TEST_ENV_GUARD_DOUBLE").as_deref(),
            Ok("original")
        );
        remove_baseline("KYOMI_TEST_ENV_GUARD_DOUBLE");
    }
}
