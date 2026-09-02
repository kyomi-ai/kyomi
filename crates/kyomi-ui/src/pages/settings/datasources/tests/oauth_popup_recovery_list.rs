//! List-level OAuth popup lifecycle: `DatasourceRow`'s own
//! "Connect"/"Reconnect" button — the second connect entry point,
//! reached without ever opening the edit modal — arming its own popup
//! monitor, stashing and invoking its own teardown cleanup, draining a
//! superseded attempt's monitor before installing a new one, and
//! recovery re-checking the "datasources" list query (rather than an
//! `OAuthStatusSource`) before it can ever report a failure (KYO-440),
//! plus the KYO-524 fix for the duplicate-toast regression that shipped
//! in it.
//!
//! The modal-level (`ModalOAuthStatusPanel`) counterpart of this same
//! lifecycle lives in the sibling `oauth_popup_recovery_modal.rs`. See
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.

use super::{appears_shortly_before, extract_between, SRC};

// ── KYO-440: list-level popup recovery (the second connect entry point) ──
//
// KYO-437 fixed the spinner-forever bug for `ModalOAuthStatusPanel` only.
// `DatasourceRow`'s own "Connect"/"Reconnect" button — the list-level entry
// point, reached without ever opening the edit modal — called
// `open_oauth_popup` directly and never armed a monitor, so the identical
// bug survived at that call site. The four guards below pin the four ways
// this wiring differs from the modal's (a shared, row-scoped
// `oauth_connecting: Option<String>` instead of a private `bool`; recovery
// re-checks the "datasources" list query instead of an `OAuthStatusSource`;
// and its own `on_cleanup`/second-connect teardown) plus the shared
// "recheck before reporting failure" ordering `ModalOAuthStatusPanel`
// already established.

use super::super::credential_status_indicates_connected;

#[test]
fn credential_status_indicates_connected_matches_the_cred_action_gate() {
    // The two states `DatasourceRow`'s `cred_action` shows an OAuth
    // Connect/Reconnect button for — anything else must read as
    // "connected" here, or a recovered row's own re-check could disagree
    // with the button that prompted the connect attempt in the first
    // place.
    for status in ["missing", "expired"] {
        assert!(
            !credential_status_indicates_connected(status),
            "{status:?} must not be read as connected — DatasourceRow still shows a \
             Connect/Reconnect button for it"
        );
    }
    for status in ["valid", "shared"] {
        assert!(
            credential_status_indicates_connected(status),
            "{status:?} must be read as connected — DatasourceRow shows no \
             Connect/Reconnect button for it"
        );
    }
}

/// Mirrors `start_connect_arms_the_popup_monitor_and_stashes_cleanup`
/// above for the list-level entry point: `on_oauth_click` must arm the
/// popup monitor after a successful `open_popup`, not just open the
/// popup, and stash its cleanup for teardown. A regression that reverts
/// to calling only `open_popup` would compile and look identical in a
/// diff of the success path alone.
#[test]
fn on_oauth_click_arms_the_popup_monitor_and_stashes_cleanup() {
    let body = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "view! {\n                <Button",
    );
    assert!(
        body.contains("monitor_oauth_popup("),
        "on_oauth_click must call monitor_oauth_popup after a successful open_popup — \
         without it, a closed/abandoned popup at the list level never clears \
         oauth_connecting (KYO-440)"
    );
    assert!(
        body.contains("popup_monitor.update_value"),
        "the monitor's cleanup must be stashed in popup_monitor so this row's \
         on_cleanup can stop it on teardown instead of leaking the interval/timeout"
    );
}

