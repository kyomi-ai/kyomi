// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for KYO-636: a `tracing` callsite reached for the first
//! time on a thread with no subscriber of its own must not be permanently
//! poisoned to `Interest::never()`, even when a capturing subscriber is
//! concurrently active on a *different* thread.
//!
//! This has to live in its own `tests/*.rs` integration-test binary, not the
//! crate's `#[cfg(test)]` unit-test module, for two reasons:
//!
//! 1. Every `#[test]` inside one crate shares one process, and the fix is a
//!    process-wide, `Once`-guarded global install: once any earlier test in
//!    that process has called `capture_tracing()`, the global default
//!    already exists and no callsite can be poisoned again for the rest of
//!    the run. A unit test asserting the same thing would pass no matter
//!    what, because by the time it runs the race window is already
//!    permanently closed by some other test.
//! 2. This crate's own callsite (below) must be reached for the very first
//!    time by *this test's* spawned thread, with nothing else in the
//!    process having touched it yet.
//!
//! ## Why a simple "emit, then capture, then emit again" test doesn't work
//!
//! An earlier version of this test emitted on a single thread before and
//! after `capture_tracing()`, expecting the pre-capture emission to poison
//! the callsite. It didn't reproduce the bug — and, per
//! `docs/standards/testing/prove-test-fails-without-fix.md`, was caught by
//! actually trying to break it: it kept passing even with the entire fix
//! deleted. The reason: `capture_tracing()`'s own `tracing::subscriber::
//! set_default(..)` call constructs a `Dispatch` via `Dispatch::new(..)`,
//! and `tracing-core` unconditionally re-resolves the `Interest` of every
//! *already-registered* callsite whenever a `Dispatch` is constructed (see
//! the `callsite` module's "Rebuilding Cached Interest" docs) — so on a
//! single thread, `capture_tracing()`'s own setup call always repairs
//! whatever a same-thread emission poisoned before it, with or without this
//! ticket's fix. That auto-repair is scoped to callsites already known to
//! the registry at that instant, not to threads. It does nothing for a
//! callsite a *different* thread reaches for the *first* time afterward.
//!
//! The actual bug needs a second thread: `tracing-core`'s first-registration
//! path has a fast path (`Dispatchers::rebuilder`'s `has_just_one`) that,
//! while at most one *other* dispatcher is alive process-wide, resolves
//! interest by checking only the *registering thread's own* current
//! dispatcher — never the full registered set. A thread-local capture
//! (`set_default`) is invisible to any other thread's lookup ("with_default
//! will not propagate the current thread's default subscriber to any
//! threads spawned within [it]" — tracing-core's own docs). So a second
//! thread with no subscriber of its own, reaching the callsite for the
//! first time while only the capturing thread's dispatch is alive, still
//! resolves against nothing and gets `Interest::never()`, cached forever —
//! even though a capture is concurrently active elsewhere in the process.
//! `std::thread::spawn(..).join()` reproduces exactly this deterministically
//! (no timing-dependent flakiness): the join guarantees the spawned
//! thread's first-ever registration of the callsite happens strictly
//! between `capture_tracing()`'s setup and the main thread's own emission.

/// Emit one `tracing::info!` event. Kept as its own function (rather than
/// inlining the macro at both call sites below) specifically so both calls
/// hit the exact same callsite — `tracing`'s callsite identity is the
/// macro's expansion site, not the emitted text, so two textually identical
/// macro invocations at two different source locations would be two
/// different callsites with two independent interest caches, which
/// wouldn't exercise the bug at all.
fn emit_at_shared_callsite() {
    tracing::info!(target: "kyo636_regression", "shared callsite emission");
}

/// See the module doc comment for the full mechanism and why this needs a
/// second, joined thread rather than a single-threaded before/after emit.
#[test]
fn capture_tracing_protects_a_callsite_first_reached_by_another_thread() {
    // Starts capturing on this (main) thread. With the fix, this is also
    // where the permanent, always-interested global default gets installed
    // — see `kyomi_test_tracing::capture_tracing`'s module-level comment.
    let logs = kyomi_test_tracing::capture_tracing();

    // A fresh thread with no subscriber of its own — thread-local defaults
    // set via `set_default` do not propagate to threads spawned after
    // they're installed. `.join()` guarantees this thread's first-ever
    // registration of `emit_at_shared_callsite`'s callsite happens, and
    // completes, strictly before the main thread's own emission below.
    std::thread::spawn(emit_at_shared_callsite)
        .join()
        .expect("spawned thread panicked");

    // Same callsite, now on the capturing (main) thread. Without the fix,
    // the spawned thread above permanently cached this callsite's interest
    // as `Never`, and `tracing` skips constructing the `Event` entirely for
    // a `Never`-cached callsite — no subscriber, capturing or otherwise,
    // ever sees it, on any thread, including this one.
    emit_at_shared_callsite();

    let events = logs.events();
    assert_eq!(
        events.len(),
        1,
        "expected exactly the main-thread emission to be captured (the spawned \
         thread has no subscriber to be captured by); got {events:?} — a callsite \
         poisoned by the spawned thread's first-time registration would instead \
         capture 0 events, because the poisoning also silences this same-callsite \
         emission on the main thread"
    );
    assert!(
        events[0].1.contains("shared callsite emission"),
        "captured event has unexpected content: {:?}",
        events[0]
    );
}
