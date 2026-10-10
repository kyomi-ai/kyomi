// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure "should the sync engine (re)start" decision (KYO-833).
//!
//! `components::layout::SyncEngineStarter` calls [`start_sync_engine`]
//! (`crate::cache::sync_engine`, `wasm32`-only) from inside an `Effect` that
//! watches the reactive `workspace_id` signal. The engine's own
//! subscriptions (`sync_action`, `sync_complete`, `sync_reset`,
//! `billing_status_changed`, `error` — see `cache::sync_engine`'s module
//! doc) are registered via `on_cleanup` against whatever `Owner` is current
//! when `start_sync_engine` runs.
//!
//! In `reactive_graph` 0.2.14, an `Effect`'s closure runs inside the SAME
//! `Owner` on every re-run (`reactive_graph::effect::render_effect::prep`
//! creates it once), but that owner's `cleanup()` — which disposes every
//! child `Owner` and runs every `on_cleanup` registered since the last run —
//! fires at the START of each subsequent run, before the closure body
//! executes again (`Owner::with_cleanup`). So if `SyncEngineStarter`'s effect
//! calls `start_sync_engine` directly inside its own closure, and that effect
//! ever re-runs for ANY reason (not just a genuine workspace change — e.g. a
//! transient `None` while an upstream resource is refetching), every sync
//! engine subscription is silently torn down. Because the old code also
//! tracked "have I started for this workspace" in a `StoredValue` that never
//! resets on that teardown, nothing ever re-registers: the tab stops reacting
//! to `billing_status_changed` (and everything else the sync engine
//! delivers) forever, with no visible error — this was the KYO-833 bug.
//!
//! The fix makes the engine's lifetime independent of how many times the
//! effect re-runs: it lives in a child `Owner` of the *component's* owner
//! (captured once, when `SyncEngineStarter` itself is instantiated — not
//! re-captured per effect run), explicitly disposed and replaced only when
//! the workspace id actually, meaningfully changes. This module is the pure
//! decision half of that: given the workspace id the engine is currently
//! running for (if any) and the latest value read off the reactive signal,
//! decide whether to leave it alone, start it for the first time, or
//! dispose-and-restart it for a new workspace. Extracted out of
//! `components::layout` (which is not `wasm32`-gated as a whole, but whose
//! `SyncEngineStarter` body is — see that component's doc comment) into its
//! own ungated module so the decision is unit-tested on the host target, the
//! same split `cache::schema_hash` (KYO-479) and `cache::reconcile`
//! (KYO-480) established for their own sync-engine decisions.

/// What `SyncEngineStarter`'s effect should do about the running sync engine
/// scope, given the workspace id it is currently running for (if any) and the
/// workspace id signal's latest value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEngineAction {
    /// Leave the currently running engine (if any) untouched. Covers both
    /// "nothing is running and there is nothing to start for" (signal is
    /// `None`/empty) and "the signal still names the workspace already
    /// running" — neither is a change that should tear anything down.
    Keep,
    /// No engine is currently running, and the signal names a real
    /// workspace — start one.
    Start,
    /// An engine is currently running for a DIFFERENT, real workspace id —
    /// dispose it and start a fresh one for the new id.
    Restart,
}

/// Decide the action for `SyncEngineStarter`'s effect.
///
/// `currently_running` is the workspace id the live engine scope is running
/// for, or `None` if no engine is running yet. `new_workspace_id` is the
/// latest value read off the reactive `workspace_id` signal this effect run —
/// `None` (not yet authenticated, or a resource transiently re-fetching) or
/// `Some("")` (defensive: the signal's documented empty-string case, mirrored
/// from the pre-KYO-833 check) both mean "nothing to start for right now",
/// and per this function's whole purpose must NEVER be read as "tear down
/// the engine that's already running" — that was the KYO-833 bug, just moved
/// from "an effect rerun with no signal change at all" to "an effect rerun
/// triggered by the signal itself going transiently empty and back". Only an
/// actual, different, non-empty workspace id is a real "switch workspaces"
/// signal.
pub fn sync_engine_action(
    currently_running: Option<&str>,
    new_workspace_id: Option<&str>,
) -> SyncEngineAction {
    match new_workspace_id {
        None | Some("") => SyncEngineAction::Keep,
        Some(id) => {
            if currently_running == Some(id) {
                SyncEngineAction::Keep
            } else if currently_running.is_none() {
                SyncEngineAction::Start
            } else {
                SyncEngineAction::Restart
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Keep: nothing running, nothing to start for ────────────────────────

    #[test]
    fn nothing_running_and_signal_none_keeps() {
        assert_eq!(sync_engine_action(None, None), SyncEngineAction::Keep);
    }

    #[test]
    fn nothing_running_and_signal_empty_string_keeps() {
        assert_eq!(sync_engine_action(None, Some("")), SyncEngineAction::Keep);
    }

    // ── Keep: an engine is running and must not be torn down ───────────────

    /// The core KYO-833 regression case: the effect reruns with the signal
    /// transiently `None` (e.g. an upstream resource re-fetching) while an
    /// engine is already running for a real workspace — must never tear it
    /// down.
    #[test]
    fn running_engine_survives_signal_going_none() {
        assert_eq!(
            sync_engine_action(Some("ws-1"), None),
            SyncEngineAction::Keep
        );
    }

    #[test]
    fn running_engine_survives_signal_going_empty_string() {
        assert_eq!(
            sync_engine_action(Some("ws-1"), Some("")),
            SyncEngineAction::Keep
        );
    }

    /// The signal re-reporting the SAME workspace id (any other unrelated
    /// effect rerun) must not restart anything already running for it.
    #[test]
    fn running_engine_survives_signal_repeating_same_id() {
        assert_eq!(
            sync_engine_action(Some("ws-1"), Some("ws-1")),
            SyncEngineAction::Keep
        );
    }

    // ── Start: nothing running yet, a real workspace id arrives ────────────

    #[test]
    fn first_real_workspace_id_starts() {
        assert_eq!(
            sync_engine_action(None, Some("ws-1")),
            SyncEngineAction::Start
        );
    }

    // ── Restart: a genuinely different workspace id ─────────────────────────

    #[test]
    fn switching_to_a_different_workspace_restarts() {
        assert_eq!(
            sync_engine_action(Some("ws-1"), Some("ws-2")),
            SyncEngineAction::Restart
        );
    }
}