/// KYO-440 cycle 4 (Option A): the popup monitor's `still_connecting`
/// closure must read this row's own private per-attempt token
/// (`connect_attempt_live`), never the list-shared `oauth_connecting`
/// signal in any form — not even scoped by this row's own id, which is
/// what cycle 3 did and which the cycle-4 review found still reachable by
/// a writer outside this row (the settings modal's own, independent
/// `start_connect`, which never touches `oauth_connecting` at all — see
/// `credential_status_indicates_connected`'s neighbouring doc comment in
/// production for the full trace). A signal read that depends on
/// `oauth_connecting` in any way can be desynced by a writer this row has
/// no way to know about; only a token private to this row's own click
/// handler, drain-before-arm, and `on_cleanup` is immune to all of them at
/// once.
#[test]
fn still_connecting_does_not_read_the_shared_oauth_connecting_signal() {
    let still_connecting = extract_between(
        SRC,
        "let cleanup = monitor_oauth_popup(",
        "move |outcome| {",
    );
    assert!(
        !still_connecting.contains("oauth_connecting"),
        "the still_connecting closure passed to monitor_oauth_popup must not reference \
         oauth_connecting at all — comparing the list-shared signal, even scoped by this \
         row's own id, is reachable by writers outside this row (the settings modal's \
         independent start_connect, and any future writer) that have no reason to know \
         this row's popup even exists (KYO-440 cycle 4) — found: {still_connecting:?}"
    );
    assert!(
        still_connecting.contains("connect_attempt_live"),
        "the still_connecting closure must read this row's own private \
         connect_attempt_live token instead — found: {still_connecting:?}"
    );
}

/// `on_oauth_click`'s click guard and the button's `disabled` prop must
/// both read this row's own `is_connecting` (KYO-440 cycle 4 reverts cycle
/// 3's "any row connecting" widening — see the neighbouring doc comments
/// in production for why that widening's justification collapsed: the
/// settings modal, one of the writers it was meant to guard against, never
/// reads or writes `oauth_connecting` at all, so the wider guard paid a
/// real UX cost — locking every other row's button while any one row
/// connects — for an invariant that never actually held). Concurrent
/// per-row connects are allowed again; correctness no longer depends on
/// this guard at all, since `still_connecting` now reads a token private
/// to each row (see the test above) — this guard is purely a UI throttle.
#[test]
fn on_oauth_click_guard_and_disabled_prop_are_scoped_to_this_rows_state() {
    let guard = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        // KYO-442: the end marker is anchored on the `list_connect_action`
        // call — structure the KYO-442 gate change cannot remove without
        // also failing `list_connect_gate.rs`'s own dispatch-guard test —
        // rather than the `oauth_url_for_datasource` call that used to sit
        // directly in this guard. That call now lives inside
        // `list_connect_action` itself, so anchoring on it here would make
        // this test panic on a missing marker instead of asserting anything
        // (docs/standards/testing/anchor-source-text-markers-on-code-not-copy.md).
        "let action = list_connect_action(",
    );
    assert!(
        guard.contains("is_connecting.get_untracked()"),
        "on_oauth_click's guard must check is_connecting.get_untracked() (this row's own \
         state) — found guard body: {guard:?}"
    );
    assert!(
        !guard.contains("oauth_connecting.get_untracked().is_some()"),
        "the guard must not compare the shared oauth_connecting signal directly — that \
         was cycle 3's Option B, reverted in cycle 4 because it never closed the gap it \
         was meant to (the settings modal never touches this signal) while still costing \
         every other row's button its availability during a connect"
    );
    assert!(
        appears_shortly_before(
            SRC,
            "disabled=Signal::derive(move || is_connecting.get() || is_deleting.get())",
            "on:click=on_oauth_click",
            200,
        ),
        "the OAuth Connect/Reconnect button's disabled prop must be \
         is_connecting.get() || is_deleting.get() (this row's own state), matching \
         on_oauth_click's guard — not oauth_connecting.get().is_some(), which would \
         disable every row's button while any one row is connecting even though \
         concurrent per-row connects are supported again (KYO-440 cycle 4)"
    );
}

/// `on_cleanup` must actually *call* the stashed popup-monitor cleanup
/// for this row, not merely drop it — same reasoning as
/// `popup_monitor_cleanup_is_invoked_not_merely_dropped_on_teardown`
/// below, pinned separately here because `DatasourceRow` stashes and
/// tears down its own monitor rather than sharing `ModalOAuthStatusPanel`'s.
/// A row unmounting mid-connect (the list refetches and re-renders
/// constantly) must stop the timers immediately, not on the next poll
/// tick.
#[test]
fn datasource_row_on_cleanup_calls_the_stashed_popup_monitor_cleanup() {
    let row = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    let cleanup_block = extract_between(row, "on_cleanup(move || {", "\n    });");
    assert!(
        cleanup_block.contains("cleanup.take()()"),
        "DatasourceRow's on_cleanup must call the stashed cleanup (SendWrapper::take() \
         then invoke it), not just drop it — a dropped-not-called Box<dyn FnOnce()> \
         leaves the popup's gloo_timers Interval/Timeout still armed, since they're kept \
         alive by the running JS timers rather than by this Box (KYO-440)"
    );
}

/// The list row has no `fetch_oauth_status_once` / `OAuthStatusSource`
/// wiring like `ModalOAuthStatusPanel` — its only source of truth is the
/// "datasources" list query, so recovery must re-fetch that list and
/// inspect this row's own credential_status *before* it can report a
/// failure, mirroring `recovery_callback_rechecks_status_before_reporting_failure`
/// below. A recovered connection (postMessage lost, OAuth actually
/// succeeded) must adopt the recovered state — invalidate the cache, no
/// error toast — never fall through to reporting cancelled/timed out.
#[test]
fn on_oauth_click_recovery_rechecks_datasources_before_reporting_failure() {
    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");
    let fetch_pos = recovery
        .find("list_datasources()")
        .expect("recovery must call list_datasources() to re-check this row's status");
    let report_pos = recovery
        .find("popup_monitor_outcome_message(")
        .expect("recovery must build a failure message via popup_monitor_outcome_message");
    assert!(
        fetch_pos < report_pos,
        "the status recheck (list_datasources) must be positioned — and therefore run — \
         before the failure message is ever built, so a recovered connection never gets \
         reported as cancelled/timed out (KYO-440)"
    );

    let success_arm = extract_between(recovery, "if recovered {", "} else {");
    assert!(
        !success_arm.contains("toast_error"),
        "a recovered connection must not show an error toast — the OAuth handshake \
         actually succeeded, only the postMessage notifying this row was lost (KYO-440)"
    );
    assert!(
        success_arm.contains("query_cache.invalidate(\"datasources\")"),
        "a recovered connection must invalidate the datasources query cache, adopting \
         the recovered state exactly as the list-level postMessage success handler does \
         (KYO-440)"
    );
}

/// KYO-524: this diff (KYO-440) introduced a duplicate-toast regression —
/// `on_outcome` fires from the popup monitor regardless of whether an
/// OAuth `postMessage` already resolved this attempt, because
/// `still_connecting` (`connect_attempt_live`) is deliberately private to
/// this row and never observes that message (see
/// `still_connecting_does_not_read_the_shared_oauth_connecting_signal`
/// above). Without a check, a genuine error already toasted accurately by
/// the list-level listener would get a second, false "connection
/// cancelled." toast on top of it once this row's popup closed itself.
/// The fix reads the list-shared `oauth_connecting` signal — cleared to
/// `None` by that listener the instant ANY recognized postMessage arrives
/// — and, when `None`, skips the recovery fetch and both toast branches
/// entirely rather than merely suppressing one of them.
#[test]
fn on_outcome_says_nothing_when_a_postmessage_already_resolved_the_attempt() {
    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");

    let guard_pos = recovery
        .find("oauth_connecting.try_get_untracked().flatten().is_none()")
        .expect(
            "on_outcome must check whether the list-shared oauth_connecting signal is None \
             — None means a postMessage already resolved this attempt and the listener \
             already gave the user an accurate toast, so recovering again would be at best \
             redundant and at worst a false toast over a real one (KYO-524)",
        );
    let return_pos = recovery[guard_pos..]
        .find("return;")
        .map(|i| guard_pos + i)
        .expect(
            "the oauth_connecting-is-None check must actually early-return out of the \
             closure — merely reading the signal without acting on it fixes nothing \
             (KYO-524)",
        );
    let fetch_pos = recovery
        .find("list_datasources()")
        .expect("recovery must still call list_datasources() for the Some(_) case");

    assert!(
        guard_pos < return_pos && return_pos < fetch_pos,
        "the None-check and its early return must both be positioned — and therefore run — \
         strictly before the list_datasources() recovery fetch, so an attempt already \
         resolved by a postMessage never reaches the fetch or either toast branch at all \
         (KYO-524); found guard at {guard_pos}, return at {return_pos}, fetch at {fetch_pos}"
    );
}

/// Companion to the test above: the KYO-524 "was this already resolved?"
/// check must live in the RECOVERY closure (`on_outcome`) only, never
/// migrate into the MONITORING closure (`still_connecting`) passed as
/// `monitor_oauth_popup`'s second argument. The two closures answer
/// deliberately different questions — see the doc comment on the guard in
/// production — and `still_connecting` must stay a pure read of this
/// row's own private `connect_attempt_live` so the monitor keeps running
/// to completion no matter what an external writer does to the shared
/// signal (the cycle-3 property, pinned separately above). Reading
/// `oauth_connecting` from `still_connecting` instead would let an
/// unrelated writer stop this row's OWN monitor early, reintroducing
/// cycle 3's bug in a new shape.
#[test]
fn oauth_connecting_check_lives_in_the_outcome_closure_not_the_still_connecting_closure() {
    let still_connecting = extract_between(
        SRC,
        "let cleanup = monitor_oauth_popup(",
        "move |outcome| {",
    );
    assert!(
        !still_connecting.contains("try_get_untracked().flatten().is_none()"),
        "the KYO-524 already-resolved check must live in the on_outcome closure, not in \
         still_connecting — still_connecting must stay a pure read of connect_attempt_live \
         alone (the cycle-3 property), never touching oauth_connecting — found in \
         still_connecting body: {still_connecting:?}"
    );

    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");
    assert!(
        recovery.contains("oauth_connecting.try_get_untracked().flatten().is_none()"),
        "the KYO-524 check must live in the on_outcome closure — found: {recovery:?}"
    );
}

/// `on_oauth_click`'s click guard is scoped to this row's own state again
/// (KYO-440 cycle 4 reverted the "any row connecting" widening — see
/// `on_oauth_click_guard_and_disabled_prop_are_scoped_to_this_rows_state`
/// above), so a *different* row's click can freely overwrite the shared
/// `oauth_connecting` display signal at any time. That can make THIS
/// row's own guard read "not connecting" — since it only compares against
/// this row's id — and let a re-click through while this row's own
/// earlier popup and monitor are still unresolved: drain-before-arm is
/// what actually stops that superseded attempt's `Interval`/`Timeout`,
/// making it load-bearing rather than merely defensive. It must do so
/// synchronously in the same `on:click` handler that arms the new
/// monitor, since wasm32 is single-threaded and no tick of a superseded
/// monitor's poll can land between the drain and the new monitor
/// replacing it in the slot. This pins that ordering directly.
#[test]
fn on_oauth_click_drains_the_previous_monitor_before_installing_a_new_one() {
    let body = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "view! {\n                <Button",
    );
    let drain_pos = body.find("if let Some(previous) = slot.take() {").expect(
        "on_oauth_click must drain popup_monitor's previously-stashed cleanup before \
         installing a new one — load-bearing since a different row's click can freely \
         overwrite the shared oauth_connecting display signal and defeat this row's own \
         click guard (KYO-440 cycle 4)",
    );
    let invoke_pos = body.find("previous.take()()").expect(
        "the drained cleanup must actually be invoked, not merely dropped — dropping a \
         Box<dyn FnOnce()> does nothing, since the interval/timeout closures are kept alive \
         by the running JS timers, not by this Box, so only calling the cleanup reaches into \
         those and cancels them (KYO-440)",
    );
    let install_pos = body
        .find("*slot = Some(send_wrapper::SendWrapper::new(")
        .expect("on_oauth_click must install the new monitor's cleanup into popup_monitor");
    assert!(
        drain_pos < invoke_pos && invoke_pos < install_pos,
        "the previous monitor's cleanup must be found AND invoked strictly before the new \
         monitor's cleanup is installed into the same slot — found the drain at byte \
         {drain_pos}, the invocation at {invoke_pos}, and the install at {install_pos}; out \
         of order here means a stale monitor from a superseded attempt on this row can \
         outlive the one that replaces it, and later misfire a false cancelled/timed-out \
         report over a genuinely in-flight second attempt (KYO-440)"
    );
}
